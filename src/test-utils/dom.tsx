/**
 * 无第三方依赖的组件测试工具。
 *
 * 项目里没有 @testing-library/*，既有测试都是 react-dom/client + act 手写；
 * 这里把公共部分（挂载、事件派发、jsdom 布局桩）收敛成一个小模块，
 * 供新增的组件测试复用，避免每个测试文件复制一遍。
 *
 * 本文件不 import vitest，因此能被 `tsc -b` 严格模式直接编译。
 */
import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

/** 单行高度（与虚拟滚动的 estimateSize 保持一致） */
const ROW_HEIGHT = 118;
/** 模拟的滚动视口高度 */
const VIEWPORT_HEIGHT = 600;

/**
 * jsdom 不做布局计算。@tanstack/react-virtual 依赖 getBoundingClientRect /
 * offsetHeight / offsetWidth 才能算出可见区间：这里给滚动容器 800x600、
 * 给每个条目行（带 data-index）118 的高度。返回还原函数。
 */
export function stubVirtualScrollLayout(): () => void {
  const originalRect = Element.prototype.getBoundingClientRect;
  const originalOffsetHeight = Object.getOwnPropertyDescriptor(
    HTMLElement.prototype,
    "offsetHeight",
  );
  const originalOffsetWidth = Object.getOwnPropertyDescriptor(
    HTMLElement.prototype,
    "offsetWidth",
  );
  const globals = globalThis as { ResizeObserver?: unknown };
  const originalResizeObserver = globals.ResizeObserver;

  globals.ResizeObserver = class {
    observe() {}
    unobserve() {}
    disconnect() {}
  };

  Element.prototype.getBoundingClientRect = function (this: Element) {
    const height = this.hasAttribute("data-index") ? ROW_HEIGHT : VIEWPORT_HEIGHT;
    return {
      x: 0,
      y: 0,
      top: 0,
      left: 0,
      right: 800,
      bottom: height,
      width: 800,
      height,
      toJSON: () => ({}),
    } as DOMRect;
  };
  Object.defineProperty(HTMLElement.prototype, "offsetHeight", {
    configurable: true,
    get(this: HTMLElement) {
      return this.hasAttribute("data-index") ? ROW_HEIGHT : VIEWPORT_HEIGHT;
    },
  });
  Object.defineProperty(HTMLElement.prototype, "offsetWidth", {
    configurable: true,
    get() {
      return 800;
    },
  });

  return () => {
    Element.prototype.getBoundingClientRect = originalRect;
    if (originalOffsetHeight) {
      Object.defineProperty(
        HTMLElement.prototype,
        "offsetHeight",
        originalOffsetHeight,
      );
    }
    if (originalOffsetWidth) {
      Object.defineProperty(HTMLElement.prototype, "offsetWidth", originalOffsetWidth);
    }
    if (originalResizeObserver === undefined) {
      delete globals.ResizeObserver;
    } else {
      globals.ResizeObserver = originalResizeObserver;
    }
  };
}

export interface Mounted {
  container: HTMLDivElement;
  root: Root;
  /** 渲染（或更新）组件树，并 flush 首轮 Promise 链 */
  render: (node: ReactNode) => Promise<void>;
  unmount: () => void;
}

/** 在 document.body 下建一个容器并挂载 React 根 */
export function mountContainer(): Mounted {
  const container = document.createElement("div");
  document.body.appendChild(container);
  const root = createRoot(container);

  return {
    container,
    root,
    render: async (node: ReactNode) => {
      await act(async () => {
        root.render(node);
      });
      // 让 mock 的 Promise 链 + 状态更新跑完
      await act(async () => {
        await Promise.resolve();
      });
    },
    unmount: () => {
      act(() => root.unmount());
      container.remove();
    },
  };
}

/** 等真实的若干毫秒（用于 rAF / setTimeout 调度的批处理落地） */
export async function waitMs(ms: number): Promise<void> {
  await act(async () => {
    await new Promise((resolve) => setTimeout(resolve, ms));
  });
}

/** 按 React 的方式写入受控输入值（含 input 事件冒泡），并在 act 中 flush */
export function setInputValue(
  input: HTMLInputElement | HTMLTextAreaElement,
  value: string,
): void {
  const proto =
    input instanceof HTMLTextAreaElement
      ? HTMLTextAreaElement.prototype
      : HTMLInputElement.prototype;
  const setter = Object.getOwnPropertyDescriptor(proto, "value")!.set!;
  act(() => {
    setter.call(input, value);
    input.dispatchEvent(new Event("input", { bubbles: true }));
  });
}

/** 派发一次冒泡 click 并 flush 同步的 React 更新 */
export function click(element: Element): void {
  act(() => {
    element.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

/** 派发一次键盘事件（Enter / 空格等）并 flush */
export function pressKey(element: Element, key: string): void {
  act(() => {
    element.dispatchEvent(
      new KeyboardEvent("keydown", { key, bubbles: true, cancelable: true }),
    );
  });
}

/** 按可见文本前缀查找按钮 */
export function findButton(
  container: ParentNode,
  prefix: string,
): HTMLButtonElement | null {
  return (
    [...container.querySelectorAll("button")].find((btn) =>
      (btn.textContent ?? "").trim().startsWith(prefix),
    ) ?? null
  );
}

/** 按 title 属性查找元素（用于同名按钮区分，例如「默认」与「默认 0.3」） */
export function findByTitle<T extends Element = HTMLElement>(
  container: ParentNode,
  title: string,
): T | null {
  return container.querySelector<T>(`[title="${title}"]`);
}

/** 按 aria-label 查找元素 */
export function findByLabel<T extends Element = HTMLElement>(
  container: ParentNode,
  label: string,
): T | null {
  return container.querySelector<T>(`[aria-label="${label}"]`);
}
