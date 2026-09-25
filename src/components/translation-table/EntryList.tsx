import { useEffect, useRef } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import { Languages, Loader2, Search } from "lucide-react";
import { Button } from "@/components/ui/button";
import { EmptyState } from "@/components/ui/empty-state";
import { ESTIMATED_ROW_HEIGHT } from "./constants";
import { EntryRow } from "./EntryRow";
import { TableStatusBar } from "./TableStatusBar";
import type { TranslationEntry } from "@/lib/types";

interface EntryListProps {
  visibleEntries: TranslationEntry[];
  /** 可翻译条目总数（用于空状态判断） */
  total: number;
  loading: boolean;
  /** 搜索/过滤条件变化时用它触发回到顶部 */
  scrollResetKey: string;
  filtered: boolean;
  editingId: string | null;
  draft: string;
  busy: boolean;
  onDraftChange: (value: string) => void;
  onStartEdit: (entry: TranslationEntry) => void;
  onSaveEdit: () => void;
  onCancelEdit: () => void;
  onRevert: (entry: TranslationEntry) => void;
  onRetry: (id: string) => void;
  onClearFilters: () => void;
}

/**
 * 条目列表区：虚拟滚动 + 空状态 + 底部状态条。
 *
 * 2 万条条目只挂载视口附近的十余行，滚动容器的高度由 virtualizer 计算，
 * 行高由 measureElement 动态修正。
 */
export function EntryList({
  visibleEntries,
  total,
  loading,
  scrollResetKey,
  filtered,
  editingId,
  draft,
  busy,
  onDraftChange,
  onStartEdit,
  onSaveEdit,
  onCancelEdit,
  onRevert,
  onRetry,
  onClearFilters,
}: EntryListProps) {
  const scrollRef = useRef<HTMLDivElement>(null);

  const virtualizer = useVirtualizer({
    count: visibleEntries.length,
    getScrollElement: () => scrollRef.current,
    estimateSize: () => ESTIMATED_ROW_HEIGHT,
    overscan: 8,
    getItemKey: (index) => visibleEntries[index]?.id ?? index,
  });

  // 搜索 / 过滤变化后回到顶部，避免停留在空白区域
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = 0;
  }, [scrollResetKey]);

  const virtualItems = virtualizer.getVirtualItems();

  return (
    <>
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <EmptyState
            icon={<Loader2 className="h-6 w-6 animate-spin" />}
            title="加载条目…"
          />
        ) : total === 0 ? (
          <EmptyState
            icon={<Languages className="h-8 w-8" />}
            title="选中文件没有可翻译条目"
            description="这些文件里可能没有非空的原文，试试勾选其它本地化文件。"
          />
        ) : visibleEntries.length === 0 ? (
          <EmptyState
            icon={<Search className="h-8 w-8" />}
            title="没有匹配的条目"
            description="换个关键词，或把状态过滤切回「全部」。"
            action={
              <Button size="sm" variant="outline" onClick={onClearFilters}>
                清除筛选
              </Button>
            }
          />
        ) : (
          <div
            className="relative w-full"
            style={{ height: virtualizer.getTotalSize() }}
          >
            {virtualItems.map((virtualRow) => {
              const entry = visibleEntries[virtualRow.index];
              if (!entry) return null;
              return (
                <div
                  key={virtualRow.key}
                  data-index={virtualRow.index}
                  ref={virtualizer.measureElement}
                  className="absolute left-0 top-0 w-full"
                  style={{ transform: `translateY(${virtualRow.start}px)` }}
                >
                  <EntryRow
                    entry={entry}
                    editing={editingId === entry.id}
                    draft={draft}
                    busy={busy}
                    onDraftChange={onDraftChange}
                    onStartEdit={onStartEdit}
                    onSaveEdit={onSaveEdit}
                    onCancelEdit={onCancelEdit}
                    onRevert={onRevert}
                    onRetry={onRetry}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>

      {total > 0 && (
        <TableStatusBar
          total={total}
          visibleCount={visibleEntries.length}
          filtered={filtered}
          renderedRows={virtualItems.length}
        />
      )}
    </>
  );
}
