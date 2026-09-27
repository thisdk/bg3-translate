import { useEffect, useMemo, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { readTextFile } from "@tauri-apps/plugin-fs";
import {
  Check,
  Edit3,
  Loader2,
  Plus,
  RotateCcw,
  Search,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import { ErrorBanner } from "@/components/ui/error-banner";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { cn } from "@/lib/utils";
import {
  addGlossaryEntry,
  deleteGlossaryEntry,
  importGlossary,
  listGlossary,
  resetGlossary,
  updateGlossaryEntry,
} from "@/lib/tauri";
import { AppTopBar } from "@/components/AppTopBar";
import type { Glossary, GlossaryEntry } from "@/lib/types";

function newEmptyEntry(): GlossaryEntry {
  return {
    source: "",
    target: "",
    sourceKind: "user",
    enabled: true,
    ambiguous: false,
    wholeWord: true,
    caseSensitive: false,
    count: 0,
  };
}

/**
 * 每一行的稳定定位键：`source#组内序号`（第几次出现）。
 *
 * 为什么不能只用 `source`：术语表里**真的有重复 source** —— 官方样本
 * `samples/bg3-official-glossary.json` 清洗后就有 6 组（`'Jaheira'` 与 `Jaheira`
 * 这类「带引号 / 不带引号」变体去掉引号后同名，组内译文相同），Rust 侧
 * `real_glossary_already_contains_duplicate_sources` 把它钉住了；用户导入带重复的
 * JSON、或把某条的 source 改成已有的 source（后端 `update` 只替换第一条）同样会产生。
 * 用 `source` 当 React key 会触发 React 的重复 key 警告，并在列表增删时复用错行。
 *
 * 序号在**整个术语表**范围内计算，与搜索 / 过滤 / 「加载更多」无关：过滤只缩小
 * 可见子集，不改变任何一行的序号取值，所以键不会在过滤前后错位。
 *
 * 为什么不用「稳定 id」：`GlossaryEntry` 没有 id 字段，加它要动 IPC 契约
 * （`src/lib/types.ts` + Rust 结构体 + check_ipc_contract.py），本轮不做，已列 backlog。
 */
function buildRowKeys(terms: GlossaryEntry[]): Map<GlossaryEntry, string> {
  const seen = new Map<string, number>();
  const keys = new Map<GlossaryEntry, string>();
  for (const term of terms) {
    const ordinal = seen.get(term.source) ?? 0;
    seen.set(term.source, ordinal + 1);
    keys.set(term, `${term.source}#${ordinal}`);
  }
  return keys;
}

export function GlossaryPanel({
  onClose,
  embedded = false,
}: {
  onClose: () => void;
  embedded?: boolean;
}) {
  const [glossary, setGlossary] = useState<Glossary | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [search, setSearch] = useState("");
  const [editing, setEditing] = useState<GlossaryEntry | null>(null);
  const [editOriginal, setEditOriginal] = useState<string | null>(null);
  const [limit, setLimit] = useState(200);
  const [saving, setSaving] = useState(false);
  /**
   * 正在删除的那一行的**行键**（不是 source）。
   *
   * 术语表里存在重复 source（官方样本清洗后就有 6 组，见 `buildRowKeys`），
   * 按 source 置忙会让重复的两行同时转圈，而且用户根本分不清点中了哪一行。
   */
  const [busyRow, setBusyRow] = useState<string | null>(null);

  useEffect(() => {
    refresh();
  }, []);

  const refresh = async () => {
    setLoading(true);
    setError(null);
    try {
      const g = await listGlossary();
      setGlossary(g);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  // 过滤 + 搜索（只渲染前 limit 条，避免 20K 条卡顿）
  const visible = useMemo(() => {
    if (!glossary) return [];
    const q = search.trim().toLowerCase();
    return glossary.terms.filter(
      (t) =>
        !q ||
        t.source.toLowerCase().includes(q) ||
        t.target.toLowerCase().includes(q),
    );
  }, [glossary, search]);

  /** 行键索引：条目对象 → 稳定行键（同一份 glossary 内对象引用唯一） */
  const rowKeys = useMemo(
    () => buildRowKeys(glossary?.terms ?? []),
    [glossary],
  );

  const onSave = async () => {
    if (!editing || saving) return;
    if (!editing.source.trim() || !editing.target.trim()) {
      setError("术语的中英文均不能为空");
      return;
    }
    setError(null);
    setSaving(true);
    try {
      let g: Glossary;
      if (editOriginal) {
        g = await updateGlossaryEntry(editOriginal, editing);
      } else {
        g = await addGlossaryEntry(editing);
      }
      setGlossary(g);
      setEditing(null);
      setEditOriginal(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  /**
   * 删除一行。`rowKey` 用来定位「是哪一行在转圈」，`source` 才是发给后端的主键
   * （后端按 source 命中；重复 source 时它会删掉该 source 的全部条目，见报告）。
   */
  const onDelete = async (rowKey: string, source: string) => {
    setError(null);
    setBusyRow(rowKey);
    try {
      const g = await deleteGlossaryEntry(source);
      setGlossary(g);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyRow(null);
    }
  };

  const onReset = async () => {
    if (!confirm("确定要重置术语表为内置官方种子吗？所有用户自定义将被清除。")) return;
    setError(null);
    try {
      const g = await resetGlossary();
      setGlossary(g);
    } catch (e) {
      setError(String(e));
    }
  };

  const onImport = async () => {
    // 用 Tauri 原生对话框（浏览器 <input> 在 Tauri 沙箱里读不到真实文件内容）
    let selected: string | string[] | null = null;
    try {
      selected = await openDialog({
        multiple: false,
        filters: [{ name: "术语表 JSON", extensions: ["json"] }],
      });
    } catch (e) {
      // 对话框 reject（IPC 不可用 / 权限问题）不能变成未捕获的 Promise rejection：
      // 用户看不到任何反馈，控制台只剩一条 unhandled rejection
      setError(String(e));
      return;
    }
    const filePath = typeof selected === "string" ? selected : null;
    if (!filePath) return;

    setError(null);
    setLoading(true);
    try {
      const text = await readTextFile(filePath);
      const g = await importGlossary(text);
      setGlossary(g);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  };

  const startEdit = (entry: GlossaryEntry) => {
    setEditing({ ...entry });
    setEditOriginal(entry.source);
  };

  const startAdd = () => {
    setEditing(newEmptyEntry());
    setEditOriginal(null);
  };

  return (
    <div className="flex h-full flex-col">
      {!embedded && (
        <AppTopBar
          title="术语表"
          subtitle={
            glossary
              ? `${glossary.terms.length} 条术语${
                  visible.length !== glossary.terms.length
                    ? `，当前显示 ${visible.length}`
                    : ""
                }`
              : "加载中..."
          }
          onClose={onClose}
          actions={
            <div className="flex items-center gap-2">
              <Button size="sm" variant="outline" onClick={onImport}>
                <Upload className="h-4 w-4" />
                <span className="hidden sm:inline">导入</span>
              </Button>
              <Button size="sm" variant="outline" onClick={onReset}>
                <RotateCcw className="h-4 w-4" />
                <span className="hidden sm:inline">重置</span>
              </Button>
              <Button size="sm" onClick={startAdd}>
                <Plus className="h-4 w-4" />
                <span className="hidden sm:inline">新增</span>
              </Button>
            </div>
          }
        />
      )}
      {embedded && (
        <div className="flex shrink-0 items-center justify-between gap-2 border-b px-4 py-2">
          <div className="min-w-0 text-xs text-muted-foreground">
            {glossary
              ? `${glossary.terms.length} 条术语${
                  visible.length !== glossary.terms.length
                    ? `，当前显示 ${visible.length}`
                    : ""
                }`
              : "加载中..."}
          </div>
          <div className="flex shrink-0 items-center gap-2">
            <Button size="sm" variant="outline" onClick={onImport}>
              <Upload className="h-4 w-4" />
              <span className="hidden sm:inline">导入</span>
            </Button>
            <Button size="sm" variant="outline" onClick={onReset}>
              <RotateCcw className="h-4 w-4" />
              <span className="hidden sm:inline">重置</span>
            </Button>
            <Button size="sm" onClick={startAdd}>
              <Plus className="h-4 w-4" />
              <span className="hidden sm:inline">新增</span>
            </Button>
          </div>
        </div>
      )}
      {error && (
        <ErrorBanner message={error} onClose={() => setError(null)} compact />
      )}

      {/* 搜索 */}
      <div className="flex flex-wrap items-center gap-2 border-b px-4 py-2">
        <div className="relative flex-1">
          <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
          <Input
            placeholder="搜索术语（英文或中文）…"
            value={search}
            onChange={(e) => {
              setSearch(e.target.value);
              setLimit(200);
            }}
            className="h-8 pl-8 text-xs"
            aria-label="搜索术语"
          />
        </div>
      </div>

      {/* 列表 */}
      <div className="min-h-0 flex-1 overflow-auto">
        {loading ? (
          <EmptyState
            icon={<Loader2 className="h-6 w-6 animate-spin" />}
            title="加载术语表…"
          />
        ) : visible.length === 0 ? (
          <EmptyState
            title={search ? "没有匹配的术语" : "术语表是空的"}
            description={
              search
                ? "换个关键词试试。"
                : "可以点右上角「新增」手工添加，或「导入」官方术语表 JSON。"
            }
            action={
              search ? (
                <Button size="sm" variant="outline" onClick={() => setSearch("")}>
                  清除搜索
                </Button>
              ) : undefined
            }
          />
        ) : (
          <table className="w-full text-sm">
            <thead className="sticky top-0 bg-muted/80 backdrop-blur">
              <tr className="text-left text-xs text-muted-foreground">
                <th className="px-4 py-2 font-medium">英文 (source)</th>
                <th className="px-4 py-2 font-medium">中文 (target)</th>
                <th className="px-4 py-2 font-medium">操作</th>
              </tr>
            </thead>
            <tbody className="divide-y">
              {visible.slice(0, limit).map((t, index) => {
                // 行键与「哪一行在转圈」都用它定位（重复 source 时 source 不唯一）
                const rowKey = rowKeys.get(t) ?? `${t.source}#${index}`;
                return (
                  <tr
                    key={rowKey}
                    className={cn(
                      "hover:bg-accent/50",
                      !t.enabled &&
                        "bg-muted/30 text-muted-foreground opacity-60 hover:bg-muted/40",
                    )}
                  >
                    <td className="max-w-[180px] truncate px-4 py-1.5 sm:max-w-[280px]" title={t.source}>
                      {t.source}
                    </td>
                    <td className="max-w-[180px] truncate px-4 py-1.5 sm:max-w-[280px]" title={t.target}>
                      {t.target}
                    </td>
                    <td className="px-4 py-1.5">
                      <div className="flex gap-1">
                        <Button
                          size="sm"
                          variant="ghost"
                          className="h-6 px-1.5"
                          onClick={() => startEdit(t)}
                          aria-label={`编辑术语 ${t.source}`}
                          title="编辑"
                        >
                          <Edit3 className="h-3 w-3" />
                        </Button>
                        {t.sourceKind !== "official" && (
                          <Button
                            size="sm"
                            variant="ghost"
                            className="h-6 px-1.5 text-destructive hover:text-destructive"
                            onClick={() => onDelete(rowKey, t.source)}
                            loading={busyRow === rowKey}
                            aria-label={`删除术语 ${t.source}`}
                            title="删除"
                          >
                            <Trash2 className="h-3 w-3" />
                          </Button>
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
        {visible.length > limit && (
          <div className="border-t p-3 text-center">
            <Button variant="outline" size="sm" onClick={() => setLimit(limit + 200)}>
              加载更多（剩余 {visible.length - limit} 条）
            </Button>
          </div>
        )}
      </div>

      {/* 编辑弹层 */}
      {editing && (
        <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/40 p-4">
          <Card className="w-full max-w-md p-5">
            <div className="mb-4 flex items-center justify-between">
              <h3 className="text-sm font-semibold">
                {editOriginal ? "编辑术语" : "新增术语"}
              </h3>
              <Button
                size="sm"
                variant="ghost"
                className="h-7 w-7 p-0"
                onClick={() => {
                  setEditing(null);
                  setEditOriginal(null);
                }}
                aria-label="关闭"
                title="关闭"
              >
                <X className="h-4 w-4" />
              </Button>
            </div>
            <div className="space-y-3">
              <div className="space-y-1">
                <Label className="text-xs">英文 (source)</Label>
                <Textarea
                  value={editing.source}
                  onChange={(e) => setEditing({ ...editing, source: e.target.value })}
                  className="min-h-[40px] text-sm"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">中文 (target)</Label>
                <Textarea
                  value={editing.target}
                  onChange={(e) => setEditing({ ...editing, target: e.target.value })}
                  className="min-h-[40px] text-sm"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">选项</Label>
                <div className="flex flex-wrap gap-4 pt-1 text-xs">
                  <label className="flex items-center gap-1.5">
                    <input
                      type="checkbox"
                      checked={editing.enabled}
                      onChange={(e) =>
                        setEditing({ ...editing, enabled: e.target.checked })
                      }
                    />
                    启用
                  </label>
                  <label className="flex items-center gap-1.5">
                    <input
                      type="checkbox"
                      checked={editing.wholeWord}
                      onChange={(e) =>
                        setEditing({ ...editing, wholeWord: e.target.checked })
                      }
                    />
                    整词匹配
                  </label>
                </div>
              </div>
            </div>
            <div className="mt-4 flex justify-end gap-2">
              <Button
                variant="outline"
                size="sm"
                onClick={() => {
                  setEditing(null);
                  setEditOriginal(null);
                }}
              >
                取消
              </Button>
              <Button size="sm" onClick={onSave} loading={saving}>
                {!saving && <Check className="h-4 w-4" />}
                {saving ? "保存中…" : "保存"}
              </Button>
            </div>
          </Card>
        </div>
      )}
    </div>
  );
}
