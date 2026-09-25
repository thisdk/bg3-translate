/** 测试数据构造器（纯函数，无 vitest 依赖） */
import type { PakFile, TranslationEntry } from "@/lib/types";

/** 构造一个可选中的本地化文件 */
export function pakFile(
  name = "Localization/English/big.xml",
  patch: Partial<PakFile> = {},
): PakFile {
  return { name, size: 1, kind: "localization-xml", language: "English", ...patch };
}

/**
 * 构造 n 条条目：状态在 pending/translated/translating/error 间轮转，
 * 每 4 条留一条空译文（可翻译），与真实 PAK 的分布近似。
 */
export function buildEntries(
  count: number,
  sourceFile = "Localization/English/big.xml",
): TranslationEntry[] {
  const statuses = ["pending", "translated", "translating", "error"] as const;
  return Array.from({ length: count }, (_, i) => ({
    id: `e-${i}`,
    sourceFile,
    source: `Entry number ${i}`,
    target: i % 4 === 0 ? "" : `译文 ${i}`,
    contentuid: `uid-${i}`,
    version: "1",
    status: statuses[i % statuses.length],
    error: null,
  }));
}
