/**
 * 流式 delta 的按帧批处理。
 *
 * 为什么需要它
 * ------------
 * 后端通过 Tauri Channel 推送 `delta` 事件，一个条目可能每秒几百个 token。
 * 如果每个 token 都提交一次 store，React 就要为每个 token 重算一遍
 * 2 万条条目的派生数据（过滤、统计、计数、虚拟列表）。
 *
 * 这里把「不足一帧内的多个 delta」合并成一次提交：
 *   - 同一条目的文本直接拼接；
 *   - 一帧内所有条目合成一个批次，交给 store 的 `applyDeltas` 一次写完
 *     （一次 `set()` = 一次通知 = 一次派生重算，与并发条目数无关）；
 *   - 调度器可注入，测试里可以手动控制 flush 时机。
 *
 * 丢弃语义（避免污染已完成的条目）：
 *   - `done` / `error` 携带权威文本，此时该条目在同一帧内尚未提交的 delta
 *     必须用 `discard(id)` 丢掉，不能落后于权威文本再追加；
 *   - 取消或收尾回滚时用 `discardAll()` 丢掉整批未提交内容。
 *
 * 生命周期：`dispose()`（落地并停止）与 `resume()`（重新开始）必须成对使用 ——
 * React StrictMode 会把 effect 重放一遍，而 `useMemo` 里这个实例不会重建，
 * 只 dispose 不 resume 就等于让流式文本永久静默丢失（见 `resume` 的说明）。
 */

/** 一次 flush 里提交的一条合并结果 */
export interface DeltaCommit {
  id: string;
  /** 本帧内该条目累积的全部文本（已按到达顺序拼接） */
  text: string;
}

export interface DeltaBatcherOptions {
  /** 收到合并结果后的提交回调（store 的 applyDeltas） */
  commit: (deltas: DeltaCommit[]) => void;
  /**
   * 把 flush 排到下一帧。默认优先 requestAnimationFrame，
   * 没有 rAF 的环境（部分测试/后台）退回 setTimeout(16)。
   * 返回取消函数。
   */
  schedule?: (run: () => void) => () => void;
}

export interface DeltaBatcher {
  /** 追加一个 delta。同一个条目多次调用会在本帧内合并成一次提交。 */
  push: (id: string, text: string) => void;
  /** 立即提交所有待处理 delta */
  flush: () => void;
  /** 丢弃某个条目尚未提交的 delta（done / error 已给出权威文本） */
  discard: (id: string) => void;
  /** 丢弃全部尚未提交的 delta（取消 / 回滚收尾） */
  discardAll: () => void;
  /**
   * 恢复接受 `push`（与 [`DeltaBatcher.dispose`] 配对）。
   *
   * 为什么需要它：React StrictMode（`src/main.tsx` 就是）会在同一次挂载里先跑
   * 一遍 effect cleanup 再重跑 effect，而挂在 `useMemo` 上的 batcher 实例
   * **不会重建** —— 只 dispose 不 resume 的话，重新挂载后每一次 `push` 都被
   * dispose 的永久闩锁静默丢掉：开发模式下流式预览整个失效（译文只在 `done`
   * 一跳出现），而且没有任何报错。所以「dispose（落地并停止）+ resume（重新
   * 开始）」必须成对出现，无论生命周期怎么重放都不会卡在停止态。
   */
  resume: () => void;
  /** 组件卸载：先 flush 落地，之后不再接受 push */
  dispose: () => void;
  /** 待提交的条目数（测试/调试用） */
  pendingCount: () => number;
}

/** 默认调度器：优先 rAF（跟着渲染帧走），否则 16ms 定时器 */
function defaultSchedule(run: () => void): () => void {
  if (typeof requestAnimationFrame === "function") {
    const handle = requestAnimationFrame(run);
    return () => cancelAnimationFrame(handle);
  }
  const handle = setTimeout(run, 16);
  return () => clearTimeout(handle);
}

export function createDeltaBatcher({
  commit,
  schedule = defaultSchedule,
}: DeltaBatcherOptions): DeltaBatcher {
  /** id → 本帧累积文本，插入顺序即提交顺序 */
  const pending = new Map<string, string>();
  let scheduled = false;
  let cancelScheduled: (() => void) | null = null;
  let disposed = false;

  const cancelFrame = () => {
    if (cancelScheduled) {
      cancelScheduled();
      cancelScheduled = null;
    }
    scheduled = false;
  };

  const commitPending = () => {
    if (pending.size === 0) return;
    const batch: DeltaCommit[] = [];
    for (const [id, text] of pending) batch.push({ id, text });
    pending.clear();
    commit(batch);
  };

  const flush = () => {
    cancelFrame();
    commitPending();
  };

  const push = (id: string, text: string) => {
    if (disposed || text === "") return;
    pending.set(id, (pending.get(id) ?? "") + text);
    if (scheduled) return;
    scheduled = true;
    const cancel = schedule(() => {
      scheduled = false;
      cancelScheduled = null;
      commitPending();
    });
    // 调度器同步执行时上面已经提交过，此时不能再挂上过期的取消函数，
    // 否则后续 push 会误以为已经排过帧，永远不再提交
    if (scheduled) cancelScheduled = cancel;
  };

  const discard = (id: string) => {
    pending.delete(id);
  };

  const discardAll = () => {
    cancelFrame();
    pending.clear();
  };

  return {
    push,
    flush,
    discard,
    discardAll,
    resume: () => {
      disposed = false;
    },
    dispose: () => {
      if (disposed) return;
      disposed = true;
      flush();
    },
    pendingCount: () => pending.size,
  };
}
