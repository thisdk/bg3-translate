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

  const batcher = useMemo(
    () => createDeltaBatcher({ commit: (batch) => applyDeltas(batch) }),
    [applyDeltas],
  );

  // 卸载时把尚未提交的 delta 落地（组件已经不在，但 store 里的文本要完整）
  useEffect(() => () => batcher.dispose(), [batcher]);

  /** 统一的一次翻译执行：事件回调 + 取消回滚 */
  const runTranslation = useCallback(
    async (request: TranslationEntry[]) => {
      if (!workDir || request.length === 0 || runningRef.current) return;
      runningRef.current = true;
      cancelRequestedRef.current = false;
      completedIdsRef.current = new Set();
      settledIdsRef.current = new Set();
      inFlightIdsRef.current = new Set();
      attemptedIdsRef.current = new Set();
      setTranslating(true);
      setCancelling(false);
      setRunSummary(null);
      setError(null);

      try {
        await translateEntries(workDir, request, styleHint.trim(), (event) => {
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
        if (cancelRequestedRef.current) {
          // 取消：未完成的条目回滚为待翻译
          for (const entry of request) {
            if (!completedIdsRef.current.has(entry.id)) {
              updateEntry(entry.id, {
                target: "",
                status: "pending",
                error: null,
              });
            }
          }
        } else {
          // 正常结束：把「翻译中」但没收到 done/error 的条目回滚，避免计数卡住
          for (const id of inFlightIdsRef.current) {
            updateEntry(id, { target: "", status: "pending", error: null });
          }
        }
        inFlightIdsRef.current = new Set();
        runningRef.current = false;
        setTranslating(false);
        setCancelling(false);
        cancelRequestedRef.current = false;
      }
    },
    [
      batcher,
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
