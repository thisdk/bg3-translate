import { CircleStop, Play, RotateCcw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Progress } from "@/components/ui/progress";
import { cn } from "@/lib/utils";
import type { RunSummary } from "./constants";
import type { EntryStats } from "@/lib/entries";

interface TranslationToolbarProps {
  /** 已勾选文件数 */
  fileCount: number;
  stats: EntryStats;
  loading: boolean;
  translating: boolean;
  cancelling: boolean;
  /** 当前可翻译（译文为空）的条目数 */
  translatableCount: number;
  /** 全部条目都已有译文 → 按钮变成「重新翻译全部」 */
  retranslateAll: boolean;
  runSummary: RunSummary | null;
  onTranslate: () => void;
  onRetryFailed: () => void;
  onCancel: () => void;
}

/** 翻译工作区顶部：标题 / 操作按钮 / 整体进度与状态计数 */
export function TranslationToolbar({
  fileCount,
  stats,
  loading,
  translating,
  cancelling,
  translatableCount,
  retranslateAll,
  runSummary,
  onTranslate,
  onRetryFailed,
  onCancel,
}: TranslationToolbarProps) {
  return (
    <>
      <div className="flex min-h-8 flex-wrap items-center justify-between gap-3 px-3 pb-2 pt-2.5">
        <div className="min-w-0">
          <h3 className="text-sm font-semibold">
            翻译工作区
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              {fileCount} 个文件，{stats.total} 条
            </span>
          </h3>
          <p className="text-xs text-muted-foreground">
            {loading
              ? "加载中…"
              : `已翻译 ${stats.done}/${stats.total}${
                  runSummary
                    ? ` · 上次任务 ${runSummary.total} 条，失败 ${runSummary.failed} 条`
                    : ""
                }`}
          </p>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {stats.error > 0 && (
            <Button
              size="sm"
              variant="outline"
              onClick={onRetryFailed}
              disabled={translating || loading}
              title="只把出错的条目重新发给模型"
            >
              <RotateCcw className="h-3.5 w-3.5" />
              重试 {stats.error} 条失败
            </Button>
          )}
          <Button
            size="sm"
            onClick={onTranslate}
            loading={translating && !cancelling}
            disabled={loading || stats.total === 0 || (translating && cancelling)}
          >
            {!translating &&
              (retranslateAll ? (
                <RotateCcw className="h-3.5 w-3.5" />
              ) : (
                <Play className="h-3.5 w-3.5" />
              ))}
            {translating
              ? "翻译中…"
              : retranslateAll
                ? "重新翻译全部"
                : `翻译 ${translatableCount} 条`}
          </Button>
          {translating && (
            <Button
              size="sm"
              variant="outline"
              onClick={onCancel}
              loading={cancelling}
            >
              {!cancelling && <CircleStop className="h-3.5 w-3.5" />}
              {cancelling ? "取消中…" : "取消翻译"}
            </Button>
          )}
        </div>
      </div>

      {/* 整体进度 */}
      <div className="px-3 pb-2.5">
        <Progress value={stats.done} max={Math.max(stats.total, 1)} />
        <div className="mt-1 flex flex-wrap items-center justify-between gap-2 text-[11px] tabular-nums text-muted-foreground">
          <span>
            已翻译 {stats.done}/{stats.total}（{stats.percent}%）
          </span>
          <span className="flex flex-wrap items-center gap-x-2">
            <span>待翻译 {stats.pending}</span>
            <span>翻译中 {stats.translating}</span>
            {stats.edited > 0 && <span>已编辑 {stats.edited}</span>}
            <span className={cn(stats.error > 0 && "text-destructive")}>
              失败 {stats.error}
            </span>
          </span>
        </div>
      </div>
    </>
  );
}
