/**
 * 「翻译途中切换 MOD」的确定性用例。
 *
 * 场景：用户在工作台翻译还没结束时点「返回」回到首页（工作台卸载），
 * 随后重新打开 MOD。条目的 id 是 `{PAK 内路径}#{contentuid}`
 * （见 crates/bg3-translate-core/src/types.rs 的 `TranslationEntry::new`），
 * 同一个 MOD 重新打开后 id 完全一样 —— 于是**上一轮工作台的迟到事件与收尾
 * 回滚会命中新 MOD 里同名同 id 的条目**，把刚翻译好的结果改成 `translating`
 * 甚至清空。用例用受控 Promise 手动投递旧回调，不依赖 sleep。
 *
 * 跑法：`bunx vitest run src/components/translation-table/TranslationTable.mod-switch.test.tsx`
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

const FILE_NAME = "Localization/English/same.xml";

const backend = vi.hoisted(() => ({
  entries: [] as import("@/lib/types").TranslationEntry[],
  onEvent: null as null | ((event: import("@/lib/types").TranslationEvent) => void),
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

let restoreLayout: () => void;
const opened: Mounted[] = [];

beforeEach(() => {
  restoreLayout = stubVirtualScrollLayout();
  backend.entries = [
    {
      id: "same-0",
      sourceFile: FILE_NAME,
      source: "Source same-0",
      target: "",
      contentuid: "uid-same-0",
      version: "1",
      status: "pending",
      error: null,
    },
  ];
  backend.onEvent = null;
  backend.finish = null;
  useAppStore.getState().reset();
});

afterEach(() => {
  for (const view of opened) view.unmount();
  opened.length = 0;
  restoreLayout();
});

function openView(): Mounted {
  const view = mountContainer();
  opened.push(view);
  return view;
}

function storedEntry(id: string): TranslationEntry {
  const list = useAppStore.getState().entriesByFile[FILE_NAME] ?? [];
  const entry = list.find((e) => e.id === id);
  expect(entry).toBeDefined();
  return entry!;
}

/** 打开一个 MOD 并挂载工作台 */
async function openModAndRender(view: Mounted, pakPath: string, workDir: string) {
  const api = useAppStore.getState();
  api.setModOpened(pakPath, workDir, [pakFile(FILE_NAME)]);
  api.setSelectedFiles([pakFile(FILE_NAME)]);
  await view.render(<TranslationTable />);
  await waitMs(0);
}

/** 点击「翻译 N 条」，返回这一轮的事件回调与结束句柄 */
async function startTranslation(view: Mounted): Promise<{
  send: (event: TranslationEvent) => void;
  finish: () => void;
}> {
  const button = findButton(view.container, "翻译 ");
  expect(button).not.toBeNull();
  await act(async () => {
    click(button!);
  });
  await waitMs(0);
  expect(backend.onEvent).not.toBeNull();
  const finish = backend.finish;
  backend.finish = null;
  expect(finish).not.toBeNull();
  return { send: backend.onEvent!, finish: finish! };
}

describe("翻译途中切换 MOD", () => {
  it("旧工作台的迟到事件与收尾回滚不得改动新 MOD 的同 id 条目", async () => {
    // ── 第一份 MOD：翻译进行中 ──
    const first = openView();
    await openModAndRender(first, "/tmp/a.pak", "/tmp/work-a");
    const oldRun = await startTranslation(first);

    await act(async () => {
      oldRun.send({ type: "progress", entryId: "same-0", status: "translating" });
      oldRun.send({ type: "delta", entryId: "same-0", text: "旧 MOD 的半截文本" });
      await Promise.resolve();
    });

    // 用户点「返回」回首页：工作台卸载（翻译命令仍在后端跑）
    first.unmount();
    opened.pop();

    // ── 重新打开 MOD：条目 id 与上一份完全相同 ──
    const second = openView();
    await openModAndRender(second, "/tmp/a-again.pak", "/tmp/work-a2");
    expect(storedEntry("same-0")).toMatchObject({ target: "", status: "pending" });

    const newRun = await startTranslation(second);
    await act(async () => {
      newRun.send({ type: "progress", entryId: "same-0", status: "translating" });
      newRun.send({ type: "done", entryId: "same-0", text: "新 MOD 翻好的译文" });
      await Promise.resolve();
    });
    expect(storedEntry("same-0")).toMatchObject({
      target: "新 MOD 翻好的译文",
      status: "translated",
    });

    // 旧工作台的迟到 progress（旧闭包）不得把新条目重新点着
    await act(async () => {
      oldRun.send({ type: "progress", entryId: "same-0", status: "translating" });
      await Promise.resolve();
    });
    expect(storedEntry("same-0").status).toBe("translated");

    // 旧工作台这一轮收尾：未完成条目的回滚不得清空新 MOD 的条目
    await act(async () => {
      oldRun.finish();
      await Promise.resolve();
    });
    expect(storedEntry("same-0")).toMatchObject({
      target: "新 MOD 翻好的译文",
      status: "translated",
    });
    // 旧一轮的收尾也不得关掉新一轮刚打开的写回闸门
    expect(useAppStore.getState().runToken).not.toBeNull();

    // 收尾：结束新一轮
    await act(async () => {
      newRun.finish();
      await Promise.resolve();
    });
  });
});
