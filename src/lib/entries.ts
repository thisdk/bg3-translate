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
 * 文本里是否有**任何可见字符**。
 *
 * **事实来源是 Rust 的 `crates/bg3-translate-core/src/types.rs::has_visible_text`，
 * 两份实现必须一起改**（这层语义不在 IPC 契约检查的射程内，只能靠这里与测试钉住）。
 *
 * 空白、控制符与「确定不可见」的零宽 / 格式类字符（BOM、零宽空格、软连字符、
 * 双向控制符、变体选择符……）都不算可见：只由它们组成的「译文」在游戏里什么都
 * 不显示，写回等于把原文删掉。`trim()` 只吃得掉空白，拦不住 U+FEFF / U+200B。
 *
 * 只判**整条**文本，不改文本本身：零宽字符夹在正常文字中间是有意义的
 * （emoji 的变体选择符 U+FE0F、防止断行的 U+2060），不能当成空。
 */
export function hasVisibleText(text: string): boolean {
  for (const ch of text) {
    if (!isBlankOrInvisible(ch)) return true;
  }
  return false;
}

/**
 * 空白 / 控制符 / 零宽与格式类不可见字符（Rust `is_blank_or_invisible` 的镜像）。
 *
 * 刻意不按「全部 Cf 类别」一刀切：`U+06DD`（阿拉伯文节末标记）这类 Cf 字符是
 * **可见**的，把它们算成空白会把正常译文误判成空。
 */
function isBlankOrInvisible(ch: string): boolean {
  // Rust `char::is_whitespace()`（Unicode White_Space）。JS 的 `\s` 多含一个
  // U+FEFF，而它本来就在下面的不可见表里，两种写法等价。
  if (/\s/u.test(ch)) return true;
  const code = ch.codePointAt(0) ?? 0;
  // Rust `char::is_control()`（Cc：C0 与 C1 控制符）
  if (code <= 0x1f || (code >= 0x7f && code <= 0x9f)) return true;
  return (
    code === 0x00ad || // 软连字符
    code === 0x061c || // 阿拉伯字母标记
    code === 0x180e || // 蒙古文元音分隔符
    (code >= 0x200b && code <= 0x200f) || // 零宽空格 / ZWNJ / ZWJ / LRM / RLM
    (code >= 0x202a && code <= 0x202e) || // 双向嵌入与覆盖
    (code >= 0x2060 && code <= 0x2064) || // word joiner 等不可见运算符
    (code >= 0x2066 && code <= 0x206f) || // 双向隔离符与废弃格式符
    code === 0xfeff || // BOM / 零宽不换行空格
    (code >= 0xfff9 && code <= 0xfffb) || // 注释锚点
    (code >= 0xfe00 && code <= 0xfe0f) || // 变体选择符
    (code >= 0xe0100 && code <= 0xe01ef) // 变体选择符补充
  );
}

/**
 * 是否有「可以写回 PAK」的译文。
 *
 * 后端 `TranslationEntry::has_writable_target()` 的前端镜像：`target` 里要有
 * **可见文本**（见 [`hasVisibleText`]，不只是 `trim()` 非空）**且** 状态不是
 * `error`（error 条目一律退回原文）。写回链路的每个判断都必须复用这一个定义。
 *
 * 为什么不能用 `target.trim() !== ""`：只有 BOM / 零宽空格的译文会被判成「有译文」，
 * 在 `mergeWithExistingTarget` 里挤掉 MOD 自带的真实中文，而后端按自己的语义把它
 * 当空、写回英文原文 —— 自带中文就这样丢了。
 */
export function hasWritableTarget(entry: TranslationEntry): boolean {
  return entry.status !== "error" && hasVisibleText(entry.target);
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
 * 业务语义：**只翻译 source 非空且 target 为空的条目**（后端
 * `is_pending_translation()` 也是这条判据，core 会自己再过滤一遍）。
 * 当所选条目全部已有译文时，退回「清空全部并重译」的模式。
 *
 * 正常分支的 payload 是**全部工作条目**（`work`），不是「待翻译的那批」：
 * core 用整份 payload 建一致性记忆（`planner::build_consistency_memory`，只认
 * `has_writable_target()` 的条目），已经有了译文的条目正是「本 MOD 已确定译名」
 * 与系列 base 复用的唯一来源。只发 pending 的话这份记忆恒为空 —— 专名跨批次
 * 不一致、`plan.skipped` 恒 0、【本 MOD 已确定译名】段落从不出现。带上去的
 * 上下文条目后端不会翻译（不发事件、不计入 `plan.total`），但调用方必须知道
 * 「payload ≠ 本轮在跑的条目」：收尾回滚只能动 `filter(isTranslatableNow)` 那批
 * （见 `useTranslationRun`），否则会把上下文条目的译文清空。
 *
 * 重译分支（`retranslateAll`）依旧把全部条目清空成 pending、全部送翻，
 * 分支条件与返回结构都不变。
 *
 * 重译分支同样**保留 `error`**：它可能带着上一轮的结构校验诊断，后端会当纠错
 * 提示注入（界面上的文案由调用方清掉，见 `buildRetryRequest` 的说明）。
 */
export function buildTranslationRequest(
  entries: TranslationEntry[],
): TranslationRequestPlan {
  const work = entries.filter(isTranslationWorkItem);
  const pending = work.filter(isTranslatableNow);

  if (pending.length > 0) {
    return { request: work, retranslateAll: false };
  }

  return {
    request: work.map((entry) => ({
      ...entry,
      target: "",
      status: "pending" as TranslationStatus,
    })),
    retranslateAll: work.length > 0,
  };
}

/**
 * 把错误条目重置为待翻译，供「重试失败条目」使用。
 * 返回可直接发给后端的条目数组（译文已清空）。
 *
 * **错误文案保留**：它是上一轮结构校验给出的诊断，后端会把它当纠错提示注入
 * 重试 prompt（见 `planner::previous_failure_of`）—— 模型知道上次错在哪，才有
 * 机会避开同一个坑。界面上那份由调用方的 `updateEntry(..., { error: null })`
 * 清掉，所以用户不会看到过期错误。
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
    }));
}
