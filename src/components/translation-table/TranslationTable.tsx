import { useEffect, useState } from "react";
import { Languages } from "lucide-react";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { useAppStore } from "@/store/app-store";
import type { EntryFilter } from "@/lib/entries";
import { EntryFilterBar } from "./EntryFilterBar";
import { EntryList } from "./EntryList";
import { TranslationToolbar } from "./TranslationToolbar";
import { useEntryDerivation } from "./hooks/useEntryDerivation";
import { useEntryEditing } from "./hooks/useEntryEditing";
import { useEntryLoading } from "./hooks/useEntryLoading";
import { useTranslationRun } from "./hooks/useTranslationRun";

/**
 * 翻译工作区容器：只负责组合。
 *
 * 数据来源与交互分别交给：
 *   - useEntryLoading    为勾选的文件加载条目
 *   - useEntryDerivation 聚合 + 统计 / 计数 / 过滤
 *   - useTranslationRun  翻译任务生命周期 + delta 按帧批处理
 *   - useEntryEditing    单条译文的就地编辑
 * 视图拆成 TranslationToolbar / EntryFilterBar / EntryList（含 EntryRow）。
 */
export function TranslationTable() {
  const workDir = useAppStore((s) => s.workDir);
  const selectedFiles = useAppStore((s) => s.selectedFiles);
  const entriesByFile = useAppStore((s) => s.entriesByFile);

  const [search, setSearch] = useState("");
  const [statusFilter, setStatusFilter] = useState<EntryFilter>("all");
  const [styleHint, setStyleHint] = useState("");

  const { loading } = useEntryLoading();
  const editing = useEntryEditing();
  const {
    workEntries,
    stats,
    counts,
    visibleEntries,
    translatableCount,
    retranslateAll,
  } = useEntryDerivation(selectedFiles, entriesByFile, search, statusFilter);
  const run = useTranslationRun({ workDir, entries: workEntries, styleHint });

  const { reset: resetEditing } = editing;
  const { resetSummary } = run;

  // 切换 MOD（workDir 变化）时重置界面态
  useEffect(() => {
    setStyleHint("");
    setSearch("");
    setStatusFilter("all");
    resetSummary();
    resetEditing();
  }, [workDir, resetEditing, resetSummary]);

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

  return (
    <Card variant="flat" className="flex h-full min-h-0 flex-col">
      <div className="shrink-0 border-b">
        <TranslationToolbar
          fileCount={selectedFiles.length}
          stats={stats}
          loading={loading}
          translating={run.translating}
          cancelling={run.cancelling}
          translatableCount={translatableCount}
          retranslateAll={retranslateAll}
          runSummary={run.runSummary}
          onTranslate={run.translateAll}
          onRetryFailed={run.retryFailed}
          onCancel={run.cancel}
        />
        <EntryFilterBar
          search={search}
          onSearchChange={setSearch}
          counts={counts}
          filter={statusFilter}
          onFilterChange={setStatusFilter}
          styleHint={styleHint}
          onStyleHintChange={setStyleHint}
          disabled={stats.total === 0}
        />
      </div>

      <EntryList
        visibleEntries={visibleEntries}
        total={stats.total}
        loading={loading}
        scrollResetKey={`${search}\u0000${statusFilter}`}
        filtered={statusFilter !== "all" || search !== ""}
        editingId={editing.editingId}
        draft={editing.draft}
        busy={run.translating}
        onDraftChange={editing.onDraftChange}
        onStartEdit={editing.startEdit}
        onSaveEdit={editing.saveEdit}
        onCancelEdit={editing.cancelEdit}
        onRevert={editing.revert}
        onRetry={run.retryOne}
        onClearFilters={() => {
          setSearch("");
          setStatusFilter("all");
        }}
      />
    </Card>
  );
}
