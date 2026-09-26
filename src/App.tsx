import { useEffect, useState, type ReactNode } from "react";
import {
  ArrowLeft,
  CheckCircle2,
  ChevronRight,
  FileArchive,
  GripVertical,
  Package,
  RotateCcw,
  X,
} from "lucide-react";
import { AppTopBar } from "@/components/AppTopBar";
import { Button } from "@/components/ui/button";
import { ErrorBanner } from "@/components/ui/error-banner";
import { FileDropZone } from "@/components/FileDropZone";
import { FileTree, LOCALIZATION_KINDS } from "@/components/FileTree";
import { GlossaryPanel } from "@/components/GlossaryPanel";
import { SettingsPanel } from "@/components/SettingsPanel";
import { TranslationTable } from "@/components/TranslationTable";
import { useAppStore } from "@/store/app-store";
import {
  mergeWithExistingTarget,
  planLocalizationWrites,
  type LocalizationWritePlan,
} from "@/lib/localization";
import {
  closeMod,
  loadLlmSettings,
  pickSavePath,
  readFileEntries,
  repackMod,
  writeFileEntries,
} from "@/lib/tauri";
import { cn } from "@/lib/utils";
import type { TranslationEntry } from "@/lib/types";

function HomePage() {
  return (
    <div className="flex min-h-0 flex-1 p-6 md:p-10">
      <FileDropZone className="h-full w-full flex-1" />
    </div>
  );
}

function FilesPage() {
  const files = useAppStore((s) => s.files);
  const selectedFiles = useAppStore((s) => s.selectedFiles);
  const setSelectedFiles = useAppStore((s) => s.setSelectedFiles);
  const workDir = useAppStore((s) => s.workDir);
  const entriesByFile = useAppStore((s) => s.entriesByFile);
  const runActive = useAppStore((s) => s.runToken !== null);
  const setStage = useAppStore((s) => s.setStage);
  const setError = useAppStore((s) => s.setError);

  const onGoPack = async () => {
    if (!workDir) return;
    // 写回闸门：`translating` 条目的半截流式文本会被后端当成真译文写进 PAK
    // （后端只在 status === "error" 时退回原文）。翻译收尾回滚之后才允许写回。
    if (useAppStore.getState().runToken !== null) {
      setError(
        "翻译仍在进行中：请等待翻译结束，或点「取消翻译」等译文本轮收尾后再打包——" +
          "否则半截译文会被写进 PAK。",
      );
      return;
    }
    try {
      // 多语言文件可能映射到同一个目标路径（Localization/Chinese/xxx），
      // 只写回优先级最高的那一份（英文 > 中文 > 其它语言）。
      const plans = planLocalizationWrites(selectedFiles, entriesByFile);
      // MOD 自带的目标文件（例如 Localization/Chinese/x.xml）要当底稿：英文
      // 文件里未翻译的条目否则会退回英文原文，把文件里已有的中文覆盖掉（R-05）。
      const shippedFiles = new Set(files.map((f) => f.name));
      for (const plan of plans) {
        const entries = shippedFiles.has(plan.fileName)
          ? await mergeWithShippedTarget(workDir, plan)
          : plan.entries;
        await writeFileEntries(workDir, plan.fileName, entries);
      }
    } catch (e) {
      setError(String(e));
      return;
    }
    setStage("done");
  };

  /**
   * 取 MOD 自带目标文件的现有内容作为写回底稿。
   * 读不到底稿时**不阻断写回**（退回「直接用计划条目」的既有行为），
   * 只在控制台留痕，避免把一次可恢复的读取失败变成打包失败。
   */
  const mergeWithShippedTarget = async (
    dir: string,
    plan: LocalizationWritePlan,
  ): Promise<TranslationEntry[]> => {
    try {
      const existing = await readFileEntries(dir, plan.fileName);
      return mergeWithExistingTarget(plan.entries, existing);
    } catch (e) {
      console.warn(`[写回] 读取底稿失败，按原样写回 ${plan.fileName}:`, e);
      return plan.entries;
    }
  };

  return (
    <div className="grid min-h-0 flex-1 grid-cols-1 gap-0 lg:grid-cols-[280px_1fr]">
      <div className="min-h-0 border-b lg:border-b-0 lg:border-r">
        <FileTree
          files={files}
          selectedFiles={selectedFiles}
          onSelectionChange={setSelectedFiles}
        />
      </div>
      <div className="flex min-h-0 min-w-0 flex-1 flex-col">
        <div className="min-h-0 flex-1">
          <TranslationTable />
        </div>
        <div className="flex shrink-0 flex-wrap items-center justify-between gap-2 border-t bg-muted/30 px-4 py-3">
          <Button variant="ghost" onClick={() => setStage("home")}>
            <ChevronRight className="h-4 w-4 rotate-180" />
            <span className="hidden sm:inline">返回</span>
          </Button>
          <Button
            onClick={onGoPack}
            disabled={!workDir || selectedFiles.length === 0}
            title={
              runActive
                ? "翻译仍在进行中：请等待翻译结束或取消后再打包"
                : undefined
            }
          >
            <CheckCircle2 className="h-4 w-4" />
            完成翻译，去打包
          </Button>
        </div>
      </div>
    </div>
  );
}

function DonePage() {
  const modFilePath = useAppStore((s) => s.modFilePath);
  const workDir = useAppStore((s) => s.workDir);
  const files = useAppStore((s) => s.files);
  const setStage = useAppStore((s) => s.setStage);
  const setError = useAppStore((s) => s.setError);
  const reset = useAppStore((s) => s.reset);
  const [packing, setPacking] = useState(false);
  const [outputPath, setOutputPath] = useState<string | null>(null);

  // 释放后端临时工作目录后再清空前端状态，避免临时文件堆到下次打开 MOD
  const onStartOver = async () => {
    try {
      await closeMod();
    } catch {
      /* 清理失败不该挡住用户重新开始 */
    }
    reset();
  };

  const onPack = async () => {
    if (!workDir || !modFilePath) return;
    setPacking(true);
    setError(null);
    try {
      const baseName = modFilePath
        .split(/[\\/]/)
        .pop()!
        .replace(/\.(pak|zip)$/i, "");
      const savePath = await pickSavePath(`${baseName}_zh.pak`);
      if (!savePath) return;
      await repackMod(workDir, savePath);
      setOutputPath(savePath);
    } catch (e) {
      setError(String(e));
    } finally {
      setPacking(false);
    }
  };

  const translatable = files.filter((f) => LOCALIZATION_KINDS.includes(f.kind));

  return (
    <div className="flex min-h-0 flex-1 overflow-auto">
      <div className="mx-auto flex w-full max-w-6xl flex-col gap-6 px-6 py-8 md:px-10">
        <div className="grid gap-5 border-b pb-6 lg:grid-cols-[minmax(0,1fr)_220px] lg:items-end">
          <div className="flex min-w-0 items-start gap-4">
            <div className="flex h-11 w-11 shrink-0 items-center justify-center rounded-md bg-primary text-primary-foreground">
              <Package className="h-5 w-5" />
            </div>
            <div className="min-w-0">
              <h2 className="text-2xl font-semibold tracking-normal">重新打包</h2>
              <p className="mt-2 max-w-2xl text-sm leading-6 text-muted-foreground">
                将已写回的本地化文件重新打包为 BG3 可加载的 .pak 文件。
              </p>
            </div>
          </div>
          <div className="rounded-md bg-muted/35 px-4 py-3">
            <div className="text-xs text-muted-foreground">可翻译文件</div>
            <div className="mt-1 text-2xl font-semibold tabular-nums">
              {translatable.length}
            </div>
          </div>
        </div>

        <div className="grid min-h-[360px] gap-6 lg:grid-cols-[minmax(0,1fr)_360px]">
          <div className="min-w-0 space-y-5">
            <section className="grid gap-3 border-b pb-5 md:grid-cols-[96px_minmax(0,1fr)]">
              <div className="text-xs font-medium text-muted-foreground">源 MOD</div>
              <div className="min-w-0 break-all text-sm font-medium leading-6">
                {modFilePath}
              </div>
            </section>

            <section
              className={cn(
                "h-[148px] rounded-md border p-4 transition-colors",
                outputPath
                  ? "border-emerald-500/30 bg-emerald-500/10"
                  : "border-dashed bg-muted/20",
              )}
            >
              {outputPath ? (
                <div className="flex h-full min-w-0 flex-col">
                  <div className="flex items-center gap-2 text-sm font-medium text-emerald-600 dark:text-emerald-400">
                    <CheckCircle2 className="h-4 w-4" />
                    打包完成
                  </div>
                  <div className="mt-3 min-h-0 flex-1 overflow-auto break-all text-sm leading-6 text-muted-foreground">
                    {outputPath}
                  </div>
                  <p className="mt-3 shrink-0 text-xs text-muted-foreground">
                    输出文件已保存，可直接导入 MOD 管理器。
                  </p>
                </div>
              ) : (
                <div className="flex h-full items-center gap-3 text-sm text-muted-foreground">
                  <FileArchive className="h-5 w-5 shrink-0" />
                  <div>
                    <div className="font-medium text-foreground">等待打包</div>
                    <div className="mt-1 text-xs">
                      选择保存位置后，输出路径会显示在这里。
                    </div>
                  </div>
                </div>
              )}
            </section>
          </div>

          <div className="flex min-h-[260px] flex-col justify-between gap-6 lg:border-l lg:pl-6">
            <div className="space-y-3">
              <Button
                onClick={onPack}
                loading={packing}
                className="h-12 w-full"
                size="lg"
              >
                <Package className="h-4 w-4" />
                {packing ? "打包中..." : "选择保存位置并打包"}
              </Button>
              <Button
                variant="outline"
                className="h-12 w-full"
                onClick={() => setStage("files")}
              >
                <ArrowLeft className="h-4 w-4" />
                返回继续编辑
              </Button>
              <Button
                variant="ghost"
                className="h-12 w-full"
                onClick={onStartOver}
              >
                <RotateCcw className="h-4 w-4" />
                开始新的 MOD
              </Button>
            </div>
            <div className="border-t pt-4 text-xs leading-5 text-muted-foreground">
              打包会使用当前工作目录中的已写回文件，不会修改原始 MOD。
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}

function ResizableSidebar({
  title,
  subtitle,
  width,
  minWidth,
  maxWidth,
  onWidthChange,
  onClose,
  children,
}: {
  title: string;
  subtitle?: string;
  width: number;
  minWidth: number;
  maxWidth: number;
  onWidthChange: (width: number) => void;
  onClose: () => void;
  children: ReactNode;
}) {
  const [resizing, setResizing] = useState(false);

  useEffect(() => {
    if (!resizing) return;

    const onMove = (event: MouseEvent) => {
      const nextWidth = window.innerWidth - event.clientX;
      onWidthChange(Math.min(maxWidth, Math.max(minWidth, nextWidth)));
    };
    const onUp = () => setResizing(false);

    window.addEventListener("mousemove", onMove);
    window.addEventListener("mouseup", onUp);
    document.body.style.cursor = "ew-resize";
    document.body.style.userSelect = "none";

    return () => {
      window.removeEventListener("mousemove", onMove);
      window.removeEventListener("mouseup", onUp);
      document.body.style.cursor = "";
      document.body.style.userSelect = "";
    };
  }, [maxWidth, minWidth, onWidthChange, resizing]);

  return (
    <div className="absolute bottom-0 right-0 top-[54px] z-30 flex">
      <button
        type="button"
        className="h-full w-screen bg-black/20 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring"
        onClick={onClose}
        aria-label="关闭侧边栏遮罩"
        tabIndex={-1}
      />
      <aside
        className="relative flex h-full shrink-0 flex-col border-l bg-background shadow-2xl"
        style={{ width }}
        role="dialog"
        aria-label={title}
      >
        <button
          type="button"
          className="absolute left-0 top-0 z-10 flex h-full w-3 -translate-x-1/2 cursor-ew-resize items-center justify-center rounded text-muted-foreground hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
          onMouseDown={(event) => {
            event.preventDefault();
            setResizing(true);
          }}
          aria-label="调整侧边栏宽度"
          title="拖动调整宽度"
        >
          <GripVertical className="h-5 w-5" />
        </button>
        <div className="flex h-[54px] shrink-0 items-center justify-between border-b px-4">
          <div className="min-w-0">
            <h2 className="truncate text-sm font-semibold">{title}</h2>
            {subtitle && (
              <p className="truncate text-xs text-muted-foreground">{subtitle}</p>
            )}
          </div>
          <Button
            size="icon"
            variant="ghost"
            className="h-9 w-9"
            onClick={onClose}
            aria-label="关闭侧边栏"
            title="关闭"
          >
            <X className="h-4 w-4" />
          </Button>
        </div>
        <div className="min-h-0 flex-1 overflow-hidden">{children}</div>
      </aside>
    </div>
  );
}

function App() {
  const stage = useAppStore((s) => s.stage);
  const error = useAppStore((s) => s.error);
  const setError = useAppStore((s) => s.setError);
  const setSettings = useAppStore((s) => s.setSettings);
  const settingsLoaded = useAppStore((s) => s.settingsLoaded);
  const [showGlossary, setShowGlossary] = useState(false);
  const [showSettings, setShowSettings] = useState(false);
  const [settingsWidth, setSettingsWidth] = useState(440);
  const [glossaryWidth, setGlossaryWidth] = useState(720);

  useEffect(() => {
    if (settingsLoaded) return;
    let cancelled = false;

    loadLlmSettings()
      .then((settings) => {
        if (!cancelled) setSettings(settings);
      })
      .catch(() => {
        /* 保留默认设置 */
      });

    return () => {
      cancelled = true;
    };
  }, [setSettings, settingsLoaded]);

  return (
    <div className="relative flex h-screen flex-col bg-background">
      <AppTopBar
        showStepper
        onOpenGlossary={() => setShowGlossary(true)}
        onOpenSettings={() => setShowSettings(true)}
      />
      <ErrorBanner message={error} onClose={() => setError(null)} />
      <div
        className={
          stage === "files"
            ? "flex min-h-0 flex-1 flex-col"
            : "flex min-h-0 flex-1 flex-col overflow-auto"
        }
      >
        {stage === "home" && <HomePage />}
        {stage === "files" && <FilesPage />}
        {stage === "done" && <DonePage />}
      </div>
      {showSettings && (
        <ResizableSidebar
          title="设置"
          subtitle="大模型与界面偏好"
          width={settingsWidth}
          minWidth={360}
          maxWidth={640}
          onWidthChange={setSettingsWidth}
          onClose={() => setShowSettings(false)}
        >
          <div className="h-full overflow-auto p-4">
            <SettingsPanel compact />
          </div>
        </ResizableSidebar>
      )}
      {showGlossary && (
        <ResizableSidebar
          title="术语表"
          subtitle="官方与自定义译名管理"
          width={glossaryWidth}
          minWidth={520}
          maxWidth={980}
          onWidthChange={setGlossaryWidth}
          onClose={() => setShowGlossary(false)}
        >
          <GlossaryPanel onClose={() => setShowGlossary(false)} embedded />
        </ResizableSidebar>
      )}
    </div>
  );
}

export default App;
