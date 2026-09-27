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

/** 指定文件名的条目（大小写变体用例用） */
function entryIn(
  fileName: string,
  contentuid: string,
  text: string,
): TranslationEntry {
  return {
    id: `${fileName}#${contentuid}`,
    sourceFile: fileName,
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
  return payloadOfCall(0);
}

/** 第 n 次写回的 payload（contentuid + 最终落盘文本 + 状态） */
function payloadOfCall(index: number): [string, string, string][] {
  const [, fileName, entries] = tauri.writeFileEntries.mock.calls[index] as [
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

  it("MOD 自带的目录大小写不同（Localization/CHINESE）时同样走合并", async () => {
    const UPPER_ZH = "Localization/CHINESE/x.xml";
    backend.byFile = {
      [EN]: [sourceEntry("uid-1", "Fireball"), sourceEntry("uid-2", "Ice")],
      [UPPER_ZH]: [
        entryIn(UPPER_ZH, "uid-1", "火球"),
        entryIn(UPPER_ZH, "uid-2", "寒冰"),
      ],
    };
    const api = useAppStore.getState();
    api.setModOpened("/tmp/upper.pak", "/tmp/work3", [
      pakFile(EN, "English"),
      pakFile(UPPER_ZH, "Chinese"),
    ]);
    api.setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();
    await packNow();

    // 逐字节比较 `shippedFiles.has(plan.fileName)` 会判定落空 → 不读底稿 →
    // payload 退化成英文原文；在大小写不敏感的文件系统（Windows/macOS）上
    // 这就是把 MOD 自带的中文覆盖成英文。
    expect(writtenPayload()).toEqual([
      ["uid-1", "火球", "pending"],
      ["uid-2", "寒冰", "pending"],
    ]);
  });

  it("同名文件以两种大小写同时存在时，底稿取精确匹配的那个", async () => {
    const UPPER_ZH = "Localization/CHINESE/x.xml";
    backend.byFile = {
      [EN]: [sourceEntry("uid-1", "Fireball")],
      [ZH]: [entryIn(ZH, "uid-1", "精确匹配的中文")],
      [UPPER_ZH]: [entryIn(UPPER_ZH, "uid-1", "大写目录里的中文")],
    };
    const api = useAppStore.getState();
    api.setModOpened("/tmp/both.pak", "/tmp/work4", [
      pakFile(EN, "English"),
      pakFile(UPPER_ZH, "Chinese"),
      pakFile(ZH, "Chinese"),
    ]);
    api.setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();
    await packNow();

    expect(writtenPayload()).toEqual([["uid-1", "精确匹配的中文", "pending"]]);
  });

  it("MOD 没列出目标文件时不去读它（不把用户新建/上次写回的文件当成自带底稿）", async () => {
    // MOD 只有英文；`Localization/Chinese/x.xml` 是上一次写回在工作目录里造出来的，
    // 它不在 open_mod 的文件列表里 —— 判定必须只认文件列表，不能凭「磁盘上存在」
    // 就把它当底稿读进来（否则会把别的文件的内容合并进写回 payload）。
    const api = useAppStore.getState();
    api.setModOpened("/tmp/en-only2.pak", "/tmp/work5", [pakFile(EN, "English")]);
    api.setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();

    const { readFileEntries } = await import("@/lib/tauri");
    vi.mocked(readFileEntries).mockClear();
    await packNow();

    expect(
      vi.mocked(readFileEntries).mock.calls.map((call) => String(call[1])),
    ).not.toContain(ZH);
    expect(writtenPayload()).toEqual([
      ["uid-1", "Fireball", "pending"],
      ["uid-2", "Ice", "pending"],
    ]);
  });

  it("第二次写回必须用打开 MOD 时的底稿：上一次写回的结果不得把「还原」掉的译文复活", async () => {
    // 真实磁盘行为：写回会把工作目录里的目标文件改成我们的产物，
    // 下一次再读它读到的就是产物本身（读取时文本落在 source 上）。
    tauri.writeFileEntries.mockImplementation(
      async (_workDir: string, fileName: string, entries: TranslationEntry[]) => {
        backend.byFile[fileName] = entries.map((e) => ({
          ...e,
          source: e.target.trim() !== "" && e.status !== "error" ? e.target : e.source,
          target: "",
          status: "pending" as const,
        }));
      },
    );
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();

    // 第一条翻出译文并写回
    await act(async () => {
      useAppStore.getState().updateEntry(`${EN}#uid-1`, {
        target: "火球术",
        status: "translated",
      });
    });
    await packNow();
    expect(payloadOfCall(0)).toEqual([
      ["uid-1", "火球术", "translated"],
      ["uid-2", "寒冰", "pending"],
    ]);

    // 回工作台继续编辑，用户把这条「还原」掉（不想用这个译法）
    await act(async () => {
      click(findButton(mounted.container, "返回继续编辑")!);
    });
    await waitMs(0);
    await act(async () => {
      useAppStore.getState().updateEntry(`${EN}#uid-1`, {
        target: "",
        status: "pending",
      });
    });
    await packNow();

    // 还原的条目必须回到「打开 MOD 时」的底稿（官方中文「火球」），
    // 而不是上一次写回留下的模型译文「火球术」——那是自己合并自己。
    expect(payloadOfCall(1)).toEqual([
      ["uid-1", "火球", "pending"],
      ["uid-2", "寒冰", "pending"],
    ]);
  });

  it("底稿读取失败必须中止写回（否则已有的中文会被英文原文整体覆盖）", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(EN, "English")]);
    await renderApp();

    const { readFileEntries } = await import("@/lib/tauri");
    vi.mocked(readFileEntries).mockImplementation(
      async (_workDir: string, fileName: string) => {
        if (fileName === ZH) throw new Error("读取失败：文件被占用");
        return backend.byFile[fileName] ?? [];
      },
    );

    await packNow();

    // 读不到底稿 = 合并无法进行。此时「退回计划条目继续写回」等于用英文原文
    // 整体覆盖这个文件：未翻译条目退回原文、底稿独有的 contentuid 丢失 ——
    // 正是 R-05 要防的那次数据损失，所以必须中止并把原因交给用户。
    expect(tauri.writeFileEntries).not.toHaveBeenCalled();
    expect(useAppStore.getState().stage).toBe("files");
    const banner = mounted.container.textContent ?? "";
    expect(banner).toContain("读取失败：文件被占用");
    expect(banner).toContain(ZH);
  });
});
