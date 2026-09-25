import { ListFilter } from "lucide-react";
import { cn } from "@/lib/utils";
import { ENTRY_FILTERS, type EntryFilter } from "@/lib/entries";

/** 状态过滤 chips（带各维度计数） */
export function FilterChips({
  counts,
  value,
  onChange,
}: {
  counts: Record<EntryFilter, number>;
  value: EntryFilter;
  onChange: (value: EntryFilter) => void;
}) {
  return (
    <div
      className="flex flex-wrap items-center gap-1"
      role="group"
      aria-label="按状态过滤"
    >
      <ListFilter className="mr-0.5 h-3.5 w-3.5 text-muted-foreground" />
      {ENTRY_FILTERS.map((filter) => {
        const active = value === filter.value;
        return (
          <button
            key={filter.value}
            type="button"
            aria-pressed={active}
            onClick={() => onChange(filter.value)}
            className={cn(
              "inline-flex h-7 items-center gap-1 rounded-full border px-2.5 text-xs transition-colors",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
              active
                ? "border-transparent bg-primary text-primary-foreground"
                : "border-input bg-background text-muted-foreground hover:bg-accent hover:text-accent-foreground",
            )}
          >
            {filter.label}
            <span className="tabular-nums opacity-70">{counts[filter.value]}</span>
          </button>
        );
      })}
    </div>
  );
}
