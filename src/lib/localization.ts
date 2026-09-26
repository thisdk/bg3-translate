import { hasWritableTarget, toWritableEntry } from "./entries";
import type { PakFile, TranslationEntry } from "./types";

/**
 * 目标语言目录名。写回本地化文件时，`Localization/<语言>/...` 中的语言段
 * 会被替换成这个值。
 */
export const TARGET_LANGUAGE = "Chinese";

/** 后端已知的中文语言标识（用于写回优先级判定） */
export const CHINESE_LANGUAGE_ALIASES = [TARGET_LANGUAGE, "ChineseSimplified"];

/**
 * 把 PAK 内路径中的本地化语言段改写为中文。
 *
 * 例：`Localization/English/foo.xml` → `Localization/Chinese/foo.xml`
 *
 * 找不到 `Localization` 段、或该段后不足两级（没有语言目录 + 文件名）时，
 * 原样返回，避免产生非法路径。
 */
export function toTargetLocalizationPath(fileName: string): string {
  const parts = fileName.split("/").filter(Boolean);
  const locIndex = parts.findIndex(
    (part) => part.toLowerCase() === "localization",
  );
  // 需要 Localization/<lang>/<file> 至少三段才改写
  if (locIndex < 0 || parts.length <= locIndex + 2) {
    return fileName;
  }
  const next = [...parts];
  next[locIndex + 1] = TARGET_LANGUAGE;
  return next.join("/");
}

/**
 * 同一目标路径可能来自多个语言文件（English / Chinese / Polish …），
 * 优先级越高越应该胜出：
 *   3 = 英文（原文，最权威）
 *   2 = 已是中文（人工/官方中文）
 *   1 = 其它语言（波兰语、法语…）
 */
export function localizationWritePriority(file: PakFile): number {
  if (file.language === "English") return 3;
  if (file.language !== null && CHINESE_LANGUAGE_ALIASES.includes(file.language)) {
    return 2;
  }
  return 1;
}

/** 一条写回计划：目标文件名 + 最终胜出的条目 */
export interface LocalizationWritePlan {
  /** 写回的目标路径（已改写为中文目录） */
  fileName: string;
  /** 该文件最终要写回的条目 */
  entries: TranslationEntry[];
  /** 胜出优先级（见 localizationWritePriority） */
  priority: number;
  /** 贡献这份条目的源文件路径 */
  sourceFile: string;
}

/**
 * 汇总「勾选的文件 → 需要写回的本地化文件」映射。
 * 多个源文件映射到同一目标路径时，只保留优先级最高的那份。
 * 返回顺序 = 源文件首次出现的顺序（与 Map 插入序一致）。
 *
 * 每条计划里的条目都会先过 `toWritableEntry`：`translating`（还没有权威
 * 结果）的条目退回原文，绝不让半截流式文本进入写回请求。
 */
export function planLocalizationWrites(
  files: PakFile[],
  entriesByFile: Record<string, TranslationEntry[]>,
): LocalizationWritePlan[] {
  const plans = new Map<string, LocalizationWritePlan>();

  for (const file of files) {
    const entries = entriesByFile[file.name];
    if (!entries || entries.length === 0) continue;

    const targetName = toTargetLocalizationPath(file.name);
    const priority = localizationWritePriority(file);
    const existing = plans.get(targetName);
    if (!existing || priority > existing.priority) {
      plans.set(targetName, {
        fileName: targetName,
        entries: entries.map(toWritableEntry),
        priority,
        sourceFile: file.name,
      });
    }
  }

  return [...plans.values()];
}

/**
 * 写回目标是「MOD 里已经存在的文件」时，用该文件现有的内容做底稿合并
 * （contentuid 级）。
 *
 * 为什么必须合并：`Localization/English/x.xml` 与 `Localization/Chinese/x.xml`
 * 会映射到同一个写回路径 `Localization/Chinese/x.xml`，按优先级让英文胜出之后，
 * 英文文件里**未翻译**的条目会退回英文原文（后端 `effective_text()`），把 MOD
 * 里已有的官方/人工中文覆盖成英文 —— 对「MOD 自带中文、只想补翻一部分」的用户
 * 是净损失。FileTree 还有一键「全选」，这条路非常好走。
 *
 * 合并规则：
 *   1. incoming 有可写回译文（`hasWritableTarget`）→ 用 incoming（新译文/人工译文）；
 *   2. incoming 没有可写回译文（target 空 / `status === "error"`）→ 底稿同
 *      contentuid 有条目且文本非空时，保留底稿（文本与状态都不动）；
 *   3. 底稿有、incoming 没有的 contentuid 一并保留，避免丢条目；
 *   4. 顺序：incoming 的顺序在前，底稿独有的条目按底稿顺序追加。
 *
 * 目标文件不存在（首次生成中文文件）时不要调用它 —— 那种情况没有底稿可谈。
 */
export function mergeWithExistingTarget(
  incoming: TranslationEntry[],
  existing: TranslationEntry[],
): TranslationEntry[] {
  // 底稿按 contentuid 索引；同一 contentuid 重复出现时保留第一条
  const base = new Map<string, TranslationEntry>();
  for (const entry of existing) {
    if (!base.has(entry.contentuid)) base.set(entry.contentuid, entry);
  }

  const merged: TranslationEntry[] = [];
  const used = new Set<string>();

  // 先归一化（`translating` 的半截译文不算可写回译文），保证无论调用方是否
  // 已经过 `toWritableEntry`，半截文本都不会赢过底稿本身已有的人工中文
  for (const entry of incoming.map(toWritableEntry)) {
    used.add(entry.contentuid);
    const keeper = base.get(entry.contentuid);
    // 新译文优先；没有新译文时保留底稿文本（底稿文本为空则退回 incoming，
    // 至少还能写回原文，不会写出空条目）
    if (keeper && !hasWritableTarget(entry) && entryText(keeper).trim() !== "") {
      merged.push(keeper);
      continue;
    }
    merged.push(entry);
  }

  for (const entry of existing) {
    if (used.has(entry.contentuid)) continue;
    used.add(entry.contentuid);
    merged.push(entry);
  }

  return merged;
}

/** 条目最终会落盘的文本（与后端 `effective_text()` 同义） */
function entryText(entry: TranslationEntry): string {
  return hasWritableTarget(entry) ? entry.target : entry.source;
}
