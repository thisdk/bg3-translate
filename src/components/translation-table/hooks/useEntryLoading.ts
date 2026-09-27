import { useEffect, useRef, useState } from "react";
import { readFileEntries } from "@/lib/tauri";
import { useAppStore } from "@/store/app-store";

/**
 * 为「已勾选但尚未加载」的文件加载条目。
 *
 * 两道账本，缺一不可：
 *   - `loadedRef`：**本次挂载**里已发起（含仍在读）的文件，防止 effect 重入
 *     时重复请求，取消勾选时未完成的那次会被放回；
 *   - store 的 `loadedFileNames`：**本次会话**里已经写进 store 的文件。
 *     工作台销毁重建后 ref 归零，但条目还在 store 里，不能再读一遍
 *     （重读会用磁盘内容覆盖内存里的译文，见 effect 内的说明）。
 * 用 ref 而不是把 Set 放进依赖，是因为 Set 每次更新都产生新引用，
 * 会导致 effect 反复触发。
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
    // store 里的 `loadedFileNames` 必须一起看：工作台会被销毁重建
    // （打包页 →「返回继续编辑」、切到 home 再回来），而 `loadedRef` 随组件
    // 一起消失。只信 ref 就会把已经加载过的文件再读一遍，用磁盘内容整体替换
    // store 里的条目 —— 磁盘上那个源文件还是英文原文，于是刚翻译/手工编辑的
    // 结果全变回「待翻译」，用户再点一次打包就把英文原文写回
    // `Localization/Chinese/...`，覆盖掉上一次已经写好的中文（不可逆）。
    // 这里直接读 store 快照而不订阅它：Set 每次更新都是新引用，订阅会让
    // effect 在每次 `setFileEntries` 后多跑一遍（无意义）。
    const loaded = useAppStore.getState().loadedFileNames;
    const toLoad = selectedFiles.filter(
      (f) => !loadedRef.current.has(f.name) && !loaded.has(f.name),
    );
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
      // 这一轮请求已经被取消（换勾选 / 切 MOD / 卸载），它的 finally 不会再
      // 调用 setLoading(false)。这里必须补上，否则本 hook 的 loading 会一直
      // 留在 true：条目行不渲染（显示「加载条目…」）、翻译按钮永久禁用，
      // 直到用户再勾选一个尚未加载的文件才会恢复。
      // （卸载时这个 setState 是空操作，React 会忽略。）
      setLoading(false);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workDir, selectedKey]);

  return { loading };
}
