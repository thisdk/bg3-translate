/**
 * 迟到事件的确定性回归用例（无 sleep、无真实计时器依赖）。
 *
 * 背景（跨层写回不变量）：后端 `TranslationEntry::has_writable_target()` 只在
 * `status == "error"` 时退回原文，`status == "translating"` 的半截流式文本
 * **照样会被写进 PAK**（见 crates/bg3-translate-core/src/types.rs 的注释：
 * 「该路径由前端把关」）。所以前端必须保证：一轮翻译结束后，任何条目都不能
 * 停留在 `translating` + 非空 target。
 *
 * 真实 Tauri 里 Channel 消息的投递与命令 Promise 的 settle 不是同一个队列，
 * 后端发完最后一帧 delta / `all_done` 之后仍可能有消息迟到。这里用受控
 * Promise + 手动调用旧回调来复现，全部是同步可观测的断言。
 *
 * 跑法：`bunx vitest run src/components/translation-table/TranslationTable.late-events.test.tsx`
 */
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TranslationTable } from "@/components/TranslationTable";
import { useAppStore } from "@/store/app-store";
import { pakFile } from "@/test-utils/fixtures";
import {
  click,
  findButton,
  mountContainer,
  stubVirtualScrollLayout,
  waitMs,
  type Mounted,
} from "@/test-utils/dom";
import type { TranslationEntry, TranslationEvent } from "@/lib/types";

const FILE_NAME = "Localization/English/small.xml";

const backend = vi.hoisted(() => ({
  entries: [] as import("@/lib/types").TranslationEntry[],
  onEvent: null as null | ((event: import("@/lib/types").TranslationEvent) => void),
  /** 手动 resolve：模拟「翻译命令还没返回」 */
  finish: null as null | (() => void),
}));

vi.mock("@/lib/tauri", () => ({
  readFileEntries: vi.fn(async () => backend.entries),
  translateEntries: vi.fn(
    async (
      _workDir: string,
      _entries: TranslationEntry[],
      _styleHint: string,
      onEvent: (event: TranslationEvent) => void,
    ) => {
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
  backend.entries = [];
  backend.onEvent = null;
  backend.finish = null;

  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/small.pak", "/tmp/work", [pakFile(FILE_NAME)]);
  api.setSelectedFiles([pakFile(FILE_NAME)]);

  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
  restoreLayout();
});

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
  const list = useAppStore.getState().entriesByFile[FILE_NAME] ?? [];
  const entry = list.find((e) => e.id === id);
  expect(entry).toBeDefined();
  return entry!;
}

async function renderTable(): Promise<void> {
  await mounted.render(<TranslationTable />);
  await waitMs(0);
}

/** 点击「翻译 N 条」，返回这一轮的事件回调（受控 Promise 不会自动结束） */
async function startTranslation(): Promise<(event: TranslationEvent) => void> {
  const button = findButton(mounted.container, "翻译 ");
  expect(button).not.toBeNull();
  await act(async () => {
    click(button!);
  });
  await waitMs(0);
  expect(backend.onEvent).not.toBeNull();
  return backend.onEvent!;
}

/** 结束当前一轮（模拟后端命令返回），并 flush 微任务 */
async function finishRun(): Promise<void> {
  const finish = backend.finish;
  backend.finish = null;
  await act(async () => {
    finish?.();
    await Promise.resolve();
  });
}

describe("一轮结束后到达的迟到事件", () => {
  it("取消并收尾之后到达的 delta 不得复活半截译文（否则会被写回 PAK）", async () => {
    backend.entries = [smallEntry("e-0"), smallEntry("e-1")];
    await renderTable();
    const send = await startTranslation();

    // 流式进行中：先落一段 delta（进入 translating + 半截 target）
    await act(async () => {
      send({ type: "progress", entryId: "e-0", status: "translating" });
      send({ type: "delta", entryId: "e-0", text: "半截" });
      await Promise.resolve();
    });

    // 用户点取消 → 后端命令返回（收尾回滚已经发生）
    const cancelButton = findButton(mounted.container, "取消翻译");
    expect(cancelButton).not.toBeNull();
    await act(async () => {
      click(cancelButton!);
    });
    await finishRun();

    expect(storedEntry("e-0")).toMatchObject({ target: "", status: "pending" });

    // 命令返回之后，IPC 通道里仍有迟到的 delta / all_done
    await act(async () => {
      send({ type: "delta", entryId: "e-0", text: "幽灵半截译文" });
      // all_done 会同步 flush 批处理，把挂起的 delta 真正写进 store
      send({ type: "all_done", total: 2, failed: 1 });
      await Promise.resolve();
    });

    // 半截译文一旦落进 status=translating 的条目，打包时会被当真译文写进 PAK
    expect(storedEntry("e-0").target).toBe("");
    expect(storedEntry("e-0").status).toBe("pending");
  });

  it("正常结束之后到达的 progress/delta 不得把 pending 条目重新点着", async () => {
    backend.entries = [smallEntry("e-0"), smallEntry("e-1")];
    await renderTable();
    const send = await startTranslation();

    await act(async () => {
      send({ type: "done", entryId: "e-0", text: "权威译文" });
      send({ type: "done", entryId: "e-1", text: "权威译文 2" });
      send({ type: "all_done", total: 2, failed: 0 });
      await Promise.resolve();
    });
    await finishRun();

    expect(storedEntry("e-0")).toMatchObject({ target: "权威译文", status: "translated" });

    await act(async () => {
      send({ type: "progress", entryId: "e-0", status: "translating" });
      send({ type: "delta", entryId: "e-0", text: "幽灵尾巴" });
      await Promise.resolve();
    });

    expect(storedEntry("e-0")).toMatchObject({ target: "权威译文", status: "translated" });
  });

  it("上一轮的迟到 delta 不得污染新一轮里已完成的条目", async () => {
    backend.entries = [smallEntry("e-a"), smallEntry("e-b")];
    await renderTable();

    // 第一轮：e-a 拿到权威译文，e-b 仍在流式中
    const firstRun = await startTranslation();
    await act(async () => {
      firstRun({ type: "progress", entryId: "e-a", status: "translating" });
      firstRun({ type: "done", entryId: "e-a", text: "权威译文 A" });
      firstRun({ type: "all_done", total: 2, failed: 0 });
      await Promise.resolve();
    });
    await finishRun();
    expect(storedEntry("e-a")).toMatchObject({ target: "权威译文 A", status: "translated" });

    // 第二轮：只剩 e-b 待翻译（e-a 已完成，不应再被任何事件碰到）
    const secondRun = await startTranslation();
    expect(backend.onEvent).not.toBe(firstRun);

    await act(async () => {
      // 第一轮回调的迟到 delta（旧闭包）+ 第二轮 all_done 逼出批处理 flush
      firstRun({ type: "delta", entryId: "e-a", text: "（上一轮的尾巴）" });
      secondRun({ type: "all_done", total: 1, failed: 0 });
      await Promise.resolve();
    });

    expect(storedEntry("e-a")).toMatchObject({
      target: "权威译文 A",
      status: "translated",
    });
  });
});
