import { create } from "zustand";
import type {
  LlmSettings,
  PakFile,
  TranslationEntry,
  TranslationStatus,
} from "@/lib/types";

/** 应用整体流程阶段 */
export type AppStage = "home" | "files" | "done";

/** 主题 */
export type Theme = "light" | "dark" | "cyberpunk" | "dungeon";

export const THEMES: { value: Theme; label: string; desc: string }[] = [
  { value: "light", label: "浅色", desc: "明亮简洁" },
  { value: "dark", label: "深色", desc: "护眼夜间" },
  { value: "cyberpunk", label: "赛博朋克", desc: "黄青高对比" },
  { value: "dungeon", label: "地下城", desc: "羊皮纸烛火" },
];

/** 按文件名缓存条目，key = PakFile.name */
type EntriesByFile = Record<string, TranslationEntry[]>;

interface AppState {
  // ── 流程状态 ──
  stage: AppStage;
  /** 原始 MOD 文件路径 */
  modFilePath: string | null;
  /** 工作目录（后端返回） */
  workDir: string | null;

  // ── 文件列表 ──
  files: PakFile[];
  /** 多选的文件列表 */
  selectedFiles: PakFile[];
  /** 已加载条目的文件名集合（避免重复加载） */
  loadedFileNames: Set<string>;

  // ── 翻译条目（按文件缓存）──
  entriesByFile: EntriesByFile;
  /** entryId → fileName 反查索引，加速 delta 高频更新 */
  entryIdToFile: Record<string, string>;
  /**
   * entryId → 在该文件数组里的下标。
   *
   * 与 `entryIdToFile` 一起构成「文件 + 下标」定位，使 updateEntry /
   * appendDelta 从 O(N) 线性扫描降为 O(1)。条目数组只做整体替换或
   * 单元素替换（从不在中间插入/删除），所以下标保持稳定；
   * `setFileEntries` 整体替换某个文件时会重建该文件的索引。
   */
  entryIdToIndex: Record<string, number>;
  // ── LLM 设置 ──
  settings: LlmSettings;
  settingsLoaded: boolean;

  // ── 主题 ──
  theme: Theme;

  // ── 加载态 ──
  loading: boolean;
  error: string | null;
  /**
   * 当前活跃的翻译轮次 token（`null` = 没有翻译在跑）。
   *
   * 写回（打包）必须看它：后端只在 `status === "error"` 时退回原文，处于
   * `translating` 的半截流式文本会被当成真译文写进 PAK。
   *
   * 之所以用 token 而不是 boolean：只有**登记它的那一轮**才能注销它。否则
   * 「旧工作台的收尾」会把「新一轮刚打开的闸门」误关掉 —— 那正好又是半截
   * 译文被写回 PAK 的入口（旧一轮与新一纶的条目 id 可以完全相同）。
   */
  runToken: number | null;

  // ── Actions ──
  setStage: (stage: AppStage) => void;
  setModOpened: (filePath: string, workDir: string, files: PakFile[]) => void;
  /** 多选变化 */
  setSelectedFiles: (files: PakFile[]) => void;
  /** 设置某个文件的条目（加载后写入） */
  setFileEntries: (fileName: string, entries: TranslationEntry[]) => void;
  /** 更新单条（按 entry.id 定位） */
  updateEntry: (id: string, patch: Partial<TranslationEntry>) => void;
  setEntryStatus: (id: string, status: TranslationStatus) => void;
  appendDelta: (id: string, delta: string) => void;
  /**
   * 一次提交一帧内累积的所有 delta（同一条目可多次出现，文本按顺序拼接）。
   *
   * 关键点：整个批次只发 **一次** `set()`，也就是一次订阅通知、一次派生重算；
   * 同一个文件只 `slice()` 一次数组。这样一帧内 6 条并发流不会打出 6 次通知。
   */
  applyDeltas: (batch: { id: string; text: string }[]) => void;
  /** 获取所有选中文件的扁平化条目（派生） */
  getAllEntries: () => TranslationEntry[];
  /** 获取某个文件的条目，用于写回 */
  getFileEntries: (fileName: string) => TranslationEntry[];
  /**
   * 按 id 取当前条目（不存在返回 undefined）。
   *
   * 写回收尾用它判断条目是否已经被用户手工改过（`edited`）：人工成果优先于
   * 本轮的流式文本，既不能被迟到的 delta/done 覆盖，也不能被收尾回滚清掉。
   */
  getEntryById: (id: string) => TranslationEntry | undefined;
  setSettings: (settings: LlmSettings) => void;
  setTheme: (theme: Theme) => void;
  setLoading: (loading: boolean) => void;
  setError: (error: string | null) => void;
  /** 登记一轮翻译，返回该轮的 token（写回闸门据此判断是否还在翻译） */
  beginRun: () => number;
  /** 注销某一轮翻译；只有当前登记的 token 匹配时才真正关闭闸门 */
  endRun: (token: number) => void;
  reset: () => void;
}

/** 默认 LLM 设置（首次启动 / 后端无配置时） */
export const DEFAULT_SETTINGS: LlmSettings = {
  baseUrl: "https://api.deepseek.com",
  apiKey: "",
  model: "deepseek-chat",
  concurrency: 6,
  temperature: 0.3,
};

/** 主题持久化 key */
export const THEME_STORAGE_KEY = "bg3-translate-theme";

/** 翻译轮次 token 的全局序号（跨组件实例唯一，见 AppState.runToken） */
let runTokenSeq = 0;

const THEME_KEY = THEME_STORAGE_KEY;

function loadTheme(): Theme {
  try {
    const saved = localStorage.getItem(THEME_KEY) as Theme | null;
    const theme = saved ?? "dark";
    applyTheme(theme);
    return theme;
  } catch {
    return "dark";
  }
}

function saveTheme(theme: Theme) {
  try {
    localStorage.setItem(THEME_KEY, theme);
  } catch {
    /* ignore */
  }
}

/** 给 <html> 元素设置主题 class（CSS 变量据此切换）*/
function applyTheme(theme: Theme) {
  if (typeof document === "undefined") return;
  const el = document.documentElement;
  el.classList.remove(
    "theme-light",
    "theme-dark",
    "theme-cyberpunk",
    "theme-dungeon",
    "dark",
  );
  el.classList.add(`theme-${theme}`);
  // 兼容 Tailwind dark: 变体（深色系主题也激活 dark）
  if (theme === "dark" || theme === "cyberpunk" || theme === "dungeon") {
    el.classList.add("dark");
  }
}

/**
 * 用「文件 + 下标」索引 O(1) 定位条目（不再线性扫描条目数组）。
 *
 * 下标越界、或下标处已经是别的条目（例如整表被替换过、索引尚未重建）时
 * 返回 null：宁可放弃这次更新，也不能把文本写到错误的条目上。
 */
function locateEntry(
  state: Pick<AppState, "entriesByFile" | "entryIdToFile" | "entryIdToIndex">,
  id: string,
): { fileName: string; list: TranslationEntry[]; index: number } | null {
  const fileName = state.entryIdToFile[id];
  if (!fileName) return null;
  const list = state.entriesByFile[fileName];
  const index = state.entryIdToIndex[id];
  if (!list || index === undefined) return null;
  if (list[index]?.id !== id) return null;
  return { fileName, list, index };
}

export const useAppStore = create<AppState>((set, get) => ({
  stage: "home",
  modFilePath: null,
  workDir: null,
  files: [],
  selectedFiles: [],
  loadedFileNames: new Set(),
  entriesByFile: {},
  entryIdToFile: {},
  entryIdToIndex: {},
  settings: DEFAULT_SETTINGS,
  settingsLoaded: false,
  theme: loadTheme(),
  loading: false,
  error: null,
  runToken: null,

  setStage: (stage) => set({ stage }),
  setModOpened: (filePath, workDir, files) =>
    set({
      modFilePath: filePath,
      workDir,
      files,
      stage: "files",
      selectedFiles: [],
      loadedFileNames: new Set(),
      entriesByFile: {},
      entryIdToFile: {},
      entryIdToIndex: {},
      error: null,
      // 换了 MOD：上一轮翻译的运行态与本次无关（旧一轮的回调也会因为
      // runId / 卸载而停止写 store）
      runToken: null,
    }),
  setSelectedFiles: (files) => set({ selectedFiles: files }),
  setFileEntries: (fileName, entries) =>
    set((s) => {
      // 重建该文件的 id → (文件, 下标) 索引：
      // 先删掉旧条目（整表替换后旧下标可能指向别的条目），再按新顺序写入
      const entryIdToFile = { ...s.entryIdToFile };
      const entryIdToIndex = { ...s.entryIdToIndex };
      for (const old of s.entriesByFile[fileName] ?? []) {
        delete entryIdToFile[old.id];
        delete entryIdToIndex[old.id];
      }
      for (let i = 0; i < entries.length; i += 1) {
        const id = entries[i].id;
        entryIdToFile[id] = fileName;
        entryIdToIndex[id] = i;
      }
      return {
        entriesByFile: { ...s.entriesByFile, [fileName]: entries },
        loadedFileNames: new Set([...s.loadedFileNames, fileName]),
        entryIdToFile,
        entryIdToIndex,
      };
    }),
  updateEntry: (id, patch) =>
    set((s) => {
      const hit = locateEntry(s, id);
      if (!hit) return s;
      const updated = hit.list.slice();
      updated[hit.index] = { ...updated[hit.index], ...patch };
      return {
        entriesByFile: { ...s.entriesByFile, [hit.fileName]: updated },
      };
    }),
  setEntryStatus: (id, status) => get().updateEntry(id, { status }),
  appendDelta: (id, delta) => get().applyDeltas([{ id, text: delta }]),
  applyDeltas: (batch) =>
    set((s) => {
      /** fileName → (元素下标 → 本批累积文本)，同一条目多次出现会拼接 */
      const updates = new Map<string, Map<number, string>>();
      for (const { id, text } of batch) {
        if (text === "") continue;
        const hit = locateEntry(s, id);
        if (!hit) continue;
        // 用户已经手工保存过译文（status === "edited"）：模型后续的流式文本
        // 不得再追加到人工成果上（否则会拼出「人工译文 + 模型尾巴」的混合文本，
        // 而且状态会被改回 translating —— 那是会被写进 PAK 的）
        if (hit.list[hit.index].status === "edited") continue;
        const byIndex = updates.get(hit.fileName) ?? new Map<number, string>();
        byIndex.set(hit.index, (byIndex.get(hit.index) ?? "") + text);
        updates.set(hit.fileName, byIndex);
      }
      if (updates.size === 0) return s;

      const entriesByFile: EntriesByFile = { ...s.entriesByFile };
      for (const [fileName, byIndex] of updates) {
        const list = s.entriesByFile[fileName];
        // 每个文件只拷贝一次数组，随后在同一份拷贝里改完所有命中条目
        const updated = list.slice();
        for (const [index, text] of byIndex) {
          updated[index] = {
            ...updated[index],
            target: updated[index].target + text,
            status: "translating",
          };
        }
        entriesByFile[fileName] = updated;
      }
      return { entriesByFile };
    }),
  getAllEntries: () => {
    const { selectedFiles, entriesByFile } = get();
    const all: TranslationEntry[] = [];
    for (const f of selectedFiles) {
      const list = entriesByFile[f.name];
      if (list) all.push(...list);
    }
    return all;
  },
  getFileEntries: (fileName) => get().entriesByFile[fileName] ?? [],
  getEntryById: (id) => {
    const hit = locateEntry(get(), id);
    return hit ? hit.list[hit.index] : undefined;
  },
  setSettings: (settings) => set({ settings, settingsLoaded: true }),
  setTheme: (theme) => {
    saveTheme(theme);
    applyTheme(theme);
    set({ theme });
  },
  setLoading: (loading) => set({ loading }),
  setError: (error) => set({ error }),
  beginRun: () => {
    runTokenSeq += 1;
    set({ runToken: runTokenSeq });
    return runTokenSeq;
  },
  endRun: (token) =>
    set((s) => (s.runToken === token ? { runToken: null } : {})),
  reset: () =>
    set({
      stage: "home",
      modFilePath: null,
      workDir: null,
      files: [],
      selectedFiles: [],
      loadedFileNames: new Set(),
      entriesByFile: {},
      entryIdToFile: {},
      entryIdToIndex: {},
      error: null,
      runToken: null,
    }),
}));
