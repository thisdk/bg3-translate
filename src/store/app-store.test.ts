import { beforeEach, describe, expect, it, vi } from "vitest";
import { DEFAULT_SETTINGS, THEME_STORAGE_KEY, useAppStore } from "./app-store";
import type { PakFile, TranslationEntry } from "@/lib/types";

function pakFile(name: string): PakFile {
  return { name, size: 1, kind: "localization-xml", language: "English" };
}

function entry(id: string, patch: Partial<TranslationEntry> = {}): TranslationEntry {
  return {
    id,
    sourceFile: "Localization/English/foo.xml",
    source: `source-${id}`,
    target: "",
    contentuid: `uid-${id}`,
    version: "1",
    status: "pending",
    error: null,
    ...patch,
  };
}

const state = () => useAppStore.getState();

beforeEach(() => {
  localStorage.clear();
  state().reset();
});

describe("app-store 初始状态", () => {
  it("默认设置为 deepseek + temperature 0.3，且没有 batchSize", () => {
    expect(DEFAULT_SETTINGS).toEqual({
      baseUrl: "https://api.deepseek.com",
      apiKey: "",
      model: "deepseek-chat",
      concurrency: 6,
      temperature: 0.3,
    });
    expect("batchSize" in DEFAULT_SETTINGS).toBe(false);
  });

  it("reset 后回到 home 阶段", () => {
    expect(state().stage).toBe("home");
    expect(state().entriesByFile).toEqual({});
    expect(state().selectedFiles).toEqual([]);
  });
});

describe("setModOpened", () => {
  it("记录 mod 路径 / 工作目录 / 文件列表并进入 files 阶段", () => {
    state().setModOpened("/tmp/a.pak", "/tmp/work", [pakFile("a.xml")]);
    expect(state().stage).toBe("files");
    expect(state().modFilePath).toBe("/tmp/a.pak");
    expect(state().workDir).toBe("/tmp/work");
    expect(state().files).toHaveLength(1);
    expect(state().selectedFiles).toEqual([]);
  });

  it("重新打开 MOD 会清空上一份条目缓存", () => {
    state().setModOpened("/tmp/a.pak", "/tmp/work", [pakFile("a.xml")]);
    state().setFileEntries("a.xml", [entry("1")]);
    state().setModOpened("/tmp/b.pak", "/tmp/work2", [pakFile("b.xml")]);
    expect(state().entriesByFile).toEqual({});
    expect(state().entryIdToFile).toEqual({});
    expect(state().loadedFileNames.size).toBe(0);
  });
});

describe("setFileEntries", () => {
  it("写入条目并建立 entryId → 文件名反查索引", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    expect(state().entriesByFile["a.xml"]).toHaveLength(2);
    expect(state().entryIdToFile["1"]).toBe("a.xml");
    expect(state().entryIdToFile["2"]).toBe("a.xml");
    expect(state().loadedFileNames.has("a.xml")).toBe(true);
  });

  it("重复写入同一文件会整体替换", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    state().setFileEntries("a.xml", [entry("3")]);
    expect(state().entriesByFile["a.xml"].map((e) => e.id)).toEqual(["3"]);
    // 旧索引仍指向该文件（不会误伤新数据），新索引可用
    expect(state().entryIdToFile["3"]).toBe("a.xml");
  });
});

describe("updateEntry", () => {
  beforeEach(() => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    state().setFileEntries("b.xml", [entry("3")]);
  });

  it("按 id 定位并合并 patch，不影响其它条目", () => {
    state().updateEntry("1", { target: "你好", status: "translated" });
    const list = state().entriesByFile["a.xml"];
    expect(list[0]).toMatchObject({ target: "你好", status: "translated" });
    expect(list[1]).toMatchObject({ target: "", status: "pending" });
  });

  it("跨文件按 id 更新命中正确文件", () => {
    state().updateEntry("3", { target: "三" });
    expect(state().entriesByFile["b.xml"][0].target).toBe("三");
    expect(state().entriesByFile["a.xml"][0].target).toBe("");
  });

  it("未知 id 不改变状态（引用保持不变）", () => {
    const before = state().entriesByFile;
    state().updateEntry("nope", { target: "x" });
    expect(state().entriesByFile).toBe(before);
  });

  it("更新时替换数组引用，保证 React 重渲染", () => {
    const before = state().entriesByFile["a.xml"];
    state().updateEntry("1", { target: "x" });
    expect(state().entriesByFile["a.xml"]).not.toBe(before);
  });
});

describe("setEntryStatus", () => {
  it("只改状态", () => {
    state().setFileEntries("a.xml", [entry("1", { target: "已有" })]);
    state().setEntryStatus("1", "translating");
    expect(state().entriesByFile["a.xml"][0]).toMatchObject({
      status: "translating",
      target: "已有",
    });
  });
});

describe("appendDelta", () => {
  it("追加流式文本并把状态改为 translating", () => {
    state().setFileEntries("a.xml", [entry("1")]);
    state().appendDelta("1", "你");
    state().appendDelta("1", "好");
    expect(state().entriesByFile["a.xml"][0]).toMatchObject({
      target: "你好",
      status: "translating",
    });
  });

  it("未知 id 是空操作", () => {
    state().setFileEntries("a.xml", [entry("1")]);
    state().appendDelta("ghost", "x");
    expect(state().entriesByFile["a.xml"][0].target).toBe("");
  });
});

describe("getAllEntries / getFileEntries", () => {
  it("只聚合被勾选的文件，顺序与 selectedFiles 一致", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    state().setFileEntries("b.xml", [entry("3")]);
    state().setSelectedFiles([pakFile("b.xml"), pakFile("a.xml")]);
    expect(state().getAllEntries().map((e) => e.id)).toEqual(["3", "1", "2"]);
  });

  it("未勾选任何文件时返回空数组", () => {
    state().setFileEntries("a.xml", [entry("1")]);
    expect(state().getAllEntries()).toEqual([]);
  });

  it("勾选但未加载的文件被忽略", () => {
    state().setFileEntries("a.xml", [entry("1")]);
    state().setSelectedFiles([pakFile("a.xml"), pakFile("missing.xml")]);
    expect(state().getAllEntries().map((e) => e.id)).toEqual(["1"]);
  });

  it("getFileEntries 对未知文件返回空数组", () => {
    expect(state().getFileEntries("nope.xml")).toEqual([]);
  });
});

describe("setSettings / setTheme", () => {
  it("setSettings 标记 settingsLoaded", () => {
    state().setSettings({ ...DEFAULT_SETTINGS, apiKey: "sk-1", temperature: 0.7 });
    expect(state().settings.apiKey).toBe("sk-1");
    expect(state().settings.temperature).toBe(0.7);
    expect(state().settingsLoaded).toBe(true);
  });

  it("setTheme 持久化到 localStorage 并给 <html> 加 class", () => {
    state().setTheme("cyberpunk");
    expect(localStorage.getItem(THEME_STORAGE_KEY)).toBe("cyberpunk");
    expect(document.documentElement.classList.contains("theme-cyberpunk")).toBe(true);
    expect(document.documentElement.classList.contains("dark")).toBe(true);
  });

  it("浅色主题不激活 dark class 且会清掉旧主题 class", () => {
    state().setTheme("dark");
    expect(document.documentElement.classList.contains("dark")).toBe(true);
    state().setTheme("light");
    expect(document.documentElement.classList.contains("theme-light")).toBe(true);
    expect(document.documentElement.classList.contains("theme-dark")).toBe(false);
    expect(document.documentElement.classList.contains("dark")).toBe(false);
  });

  it("四套主题都能正确落到 class 上", () => {
    for (const theme of ["light", "dark", "cyberpunk", "dungeon"] as const) {
      state().setTheme(theme);
      expect(document.documentElement.classList.contains(`theme-${theme}`)).toBe(true);
    }
  });
});

describe("setError / setLoading", () => {
  it("setError 可为空表示关闭提示", () => {
    state().setError("boom");
    expect(state().error).toBe("boom");
    state().setError(null);
    expect(state().error).toBeNull();
  });

  it("setLoading 切换加载态", () => {
    state().setLoading(true);
    expect(state().loading).toBe(true);
    state().setLoading(false);
    expect(state().loading).toBe(false);
  });
});

describe("reset", () => {
  it("清空所有流程数据但保留主题与设置", () => {
    state().setModOpened("/tmp/a.pak", "/tmp/work", [pakFile("a.xml")]);
    state().setSelectedFiles([pakFile("a.xml")]);
    state().setFileEntries("a.xml", [entry("1")]);
    state().setError("boom");
    state().setTheme("dungeon");
    state().setSettings({ ...DEFAULT_SETTINGS, apiKey: "sk-keep" });

    state().reset();

    expect(state().stage).toBe("home");
    expect(state().modFilePath).toBeNull();
    expect(state().workDir).toBeNull();
    expect(state().files).toEqual([]);
    expect(state().selectedFiles).toEqual([]);
    expect(state().entriesByFile).toEqual({});
    expect(state().entryIdToFile).toEqual({});
    expect(state().loadedFileNames.size).toBe(0);
    expect(state().error).toBeNull();
    // 主题与设置属于用户偏好，reset 不清空
    expect(state().theme).toBe("dungeon");
    expect(state().settings.apiKey).toBe("sk-keep");
  });
});

describe("entryIdToIndex（O(1) 定位索引）", () => {
  it("setFileEntries 按数组下标建立索引", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2"), entry("3")]);
    expect(state().entryIdToIndex).toEqual({ "1": 0, "2": 1, "3": 2 });
  });

  it("整表替换时清理旧下标，旧 id 不再命中", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    state().setFileEntries("a.xml", [entry("9")]);
    expect(state().entryIdToIndex).toEqual({ "9": 0 });
    expect(state().entryIdToFile).toEqual({ "9": "a.xml" });

    // 旧 id 已经不在表里：更新是空操作，不会写到别的条目上
    state().appendDelta("1", "污染");
    expect(state().entriesByFile["a.xml"][0]).toMatchObject({
      id: "9",
      target: "",
    });
  });

  it("索引错位时拒绝写入（宁可丢更新也不写错条目）", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    // 人为制造陈旧索引：让 "2" 指向下标 0（实际是 "1"）
    useAppStore.setState({ entryIdToIndex: { "1": 0, "2": 0 } });

    state().appendDelta("2", "污染");
    expect(state().entriesByFile["a.xml"][0].target).toBe("");
    expect(state().entriesByFile["a.xml"][1].target).toBe("");
  });

  it("updateEntry / appendDelta / applyDeltas 不再线性扫描数组", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2"), entry("3")]);
    const findIndex = vi.spyOn(Array.prototype, "findIndex");

    state().updateEntry("2", { target: "x" });
    state().appendDelta("1", "字");
    state().applyDeltas([
      { id: "1", text: "a" },
      { id: "3", text: "b" },
    ]);

    const calls = findIndex.mock.calls.length;
    expect(calls).toBe(0);
    expect(state().entriesByFile["a.xml"][0].target).toBe("字a");
    expect(state().entriesByFile["a.xml"][2].target).toBe("b");
  });
});

describe("applyDeltas（一帧一次提交）", () => {
  /** 统计 zustand 通知次数 */
  function countNotifications(): { count: () => number; stop: () => void } {
    let count = 0;
    const unsubscribe = useAppStore.subscribe(() => {
      count += 1;
    });
    return { count: () => count, stop: unsubscribe };
  }

  it("一批跨文件多条 delta 只发一次通知", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    state().setFileEntries("b.xml", [entry("3")]);
    const notifications = countNotifications();

    state().applyDeltas([
      { id: "1", text: "甲" },
      { id: "3", text: "丙" },
      { id: "2", text: "乙" },
    ]);

    notifications.stop();
    expect(notifications.count()).toBe(1);
    expect(state().entriesByFile["a.xml"][0].target).toBe("甲");
    expect(state().entriesByFile["a.xml"][1].target).toBe("乙");
    expect(state().entriesByFile["b.xml"][0].target).toBe("丙");
    // 同一条目在一批里出现多次 → 按出现顺序拼接
    expect(state().entriesByFile["a.xml"][0].status).toBe("translating");
  });

  it("同一条目在一批里多次出现按顺序拼接", () => {
    state().setFileEntries("a.xml", [entry("1")]);
    state().applyDeltas([
      { id: "1", text: "你" },
      { id: "1", text: "好" },
    ]);
    expect(state().entriesByFile["a.xml"][0].target).toBe("你好");
  });

  it("每个文件只替换一次数组引用，未命中的文件不动", () => {
    state().setFileEntries("a.xml", [entry("1"), entry("2")]);
    state().setFileEntries("b.xml", [entry("3")]);
    const aBefore = state().entriesByFile["a.xml"];
    const bBefore = state().entriesByFile["b.xml"];

    state().applyDeltas([
      { id: "1", text: "甲" },
      { id: "2", text: "乙" },
    ]);

    expect(state().entriesByFile["a.xml"]).not.toBe(aBefore);
    expect(state().entriesByFile["b.xml"]).toBe(bBefore);
  });

  it("未知 id 与空文本被忽略；整批无效时不发通知", () => {
    state().setFileEntries("a.xml", [entry("1")]);
    const notifications = countNotifications();

    state().applyDeltas([
      { id: "ghost", text: "幽灵" },
      { id: "1", text: "" },
    ]);
    state().applyDeltas([]);

    notifications.stop();
    expect(notifications.count()).toBe(0);
    expect(state().entriesByFile["a.xml"][0].target).toBe("");
  });

  it("appendDelta 等价于单元素 applyDeltas（一次通知）", () => {
    state().setFileEntries("a.xml", [entry("1")]);
    const notifications = countNotifications();
    state().appendDelta("1", "好");
    notifications.stop();
    expect(notifications.count()).toBe(1);
    expect(state().entriesByFile["a.xml"][0].target).toBe("好");
  });
});
