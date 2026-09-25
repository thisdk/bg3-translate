import type { ReactNode } from "react";
import { cn } from "@/lib/utils";

export interface EmptyStateProps {
  /** 顶部图标 */
  icon?: ReactNode;
  title: string;
  description?: ReactNode;
  /** 底部操作区 */
  action?: ReactNode;
  className?: string;
}

/** 统一的空状态 / 占位展示 */
export function EmptyState({
  icon,
  title,
  description,
  action,
  className,
}: EmptyStateProps) {
  return (
    <div
      className={cn(
        "flex h-full min-h-0 flex-col items-center justify-center gap-2 p-8 text-center",
        className,
      )}
    >
      {icon && (
        <div className="mb-1 text-muted-foreground/70" aria-hidden>
          {icon}
        </div>
      )}
      <p className="text-sm font-medium text-foreground">{title}</p>
      {description && (
        <div className="max-w-md text-xs leading-5 text-muted-foreground">
          {description}
        </div>
      )}
      {action && <div className="mt-2">{action}</div>}
    </div>
  );
}
