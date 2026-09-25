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
        entries,
        priority,
        sourceFile: file.name,
      });
    }
  }

  return [...plans.values()];
}
