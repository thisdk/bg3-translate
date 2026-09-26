import { useEffect, useRef, useState } from "react";
import { readFileEntries } from "@/lib/tauri";
import { useAppStore } from "@/store/app-store";

/**
 * 为「已勾选但尚未加载」的文件加载条目。
 *
 * 用 ref 追踪已加载的文件，避免它进入 useEffect 依赖造成循环
 * （store 的 loadedFileNames Set 每次更新都产生新引用，会导致 effect 反复触发）。
 */
export function useEntryLoading(): { loading: boolean } {
  const workDir = useAppStore((s) => s.workDir);
  const selectedFiles = useAppStore((s) => s.selectedFiles);
  const setFileEntries = useAppStore((s) => s.setFileEntries);
  const setError = useAppStore((s) => s.setError);

  const [loading, setLoading] = useState(false);
  const loadedRef = useRef<Set<string>>(new Set());
  const lastWorkDir = useRef<string | null>(null);

  // 切换 MOD（workDir 变化）时清空已加载记录
  if (workDir !== lastWorkDir.current) {
    lastWorkDir.current = workDir;
    loadedRef.current = new Set();
  }

  // 依赖只用 workDir 和 selectedFiles 的稳定派生值（文件名列表），避免循环
  const selectedKey = selectedFiles.map((f) => f.name).join("|");

  useEffect(() => {
    if (!workDir || selectedFiles.length === 0) return;
    const toLoad = selectedFiles.filter((f) => !loadedRef.current.has(f.name));
    if (toLoad.length === 0) return;
    // 立即标记为已加载，防止 effect 重入时重复请求
    toLoad.forEach((f) => loadedRef.current.add(f.name));

    let cancelled = false;
    /**
     * 本次 effect 里已经拿到结果的请求。
     *
     * cleanup 只能把「还没加载完」的文件放回待加载集合：已经加载完成的文件
     * 若被放回去，之后每次新增勾选都会重读一遍全部文件 —— 既重复 IPC，
     * 又会用磁盘内容整体替换 store 里的条目，把用户手工编辑过的译文和本轮
     * 已翻译的结果悄悄丢掉。
     */
    const settled = new Set<string>();
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
          })
          .finally(() => {
            settled.add(f.name);
          }),
      ),
    ).finally(() => {
      if (!cancelled) setLoading(false);
    });
    return () => {
      cancelled = true;
      // 未完成的加载要把文件名从已加载集合里移除，
      // 否则 StrictMode 双调用 / 快速切换选中时会漏加载条目
      toLoad.forEach((f) => {
        if (!settled.has(f.name)) loadedRef.current.delete(f.name);
      });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workDir, selectedKey]);

  return { loading };
}
