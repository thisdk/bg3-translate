/**
 * AppTopBar 窗口控制的错误通道。
 *
 * `void win.minimize()` 只丢弃返回值，**不处理 rejection**：Tauri 的 window API
 * 在 IPC 不可用 / 权限问题时会 reject，于是变成未捕获的 Promise rejection
 * （与第三轮 F-26「文件对话框 reject」同型）—— 用户看到的是「点了没反应」，
 * 控制台只剩一条 unhandled rejection。
 *
 * 跑法：`bunx vitest run src/components/AppTopBar.test.tsx`
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { AppTopBar } from "./AppTopBar";
import { useAppStore } from "@/store/app-store";
import {
  click,
  findByLabel,
  mountContainer,
  waitMs,
  type Mounted,
} from "@/test-utils/dom";

const win = vi.hoisted(() => ({
  minimize: vi.fn(),
  toggleMaximize: vi.fn(),
  close: vi.fn(),
  startDragging: vi.fn(),
}));

vi.mock("@tauri-apps/api/window", () => ({
  getCurrentWindow: () => win,
}));

let mounted: Mounted;

beforeEach(() => {
  for (const fn of Object.values(win)) fn.mockReset();
  useAppStore.getState().reset();
  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

describe("AppTopBar 窗口控制", () => {
  it("最小化失败时给出错误提示，而不是未捕获的 Promise rejection", async () => {
    win.minimize.mockRejectedValue(new Error("IPC 不可用"));
    await mounted.render(<AppTopBar />);
    await waitMs(0);

    click(findByLabel(mounted.container, "最小化")!);
    await waitMs(0);

    expect(useAppStore.getState().error).toContain("IPC 不可用");
  });

  it("关闭窗口失败时同样有错误提示", async () => {
    win.close.mockRejectedValue(new Error("窗口已失效"));
    await mounted.render(<AppTopBar />);
    await waitMs(0);

    click(findByLabel(mounted.container, "关闭窗口")!);
    await waitMs(0);

    expect(useAppStore.getState().error).toContain("窗口已失效");
  });

  it("双击标题栏最大化失败时同样有错误提示", async () => {
    win.toggleMaximize.mockRejectedValue(new Error("IPC 不可用"));
    await mounted.render(<AppTopBar />);
    await waitMs(0);

    // 标题栏的双击最大化挂在容器 div 上：对 h1 派发，事件冒泡上去
    const title = mounted.container.querySelector<HTMLElement>("h1");
    title!.dispatchEvent(new MouseEvent("dblclick", { bubbles: true }));
    await waitMs(0);

    expect(useAppStore.getState().error).toContain("IPC 不可用");
  });
});
