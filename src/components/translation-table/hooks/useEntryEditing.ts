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
