interface TableStatusBarProps {
  /** 可翻译条目总数 */
  total: number;
  /** 当前可见条数 */
  visibleCount: number;
  /** 是否处于搜索/过滤状态 */
  filtered: boolean;
  /** 虚拟滚动实际挂载的行数 */
  renderedRows: number;
}

/** 底部状态条：可见条数 + 虚拟滚动实际渲染行数 */
export function TableStatusBar({
  total,
  visibleCount,
  filtered,
  renderedRows,
}: TableStatusBarProps) {
  return (
    <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t px-3 py-1.5 text-[11px] text-muted-foreground">
      <span className="tabular-nums">
        显示 {visibleCount}/{total} 条
        {filtered ? "（已筛选）" : ""}
      </span>
      <span>虚拟滚动渲染 {renderedRows} 行</span>
    </div>
  );
}
