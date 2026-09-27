/**
 * 「打包页 → 返回继续编辑」这一轮往返的确定性用例。
 *
 * 路径：翻译完成 → 写回工作目录（stage=done）→ 点「返回继续编辑」回到工作台。
 *
 * 关注点：回到工作台时 `useEntryLoading` 会**重新挂载**。它一旦把已加载过的
 * 文件当成「没加载过」，就会重新 `read_file_entries` 并用磁盘内容整体替换
 * store 里的条目。而磁盘上那个文件是**英文原文**（写回产物在另一个路径
 * `Localization/Chinese/...`，不在 `files` 列表里）——于是：
 *   1. 工作台里刚翻译/手工编辑的结果全部显示回「待翻译」（进度归零）；
 *   2. 用户如果再点一次「完成翻译，去打包」，写回的 payload 就是**英文原文**，
 *      把上一次已经写好的中文文件整体覆盖掉（不可逆的数据损失）。
 *
 * 跑法：`bunx vitest run src/App.return-to-edit.test.tsx`
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

const FILE_NAME = "Localization/English/roundtrip.xml";

const backend = vi.hoisted(() => ({
  entries: [] as import("@/lib/types").TranslationEntry[],
  onEvent: null as null | ((event: import("@/lib/types").TranslationEvent) => void),
  finish: null as null | (() => void),
  readCount: 0,
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
  readFileEntries: vi.fn(async () => {
    backend.readCount += 1;
    // 磁盘视角：源文件（英文）没被改过，读回来永远是「待翻译 + 空译文」
    return backend.entries;
  }),
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

function entry(id: string): TranslationEntry {
  return {
    id,
    sourceFile: FILE_NAME,
    source: `Source ${id}`,
    target: "",
    contentuid: `uid-${id}`,
    version: "1",
    status: "pending",
    error: null,
  };
}

beforeEach(() => {
  restoreLayout = stubVirtualScrollLayout();
  backend.entries = [entry("r-0"), entry("r-1")];
  backend.onEvent = null;
  backend.finish = null;
  backend.readCount = 0;

  for (const fn of Object.values(tauri)) fn.mockReset();
  tauri.writeFileEntries.mockResolvedValue(undefined);
  tauri.loadLlmSettings.mockRejectedValue(new Error("no config"));
  tauri.closeMod.mockResolvedValue(undefined);

  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/roundtrip.pak", "/tmp/work", [pakFile(FILE_NAME)]);
  api.setSelectedFiles([pakFile(FILE_NAME)]);

  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
  restoreLayout();
});

async function renderApp(): Promise<void> {
  await mounted.render(<App />);
  await waitMs(0);
}

/** 跑完一轮翻译：两条都拿到权威译文 */
async function finishTranslation(): Promise<void> {
  await act(async () => {
    click(findButton(mounted.container, "翻译 ")!);
  });
  await waitMs(0);
  expect(backend.onEvent).not.toBeNull();
  await act(async () => {
    backend.onEvent!({ type: "done", entryId: "r-0", text: "译文零" });
    backend.onEvent!({ type: "done", entryId: "r-1", text: "译文一" });
    backend.onEvent!({ type: "all_done", total: 2, failed: 0 });
    backend.finish?.();
    backend.finish = null;
    await Promise.resolve();
  });
  await waitMs(0);
}

describe("打包页返回继续编辑", () => {
  it("返回工作台不得重读磁盘：已翻译结果必须还在，第二次写回不得退回英文原文", async () => {
    await renderApp();
    expect(backend.readCount).toBe(1);

    await finishTranslation();
    expect(useAppStore.getState().entriesByFile[FILE_NAME][0]).toMatchObject({
      target: "译文零",
      status: "translated",
    });

    // 第一次写回：把中文写到 Localization/Chinese/roundtrip.xml
    await act(async () => {
      click(findButton(mounted.container, "完成翻译，去打包")!);
    });
    await waitMs(0);
    expect(tauri.writeFileEntries).toHaveBeenCalledTimes(1);
    expect(useAppStore.getState().stage).toBe("done");

    // 回工作台继续编辑
    await act(async () => {
      click(findButton(mounted.container, "返回继续编辑")!);
    });
    await waitMs(0);
    expect(useAppStore.getState().stage).toBe("files");

    // 关键 1：已加载过的文件不能再读一遍（重读 = 用磁盘英文覆盖内存里的译文）
    expect(backend.readCount).toBe(1);
    expect(useAppStore.getState().entriesByFile[FILE_NAME][0]).toMatchObject({
      target: "译文零",
      status: "translated",
    });

    // 关键 2：再点一次打包，payload 必须仍是译文，而不是退回英文原文
    await act(async () => {
      click(findButton(mounted.container, "完成翻译，去打包")!);
    });
    await waitMs(0);
    expect(tauri.writeFileEntries).toHaveBeenCalledTimes(2);
    const second = tauri.writeFileEntries.mock.calls[1][2] as TranslationEntry[];
    expect(second.map((e) => [e.contentuid, e.target, e.status])).toEqual([
      ["uid-r-0", "译文零", "translated"],
      ["uid-r-1", "译文一", "translated"],
    ]);
  });
});
