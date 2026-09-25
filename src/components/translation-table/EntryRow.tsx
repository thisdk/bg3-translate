import { memo } from "react";
import { AlertCircle, Check, Edit3, Eraser, Loader2, RotateCcw, X } from "lucide-react";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { shortSourcePath } from "@/lib/entries";
import { STATUS_META } from "./constants";
import type { TranslationEntry } from "@/lib/types";

interface EntryRowProps {
  entry: TranslationEntry;
  editing: boolean;
  draft: string;
  busy: boolean;
  onDraftChange: (value: string) => void;
  onStartEdit: (entry: TranslationEntry) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
  onRevert: (entry: TranslationEntry) => void;
  onRetry: (id: string) => void;
}

/**
 * 单条条目（原文 / 译文 / 操作）。
 *
 * 用 memo 包住：流式翻译时每帧只有少数条目变化，其余行的 props 不变，
 * 避免 2 万条列表在每次提交时整片重渲染。
 */
export const EntryRow = memo(function EntryRow({
  entry,
  editing,
  draft,
  busy,
  onDraftChange,
  onStartEdit,
  onSaveEdit,
  onCancelEdit,
  onRevert,
  onRetry,
}: EntryRowProps) {
  const meta = STATUS_META[entry.status] ?? STATUS_META.pending;

  return (
    <div
      className={cn(
        "grid grid-cols-1 gap-3 border-b p-3 md:grid-cols-2",
        entry.status === "translating" && "bg-amber-500/5",
        entry.status === "error" && "bg-destructive/5",
      )}
    >
      {/* 原文 */}
      <div className="min-w-0 space-y-1">
        <div className="flex flex-wrap items-center gap-2">
          <span className="text-[10px] text-muted-foreground">原文</span>
          <Badge variant={meta.variant} className="text-[10px]">
            {entry.status === "translating" && (
              <Loader2 className="mr-1 h-2.5 w-2.5 animate-spin" />
            )}
            {entry.status === "error" && (
              <AlertCircle className="mr-1 h-2.5 w-2.5" />
            )}
            {meta.label}
          </Badge>
          <span
            className="truncate bg-muted px-1.5 text-[10px] text-muted-foreground"
            title={entry.sourceFile}
          >
            {shortSourcePath(entry.sourceFile)}
          </span>
          <span className="ml-auto shrink-0 font-mono text-[10px] text-muted-foreground/70">
            {entry.contentuid}
          </span>
        </div>
        <p className="whitespace-pre-wrap break-words bg-muted/50 p-2 text-sm [overflow-wrap:anywhere]">
          {entry.source}
        </p>
      </div>

      {/* 译文 */}
      <div className="min-w-0 space-y-1">
        <div className="flex items-center justify-between gap-2">
          <span className="text-[10px] text-muted-foreground">译文</span>
          <div className="flex items-center gap-1">
            {editing ? (
              <>
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 px-2"
                  onClick={onCancelEdit}
                >
                  <X className="h-3 w-3" />
                  取消
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="h-6 px-2"
                  onClick={onSaveEdit}
                >
                  <Check className="h-3 w-3" />
                  保存
                </Button>
              </>
            ) : (
              <>
                {entry.status === "error" && (
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-6 px-2 text-destructive hover:text-destructive"
                    onClick={() => onRetry(entry.id)}
                    disabled={busy}
                    title="只重试这一条"
                  >
                    <RotateCcw className="h-3 w-3" />
                    重试
                  </Button>
                )}
                {entry.target && (
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-6 px-2"
                    onClick={() => onStartEdit(entry)}
                  >
                    <Edit3 className="h-3 w-3" />
                    编辑
                  </Button>
                )}
                {(entry.status === "translated" ||
                  entry.status === "edited" ||
                  entry.status === "error") && (
                  <Button
                    size="sm"
                    variant="ghost"
                    className="h-6 px-2"
                    onClick={() => onRevert(entry)}
                  >
                    <Eraser className="h-3 w-3" />
                    还原
                  </Button>
                )}
              </>
            )}
          </div>
        </div>
        {editing ? (
          <Textarea
            value={draft}
            onChange={(event) => onDraftChange(event.target.value)}
            className="min-h-[60px] text-sm"
            autoFocus
            aria-label="编辑译文"
          />
        ) : (
          <p
            className={cn(
              "min-h-[36px] whitespace-pre-wrap break-words border border-transparent p-2 text-sm [overflow-wrap:anywhere]",
              entry.target ? "bg-primary/5" : "bg-muted/30 italic text-muted-foreground",
              entry.error && "text-destructive",
            )}
          >
            {entry.error ?? entry.target ?? "等待翻译…"}
          </p>
        )}
      </div>
    </div>
  );
});
