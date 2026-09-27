import { useCallback, useState } from "react";
import { useAppStore } from "@/store/app-store";
import type { TranslationEntry } from "@/lib/types";

export interface EntryEditing {
  editingId: string | null;
  draft: string;
  startEdit: (entry: TranslationEntry) => void;
  cancelEdit: () => void;
  saveEdit: () => void;
  revert: (entry: TranslationEntry) => void;
  onDraftChange: (value: string) => void;
  /** 切换 MOD 时清空编辑态 */
  reset: () => void;
}

/** 单条译文的就地编辑（草稿 / 保存 / 取消 / 还原） */
export function useEntryEditing(): EntryEditing {
  const updateEntry = useAppStore((s) => s.updateEntry);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const startEdit = useCallback((entry: TranslationEntry) => {
    setEditingId(entry.id);
    setDraft(entry.target);
  }, []);

  const cancelEdit = useCallback(() => setEditingId(null), []);

  const saveEdit = useCallback(() => {
    if (editingId) {
      // `error` 一起清掉：人工改好的译文已经解决了上一轮的问题，留着旧诊断会让
      // 行内一直挂着过期错误；更糟的是「全选重译」时它会作为纠错提示带给模型
      // （后端按 `entry.error` 注入，见 `planner::previous_failure_of`）。
      updateEntry(editingId, {
        target: draft,
        status: "edited",
        error: null,
      });
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
  const reset = useCallback(() => setEditingId(null), []);

  return {
    editingId,
    draft,
    startEdit,
    cancelEdit,
    saveEdit,
    revert,
    onDraftChange,
    reset,
  };
}
