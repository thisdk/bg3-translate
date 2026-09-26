import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createDeltaBatcher } from "@/lib/delta-batcher";
import { buildRetryRequest, buildTranslationRequest } from "@/lib/entries";
import { cancelTranslation, translateEntries } from "@/lib/tauri";
import { useAppStore } from "@/store/app-store";
import type { RunSummary } from "../constants";
import type { TranslationEntry } from "@/lib/types";

export interface TranslationRun {
  translating: boolean;
  cancelling: boolean;
  runSummary: RunSummary | null;
  /** 翻译所有译文为空的条目（全部已翻译时清空重译） */
  translateAll: () => Promise<void>;
  /** 只重试出错的条目 */
  retryFailed: () => Promise<void>;
  /** 只重试一条 */
  retryOne: (id: string) => Promise<void>;
  cancel: () => Promise<void>;
  /** 切换 MOD 时清掉上次任务摘要 */
  resetSummary: () => void;
}

export interface UseTranslationRunOptions {
  workDir: string | null;
  /** 可翻译条目（原文非空的集合） */
  entries: TranslationEntry[];
  styleHint: string;
}

/**
 * 翻译运行的生命周期：请求组装、事件回放、取消与收尾回滚。
 *
 * 流式 delta 走 `createDeltaBatcher` 按帧合并，提交时调用 store 的
 * `applyDeltas`——**一帧内无论多少条并发流，都只产生一次 store 通知**。
 *
 * 关于「迟到的 delta」：后端可能在同一帧里先发 `done`/`error`（权威文本）
 * 再补发 delta。这类 delta 必须丢弃，否则会把已经完成的译文又追加一段。
 * 因此这里有两道防线：
 *   1. `settledIdsRef` 记录已拿到权威结果的条目，之后到达的 delta 直接忽略；
 *   2. `done`/`error` 时调用 `batcher.discard(id)`，丢掉同帧内尚未提交的文本。
 * 取消或收尾回滚时用 `batcher.discardAll()` 丢掉整批未提交内容。
 *
 * 关于「重试两轮拼接」（F-09）：一条条目可能被后端重试多次（网络退避重试、
 * 结构纠错重试），每轮的 delta 都往同一个条目推。**显示文本必须只对应最后
 * 一次尝试**，否则失败时界面上留下的是两轮拼接的垃圾文本。
 *
 * 契约前提：后端在**每次尝试**开始时都会再发一次
 * `progress(status="translating")`。前端只能靠这个信号区分「同一轮继续流式」
 * 与「新一次尝试开始」——若后端只在每个 job 开头发一次 progress，前端拿不到
 * 尝试边界，两轮 delta 只能拼接（当时 core 需要配套在尝试循环里补发 progress）。
 * 基于该契约：
 *   - 该条目本轮已经开始过尝试（`attemptedIdsRef`）→ 视为重试开始：
 *     先 `batcher.discard(id)` 丢掉上一轮尚未提交的 delta，再清空 target
 *     （同时清掉上一轮的 error 文案，否则行内显示的还是旧错误而不是新文本）；
 *   - `error` **不清空** target：保留最后一轮文本供人工抢救，用户保存后
 *     status 变 `edited`，写回链路照常工作。
 */
export function useTranslationRun({
  workDir,
  entries,
  styleHint,
}: UseTranslationRunOptions): TranslationRun {
  const updateEntry = useAppStore((s) => s.updateEntry);
  const setEntryStatus = useAppStore((s) => s.setEntryStatus);
  const applyDeltas = useAppStore((s) => s.applyDeltas);
  const setError = useAppStore((s) => s.setError);
  const beginRun = useAppStore((s) => s.beginRun);
  const endRun = useAppStore((s) => s.endRun);
  const getEntryById = useAppStore((s) => s.getEntryById);

  const [translating, setTranslating] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [runSummary, setRunSummary] = useState<RunSummary | null>(null);

  const cancelRequestedRef = useRef(false);
  /** 本轮已收到 done 的条目（取消时用于决定哪些不回滚） */
  const completedIdsRef = useRef<Set<string>>(new Set());
  /** 本轮已拿到权威结果（done / error）的条目：迟到的 delta 一律丢弃 */
  const settledIdsRef = useRef<Set<string>>(new Set());
  /** 进入过「翻译中」但还没收到 done/error 的条目，用于收尾兜底 */
  const inFlightIdsRef = useRef<Set<string>>(new Set());
  /** 本轮已开始过尝试（收到过 progress(translating) 或 delta）的条目 */
  const attemptedIdsRef = useRef<Set<string>>(new Set());
  const runningRef = useRef(false);
  /**
   * 当前活跃的那一轮翻译的序号；`null` = 没有任何一轮在跑。
   *
   * 为什么需要它：`await translateEntries(...)` settle（后端命令返回）之后，
   * IPC 通道里仍可能有消息迟到 —— 迟到的 delta 一旦被接受，就会把已经收尾
   * 回滚、或者已经拿到权威译文的条目重新改写成 `translating` + 半截文本。
   * 而 `translating` 的非空 target **是会被写进 PAK 的**（后端只在 `error`
   * 状态退回原文，见 crates/bg3-translate-core/src/types.rs），所以这类幽灵
   * 文本会直接污染用户的 MOD。
   *
   * 同一个序号还能挡住「上一轮的迟到 delta 落到下一轮」：旧回调发现
   * `activeRunRef.current !== 自己那一轮` 时直接丢弃，不会去动新一轮的状态。
   */
  const activeRunRef = useRef<number | null>(null);
  const runSeqRef = useRef(0);
  /**
   * 组件是否还挂在树上。
   *
   * 卸载（点「返回」回首页 / 打开新 MOD）后这一轮不该再触碰 store：条目 id 是
   * `{PAK 内路径}#{contentuid}`，新 MOD 里同名文件的 id 与旧 MOD 完全相同，
   * 迟到的 `progress` / 收尾回滚会命中新条目 —— 把刚翻好的译文改成
   * `translating`（会被写回 PAK）甚至清空。
   */
  const aliveRef = useRef(true);

  const batcher = useMemo(
    () => createDeltaBatcher({ commit: (batch) => applyDeltas(batch) }),
    [applyDeltas],
  );

  // 卸载时把尚未提交的 delta 落地（组件已经不在，但 store 里的文本要完整）
  useEffect(() => () => batcher.dispose(), [batcher]);

  // 卸载即本轮生命周期结束：停止接收事件、不再收尾回滚（StrictMode 会
  // 先卸载再挂载，所以这里在挂载时复位 alive）
  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
      activeRunRef.current = null;
    };
  }, []);

  /** 统一的一次翻译执行：事件回调 + 取消回滚 */
  const runTranslation = useCallback(
    async (request: TranslationEntry[]) => {
      if (!workDir || request.length === 0 || runningRef.current) return;
      runningRef.current = true;
      const runId = runSeqRef.current + 1;
      runSeqRef.current = runId;
      activeRunRef.current = runId;
      cancelRequestedRef.current = false;
      completedIdsRef.current = new Set();
      settledIdsRef.current = new Set();
      inFlightIdsRef.current = new Set();
      attemptedIdsRef.current = new Set();
      setTranslating(true);
      setCancelling(false);
      setRunSummary(null);
      setError(null);
      // 写回闸门：翻译期间禁止打包（半截译文是 translating 状态，会被写进 PAK）
      const runToken = beginRun();

      try {
        await translateEntries(workDir, request, styleHint.trim(), (event) => {
          // 本轮已经收尾（命令返回）或已经换成新一轮：任何迟到消息一律丢弃，
          // 否则会把已回滚/已完成的条目重新写成 translating + 半截文本
          if (activeRunRef.current !== runId) return;
          if (cancelRequestedRef.current && event.type !== "all_done") return;
          switch (event.type) {
            case "progress": {
              const retrying =
                event.status === "translating" &&
                attemptedIdsRef.current.has(event.entryId);
              if (retrying) {
                // 重试开始（网络退避 / 结构纠错）：显示文本只对应最后一次尝试。
                // 先丢掉该条目上一轮尚未提交的 delta，否则（下面的 flush 或下一帧）
                // 它们又会被追加回来 —— F-09：两轮文本拼接。
                batcher.discard(event.entryId);
                // 重新接受该条目的 delta（上一轮可能以 error 收尾、已被判为 settled）
                settledIdsRef.current.delete(event.entryId);
              } else if (event.status === "translating") {
                attemptedIdsRef.current.add(event.entryId);
              }
              // 其余条目的待提交内容先落地，保持「先文本后状态」的事件顺序
              batcher.flush();
              if (retrying) {
                updateEntry(event.entryId, {
                  target: "",
                  status: "translating",
                  // 清掉上一轮的错误文案，否则行内仍显示旧错误而不是新流式文本
                  error: null,
                });
              } else {
                setEntryStatus(event.entryId, event.status);
              }
              if (event.status === "translating") {
                inFlightIdsRef.current.add(event.entryId);
              } else {
                inFlightIdsRef.current.delete(event.entryId);
              }
              break;
            }
            case "delta":
              // done / error 之后到达的 delta：条目已有权威文本，直接丢弃
              if (settledIdsRef.current.has(event.entryId)) break;
              // 用户已手工保存译文：模型文本不得再追加（store 的 applyDeltas
              // 也会再挡一次，这里提前拦住、避免白白进批处理）
              if (getEntryById(event.entryId)?.status === "edited") break;
              attemptedIdsRef.current.add(event.entryId);
              inFlightIdsRef.current.add(event.entryId);
              batcher.push(event.entryId, event.text);
              break;
            case "done":
              // done 自带完整译文，同帧内还没提交的 delta 丢掉即可
              batcher.discard(event.entryId);
              settledIdsRef.current.add(event.entryId);
              completedIdsRef.current.add(event.entryId);
              inFlightIdsRef.current.delete(event.entryId);
              // 人工成果优先：用户已经保存过译文时，模型结果不得覆盖它
              if (getEntryById(event.entryId)?.status === "edited") break;
              updateEntry(event.entryId, {
                target: event.text,
                status: "translated",
                error: null,
              });
              break;
            case "error":
              // error 没有权威文本：先把「最后一次尝试」已经流出的 delta 落地，
              // 这样界面留下的是最后一轮的文本，用户可以手工改好再保存
              // （F-09：清空只发生在重试开始时，error 不清空 target）。
              // 落地后再置为 error，之后该条目判为 settled，迟到的 delta 一律丢弃。
              batcher.flush();
              settledIdsRef.current.add(event.entryId);
              inFlightIdsRef.current.delete(event.entryId);
              // 人工成果优先：用户保存过的译文不能被标成 error
              //（error 会让写回退回原文，等于丢掉人工译文）
              if (getEntryById(event.entryId)?.status === "edited") break;
              updateEntry(event.entryId, {
                status: "error",
                error: event.message,
              });
              break;
            case "all_done":
              batcher.flush();
              setRunSummary({ total: event.total, failed: event.failed });
              break;
          }
        });
      } catch (e) {
        if (!cancelRequestedRef.current) {
          setError(String(e));
        }
      } finally {
        // 收尾：丢掉尚未提交的 delta（下面的回滚会把目标重置掉，
        // 若先落地再回滚只会白白多一次通知）
        batcher.discardAll();
        if (!aliveRef.current) {
          // 工作台已卸载：条目不再属于这一轮，回滚只会误伤新 MOD 里同 id 的
          // 条目（取消回滚 / 未完成回滚都跳过）
        } else if (cancelRequestedRef.current) {
          // 取消：未完成的条目回滚为待翻译（人工保存过的译文除外）
          for (const entry of request) {
            if (completedIdsRef.current.has(entry.id)) continue;
            if (getEntryById(entry.id)?.status === "edited") continue;
            updateEntry(entry.id, {
              target: "",
              status: "pending",
              error: null,
            });
          }
        } else {
          // 正常结束：把「翻译中」但没收到 done/error 的条目回滚，避免计数卡住
          for (const id of inFlightIdsRef.current) {
            if (getEntryById(id)?.status === "edited") continue;
            updateEntry(id, { target: "", status: "pending", error: null });
          }
        }
        inFlightIdsRef.current = new Set();
        // 本轮到此为止：之后到达的任何事件都会被上面的 runId 校验丢弃
        if (activeRunRef.current === runId) activeRunRef.current = null;
        runningRef.current = false;
        setTranslating(false);
        setCancelling(false);
        // 收尾回滚已经完成，此时才允许写回（只有登记本轮 token 的那一轮能关闸门）
        endRun(runToken);
        cancelRequestedRef.current = false;
      }
    },
    [
      batcher,
      beginRun,
      endRun,
      getEntryById,
      setEntryStatus,
      setError,
      styleHint,
      updateEntry,
      workDir,
    ],
  );

  const translateAll = useCallback(async () => {
    const plan = buildTranslationRequest(entries);
    if (plan.request.length === 0) {
      setError("没有可翻译的条目");
      return;
    }
    if (plan.retranslateAll) {
      for (const entry of plan.request) {
        updateEntry(entry.id, { target: "", status: "pending", error: null });
      }
    }
    await runTranslation(plan.request);
  }, [entries, runTranslation, setError, updateEntry]);

  const retryFailed = useCallback(async () => {
    const request = buildRetryRequest(entries);
    if (request.length === 0) return;
    for (const entry of request) {
      updateEntry(entry.id, { target: "", status: "pending", error: null });
    }
    await runTranslation(request);
  }, [entries, runTranslation, updateEntry]);

  const retryOne = useCallback(
    async (id: string) => {
      const entry = entries.find((e) => e.id === id);
      if (!entry) return;
      const request: TranslationEntry = {
        ...entry,
        target: "",
        status: "pending",
        error: null,
      };
      updateEntry(id, { target: "", status: "pending", error: null });
      await runTranslation([request]);
    },
    [entries, runTranslation, updateEntry],
  );

  const cancel = useCallback(async () => {
    if (!translating || cancelling) return;
    cancelRequestedRef.current = true;
    setCancelling(true);
    setError(null);
    try {
      await cancelTranslation();
    } catch (e) {
      cancelRequestedRef.current = false;
      setError(`取消翻译失败: ${String(e)}`);
      setCancelling(false);
    }
  }, [cancelling, setError, translating]);

  const resetSummary = useCallback(() => setRunSummary(null), []);

  return {
    translating,
    cancelling,
    runSummary,
    translateAll,
    retryFailed,
    retryOne,
    cancel,
    resetSummary,
  };
}
