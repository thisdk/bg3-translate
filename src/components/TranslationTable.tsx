import {
  memo,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import {
  AlertCircle,
  Check,
  CircleStop,
  Edit3,
  Eraser,
  Languages,
  ListFilter,
  Loader2,
  Play,
  RotateCcw,
  Search,
  WandSparkles,
  X,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { Input } from "@/components/ui/input";
import { Progress } from "@/components/ui/progress";
import { Textarea } from "@/components/ui/textarea";
import { cancelTranslation, readFileEntries, translateEntries } from "@/lib/tauri";
import {
  ENTRY_FILTERS,
  buildRetryRequest,
  buildTranslationRequest,
  computeEntryStats,
  countByFilter,
  filterEntries,
  isTranslatableNow,
  isTranslationWorkItem,
  shortSourcePath,
  type EntryFilter,
} from "@/lib/entries";
import { useAppStore } from "@/store/app-store";
import type { TranslationEntry, TranslationStatus } from "@/lib/types";

const STATUS_META: Record<
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
const ESTIMATED_ROW_HEIGHT = 118;

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

const EntryRow = memo(function EntryRow({
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

function FilterChips({
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

export function TranslationTable() {
  const workDir = useAppStore((s) => s.workDir);
  const selectedFiles = useAppStore((s) => s.selectedFiles);
  const entriesByFile = useAppStore((s) => s.entriesByFile);
  const setFileEntries = useAppStore((s) => s.setFileEntries);
  const updateEntry = useAppStore((s) => s.updateEntry);
  const setEntryStatus = useAppStore((s) => s.setEntryStatus);
  const appendDelta = useAppStore((s) => s.appendDelta);
  const setError = useAppStore((s) => s.setError);

  const [loading, setLoading] = useState(false);
  const [translating, setTranslating] = useState(false);
  const [cancelling, setCancelling] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");
  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<EntryFilter>("all");
  const [styleHint, setStyleHint] = useState("");
  const [runSummary, setRunSummary] = useState<{
    total: number;
    failed: number;
  } | null>(null);

  // 用 ref 追踪已加载的文件，避免它进入 useEffect 依赖造成循环
  // （store 的 loadedFileNames Set 每次更新都产生新引用，会导致 effect 反复触发）
  const loadedRef = useRef<Set<string>>(new Set());
  const lastWorkDir = useRef<string | null>(null);
  const cancelRequestedRef = useRef(false);
  const completedIdsRef = useRef<Set<string>>(new Set());
  /** 本轮进入过「翻译中」但还没收到 done/error 的条目，用于收尾兜底 */
  const inFlightIdsRef = useRef<Set<string>>(new Set());
  const runningRef = useRef(false);
  const scrollRef = useRef<HTMLDivElement>(null);

  // 切换 MOD（workDir 变化）时清空已加载记录
  if (workDir !== lastWorkDir.current) {
    lastWorkDir.current = workDir;
    loadedRef.current = new Set();
  }

  useEffect(() => {
    setStyleHint("");
    setSearch("");
    setStatusFilter("all");
    setRunSummary(null);
    setEditingId(null);
  }, [workDir]);

  // 选中文件变化时，为新加入且未加载的文件加载条目
  // 依赖只用 workDir 和 selectedFiles 的稳定派生值（文件名列表），避免循环
  const selectedKey = selectedFiles.map((f) => f.name).join("|");
  useEffect(() => {
    if (!workDir || selectedFiles.length === 0) return;
    const toLoad = selectedFiles.filter((f) => !loadedRef.current.has(f.name));
    if (toLoad.length === 0) return;
    // 立即标记为已加载，防止 effect 重入时重复请求
    toLoad.forEach((f) => loadedRef.current.add(f.name));

    let cancelled = false;
    setLoading(true);
    setError(null);
    Promise.all(
      toLoad.map((f) =>
        readFileEntries(workDir, f.name)
          .then((entries) => {
            if (!cancelled) setFileEntries(f.name, entries);
          })
          .catch((e) => {
            if (!cancelled) {
              setError(`${f.name}: ${String(e)}`);
              setFileEntries(f.name, []);
            }
          }),
      ),
    ).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
      // 未完成的加载要把文件名从已加载集合里移除，
      // 否则 StrictMode 双调用 / 快速切换选中时会漏加载条目
      toLoad.forEach((f) => loadedRef.current.delete(f.name));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workDir, selectedKey]);

  // 聚合所有选中文件的条目
  const allEntries = useMemo(() => {
    const out: TranslationEntry[] = [];
    for (const f of selectedFiles) {
      const list = entriesByFile[f.name];
      if (list) out.push(...list);
    }
    return out;
  }, [selectedFiles, entriesByFile]);

  const workEntries = useMemo(
    () => allEntries.filter(isTranslationWorkItem),
    [allEntries],
  );

  const stats = useMemo(() => computeEntryStats(workEntries), [workEntries]);
  const counts = useMemo(() => countByFilter(workEntries), [workEntries]);

  const visibleEntries = useMemo(
    () => filterEntries(workEntries, { query: search, filter: statusFilter }),
    [workEntries, search, statusFilter],
  );

  const translatableCount = useMemo(
    () => workEntries.filter(isTranslatableNow).length,
    [workEntries],
  );
  const retranslateAll = translatableCount === 0 && stats.total > 0;

  // ── 虚拟滚动 ──
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
  }, [search, statusFilter]);

  const onTranslated = useCallback(
    (entryId: string, text: string) => {
      completedIdsRef.current.add(entryId);
      updateEntry(entryId, { target: text, status: "translated", error: null });
    },
    [updateEntry],
  );

  /** 统一的一次翻译执行：事件回调 + 取消回滚 */
  const runTranslation = useCallback(
    async (request: TranslationEntry[]) => {
      if (!workDir || request.length === 0 || runningRef.current) return;
      runningRef.current = true;
      cancelRequestedRef.current = false;
      completedIdsRef.current = new Set();
      inFlightIdsRef.current = new Set();
      setTranslating(true);
      setCancelling(false);
      setRunSummary(null);
      setError(null);

      try {
        await translateEntries(
          workDir,
          request,
          styleHint.trim(),
          (event) => {
            if (cancelRequestedRef.current && event.type !== "all_done") return;
            switch (event.type) {
              case "progress":
                if (event.status === "translating") {
                  inFlightIdsRef.current.add(event.entryId);
                } else {
                  inFlightIdsRef.current.delete(event.entryId);
                }
                setEntryStatus(event.entryId, event.status);
                break;
              case "delta":
                inFlightIdsRef.current.add(event.entryId);
                appendDelta(event.entryId, event.text);
                break;
              case "done":
                completedIdsRef.current.add(event.entryId);
                inFlightIdsRef.current.delete(event.entryId);
                onTranslated(event.entryId, event.text);
                break;
              case "error":
                inFlightIdsRef.current.delete(event.entryId);
                updateEntry(event.entryId, {
                  status: "error",
                  error: event.message,
                });
                break;
              case "all_done":
                setRunSummary({ total: event.total, failed: event.failed });
                break;
            }
          },
        );
      } catch (e) {
        if (!cancelRequestedRef.current) {
          setError(String(e));
        }
      } finally {
        if (cancelRequestedRef.current) {
          // 取消：未完成的条目回滚为待翻译
          for (const entry of request) {
            if (!completedIdsRef.current.has(entry.id)) {
              updateEntry(entry.id, {
                target: "",
                status: "pending",
                error: null,
              });
            }
          }
        } else {
          // 正常结束：把「翻译中」但没收到 done/error 的条目回滚，避免计数卡住
          for (const id of inFlightIdsRef.current) {
            updateEntry(id, { target: "", status: "pending", error: null });
          }
        }
        inFlightIdsRef.current = new Set();
        runningRef.current = false;
        setTranslating(false);
        setCancelling(false);
        cancelRequestedRef.current = false;
      }
    },
    [appendDelta, onTranslated, setEntryStatus, setError, styleHint, updateEntry, workDir],
  );

  const onTranslate = async () => {
    const plan = buildTranslationRequest(workEntries);
    if (plan.request.length === 0) {
      setError("没有可翻译的条目");
      return;
    }
    if (plan.retranslateAll) {
      for (const entry of plan.request) {
        updateEntry(entry.id, { target: "", status: "pending", error: null });
      }
    }
    await runTranslation(plan.request);
  };

  const onRetryFailed = async () => {
    const request = buildRetryRequest(workEntries);
    if (request.length === 0) return;
    for (const entry of request) {
      updateEntry(entry.id, { target: "", status: "pending", error: null });
    }
    await runTranslation(request);
  };

  const onRetryOne = async (id: string) => {
    const entry = workEntries.find((e) => e.id === id);
    if (!entry) return;
    const request = {
      ...entry,
      target: "",
      status: "pending" as TranslationStatus,
      error: null,
    };
    updateEntry(id, { target: "", status: "pending", error: null });
    await runTranslation([request]);
  };

  const onCancelTranslate = async () => {
    if (!translating || cancelling) return;
    cancelRequestedRef.current = true;
    setCancelling(true);
    setError(null);
    try {
      await cancelTranslation();
    } catch (e) {
      cancelRequestedRef.current = false;
      setError(`取消翻译失败: ${String(e)}`);
      setCancelling(false);
    }
  };

  const startEdit = useCallback((entry: TranslationEntry) => {
    setEditingId(entry.id);
    setDraft(entry.target);
  }, []);

  const cancelEdit = useCallback(() => setEditingId(null), []);

  const saveEdit = useCallback(() => {
    if (editingId) {
      updateEntry(editingId, { target: draft, status: "edited" });
    }
    setEditingId(null);
  }, [draft, editingId, updateEntry]);

  const revert = useCallback(
    (entry: TranslationEntry) => {
      updateEntry(entry.id, { target: "", status: "pending", error: null });
    },
    [updateEntry],
  );

  const onDraftChange = useCallback((value: string) => setDraft(value), []);

  if (selectedFiles.length === 0) {
    return (
      <Card variant="flat" className="flex h-full min-h-0 flex-col">
        <EmptyState
          icon={<Languages className="h-10 w-10" />}
          title="从左侧勾选本地化文件开始翻译"
          description="支持勾选多个文件一起翻译；勾选后这里会列出所有可翻译条目。"
        />
      </Card>
    );
  }

  const virtualItems = virtualizer.getVirtualItems();

  return (
    <Card variant="flat" className="flex h-full min-h-0 flex-col">
      {/* 工具栏 */}
      <div className="shrink-0 border-b">
        <div className="flex min-h-8 flex-wrap items-center justify-between gap-3 px-3 pb-2 pt-2.5">
          <div className="min-w-0">
            <h3 className="text-sm font-semibold">
              翻译工作区
              <span className="ml-2 text-xs font-normal text-muted-foreground">
                {selectedFiles.length} 个文件，{stats.total} 条
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
                onClick={onCancelTranslate}
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

        {/* 搜索 + 状态过滤 */}
        <div className="flex flex-wrap items-center gap-2 px-3 pb-2">
          <div className="relative min-w-[200px] flex-1">
            <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              placeholder="搜索原文、译文或 contentuid…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              disabled={stats.total === 0}
              className="h-8 pl-8 text-xs"
              aria-label="搜索条目"
            />
            {search && (
              <button
                type="button"
                onClick={() => setSearch("")}
                aria-label="清空搜索"
                className="absolute right-1.5 top-1/2 flex h-5 w-5 -translate-y-1/2 items-center justify-center rounded text-muted-foreground hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              >
                <X className="h-3 w-3" />
              </button>
            )}
          </div>
          <FilterChips
            counts={counts}
            value={statusFilter}
            onChange={setStatusFilter}
          />
        </div>

        {/* 语境提示 */}
        <div className="px-3 pb-2.5">
          <div className="relative">
            <WandSparkles className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
            <Input
              value={styleHint}
              onChange={(e) => setStyleHint(e.target.value)}
              placeholder="本 MOD 语境：例如 XX 是姿势名称，相关名词按姿势类型翻译"
              className="h-8 pl-8 text-xs"
              aria-label="翻译风格提示"
            />
          </div>
        </div>
      </div>

      {/* 条目列表（虚拟滚动） */}
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <EmptyState
            icon={<Loader2 className="h-6 w-6 animate-spin" />}
            title="加载条目…"
          />
        ) : stats.total === 0 ? (
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
              <Button
                size="sm"
                variant="outline"
                onClick={() => {
                  setSearch("");
                  setStatusFilter("all");
                }}
              >
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
                    busy={translating}
                    onDraftChange={onDraftChange}
                    onStartEdit={startEdit}
                    onSaveEdit={saveEdit}
                    onCancelEdit={cancelEdit}
                    onRevert={revert}
                    onRetry={onRetryOne}
                  />
                </div>
              );
            })}
          </div>
        )}
      </div>

      {/* 底部状态条 */}
      {stats.total > 0 && (
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t px-3 py-1.5 text-[11px] text-muted-foreground">
          <span className="tabular-nums">
            显示 {visibleEntries.length}/{stats.total} 条
            {statusFilter !== "all" || search ? "（已筛选）" : ""}
          </span>
          <span>虚拟滚动渲染 {virtualItems.length} 行</span>
        </div>
      )}
    </Card>
  );
}
