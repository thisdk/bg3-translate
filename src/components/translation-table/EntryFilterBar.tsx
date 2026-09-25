import { Search, WandSparkles, X } from "lucide-react";
import { Input } from "@/components/ui/input";
import type { EntryFilter } from "@/lib/entries";
import { FilterChips } from "./FilterChips";

interface EntryFilterBarProps {
  search: string;
  onSearchChange: (value: string) => void;
  counts: Record<EntryFilter, number>;
  filter: EntryFilter;
  onFilterChange: (value: EntryFilter) => void;
  styleHint: string;
  onStyleHintChange: (value: string) => void;
  /** 没有可翻译条目时禁用搜索框 */
  disabled: boolean;
}

/** 搜索框 + 状态过滤 chips + 本次翻译的语境提示 */
export function EntryFilterBar({
  search,
  onSearchChange,
  counts,
  filter,
  onFilterChange,
  styleHint,
  onStyleHintChange,
  disabled,
}: EntryFilterBarProps) {
  return (
    <>
      <div className="flex flex-wrap items-center gap-2 px-3 pb-2">
        <div className="relative min-w-[200px] flex-1">
          <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            placeholder="搜索原文、译文或 contentuid…"
            value={search}
            onChange={(e) => onSearchChange(e.target.value)}
            disabled={disabled}
            className="h-8 pl-8 text-xs"
            aria-label="搜索条目"
          />
          {search && (
            <button
              type="button"
              onClick={() => onSearchChange("")}
              aria-label="清空搜索"
              className="absolute right-1.5 top-1/2 flex h-5 w-5 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            >
              <X className="h-3 w-3" />
            </button>
          )}
        </div>
        <FilterChips counts={counts} value={filter} onChange={onFilterChange} />
      </div>

      {/* 语境提示 */}
      <div className="px-3 pb-2.5">
        <div className="relative">
          <WandSparkles className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            value={styleHint}
            onChange={(e) => onStyleHintChange(e.target.value)}
            placeholder="本 MOD 语境：例如 XX 是姿势名称，相关名词按姿势类型翻译"
            className="h-8 pl-8 text-xs"
            aria-label="翻译风格提示"
          />
        </div>
      </div>
    </>
  );
}
