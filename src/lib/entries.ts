import type { TranslationEntry, TranslationStatus } from "./types";

/** 翻译表格的状态过滤维度 */
export type EntryFilter =
  | "all"
  | "pending"
  | "translating"
  | "translated"
  | "error";

export const ENTRY_FILTERS: { value: EntryFilter; label: string }[] = [
  { value: "all", label: "全部" },
  { value: "pending", label: "待翻译" },
  { value: "translating", label: "翻译中" },
  { value: "translated", label: "已翻译" },
  { value: "error", label: "出错" },
];

/** 只有原文非空的条目才值得翻译（本地化文件里存在空 source 的占位条目） */
export function isTranslationWorkItem(entry: TranslationEntry): boolean {
  return entry.source.trim().length > 0;
}

/** 当前就需要送去翻译的条目：原文非空且译文为空 */
export function isTranslatableNow(entry: TranslationEntry): boolean {
  return entry.source.trim().length > 0 && entry.target.trim() === "";
}

/** 已完成的条目（人工编辑也算完成） */
export function isDoneStatus(status: TranslationStatus): boolean {
  return status === "translated" || status === "edited";
}

/** 单条是否命中过滤维度 */
export function matchesFilter(
  entry: TranslationEntry,
  filter: EntryFilter,
): boolean {
  switch (filter) {
    case "all":
      return true;
    case "pending":
      return entry.status === "pending";
    case "translating":
      return entry.status === "translating";
    case "translated":
      return isDoneStatus(entry.status);
    case "error":
      return entry.status === "error";
    default:
      return true;
  }
}

/** 搜索匹配：原文 / 译文 / contentuid（大小写不敏感） */
export function matchesQuery(entry: TranslationEntry, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return (
    entry.source.toLowerCase().includes(q) ||
    entry.target.toLowerCase().includes(q) ||
    entry.contentuid.toLowerCase().includes(q)
  );
}

/**
 * 从完整 PAK 路径提取简短来源标签，如
 * `Mods/Foo/Localization/English/x.xml` → `English/x.xml`
 */
export function shortSourcePath(fileName: string): string {
  const parts = fileName.split("/").filter(Boolean);
  if (parts.length <= 2) return fileName;
  return parts.slice(-2).join("/");
}

/**
 * 是否有「可以写回 PAK」的译文。
 *
 * 后端 `TranslationEntry::has_writable_target()` 的前端镜像：`target` 非空
 * **且** 状态不是 `error`（error 条目一律退回原文）。写回链路的每个判断都
 * 必须复用这一个定义，别再各写各的。
 */
export function hasWritableTarget(entry: TranslationEntry): boolean {
  return entry.target.trim() !== "" && entry.status !== "error";
}

/**
 * 写回前的最后一道闸门：把「还没有权威结果」的条目降级为待翻译。
 *
 * 为什么必须在这里做：后端写回时只在 `status === "error"` 时退回原文
 * （`TranslationEntry::has_writable_target()`），**`status === "translating"`
 * 的非空 target 会被当成真译文写进 PAK**。取消 / 收尾回滚与迟到事件过滤
 * 已经在 `useTranslationRun` 里把关，这里是纵深防御：任何原因残留下来的
 * `translating` 条目都不允许把半截文本带进写回请求（退回原文比写坏 MOD 好）。
 */
export function toWritableEntry(entry: TranslationEntry): TranslationEntry {
  if (entry.status !== "translating") return entry;
  return { ...entry, target: "", status: "pending", error: null };
}

/** 过滤 + 搜索（纯函数，供表格与单测使用） */
export function filterEntries(
  entries: TranslationEntry[],
  options: { query?: string; filter?: EntryFilter } = {},
): TranslationEntry[] {
  const { query = "", filter = "all" } = options;
  if (!query.trim() && filter === "all") return entries;
  return entries.filter(
    (entry) => matchesFilter(entry, filter) && matchesQuery(entry, query),
  );
}

/** 各过滤维度的计数（用于过滤 chips 上的数字） */
export function countByFilter(
  entries: TranslationEntry[],
): Record<EntryFilter, number> {
  const counts: Record<EntryFilter, number> = {
    all: entries.length,
    pending: 0,
    translating: 0,
    translated: 0,
    error: 0,
  };
  for (const entry of entries) {
    if (entry.status === "pending") counts.pending += 1;
    else if (entry.status === "translating") counts.translating += 1;
    else if (isDoneStatus(entry.status)) counts.translated += 1;
    else if (entry.status === "error") counts.error += 1;
  }
  return counts;
}

export interface EntryStats {
  /** 可翻译条目总数（原文非空） */
  total: number;
  pending: number;
  translating: number;
  translated: number;
  edited: number;
  error: number;
  /** translated + edited */
  done: number;
  /** 0–100 的整数百分比 */
  percent: number;
}

/** 整体进度统计（进度条 / 计数展示） */
export function computeEntryStats(entries: TranslationEntry[]): EntryStats {
  let pending = 0;
  let translating = 0;
  let translated = 0;
  let edited = 0;
  let error = 0;

  for (const entry of entries) {
    switch (entry.status) {
      case "pending":
        pending += 1;
        break;
      case "translating":
        translating += 1;
        break;
      case "translated":
        translated += 1;
        break;
      case "edited":
        edited += 1;
        break;
      case "error":
        error += 1;
        break;
      default:
        break;
    }
  }

  const total = entries.length;
  const done = translated + edited;
  return {
    total,
    pending,
    translating,
    translated,
    edited,
    error,
    done,
    percent: progressPercent(done, total),
  };
}

/** 百分比（整数，0–100），total 为 0 时返回 0 */
export function progressPercent(done: number, total: number): number {
  if (total <= 0) return 0;
  const pct = (done / total) * 100;
  if (!Number.isFinite(pct)) return 0;
  return Math.max(0, Math.min(100, Math.round(pct)));
}

export interface TranslationRequestPlan {
  /** 需要发给后端翻译的条目（原文非空、译文为空） */
  request: TranslationEntry[];
  /**
   * 是否触发了「重新翻译全部」：所有条目都已有译文时，
   * 清空整组译文重算一遍以保证术语一致性。
   */
  retranslateAll: boolean;
}

/**
 * 计算一次翻译请求要发送的条目。
 *
 * 业务语义保持不变：**只翻译 source 非空且 target 为空的条目**。
 * 当所选条目全部已有译文时，退回「清空全部并重译」的模式。
 */
export function buildTranslationRequest(
  entries: TranslationEntry[],
): TranslationRequestPlan {
  const work = entries.filter(isTranslationWorkItem);
  const pending = work.filter(isTranslatableNow);

  if (pending.length > 0) {
    return { request: pending, retranslateAll: false };
  }

  return {
    request: work.map((entry) => ({
      ...entry,
      target: "",
      status: "pending" as TranslationStatus,
      error: null,
    })),
    retranslateAll: work.length > 0,
  };
}

/**
 * 把错误条目重置为待翻译，供「重试失败条目」使用。
 * 返回可直接发给后端的条目数组（译文已清空）。
 */
export function buildRetryRequest(
  entries: TranslationEntry[],
): TranslationEntry[] {
  return entries
    .filter((entry) => entry.status === "error" && isTranslationWorkItem(entry))
    .map((entry) => ({
      ...entry,
      target: "",
      status: "pending" as TranslationStatus,
      error: null,
    }));
}
