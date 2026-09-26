/**
 * R-05 回归用例：写回目标是「MOD 里已经存在的文件」时，未翻译条目必须保留
 * 文件里已有的中文，而不是退回英文原文把中文覆盖掉。
 *
 * 场景：MOD 同时带 `Localization/English/x.xml`（Fireball / Ice）与
 * `Localization/Chinese/x.xml`（火球 / 寒冰）。两者都映射到写回路径
 * `Localization/Chinese/x.xml`，按优先级英文胜出 —— 于是英文文件里**未翻译**
 * 的条目会以英文原文写回去，把已有的中文覆盖成英文（用户净损失）。
 * FileTree 还有一键「全选」，所以这条路非常好走。
 *
 * 全部用受控 mock（立即 resolve）+ 显式 flush，不依赖 sleep。
 *
 * 跑法：`bunx vitest run src/App.localization-merge.test.tsx`
 */
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import App from "@/App";
import { useAppStore } from "@/store/app-store";
import { click, findButton, mountContainer, waitMs, type Mounted } from "@/test-utils/dom";
import type { PakFile, TranslationEntry } from "@/lib/types";

const EN = "Localization/English/x.xml";
const ZH = "Localization/Chinese/x.xml";

const backend = vi.hoisted(() => ({
  /** 文件名 → 该文件当前的内容（读取与翻译都从这里取） */
  byFile: {} as Record<string, import("@/lib/types").TranslationEntry[]>,
}));

const tauri = vi.hoisted(() => ({
  writeFileEntries: vi.fn(),
  closeMod: vi.fn(),
  repackMod: vi.fn(),
  pickSavePath: vi.fn(),
  loadLlmSettings: vi.fn(),
  openMod: vi.fn(),
  extractMod: vi.fn(),
  pickModFile: vi.fn(),
  pickExtractDirectory: vi.fn(),
}));

vi.mock("@/lib/tauri", () => ({
  ...tauri,
  readFileEntries: vi.fn(async (_workDir: string, fileName: string) =>
    backend.byFile[fileName] ?? [],
  ),
  translateEntries: vi.fn(async () => undefined),
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
  getCurrentWebview: () => ({ onDragDropEvent: async () => () => undefined }),
}));

function pakFile(name: string, language: string): PakFile {
  return { name, size: 1, kind: "localization-xml", language };
}

/** 英文原文文件里的条目（未翻译） */
function sourceEntry(contentuid: string, text: string): TranslationEntry {
  return {
    id: `${EN}#${contentuid}`,
    sourceFile: EN,
    source: text,
    target: "",
    contentuid,
    version: "1",
    status: "pending",
    error: null,
  };
}

/** 已有的中文文件里的条目（读取时文本落在 source 上） */
function chineseEntry(contentuid: string, text: string): TranslationEntry {
  return {
    id: `${ZH}#${contentuid}`,
    sourceFile: ZH,
    source: text,
    target: "",
    contentuid,
    version: "1",
    status: "pending",
    error: null,
  };
}

let mounted: Mounted;

beforeEach(() => {
  for (const fn of Object.values(tauri)) fn.mockReset();
  tauri.writeFileEntries.mockResolvedValue(undefined);
  tauri.loadLlmSettings.mockRejectedValue(new Error("no config"));
  tauri.closeMod.mockResolvedValue(undefined);

  backend.byFile = {
    [EN]: [sourceEntry("uid-1", "Fireball"), sourceEntry("uid-2", "Ice")],
    [ZH]: [chineseEntry("uid-1", "火球"), chineseEntry("uid-2", "寒冰")],
  };

  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/zh.pak", "/tmp/work", [pakFile(EN, "English"), pakFile(ZH, "Chinese")]);

  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

async function renderApp(): Promise<void> {
  await mounted.render(<App />);
  await waitMs(0);
}

async function packNow(): Promise<void> {
  const packButton = findButton(mounted.container, "完成翻译，去打包");
  expect(packButton).not.toBeNull();
  await act(async () => {
    click(packButton!);
  });
  await waitMs(0);
}

/** 写回 payload：contentuid + 最终落盘的文本（有译文用译文，否则用原文） */
function writtenPayload(): [string, string, string][] {
  expect(tauri.writeFileEntries).toHaveBeenCalledTimes(1);
  const [, fileName, entries] = tauri.writeFileEntries.mock.calls[0] as [
    string,
    string,
    TranslationEntry[],
  ];
  expect(fileName).toBe(ZH);
  return entries.map((e) => [
    e.contentuid,
    e.target.trim() !== "" && e.status !== "error" ? e.target : e.source,
    e.status,
  ]);
}

describe("R-05 写回已存在的中文文件", () => {
  it("英文 + 中文都勾选：未翻译条目保留文件里已有的中文", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English"), pakFile(ZH, "Chinese")]);
    await renderApp();
    await packNow();

    // 修复前：["Fireball", "Ice"]（英文原文覆盖了已有的中文）
    expect(writtenPayload()).toEqual([
      ["uid-1", "火球", "pending"],
      ["uid-2", "寒冰", "pending"],
    ]);
  });

  it("只勾英文、MOD 自带中文：同样走合并（更常见的路径）", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();
    await packNow();

    expect(writtenPayload()).toEqual([
      ["uid-1", "火球", "pending"],
      ["uid-2", "寒冰", "pending"],
    ]);
  });

  it("已翻译的条目用新译文，未翻译的条目保留底稿中文", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();
    // 用户只补翻了第一条（act 里改，确保 React 已重渲染 —— 真实用户点击前
    // 组件一定已经拿到最新 store）
    await act(async () => {
      useAppStore.getState().updateEntry(`${EN}#uid-1`, {
        target: "火球术",
        status: "translated",
      });
    });
    await packNow();

    expect(writtenPayload()).toEqual([
      ["uid-1", "火球术", "translated"],
      ["uid-2", "寒冰", "pending"],
    ]);
  });

  it("error 条目不得把底稿中文覆盖掉（也不得写回被拒译文）", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();
    await act(async () => {
      useAppStore.getState().updateEntry(`${EN}#uid-1`, {
        target: "被拒的半截译文",
        status: "error",
        error: "结构校验未通过",
      });
    });
    await packNow();

    expect(writtenPayload()).toEqual([
      // error 条目：底稿中文保留，被拒译文不进 payload
      ["uid-1", "火球", "pending"],
      ["uid-2", "寒冰", "pending"],
    ]);
  });

  it("底稿有、英文文件里没有的 contentuid 要保留", async () => {
    backend.byFile[ZH] = [
      chineseEntry("uid-1", "火球"),
      chineseEntry("uid-2", "寒冰"),
      chineseEntry("uid-9", "只有中文文件才有的条目"),
    ];
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English"), pakFile(ZH, "Chinese")]);
    await renderApp();
    await packNow();

    expect(writtenPayload()).toEqual([
      ["uid-1", "火球", "pending"],
      ["uid-2", "寒冰", "pending"],
      ["uid-9", "只有中文文件才有的条目", "pending"],
    ]);
  });

  it("MOD 里不存在目标中文文件时行为不变（首次生成中文文件）", async () => {
    const api = useAppStore.getState();
    api.setModOpened("/tmp/en-only.pak", "/tmp/work2", [pakFile(EN, "English")]);
    api.setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();
    await packNow();

    // 没有底稿可合并：未翻译条目仍然写英文原文（与今天一致）
    expect(writtenPayload()).toEqual([
      ["uid-1", "Fireball", "pending"],
      ["uid-2", "Ice", "pending"],
    ]);
  });

  it("底稿读取失败不阻断写回（退回原行为，只留痕）", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();

    const { readFileEntries } = await import("@/lib/tauri");
    vi.mocked(readFileEntries).mockImplementation(
      async (_workDir: string, fileName: string) => {
        if (fileName === ZH) throw new Error("读取失败：文件被占用");
        return backend.byFile[fileName] ?? [];
      },
    );
    const warn = vi.spyOn(console, "warn").mockImplementation(() => undefined);

    try {
      await packNow();

      // 读取底稿失败 ≠ 打包失败：仍然写回，并用计划条目（英文原文）
      expect(writtenPayload()).toEqual([
        ["uid-1", "Fireball", "pending"],
        ["uid-2", "Ice", "pending"],
      ]);
      expect(useAppStore.getState().stage).toBe("done");
      expect(warn).toHaveBeenCalled();
    } finally {
      warn.mockRestore();
    }
  });
});
