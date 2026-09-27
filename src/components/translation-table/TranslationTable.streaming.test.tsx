/**
 * 流式翻译端到端（批处理）测试：2 万条条目 + 3000 个 delta。
 *
 * 这条用例盯的是 G1 的端到端效果，全部用结构断言（通知次数、文本内容、
 * 状态），不依赖墙钟阈值：
 *   1. 一帧内 3000 个 delta（跨 6 个并发条目）只产生 **1 次** store 通知；
 *   2. 文本一字不差、顺序不变地落到各自条目上；
 *   3. `done` 之后到达的 delta 不得再写进 target（同帧未提交的也要丢掉）；
 *   4. 取消后未完成的条目回滚为待翻译，缓存的 delta 不会写回；
 *   5. 重试（网络退避 / 结构纠错）时显示文本只对应**最后一次尝试**（F-09）。
 *
 * **整个文件用 `<StrictMode>` 挂载**（`src/main.tsx` 就是 StrictMode，
 * `bun run dev` 跑的就是这条生命周期）：effect 会走一遍「挂载 → cleanup →
 * 再挂载」，而 `useMemo` 里的对象（delta 批处理器）**不会重建** —— 这正是
 * R5-02 的现场：cleanup 里的 `dispose()` 一旦是永久闩锁，重挂载后每一次
 * `push` 都被静默丢掉，开发模式下流式预览整个失效。非 StrictMode 挂载看不见
 * 这条缝，所以这里刻意用严格生命周期跑（实测：把 `resume` 变异成空实现，
 * 本文件 8 条全红 + `TranslationTable.edit-race.test.tsx` 5 条全红）。
 *
 * 跑法：`bunx vitest run src/components/translation-table/TranslationTable.streaming.test.tsx`
 * 会打印 `[perf] table-streaming: ...` 实测数字（仅记录，不断言）。
 */
import { StrictMode, act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TranslationTable } from "@/components/TranslationTable";
import { useAppStore } from "@/store/app-store";
import { buildEntries, pakFile } from "@/test-utils/fixtures";
import {
  click,
  findButton,
  mountContainer,
  stubVirtualScrollLayout,
  waitMs,
  type Mounted,
} from "@/test-utils/dom";
import type { TranslationEntry, TranslationEvent } from "@/lib/types";

const ENTRY_COUNT = 20_000;
const DELTA_COUNT = 3_000;
const FILE_NAME = "Localization/English/big.xml";
/** 流式翻译的条目：target 为空、状态 pending（属于可翻译条目） */
const STREAM_IDS = ["e-0", "e-4", "e-8", "e-12", "e-16", "e-20"];
const FIRST_ID = STREAM_IDS[0];

const backend = vi.hoisted(() => ({
  entries: [] as import("@/lib/types").TranslationEntry[],
  onEvent: null as null | ((event: import("@/lib/types").TranslationEvent) => void),
  /** 手动 resolve：模拟"翻译仍在进行中" */
  finish: null as null | (() => void),
  /** 每次 `translate_entries` 收到的条目 payload（断言「重试是否带回上一次原因」） */
  sent: [] as import("@/lib/types").TranslationEntry[][],
}));

vi.mock("@/lib/tauri", () => ({
  readFileEntries: vi.fn(async () => backend.entries),
  translateEntries: vi.fn(
    async (
      _workDir: string,
      entries: TranslationEntry[],
      _styleHint: string,
      onEvent: (event: TranslationEvent) => void,
    ) => {
      backend.sent.push(entries);
      backend.onEvent = onEvent;
      await new Promise<void>((resolve) => {
        backend.finish = resolve;
      });
    },
  ),
  cancelTranslation: vi.fn(async () => undefined),
}));

let mounted: Mounted;
let restoreLayout: () => void;

beforeEach(() => {
  restoreLayout = stubVirtualScrollLayout();
  backend.entries = buildEntries(ENTRY_COUNT, FILE_NAME);
  backend.onEvent = null;
  backend.finish = null;
  backend.sent = [];

  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/big.pak", "/tmp/work", [pakFile(FILE_NAME)]);
  api.setSelectedFiles([pakFile(FILE_NAME)]);

  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
  restoreLayout();
});

/** 挂载表格并等条目加载完成 */
async function renderTable(): Promise<void> {
  await mounted.render(<StrictMode><TranslationTable /></StrictMode>);
  await waitMs(0);
}

/** 挂载表格 → 等条目加载完成 → 点击「翻译 N 条」 */
async function startStreaming(): Promise<(event: TranslationEvent) => void> {
  await renderTable();
  const translateButton = findButton(mounted.container, "翻译 ");
  expect(translateButton).not.toBeNull();
  await act(async () => {
    click(translateButton!);
  });
  await waitMs(0);
  expect(backend.onEvent).not.toBeNull();
  return backend.onEvent!;
}

/** 小规模条目（重试语义用例用，避免 2 万条的噪声） */
function smallEntry(id: string, patch: Partial<TranslationEntry> = {}): TranslationEntry {
  return {
    id,
    sourceFile: FILE_NAME,
    source: `Source of ${id}`,
    target: "",
    contentuid: `uid-${id}`,
    version: "1",
    status: "pending",
    error: null,
    ...patch,
  };
}

function storedEntry(id: string): TranslationEntry {
  const list = useAppStore.getState().entriesByFile[FILE_NAME];
  const entry = list.find((e) => e.id === id);
  expect(entry).toBeDefined();
  return entry!;
}

/** 直接读目标文本（不做断言，供轮询同步用） */
function entryTarget(id: string): string {
  const list = useAppStore.getState().entriesByFile[FILE_NAME] ?? [];
  return list.find((e) => e.id === id)?.target ?? "";
}

/**
 * 等批处理调度的那一帧真正落地。
 * 用「某个条目的文本已经写入」作为可观测信号轮询，而不是睡固定时长——
 * 慢 CI 上不会因为一帧没来得及跑而随机失败。
 */
async function waitForFlush(probeId: string, timeoutMs = 3000): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline && entryTarget(probeId) === "") {
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
}

/** 统计 store 通知次数（一次通知 = 一次派生重算 + 一次 React 渲染） */
function countNotifications(): { count: () => number; stop: () => void } {
  let count = 0;
  const unsubscribe = useAppStore.subscribe(() => {
    count += 1;
  });
  return { count: () => count, stop: unsubscribe };
}

/** 3000 个 token 轮流发往 6 个条目，返回每个条目应得的完整文本 */
function streamTokens(send: (event: TranslationEvent) => void): Map<string, string> {
  const expected = new Map<string, string>();
  for (let i = 0; i < DELTA_COUNT; i += 1) {
    const id = STREAM_IDS[i % STREAM_IDS.length];
    const text = `t${i % 10};`;
    expected.set(id, (expected.get(id) ?? "") + text);
    send({ type: "delta", entryId: id, text });
  }
  return expected;
}

describe("TranslationTable 流式 delta 批处理（2 万条 + 3000 delta）", () => {
  it("一帧内的 delta 只触发一次 store 通知，文本零丢失", { timeout: 120_000 }, async () => {
    const send = await startStreaming();
    const notifications = countNotifications();

    const started = performance.now();
    let expected = new Map<string, string>();
    let deliverMs = 0;
    await act(async () => {
      const deliverStarted = performance.now();
      expected = streamTokens(send);
      deliverMs = performance.now() - deliverStarted;
      // 等到这一帧真正落地（rAF 或 16ms 兜底 setTimeout）
      await waitForFlush(STREAM_IDS[0]);
    });
    const elapsed = performance.now() - started;
    const notified = notifications.count();
    notifications.stop();

    console.log(
      `[perf] table-streaming: deltas=${DELTA_COUNT} concurrent_entries=${STREAM_IDS.length} ` +
        `notifications=${notified} deliver_ms=${deliverMs.toFixed(1)} ` +
        `total_ms=${elapsed.toFixed(1)}(含等待批处理帧)`,
    );

    // 3000 个 delta、6 个并发条目 → 一帧一次提交
    expect(notified).toBe(1);
    // 每个条目拿到完整且顺序正确的文本
    for (const [id, text] of expected) {
      expect(storedEntry(id).target).toBe(text);
      expect(storedEntry(id).status).toBe("translating");
    }
    // 6 条并发流共 3000 个 token，一个字符都没丢
    expect(
      [...expected.values()].reduce((n, text) => n + text.length, 0),
    ).toBeGreaterThan(DELTA_COUNT);
    // 流式期间表格仍然只渲染视口附近的少量行
    expect(mounted.container.querySelectorAll("[data-index]").length).toBeLessThan(60);

    // 收尾：done 携带权威文本
    await act(async () => {
      for (const [id, text] of expected) {
        send({ type: "done", entryId: id, text });
      }
      send({ type: "all_done", total: STREAM_IDS.length, failed: 0 });
      backend.finish?.();
      await Promise.resolve();
    });

    for (const [id, text] of expected) {
      expect(storedEntry(id)).toMatchObject({ target: text, status: "translated" });
    }
    expect(mounted.container.textContent).toContain(
      `上次任务 ${STREAM_IDS.length} 条，失败 0 条`,
    );
  });

  it("done 之后到达的 delta 不得写入 target（含同帧未提交的）", async () => {
    const send = await startStreaming();

    // 同帧内：先来一段 delta，再来权威 done
    await act(async () => {
      send({ type: "delta", entryId: FIRST_ID, text: "同帧未落地的文本" });
      send({ type: "done", entryId: FIRST_ID, text: "权威译文" });
      await Promise.resolve();
    });

    // done 之后迟到的 delta：同帧内再塞一个对照条目，
    // 用「对照条目写入成功」证明这一帧确实已经 flush 过
    await act(async () => {
      send({ type: "delta", entryId: FIRST_ID, text: "迟到的尾巴" });
      send({ type: "delta", entryId: STREAM_IDS[1], text: "对照文本" });
      await waitForFlush(STREAM_IDS[1]);
    });

    expect(entryTarget(STREAM_IDS[1])).toBe("对照文本");
    expect(storedEntry(FIRST_ID).target).toBe("权威译文");
    expect(storedEntry(FIRST_ID).status).toBe("translated");
    expect(mounted.container.textContent).not.toContain("迟到的尾巴");

    // 收尾（all_done 后的兜底回滚也不该动已完成条目）
    await act(async () => {
      send({ type: "all_done", total: 1, failed: 0 });
      backend.finish?.();
      await Promise.resolve();
    });
    expect(storedEntry(FIRST_ID).target).toBe("权威译文");
    expect(storedEntry(FIRST_ID).status).toBe("translated");
  });

  it("取消后未完成的条目回滚为待翻译，缓存的 delta 不会写回", async () => {
    const send = await startStreaming();

    // 取消前塞入一个还没 flush 的 delta（留在批处理缓存里）
    send({ type: "delta", entryId: FIRST_ID, text: "取消前未落地的文本" });

    const cancelButton = findButton(mounted.container, "取消翻译");
    expect(cancelButton).not.toBeNull();
    await act(async () => {
      click(cancelButton!);
    });

    // 取消后到达的 delta 直接被丢弃
    send({ type: "delta", entryId: FIRST_ID, text: "取消后迟到的文本" });
    await act(async () => {
      backend.finish?.();
      await Promise.resolve();
    });

    expect(storedEntry(FIRST_ID).target).toBe("");
    expect(storedEntry(FIRST_ID).status).toBe("pending");

    // 再跑一轮翻译：用对照条目证明批处理帧确实会 flush，
    // 而上一轮取消时缓存的 delta 没有被写回任何条目
    const sendAgain = await startStreaming();
    await act(async () => {
      sendAgain({ type: "delta", entryId: STREAM_IDS[1], text: "对照文本" });
      await waitForFlush(STREAM_IDS[1]);
    });
    expect(entryTarget(STREAM_IDS[1])).toBe("对照文本");
    expect(storedEntry(FIRST_ID).target).toBe("");
    expect(mounted.container.textContent).not.toContain("未落地的文本");
    expect(mounted.container.textContent).not.toContain("迟到的文本");
  });
});

describe("重试 / 纠错时的显示文本（F-09）", () => {
  it("重试失败条目只清空被重试的条目，已完成条目不受影响", async () => {
    backend.entries = [
      smallEntry("e-bad", {
        status: "error",
        target: "上一轮被拒的文本",
        error: "结构校验未通过：占位符 {1} 缺失（已重试 1 次）",
      }),
      smallEntry("e-good", { status: "translated", target: "已经翻好的译文" }),
    ];
    await renderTable();

    const retryButton = findButton(mounted.container, "重试 1 条失败");
    expect(retryButton).not.toBeNull();
    await act(async () => {
      click(retryButton!);
    });
    await waitMs(0);
    expect(backend.onEvent).not.toBeNull();

    // 只有被重试的条目被清空（这是 retryFailed 路径本来就有的语义）
    expect(storedEntry("e-bad")).toMatchObject({
      target: "",
      status: "pending",
      error: null,
    });
    expect(storedEntry("e-good")).toMatchObject({
      target: "已经翻好的译文",
      status: "translated",
    });

    // 界面上清掉了错误文案，但**发给后端的那份必须留着**：后端拿它当纠错提示
    // 注入重试 prompt，模型才知道上一轮为什么被判不合格；清掉就等于原样再问一遍。
    expect(backend.sent).toHaveLength(1);
    const sentBad = backend.sent[0].find((e) => e.id === "e-bad");
    expect(sentBad).toMatchObject({
      status: "pending",
      target: "",
      error: "结构校验未通过：占位符 {1} 缺失（已重试 1 次）",
    });
    expect(backend.sent[0].map((e) => e.id)).toEqual(["e-bad"]);

    // 这一轮又失败：只保留最后一轮文本，已完成条目仍然不受影响
    const send = backend.onEvent!;
    await act(async () => {
      send({ type: "progress", entryId: "e-bad", status: "translating" });
      send({ type: "delta", entryId: "e-bad", text: "重试轮的文本" });
      send({
        type: "error",
        entryId: "e-bad",
        message: "结构校验未通过：占位符 {1} 缺失（已重试 1 次）",
      });
      send({ type: "all_done", total: 1, failed: 1 });
      backend.finish?.();
      await Promise.resolve();
    });

    expect(storedEntry("e-bad")).toMatchObject({
      target: "重试轮的文本",
      status: "error",
    });
    expect(storedEntry("e-good")).toMatchObject({
      target: "已经翻好的译文",
      status: "translated",
    });
    expect(mounted.container.textContent).toContain("已经翻好的译文");
  });

  it("网络退避重试：先清空上一轮文本，最终不出现两轮拼接", async () => {
    backend.entries = [smallEntry("e-retry"), smallEntry("e-control")];
    const send = await startStreaming();

    // 第一次尝试
    await act(async () => {
      send({ type: "progress", entryId: "e-retry", status: "translating" });
      send({ type: "delta", entryId: "e-retry", text: "第一轮" });
      await waitForFlush("e-retry");
    });
    expect(entryTarget("e-retry")).toBe("第一轮");

    // 网络失败退避后重新开始一次尝试：后端再发 progress(translating)
    await act(async () => {
      send({ type: "progress", entryId: "e-retry", status: "translating" });
      await Promise.resolve();
    });
    // 上一轮的文本必须先被清空，而不是等第二轮的 delta 追加在后面
    expect(entryTarget("e-retry")).toBe("");

    // 第二次尝试成功
    await act(async () => {
      send({ type: "delta", entryId: "e-retry", text: "第二轮完整译文" });
      await waitForFlush("e-retry");
    });
    expect(entryTarget("e-retry")).toBe("第二轮完整译文");

    await act(async () => {
      send({ type: "done", entryId: "e-retry", text: "第二轮完整译文" });
      send({ type: "all_done", total: 1, failed: 0 });
      backend.finish?.();
      await Promise.resolve();
    });

    const entry = storedEntry("e-retry");
    expect(entry.target).toBe("第二轮完整译文");
    expect(entry.target).not.toContain("第一轮");
    expect(entry.status).toBe("translated");
  });

  it("重试开始时同帧未提交的旧 delta 会被丢弃（不会清空后又追加回来）", async () => {
    backend.entries = [smallEntry("e-retry"), smallEntry("e-control")];
    const send = await startStreaming();

    // 第一轮 delta 还在批处理缓存里（同一帧内）就收到重试开始
    await act(async () => {
      send({ type: "progress", entryId: "e-retry", status: "translating" });
      send({ type: "delta", entryId: "e-retry", text: "第一轮未提交" });
      send({ type: "progress", entryId: "e-retry", status: "translating" });
      // 用对照条目的 delta 逼出一帧，证明这一帧确实 flush 过
      send({ type: "delta", entryId: "e-control", text: "对照" });
      await waitForFlush("e-control");
    });

    expect(entryTarget("e-control")).toBe("对照");
    // 旧 delta 被丢弃，没有在清空之后又追加回来
    expect(entryTarget("e-retry")).toBe("");

    await act(async () => {
      send({ type: "delta", entryId: "e-retry", text: "第二轮文本" });
      await waitForFlush("e-retry");
    });
    expect(entryTarget("e-retry")).toBe("第二轮文本");
    expect(entryTarget("e-retry")).not.toContain("第一轮未提交");
  });

  it("结构纠错一直不合格 → error：target 只保留最后一轮文本，且仍可人工编辑", async () => {
    backend.entries = [smallEntry("e-correct"), smallEntry("e-control")];
    const send = await startStreaming();

    // 第一次尝试：译文丢了占位符（结构不合格）
    await act(async () => {
      send({ type: "progress", entryId: "e-correct", status: "translating" });
      send({ type: "delta", entryId: "e-correct", text: "造成伤害" });
      await waitForFlush("e-correct");
    });
    expect(entryTarget("e-correct")).toBe("造成伤害");

    // 纠错重试开始
    await act(async () => {
      send({ type: "progress", entryId: "e-correct", status: "translating" });
      await Promise.resolve();
    });
    expect(entryTarget("e-correct")).toBe("");

    // 第二轮仍然不合格
    await act(async () => {
      send({ type: "delta", entryId: "e-correct", text: "造成 {2} 点伤害" });
      await waitForFlush("e-correct");
    });

    // 最终失败：error 不清空 target，保留最后一轮文本供人工抢救
    await act(async () => {
      send({
        type: "error",
        entryId: "e-correct",
        message: "结构校验未通过：占位符 {1} 缺失（已重试 1 次）",
      });
      send({ type: "all_done", total: 1, failed: 1 });
      backend.finish?.();
      await Promise.resolve();
    });

    const entry = storedEntry("e-correct");
    expect(entry.target).toBe("造成 {2} 点伤害");
    expect(entry.target).not.toContain("造成伤害造成"); // 没有两轮拼接
    expect(entry.status).toBe("error");
    expect(entry.error).toContain("结构校验未通过");
    // target 非空 → 行内仍有「编辑」按钮，用户可以手工改好再保存
    expect(findButton(mounted.container, "编辑")).not.toBeNull();
  });
});

/**
 * React StrictMode 下的流式落地（`src/main.tsx` 就是 `<React.StrictMode>`）。
 *
 * StrictMode 会让 effect 走一遍「挂载 → cleanup → 再挂载」，而 `useMemo` 的
 * delta 批处理器**不会重建**：cleanup 里的 `dispose()` 一旦是永久闩锁，
 * 重新挂载后的每一次 `push` 都会被静默丢掉 —— 开发模式下流式预览整个失效，
 * 译文只在 `done` 一跳出现，控制台里没有任何报错。
 *
 * 这条用例把这个语义**单独钉死**：它自带 `<StrictMode>` 包裹，不依赖本文件
 * 其它渲染路径（那些路径以后若改回非 StrictMode，这条回归仍然有效）。
 * 失败形态是 `target === ""`（delta 全部丢失）而 `status === "translating"`
 * 已经落地 —— 也就是「状态机在走、文本没了」。
 */
describe("StrictMode 下的流式落地", () => {
  it("StrictMode 双挂载之后，delta 仍必须写进 target", { timeout: 30_000 }, async () => {
    // 小规模夹具：这条用例只关心「delta 有没有落地」，不需要 2 万条的噪声
    backend.entries = buildEntries(40, FILE_NAME);

    await mounted.render(
      <StrictMode>
        <TranslationTable />
      </StrictMode>,
    );
    await waitMs(0);

    const translateButton = findButton(mounted.container, "翻译 ");
    expect(translateButton).not.toBeNull();
    await act(async () => {
      click(translateButton!);
    });
    await waitMs(0);
    expect(backend.onEvent).not.toBeNull();
    const send = backend.onEvent!;

    await act(async () => {
      send({ type: "progress", entryId: FIRST_ID, status: "translating" });
      send({ type: "delta", entryId: FIRST_ID, text: "半截流式文本" });
      await waitForFlush(FIRST_ID);
    });

    expect(entryTarget(FIRST_ID)).toBe("半截流式文本");
    expect(storedEntry(FIRST_ID).status).toBe("translating");

    // 收尾：done 的权威文本照常落地（这条路径不走批处理，用来证明
    // 「只有 delta 丢了」而不是整条流都断了）
    await act(async () => {
      send({ type: "done", entryId: FIRST_ID, text: "权威译文" });
      send({ type: "all_done", total: 1, failed: 0 });
      backend.finish?.();
      backend.finish = null;
      await Promise.resolve();
    });
    expect(storedEntry(FIRST_ID)).toMatchObject({
      target: "权威译文",
      status: "translated",
    });
  });
});

/**
 * 「已有译文」的条目会作为**上下文**一起发给后端（`buildTranslationRequest`
 * 正常分支发全部工作条目），core 用它建一致性记忆（本 MOD 已确定译名 / 系列
 * base 复用）。后端只翻 `is_pending_translation()` 的那批：上下文条目不会有
 * 任何事件，也不在 `plan.total` 里。
 *
 * 所以前端必须守住：上下文条目的 `target` / `status` 一个字节都不能被动过 ——
 * 既不能被标记成 `translating`（那是会被写进 PAK 的状态），也不能在成功收尾或
 * 取消收尾的回滚里被清空（清空 = 抹掉用户已有的译文，不可逆）。
 */
describe("已有译文作为上下文一起发（一致性记忆的输入）", () => {
  /** 一条已有译文（上下文条目）+ 一条待翻译 */
  function contextFixtures(): void {
    backend.entries = [
      smallEntry("e-ctx", { target: "已经翻好的译文", status: "translated" }),
      smallEntry("e-new"),
    ];
  }

  it("上下文条目随 payload 原样发出，且成功收尾不得改动它", async () => {
    contextFixtures();
    await renderTable();

    const button = findButton(mounted.container, "翻译 ");
    expect(button).not.toBeNull();
    // 只有 1 条要翻：上下文条目不算「可翻译」
    expect(button!.textContent).toContain("翻译 1 条");
    await act(async () => {
      click(button!);
    });
    await waitMs(0);
    expect(backend.onEvent).not.toBeNull();

    // payload 必须带上上下文条目（原样）—— 它是后端一致性记忆的唯一来源
    expect(backend.sent).toHaveLength(1);
    expect(backend.sent[0].map((e) => e.id)).toEqual(["e-ctx", "e-new"]);
    expect(backend.sent[0].find((e) => e.id === "e-ctx")).toMatchObject({
      target: "已经翻好的译文",
      status: "translated",
    });

    const send = backend.onEvent!;
    await act(async () => {
      send({ type: "progress", entryId: "e-new", status: "translating" });
      send({ type: "delta", entryId: "e-new", text: "新译文" });
      await waitForFlush("e-new");
    });
    expect(entryTarget("e-new")).toBe("新译文");
    // 上下文条目不得被标记 translating（只有后端真正在翻的条目才会）
    expect(storedEntry("e-ctx")).toMatchObject({
      target: "已经翻好的译文",
      status: "translated",
    });

    // 成功收尾（后端只对 e-new 发 done，all_done 的 total 也只算它）
    await act(async () => {
      send({ type: "done", entryId: "e-new", text: "新译文" });
      send({ type: "all_done", total: 1, failed: 0 });
      backend.finish?.();
      backend.finish = null;
      await Promise.resolve();
    });
    await waitMs(0);

    expect(storedEntry("e-new")).toMatchObject({ target: "新译文", status: "translated" });
    expect(storedEntry("e-ctx")).toMatchObject({
      target: "已经翻好的译文",
      status: "translated",
    });
    expect(mounted.container.textContent).toContain("已经翻好的译文");
  });

  it("取消收尾回滚只动本轮在跑的条目，不得清空上下文条目的译文", async () => {
    contextFixtures();
    await renderTable();

    await act(async () => {
      click(findButton(mounted.container, "翻译 ")!);
    });
    await waitMs(0);
    expect(backend.onEvent).not.toBeNull();
    const send = backend.onEvent!;

    // e-new 流到一半，用户取消
    await act(async () => {
      send({ type: "progress", entryId: "e-new", status: "translating" });
      send({ type: "delta", entryId: "e-new", text: "半截新译文" });
      await waitForFlush("e-new");
    });
    expect(entryTarget("e-new")).toBe("半截新译文");

    await act(async () => {
      click(findButton(mounted.container, "取消翻译")!);
    });
    await waitMs(0);

    // 后端收尾（取消也发 all_done），命令返回 → 回滚
    await act(async () => {
      send({ type: "all_done", total: 1, failed: 1 });
      backend.finish?.();
      backend.finish = null;
      await Promise.resolve();
    });
    await waitMs(0);

    // 在跑的那条回滚为待翻译
    expect(storedEntry("e-new")).toMatchObject({ target: "", status: "pending" });
    // 上下文条目纹丝不动（修复前这里会被回滚成 target="" / pending）
    expect(storedEntry("e-ctx")).toMatchObject({
      target: "已经翻好的译文",
      status: "translated",
    });
  });
});
