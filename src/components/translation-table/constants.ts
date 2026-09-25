import type { TranslationStatus } from "@/lib/types";

/** 条目状态徽章的文案与配色（行内与工具栏共用） */
export const STATUS_META: Record<
  TranslationStatus,
  {
    label: string;
    variant: "default" | "secondary" | "success" | "warning" | "destructive";
  }
> = {
  pending: { label: "待翻译", variant: "secondary" },
  translating: { label: "翻译中", variant: "warning" },
  translated: { label: "已翻译", variant: "success" },
  edited: { label: "已编辑", variant: "default" },
  error: { label: "错误", variant: "destructive" },
};

/** 单行估算高度（虚拟滚动初始值，实际由 measureElement 动态修正） */
export const ESTIMATED_ROW_HEIGHT = 118;

/** 最近一次翻译任务的收尾统计（后端 all_done 事件） */
export interface RunSummary {
  total: number;
  failed: number;
}
