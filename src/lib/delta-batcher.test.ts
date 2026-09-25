import { describe, expect, it } from "vitest";
import { createDeltaBatcher, type DeltaCommit } from "./delta-batcher";

/** 可手动控制「下一帧」的调度器 */
function manualScheduler() {
  let queued: (() => void) | null = null;
  let cancelled = 0;
  let scheduledCount = 0;
  return {
    schedule: (run: () => void) => {
      scheduledCount += 1;
      queued = run;
      return () => {
        cancelled += 1;
        queued = null;
      };
    },
    /** 模拟一帧到来 */
    runFrame: () => {
      const run = queued;
      queued = null;
      run?.();
    },
    hasQueued: () => queued !== null,
    cancelledCount: () => cancelled,
    scheduleCount: () => scheduledCount,
  };
}

function setup() {
  const commits: DeltaCommit[][] = [];
  const scheduler = manualScheduler();
  const batcher = createDeltaBatcher({
    commit: (batch) => commits.push(batch),
    schedule: scheduler.schedule,
  });
  return { batcher, commits, scheduler };
}

describe("createDeltaBatcher", () => {
  it("同一帧内同一条目的 delta 合并成一次提交，且顺序不变", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("e-1", "你");
    batcher.push("e-1", "好");
    batcher.push("e-1", "世");
    batcher.push("e-1", "界");

    expect(commits).toEqual([]);
    expect(batcher.pendingCount()).toBe(1);

    scheduler.runFrame();

    expect(commits).toEqual([[{ id: "e-1", text: "你好世界" }]]);
    expect(batcher.pendingCount()).toBe(0);
  });

  it("一帧内所有条目合成一个批次，顺序按首次出现", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("a", "A1");
    batcher.push("b", "B1");
    batcher.push("a", "A2");
    scheduler.runFrame();

    // 一次 commit = 一次 store 写入 = 一次通知
    expect(commits).toHaveLength(1);
    expect(commits[0]).toEqual([
      { id: "a", text: "A1A2" },
      { id: "b", text: "B1" },
    ]);
  });

  it("一帧只排一次：多次 push 不会重复调度", () => {
    const { batcher, scheduler } = setup();

    batcher.push("a", "1");
    batcher.push("a", "2");
    batcher.push("b", "3");

    expect(scheduler.scheduleCount()).toBe(1);
  });

  it("flush 立即提交，并取消已排的帧（不会二次提交）", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("a", "1");
    expect(scheduler.hasQueued()).toBe(true);

    batcher.flush();
    expect(commits).toEqual([[{ id: "a", text: "1" }]]);
    expect(scheduler.cancelledCount()).toBe(1);

    scheduler.runFrame();
    expect(commits).toHaveLength(1);
  });

  it("flush 之后可以继续累积新的一帧", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("a", "1");
    batcher.flush();
    batcher.push("a", "2");
    scheduler.runFrame();

    expect(commits).toEqual([
      [{ id: "a", text: "1" }],
      [{ id: "a", text: "2" }],
    ]);
  });

  it("discard 丢掉单个条目的待提交文本，不影响同帧其它条目", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("done-后到达的", "污染");
    batcher.push("正常条目", "文本");
    batcher.discard("done-后到达的");
    scheduler.runFrame();

    expect(commits).toEqual([[{ id: "正常条目", text: "文本" }]]);
    expect(batcher.pendingCount()).toBe(0);
  });

  it("discard 后同一条目可以重新累积", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("a", "旧");
    batcher.discard("a");
    batcher.push("a", "新");
    scheduler.runFrame();

    expect(commits).toEqual([[{ id: "a", text: "新" }]]);
  });

  it("discardAll 丢弃整批未提交内容并取消已排的帧", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("a", "1");
    batcher.push("b", "2");
    batcher.discardAll();

    expect(batcher.pendingCount()).toBe(0);
    expect(scheduler.hasQueued()).toBe(false);

    scheduler.runFrame();
    expect(commits).toEqual([]);
  });

  it("discardAll 之后仍可正常开启新一帧", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("a", "1");
    batcher.discardAll();
    batcher.push("a", "2");
    scheduler.runFrame();

    expect(commits).toEqual([[{ id: "a", text: "2" }]]);
  });

  it("没有待提交内容时 flush 不触发 commit", () => {
    const { batcher, commits } = setup();
    batcher.flush();
    expect(commits).toEqual([]);
  });

  it("空文本被忽略（不占帧、不提交）", () => {
    const { batcher, commits, scheduler } = setup();
    batcher.push("a", "");
    expect(batcher.pendingCount()).toBe(0);
    expect(scheduler.hasQueued()).toBe(false);
    scheduler.runFrame();
    expect(commits).toEqual([]);
  });

  it("dispose 先落地待提交内容，之后 push 不再接受", () => {
    const { batcher, commits, scheduler } = setup();

    batcher.push("a", "尾巴");
    batcher.dispose();

    expect(commits).toEqual([[{ id: "a", text: "尾巴" }]]);

    batcher.push("a", "迟到的");
    scheduler.runFrame();
    expect(commits).toHaveLength(1);
  });

  it("调度器同步执行时不会卡死后续 delta（回归）", () => {
    const commits: DeltaCommit[][] = [];
    const batcher = createDeltaBatcher({
      commit: (batch) => commits.push(batch),
      // 模拟「同步执行」的调度器
      schedule: (run) => {
        run();
        return () => undefined;
      },
    });

    batcher.push("a", "1");
    expect(commits).toEqual([[{ id: "a", text: "1" }]]);

    batcher.push("a", "2");
    expect(commits).toEqual([
      [{ id: "a", text: "1" }],
      [{ id: "a", text: "2" }],
    ]);
  });
});
