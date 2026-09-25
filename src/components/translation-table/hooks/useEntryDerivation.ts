import { useMemo } from "react";
import {
  computeEntryStats,
  countByFilter,
  filterEntries,
  isTranslatableNow,
  isTranslationWorkItem,
  type EntryFilter,
  type EntryStats,
} from "@/lib/entries";
import type { PakFile, TranslationEntry } from "@/lib/types";

export interface EntryDerivation {
  /** 原文非空的条目（真正参与翻译与统计的集合） */
  workEntries: TranslationEntry[];
  stats: EntryStats;
  /** 各过滤维度的计数 */
  counts: Record<EntryFilter, number>;
  /** 搜索 + 状态过滤后的可见条目 */
  visibleEntries: TranslationEntry[];
  /** 当前译文为空的条目数 */
  translatableCount: number;
  /** 所有条目都已有译文 → 需要「重新翻译全部」 */
  retranslateAll: boolean;
}

/**
 * 聚合已勾选文件的条目并派生统计 / 计数 / 可见列表。
 *
 * 单独抽出来的原因：流式翻译时每次 store 提交都会重算这一串，
 * 集中在一处才能看清成本，也方便后续继续优化（合并遍历等）。
 */
export function useEntryDerivation(
  selectedFiles: PakFile[],
  entriesByFile: Record<string, TranslationEntry[]>,
  query: string,
  filter: EntryFilter,
): EntryDerivation {
  // 扁平化时就地过滤掉空原文，省掉一个 2 万条的中间数组
  const workEntries = useMemo(() => {
    const out: TranslationEntry[] = [];
    for (const file of selectedFiles) {
      const list = entriesByFile[file.name];
      if (!list) continue;
      for (const entry of list) {
        if (isTranslationWorkItem(entry)) out.push(entry);
      }
    }
    return out;
  }, [selectedFiles, entriesByFile]);

  const stats = useMemo(() => computeEntryStats(workEntries), [workEntries]);
  const counts = useMemo(() => countByFilter(workEntries), [workEntries]);
  const visibleEntries = useMemo(
    () => filterEntries(workEntries, { query, filter }),
    [workEntries, query, filter],
  );
  const translatableCount = useMemo(
    () => workEntries.filter(isTranslatableNow).length,
    [workEntries],
  );

  return {
    workEntries,
    stats,
    counts,
    visibleEntries,
    translatableCount,
    retranslateAll: translatableCount === 0 && stats.total > 0,
  };
}
