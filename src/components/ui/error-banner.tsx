import { useEffect, useMemo, useState } from "react";
import { AlertTriangle, Check, ChevronDown, ChevronUp, Copy, X } from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/** 把后端错误文本拆成便于阅读的多行 */
export function splitErrorLines(message: string): string[] {
  const lines = message
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter((line) => line.length > 0);
  return lines.length > 0 ? lines : [message.trim()];
}

export interface ErrorBannerProps {
  /** 错误文本；为空时不渲染 */
  message: string | null;
  /** 手动关闭回调 */
  onClose?: () => void;
  /** 折叠状态下最多显示几行 */
  collapsedLines?: number;
  className?: string;
  /** 紧凑模式（侧边栏内使用） */
  compact?: boolean;
}

/**
 * 轻量错误提示条：可手动关闭、多行展示、可复制、超长自动折叠。
 * 不依赖任何第三方组件库。
 */
export function ErrorBanner({
  message,
  onClose,
  collapsedLines = 3,
  className,
  compact = false,
}: ErrorBannerProps) {
  const [expanded, setExpanded] = useState(false);
  const [copied, setCopied] = useState(false);

  const lines = useMemo(
    () => (message ? splitErrorLines(message) : []),
    [message],
  );

  // 换一条新的错误时重新折叠
  useEffect(() => {
    setExpanded(false);
    setCopied(false);
  }, [message]);

  // Esc 关闭
  useEffect(() => {
    if (!message || !onClose) return;
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") onClose();
    };
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [message, onClose]);

  if (!message || lines.length === 0) return null;

  const overflow = lines.length > collapsedLines;
  const visibleLines = expanded || !overflow ? lines : lines.slice(0, collapsedLines);
  const hiddenCount = lines.length - visibleLines.length;

  const onCopy = async () => {
    try {
      await navigator.clipboard.writeText(message);
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {
      /* 剪贴板不可用时静默忽略 */
    }
  };

  return (
    <div
      role="alert"
      aria-live="assertive"
      className={cn(
        "shrink-0 border-b border-destructive/40 bg-destructive/10 text-destructive",
        className,
      )}
    >
      <div className={cn("flex items-start gap-2", compact ? "px-3 py-2" : "px-4 py-2.5")}>
        <AlertTriangle className="mt-0.5 h-4 w-4 shrink-0" aria-hidden />
        <div className="min-w-0 flex-1">
          <div className="text-xs font-semibold">出错了</div>
          <ul
            className={cn(
              "mt-1 space-y-0.5 text-xs leading-5",
              expanded && "max-h-40 overflow-auto pr-1",
            )}
          >
            {visibleLines.map((line, index) => (
              <li
                key={`${index}-${line}`}
                className="break-words [overflow-wrap:anywhere]"
              >
                {lines.length > 1 && (
                  <span className="mr-1 select-none opacity-60">{index + 1}.</span>
                )}
                <span className="whitespace-pre-wrap">{line}</span>
              </li>
            ))}
          </ul>
          <div className="mt-1.5 flex flex-wrap items-center gap-1">
            {overflow && (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                className="h-6 gap-1 px-1.5 text-xs text-destructive hover:bg-destructive/15 hover:text-destructive"
                onClick={() => setExpanded((v) => !v)}
              >
                {expanded ? (
                  <ChevronUp className="h-3 w-3" />
                ) : (
                  <ChevronDown className="h-3 w-3" />
                )}
                {expanded ? "收起" : `展开全部（还有 ${hiddenCount} 行）`}
              </Button>
            )}
            <Button
              type="button"
              size="sm"
              variant="ghost"
              className="h-6 gap-1 px-1.5 text-xs text-destructive hover:bg-destructive/15 hover:text-destructive"
              onClick={onCopy}
            >
              {copied ? <Check className="h-3 w-3" /> : <Copy className="h-3 w-3" />}
              {copied ? "已复制" : "复制详情"}
            </Button>
          </div>
        </div>
        {onClose && (
          <Button
            type="button"
            size="icon"
            variant="ghost"
            className="h-6 w-6 shrink-0 text-destructive hover:bg-destructive/15 hover:text-destructive"
            onClick={onClose}
            aria-label="关闭错误提示"
            title="关闭（Esc）"
          >
            <X className="h-3.5 w-3.5" />
          </Button>
        )}
      </div>
    </div>
  );
}
