/**
 * G1 高频路径基准（可复现，只做结构断言，不拿墙钟阈值卡 CI）。
 *
 * 场景与独立基线对齐：2 万条条目 + 3000 次流式 delta（每帧约 100 个 token、
 * 6 条并发流），每个 delta 都会触发 TranslationTable 的整条 memo 链重算
 * （allEntries 扁平化 → workEntries 过滤 → 统计 → 计数 → 过滤列表 → 可翻译数）。
 *
 * 四个报告指标（与 Lead 的独立基线同名，便于对齐）：
 *   - `store_ms`          累计花在 store 写入上的时间
 *   - `notifications`     zustand 通知次数（= 派生重算次数）
 *   - `derived_total_ms`  累计花在派生重算上的时间
 *   - `ms_per_delta`      store_ms / delta 数
 *
 * 结构断言：通知次数（每个 token 一次 vs 每帧一次）、逐帧通知数、文本完整性；
 * 耗时只 `console.log` 记录，用于报告，不参与断言（避免慢 CI 随机变红）。
 *
 * 跑法：`bunx vitest run src/store/streaming.perf.test.ts`
 */
import { describe, expect, it } from "vitest";
import { useAppStore } from "./app-store";
import { createDeltaBatcher } from "@/lib/delta-batcher";
import {
  computeEntryStats,
  countByFilter,
  filterEntries,
  isTranslationWorkItem,
  isTranslatableNow,
} from "@/lib/entries";
import { buildEntries, pakFile } from "@/test-utils/fixtures";
import type { TranslationEntry } from "@/lib/types";

const ENTRY_COUNT = 20_000;
const DELTA_COUNT = 3_000;
/** 模拟一帧内到达的 delta 数（60fps 下的典型流式速率） */
const DELTAS_PER_FRAME = 100;
const FRAME_COUNT = DELTA_COUNT / DELTAS_PER_FRAME;
/** 并发流式条目数（与默认 concurrency 一致） */
const STREAMING_ENTRIES = 6;
/** 预热条数（JIT 暖机，不计入报告数字） */
const WARMUP_COUNT = 300;
const TOKEN = "字";
const FILE_NAME = "Localization/English/big.xml";

const ENTRIES = buildEntries(ENTRY_COUNT, FILE_NAME);
/** 流式翻译时被并发更新的条目 id */
const HOT_IDS = ENTRIES.filter((e) => e.target === "")
  .slice(0, STREAMING_ENTRIES)
  .map((e) => e.id);

interface StreamMetrics {
  label: string;
  storeMs: number;
  notifications: number;
  derivedMs: number;
  perFrameNotifications: number[];
}

/** 可手动推进「帧」的调度器 */
function createFrameScheduler() {
  let queued: (() => void) | null = null;
  return {
    schedule: (run: () => void) => {
      queued = run;
      return () => {
        queued = null;
      };
    },
    runFrame: () => {
      const run = queued;
      queued = null;
      run?.();
    },
  };
}

/** TranslationTable 每次 store 提交后要重算的派生数据（5 个 useMemo 的等价物） */
function deriveTable(entries: TranslationEntry[]): number {
  const all = entries;
  const work = all.filter(isTranslationWorkItem);
  const stats = computeEntryStats(work);
  const counts = countByFilter(work);
  const visible = filterEntries(work, { query: "", filter: "all" });
  const translatable = work.filter(isTranslatableNow).length;
  return stats.total + stats.done + counts.all + visible.length + translatable;
}

function setupStore(): void {
  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/big.pak", "/tmp/work", [pakFile(FILE_NAME)]);
  api.setSelectedFiles([pakFile(FILE_NAME)]);
  api.setFileEntries(FILE_NAME, ENTRIES);
}

/** 统计 zustand 通知次数（一次 set = 一次通知 = 一轮订阅者重算） */
function countNotifications(): { count: () => number; stop: () => void } {
  let count = 0;
  const unsubscribe = useAppStore.subscribe(() => {
    count += 1;
  });
  return { count: () => count, stop: unsubscribe };
}

/** 每个 token 提交一次 store（优化前行为）；deltaCount 用于预热 */
function runDirect(withDerive: boolean, deltaCount = DELTA_COUNT): StreamMetrics {
  setupStore();
  const notifications = countNotifications();
  let storeMs = 0;
  let derivedMs = 0;

  for (let i = 0; i < deltaCount; i += 1) {
    const id = HOT_IDS[i % HOT_IDS.length];
    const started = performance.now();
    useAppStore.getState().appendDelta(id, TOKEN);
    storeMs += performance.now() - started;

    if (withDerive) {
      const derivedStarted = performance.now();
      deriveTable(useAppStore.getState().entriesByFile[FILE_NAME]);
      derivedMs += performance.now() - derivedStarted;
    }
  }

  const count = notifications.count();
  notifications.stop();
  return {
    label: withDerive ? "direct(+derive)" : "direct(store only)",
    storeMs,
    notifications: count,
    derivedMs,
    perFrameNotifications: [],
  };
}

/**
 * 按帧合并提交（优化后行为）：走真实的 `createDeltaBatcher`，
 * 提交回调走 store 的批量动作 `applyDeltas`（整批只发一次通知）。
 */
function runBatched(withDerive: boolean): StreamMetrics {
  setupStore();
  const notifications = countNotifications();
  const scheduler = createFrameScheduler();
  let storeMs = 0;
  let derivedMs = 0;
  let seen = 0;
  const perFrameNotifications: number[] = [];

  const batcher = createDeltaBatcher({
    commit: (batch) => {
      const started = performance.now();
      useAppStore.getState().applyDeltas(batch);
      storeMs += performance.now() - started;
    },
    schedule: scheduler.schedule,
  });

  for (let frame = 0; frame < FRAME_COUNT; frame += 1) {
    for (let i = 0; i < DELTAS_PER_FRAME; i += 1) {
      const id = HOT_IDS[(frame * DELTAS_PER_FRAME + i) % HOT_IDS.length];
      batcher.push(id, TOKEN);
    }
    scheduler.runFrame();
    const now = notifications.count();
    perFrameNotifications.push(now - seen);
    seen = now;

    if (withDerive) {
      const derivedStarted = performance.now();
      deriveTable(useAppStore.getState().entriesByFile[FILE_NAME]);
      derivedMs += performance.now() - derivedStarted;
    }
  }
  batcher.dispose();

  return {
    label: withDerive ? "batched(+derive)" : "batched(store only)",
    storeMs,
    notifications: notifications.count(),
    derivedMs,
    perFrameNotifications,
  };
}

/** 需求 1 的隔离对比：同样是「整表替换 + 元素替换」，只差定位方式 */
function commitByScan(list: TranslationEntry[], id: string): TranslationEntry[] {
  const index = list.findIndex((e) => e.id === id);
  const updated = [...list];
  const hit = updated[index];
  updated[index] = { ...hit, target: hit.target + TOKEN, status: "translating" };
  return updated;
}

function commitByIndex(
  list: TranslationEntry[],
  positions: Record<string, number>,
  id: string,
): TranslationEntry[] {
  const index = positions[id];
  const updated = list.slice();
  const hit = updated[index];
  updated[index] = { ...hit, target: hit.target + TOKEN, status: "translating" };
  return updated;
}

function measureLocate(byIndex: boolean): number {
  const positions: Record<string, number> = {};
  ENTRIES.forEach((entry, i) => {
    positions[entry.id] = i;
  });
  let list = ENTRIES;
  const started = performance.now();
  for (let i = 0; i < DELTA_COUNT; i += 1) {
    const id = HOT_IDS[i % HOT_IDS.length];
    list = byIndex ? commitByIndex(list, positions, id) : commitByScan(list, id);
  }
  const ms = performance.now() - started;
  if (list.length !== ENTRY_COUNT) throw new Error("unexpected list length");
  return ms;
}

/** 批处理路径的期望文本：按 push 顺序逐条拼接 */
function expectedTexts(): Map<string, string> {
  const expected = new Map<string, string>();
  for (let i = 0; i < DELTA_COUNT; i += 1) {
    const id = HOT_IDS[i % HOT_IDS.length];
    expected.set(id, (expected.get(id) ?? "") + TOKEN);
  }
  return expected;
}

describe("流式翻译高频路径基准（2 万条 + 3000 delta）", () => {
  it("批处理只按帧通知，文本零丢失（数字见 console.log）", { timeout: 300_000 }, () => {
    // 预热 JIT（结果不计入报告）
    runDirect(false, WARMUP_COUNT);
    runBatched(false);

    const directStore = runDirect(false);
    const directPipeline = runDirect(true);
    const batchedStore = runBatched(false);
    const batchedPipeline = runBatched(true);

    const rows = [directStore, directPipeline, batchedStore, batchedPipeline];
    for (const row of rows) {
      console.log(
        `[perf] ${row.label}: store_ms=${row.storeMs.toFixed(1)} ` +
          `notifications=${row.notifications} ` +
          `derived_total_ms=${row.derivedMs.toFixed(1)} ` +
          `ms_per_delta=${(row.storeMs / DELTA_COUNT).toFixed(3)}`,
      );
    }
    console.log(
      `[perf] SUMMARY direct store_ms=${directStore.storeMs.toFixed(1)} ` +
        `notifications=${directStore.notifications} ` +
        `derived_total_ms=${directPipeline.derivedMs.toFixed(1)} | ` +
        `batched store_ms=${batchedStore.storeMs.toFixed(1)} ` +
        `notifications=${batchedStore.notifications} ` +
        `derived_total_ms=${batchedPipeline.derivedMs.toFixed(1)} ` +
        `store_speedup=${(directStore.storeMs / Math.max(batchedStore.storeMs, 0.001)).toFixed(1)}x ` +
        `derived_speedup=${(directPipeline.derivedMs / Math.max(batchedPipeline.derivedMs, 0.001)).toFixed(1)}x`,
    );

    // 定位方式对比（纯数据结构，时间只记录不断言）
    const locateScanMs = Math.min(measureLocate(false), measureLocate(false));
    const locateIndexMs = Math.min(measureLocate(true), measureLocate(true));
    console.log(
      `[perf] locate: scan=${locateScanMs.toFixed(1)}ms ` +
        `index=${locateIndexMs.toFixed(1)}ms ` +
        `per-commit scan=${((locateScanMs / DELTA_COUNT) * 1000).toFixed(1)}us ` +
        `index=${((locateIndexMs / DELTA_COUNT) * 1000).toFixed(1)}us`,
    );

    // ── 结构断言 ─────────────────────────────────────────────
    // 1) 每个 token 提交一次 → 3000 次通知
    expect(directStore.notifications).toBe(DELTA_COUNT);
    // 2) 每帧只提交一次 → 通知次数 = 帧数（与并发条目数无关）
    expect(batchedStore.notifications).toBe(FRAME_COUNT);
    expect(batchedStore.perFrameNotifications).toEqual(
      Array.from({ length: FRAME_COUNT }, () => 1),
    );
    // 3) 批处理至少少两个数量级的通知
    expect(batchedStore.notifications * 100).toBeLessThanOrEqual(
      directStore.notifications,
    );

    // 4) 文本完整性：逐条核对合并结果（不丢字、不乱序）
    setupStore();
    const lastRun = runBatched(false);
    expect(lastRun.notifications).toBe(FRAME_COUNT);
    const store = useAppStore.getState();
    for (const [id, text] of expectedTexts()) {
      expect(store.entriesByFile[FILE_NAME].find((e) => e.id === id)!.target).toBe(
        text,
      );
    }
    // 每个条目都被更新过，且总的写入字符数 = delta 数 × token 长度
    const written = [...expectedTexts().keys()].map(
      (id) => store.entriesByFile[FILE_NAME].find((e) => e.id === id)!.target.length,
    );
    expect(written.reduce((n, len) => n + len, 0)).toBe(
      DELTA_COUNT * TOKEN.length,
    );
    expect(written.every((len) => len > 0)).toBe(true);
  });
});
