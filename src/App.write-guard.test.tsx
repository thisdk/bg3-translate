/**
 * 写回闸门的确定性用例：翻译还在进行中时不允许把条目写回工作目录。
 *
 * 跨层不变量（见 docs/ARCHITECTURE.md「写回不变量」与
 * crates/bg3-translate-core/src/types.rs 的 `has_writable_target`）：
 * 后端只在 `status === "error"` 时退回原文，`status === "translating"` 的
 * **半截流式文本会被当成真译文写进 PAK**。因此「写回时不能有未收尾的条目」
 * 这条只能由前端保证 —— 本文件从用户行为出发验证它。
 *
 * 全部用例都用受控 Promise（`backend.finish`）驱动，不依赖 sleep：
 * 用某个条目的 `progress` 事件同步 flush 批处理，把半截文本真正落到 store。
 *
 * 跑法：`bunx vitest run src/App.write-guard.test.tsx`
 */
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
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

const FILE_NAME = "Localization/English/guard.xml";

const backend = vi.hoisted(() => ({
  entries: [] as import("@/lib/types").TranslationEntry[],
  onEvent: null as null | ((event: import("@/lib/types").TranslationEvent) => void),
  /** 手动 resolve：模拟「翻译命令还没返回」 */
  finish: null as null | (() => void),
}));

const tauri = vi.hoisted(() => ({
  writeFileEntries: vi.fn(),
  closeMod: vi.fn(),
  repackMod: vi.fn(),
  pickSavePath: vi.fn(),
  loadLlmSettings: vi.fn(),
  saveLlmSettings: vi.fn(),
  openMod: vi.fn(),
  extractMod: vi.fn(),
  pickModFile: vi.fn(),
  pickExtractDirectory: vi.fn(),
}));

vi.mock("@/lib/tauri", () => ({
  ...tauri,
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
  getAppInfo: vi.fn(),
  listGlossary: vi.fn(),
  addGlossaryEntry: vi.fn(),
  updateGlossaryEntry: vi.fn(),
  deleteGlossaryEntry: vi.fn(),
  resetGlossary: vi.fn(),
  importGlossary: vi.fn(),
}));

// Tauri 运行时在 jsdom 里不存在：顶层窗口 API 与 webview 事件都要替换掉
vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => ({
    minimize: vi.fn(),
    toggleMaximize: vi.fn(),
    close: vi.fn(),
    startDragging: vi.fn(),
  }),
}));
vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async () => () => undefined,
  }),
}));

let mounted: Mounted;
let restoreLayout: () => void;

beforeEach(() => {
  restoreLayout = stubVirtualScrollLayout();
  backend.entries = [
    {
      id: "g-0",
      sourceFile: FILE_NAME,
      source: "Source g-0",
      target: "",
      contentuid: "uid-g-0",
      version: "1",
      status: "pending",
      error: null,
    },
    {
      id: "g-1",
      sourceFile: FILE_NAME,
      source: "Source g-1",
      target: "",
      contentuid: "uid-g-1",
      version: "1",
      status: "pending",
      error: null,
    },
  ];
  backend.onEvent = null;
  backend.finish = null;

  for (const fn of Object.values(tauri)) fn.mockReset();
  tauri.writeFileEntries.mockResolvedValue(undefined);
  tauri.loadLlmSettings.mockRejectedValue(new Error("no config"));
  tauri.closeMod.mockResolvedValue(undefined);

  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/guard.pak", "/tmp/work", [pakFile(FILE_NAME)]);
  api.setSelectedFiles([pakFile(FILE_NAME)]);

  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
  restoreLayout();
});

function storedEntry(id: string): TranslationEntry {
  const list = useAppStore.getState().entriesByFile[FILE_NAME] ?? [];
  const entry = list.find((e) => e.id === id);
  expect(entry).toBeDefined();
  return entry!;
}

async function renderApp(): Promise<void> {
  await mounted.render(<App />);
  await waitMs(0);
}

/** 开始翻译并让 g-0 变成「translating + 半截文本」，命令仍挂起 */
async function startAndStream(): Promise<void> {
  const translateButton = findButton(mounted.container, "翻译 ");
  expect(translateButton).not.toBeNull();
  await act(async () => {
    click(translateButton!);
  });
  await waitMs(0);
  expect(backend.onEvent).not.toBeNull();

  const send = backend.onEvent!;
  await act(async () => {
    send({ type: "progress", entryId: "g-0", status: "translating" });
    send({ type: "delta", entryId: "g-0", text: "半截译文" });
    // g-1 的 progress 会同步 flush 批处理，把半截文本真正写进 store
    send({ type: "progress", entryId: "g-1", status: "translating" });
    await Promise.resolve();
  });
  expect(storedEntry("g-0")).toMatchObject({ target: "半截译文", status: "translating" });
}

describe("写回闸门（翻译进行中不得写回）", () => {
  it("翻译仍在进行时点「完成翻译，去打包」不得写回半截译文", async () => {
    await renderApp();
    await startAndStream();

    const packButton = findButton(mounted.container, "完成翻译，去打包");
    expect(packButton).not.toBeNull();
    expect(packButton!.disabled).toBe(false);
    await act(async () => {
      click(packButton!);
    });
    await waitMs(0);

    // 半截译文是 translating 状态，后端会把它当成真译文写进 PAK
    expect(tauri.writeFileEntries).not.toHaveBeenCalled();
    expect(useAppStore.getState().stage).toBe("files");
    expect(mounted.container.textContent).toContain("翻译仍在进行中");
  });

  it("翻译结束后可以正常写回（闸门不能挡住合法流程）", async () => {
    await renderApp();
    await startAndStream();

    // 后端收尾：权威译文 + all_done + 命令返回
    const send = backend.onEvent!;
    await act(async () => {
      send({ type: "done", entryId: "g-0", text: "完整译文" });
      send({ type: "done", entryId: "g-1", text: "完整译文 2" });
      send({ type: "all_done", total: 2, failed: 0 });
      backend.finish?.();
      await Promise.resolve();
    });
    await waitMs(0);

    const packButton = findButton(mounted.container, "完成翻译，去打包");
    await act(async () => {
      click(packButton!);
    });
    await waitMs(0);

    expect(tauri.writeFileEntries).toHaveBeenCalledTimes(1);
    const [workDir, fileName, entries] = tauri.writeFileEntries.mock.calls[0] as [
      string,
      string,
      TranslationEntry[],
    ];
    expect(workDir).toBe("/tmp/work");
    expect(fileName).toBe("Localization/Chinese/guard.xml");
    expect(entries.map((e) => [e.contentuid, e.target, e.status])).toEqual([
      ["uid-g-0", "完整译文", "translated"],
      ["uid-g-1", "完整译文 2", "translated"],
    ]);
    expect(useAppStore.getState().stage).toBe("done");
  });

  it("取消后 all_done 已到但命令还没返回：打包仍被拦住，返回后条目全部回滚", async () => {
    await renderApp();
    await startAndStream();

    const cancelButton = findButton(mounted.container, "取消翻译");
    expect(cancelButton).not.toBeNull();
    await act(async () => {
      click(cancelButton!);
    });
    await waitMs(0);

    // 后端发完 all_done（取消也发），但 translate_entries 命令还没返回
    const send = backend.onEvent!;
    await act(async () => {
      send({ type: "all_done", total: 2, failed: 1 });
      await Promise.resolve();
    });
    expect(storedEntry("g-0")).toMatchObject({
      target: "半截译文",
      status: "translating",
    });

    // 这个窗口里打包 = 把半截译文写进 PAK，必须被拦住
    await act(async () => {
      click(findButton(mounted.container, "完成翻译，去打包")!);
    });
    await waitMs(0);
    expect(tauri.writeFileEntries).not.toHaveBeenCalled();
    expect(useAppStore.getState().stage).toBe("files");

    // 命令返回 → 收尾回滚：未完成条目回到 pending + 空 target
    await act(async () => {
      backend.finish?.();
      backend.finish = null;
      await Promise.resolve();
    });
    expect(storedEntry("g-0")).toMatchObject({ target: "", status: "pending" });
    expect(storedEntry("g-1")).toMatchObject({ target: "", status: "pending" });
    expect(useAppStore.getState().runToken).toBeNull();

    // 回滚之后写回：translating 一律退回原文（target 为空 → 后端保留原文）
    await act(async () => {
      click(findButton(mounted.container, "完成翻译，去打包")!);
    });
    await waitMs(0);
    expect(tauri.writeFileEntries).toHaveBeenCalledTimes(1);
    const entries = tauri.writeFileEntries.mock.calls[0][2] as TranslationEntry[];
    expect(entries.map((e) => [e.contentuid, e.target, e.status])).toEqual([
      ["uid-g-0", "", "pending"],
      ["uid-g-1", "", "pending"],
    ]);
    expect(useAppStore.getState().stage).toBe("done");
  });

  it("error 条目：打包时 status 必须保持 error（后端据此退回原文，不写坏 PAK）", async () => {
    await renderApp();
    await startAndStream();

    // 第一轮失败：target 里留着被拒的半截文本，status = error
    const send = backend.onEvent!;
    await act(async () => {
      send({
        type: "error",
        entryId: "g-0",
        message: "结构校验未通过：占位符 {1} 缺失",
      });
      send({ type: "done", entryId: "g-1", text: "完整译文 2" });
      send({ type: "all_done", total: 2, failed: 1 });
      backend.finish?.();
      backend.finish = null;
      await Promise.resolve();
    });
    await waitMs(0);
    expect(storedEntry("g-0")).toMatchObject({
      target: "半截译文",
      status: "error",
    });

    await act(async () => {
      click(findButton(mounted.container, "完成翻译，去打包")!);
    });
    await waitMs(0);

    expect(tauri.writeFileEntries).toHaveBeenCalledTimes(1);
    const entries = tauri.writeFileEntries.mock.calls[0][2] as TranslationEntry[];
    expect(entries.map((e) => [e.contentuid, e.status, e.target])).toEqual([
      // 关键：status 仍是 error，后端 effective_text() 会退回原文
      ["uid-g-0", "error", "半截译文"],
      ["uid-g-1", "translated", "完整译文 2"],
    ]);
  });
});
