import { useMemo } from "react";
import {
  ChevronDown,
  FileText,
  Languages,
  Package,
} from "lucide-react";
import { cn } from "@/lib/utils";
import { Card } from "@/components/ui/card";
import { EmptyState } from "@/components/ui/empty-state";
import type { PakFile, PakFileKind } from "@/lib/types";

interface TreeNode {
  name: string;
  path: string;
  file?: PakFile;
  children: Map<string, TreeNode>;
}

const KIND_ICON: Partial<Record<PakFileKind, React.ReactNode>> = {
  "localization-xml": <Languages className="h-4 w-4 text-blue-500" />,
  "localization-loca": <Languages className="h-4 w-4 text-indigo-500" />,
};

const KIND_LABEL: Partial<Record<PakFileKind, string>> = {
  "localization-xml": "XML",
  "localization-loca": "LOCA",
};

/** 只显示本地化文件（不含 LSX 元数据等非本地化内容） */
const LOCALIZATION_KINDS: PakFileKind[] = [
  "localization-xml",
  "localization-loca",
];

function buildTree(files: PakFile[]): TreeNode {
  const root: TreeNode = {
    name: "",
    path: "",
    children: new Map(),
  };
  for (const file of files) {
    const parts = file.name.split("/").filter(Boolean);
    let cur = root;
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      const isLeaf = i === parts.length - 1;
      if (!cur.children.has(part)) {
        cur.children.set(part, {
          name: part,
          path: parts.slice(0, i + 1).join("/"),
          file: isLeaf ? file : undefined,
          children: new Map(),
        });
      }
      cur = cur.children.get(part)!;
    }
  }
  return root;
}

function isEnglishName(name: string | null | undefined): boolean {
  return (name ?? "").toLowerCase() === "english";
}

/** 收集一个节点（含子节点）下的所有文件 */
function collectFiles(node: TreeNode): PakFile[] {
  if (node.file) return [node.file];
  const out: PakFile[] = [];
  for (const child of node.children.values()) {
    out.push(...collectFiles(child));
  }
  return out;
}

function TreeRow({
  node,
  depth,
  selectedPaths,
  disabled,
  onSelectFile,
}: {
  node: TreeNode;
  depth: number;
  selectedPaths: Set<string>;
  disabled: boolean;
  onSelectFile: (file: PakFile) => void;
}) {
  const isFolder = node.children.size > 0;

  // 默认全部展开（不维护折叠状态）
  if (isFolder) {
    const childFiles = [...node.children.values()];
    childFiles.sort((a, b) => {
      const aFolder = a.children.size > 0 ? 0 : 1;
      const bFolder = b.children.size > 0 ? 0 : 1;
      if (aFolder !== bFolder) return aFolder - bFolder;
      const aEn = isEnglishName(a.file?.language ?? a.name) ? 0 : 1;
      const bEn = isEnglishName(b.file?.language ?? b.name) ? 0 : 1;
      if (aEn !== bEn) return aEn - bEn;
      return a.name.localeCompare(b.name);
    });

    const filesInFolder = collectFiles(node);

    return (
      <div>
        <div
          className="grid w-full grid-cols-[14px_16px_minmax(0,1fr)_auto] items-center gap-1.5 rounded px-2 py-1 text-sm text-muted-foreground"
          style={{ paddingLeft: depth * 16 + 4 }}
        >
          <ChevronDown className="h-3.5 w-3.5 shrink-0" />
          <Package className="h-4 w-4 shrink-0" />
          <span className="min-w-0 truncate text-foreground">{node.name}</span>
          <span className="text-right text-[10px] tabular-nums">
            {filesInFolder.length}
          </span>
        </div>
        {childFiles.map((child) => (
          <TreeRow
            key={child.path}
            node={child}
            depth={depth + 1}
            selectedPaths={selectedPaths}
            disabled={disabled}
            onSelectFile={onSelectFile}
          />
        ))}
      </div>
    );
  }

  const file = node.file!;
  const checked = selectedPaths.has(file.name);
  return (
    <div
      className={cn(
        "flex w-full items-center gap-2 rounded px-2 py-1 text-sm transition-colors",
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring",
        checked && "bg-accent font-medium",
        disabled
          ? "cursor-not-allowed opacity-50"
          : "cursor-pointer hover:bg-accent",
      )}
      style={{ paddingLeft: depth * 16 + 20 }}
      onClick={() => {
        if (!disabled) onSelectFile(file);
      }}
      onKeyDown={(event) => {
        if (disabled) return;
        if (event.key === "Enter" || event.key === " ") {
          event.preventDefault();
          onSelectFile(file);
        }
      }}
      role="radio"
      aria-checked={checked}
      aria-disabled={disabled || undefined}
      aria-label={`选择 ${node.name}`}
      tabIndex={disabled ? -1 : 0}
      title={disabled ? `${file.name}（翻译进行中，无法切换文件）` : file.name}
      data-file-path={file.name}
    >
      <span
        aria-hidden
        className={cn(
          "flex h-4 w-4 shrink-0 items-center justify-center rounded-full border transition-colors",
          checked
            ? "border-primary bg-primary text-primary-foreground"
            : "border-input bg-background",
        )}
      >
        {checked && <span className="h-1.5 w-1.5 rounded-full bg-current" />}
      </span>
      {KIND_ICON[file.kind] ?? (
        <FileText className="h-4 w-4 text-muted-foreground" />
      )}
      <span className="min-w-0 flex-1 truncate text-left">{node.name}</span>
      {KIND_LABEL[file.kind] && (
        <span className="shrink-0 bg-secondary px-1.5 text-[10px] text-secondary-foreground">
          {KIND_LABEL[file.kind]}
        </span>
      )}
      {file.language && (
        <span className="shrink-0 text-[10px] text-muted-foreground">
          {file.language}
        </span>
      )}
    </div>
  );
}

export function FileTree({
  files,
  selectedFiles,
  onSelectionChange,
  disabled = false,
}: {
  files: PakFile[];
  /** 已选中的文件列表（单选：长度为 0 或 1） */
  selectedFiles: PakFile[];
  /** 选中状态变化回调 */
  onSelectionChange: (files: PakFile[]) => void;
  /** 翻译进行中禁用文件切换 */
  disabled?: boolean;
}) {
  // 只展示本地化文件（隐藏元数据、脚本、数据等）
  const locFiles = useMemo(
    () =>
      files
        .filter((f) => LOCALIZATION_KINDS.includes(f.kind))
        // 按语言排序：英文优先，其余按字母序
        .sort((a, b) => {
          const aEn = isEnglishName(a.language) ? 0 : 1;
          const bEn = isEnglishName(b.language) ? 0 : 1;
          if (aEn !== bEn) return aEn - bEn;
          const la = a.language ?? "zzz";
          const lb = b.language ?? "zzz";
          if (la !== lb) return la.localeCompare(lb);
          return a.name.localeCompare(b.name);
        }),
    [files],
  );
  const tree = useMemo(() => buildTree(locFiles), [locFiles]);

  const selectedPaths = useMemo(
    () => new Set(selectedFiles.map((f) => f.name)),
    [selectedFiles],
  );

  const onSelectFile = (file: PakFile) => {
    if (disabled) return;
    if (selectedPaths.has(file.name) && selectedFiles.length === 1) {
      onSelectionChange([]);
    } else {
      onSelectionChange([file]);
    }
  };

  return (
    <Card variant="flat" className="flex h-full flex-col">
      <div className="border-b p-3">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-sm font-semibold">本地化文件</h3>
            <p className="text-xs text-muted-foreground">
              {locFiles.length} 个文件，已选 {selectedFiles.length}
            </p>
          </div>
        </div>
      </div>
      <div
        className="flex-1 overflow-auto p-1"
        role="radiogroup"
        aria-label="选择要翻译的本地化文件"
      >
        {locFiles.length === 0 ? (
          <EmptyState
            icon={<Languages className="h-8 w-8" />}
            title="此 MOD 没有本地化文件"
            description="只有 Localization 目录下的 XML / LOCA 文件可以翻译。"
          />
        ) : (
          [...tree.children.values()].map((node) => (
            <TreeRow
              key={node.path}
              node={node}
              depth={0}
              selectedPaths={selectedPaths}
              disabled={disabled}
              onSelectFile={onSelectFile}
            />
          ))
        )}
      </div>
    </Card>
  );
}

export { LOCALIZATION_KINDS };
