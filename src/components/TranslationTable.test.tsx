import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TranslationTable } from "./TranslationTable";
import { useAppStore } from "@/store/app-store";
import type { PakFile } from "@/lib/types";

// vi.mock 会被提升，这里用 vi.hoisted 准备 2 万条测试数据
const fixture = vi.hoisted(() => {
  const statuses = ["pending", "translated", "translating", "error"] as const;
  const entries = Array.from({ length: 20_000 }, (_, i) => ({
    id: `e-${i}`,
    sourceFile: "Localization/English/big.xml",
    source: `Entry number ${i}`,
    target: i % 4 === 0 ? "" : `译文 ${i}`,
    contentuid: `uid-${i}`,
    version: "1",
    status: statuses[i % statuses.length],
    error: null,
  }));
  return { entries };
});

const TOTAL = fixture.entries.length;

// TranslationTable 只依赖这几个后端命令；mock 掉避免加载 Tauri runtime
vi.mock("@/lib/tauri", () => ({
  readFileEntries: vi.fn(async () => fixture.entries),
  translateEntries: vi.fn(async () => undefined),
  cancelTranslation: vi.fn(async () => undefined),
}));

function pakFile(name: string): PakFile {
  return { name, size: 1, kind: "localization-xml", language: "English" };
}

const originalGetBoundingClientRect = Element.prototype.getBoundingClientRect;
const originalOffsetHeight = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  "offsetHeight",
);
const originalOffsetWidth = Object.getOwnPropertyDescriptor(
  HTMLElement.prototype,
  "offsetWidth",
);

/** 单行高度（与组件里的 estimateSize 保持一致） */
const ROW_HEIGHT = 118;
/** 模拟的滚动视口高度 */
const VIEWPORT_HEIGHT = 600;

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.stubGlobal(
    "ResizeObserver",
    class {
      observe() {}
      unobserve() {}
      disconnect() {}
    },
  );
  // jsdom 不做布局：这里给滚动容器 800x600、给每个条目行 118 的高度，
  // 让 @tanstack/react-virtual 能算出可见区间。
  Element.prototype.getBoundingClientRect = function (this: Element) {
    const isRow = this.hasAttribute("data-index");
    const height = isRow ? ROW_HEIGHT : VIEWPORT_HEIGHT;
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
  // virtual-core 的 observeElementRect 读的是 offsetHeight / offsetWidth
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

  useAppStore.getState().reset();
  const file = pakFile("Localization/English/big.xml");
  useAppStore.getState().setModOpened("/tmp/big.pak", "/tmp/work", [file]);
  useAppStore.getState().setSelectedFiles([file]);

  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  Element.prototype.getBoundingClientRect = originalGetBoundingClientRect;
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
  vi.unstubAllGlobals();
});

/** 挂载并等待 readFileEntries 的 Promise 落地 */
async function render() {
  await act(async () => {
    root.render(<TranslationTable />);
  });
  // 让 mock 的 Promise 链 + 状态更新跑完
  await act(async () => {
    await Promise.resolve();
  });
}

function renderedRows() {
  return container.querySelectorAll("[data-index]").length;
}

function setInputValue(input: HTMLInputElement, value: string) {
  const setter = Object.getOwnPropertyDescriptor(
    HTMLInputElement.prototype,
    "value",
  )!.set!;
  setter.call(input, value);
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

describe("TranslationTable 虚拟滚动", () => {
  it("2 万行条目只渲染视口附近的少量行", async () => {
    expect(TOTAL).toBe(20_000);
    await render();

    expect(container.textContent).toContain(String(TOTAL));
    expect(renderedRows()).toBeGreaterThan(0);
    // 20000 条里只挂载视口 + overscan 的十余行
    expect(renderedRows()).toBeLessThanOrEqual(40);
    expect(renderedRows()).toBeLessThan(TOTAL / 100);
  });

  it("渲染的每一行都是真实条目（内容正确）", async () => {
    await render();
    const first = container.querySelector("[data-index]");
    expect(first).not.toBeNull();
    expect(first!.textContent).toContain("Entry number 0");
  });

  it("工具栏展示总数、状态计数与重试按钮", async () => {
    await render();
    const text = container.textContent ?? "";
    expect(text).toContain("翻译工作区");
    expect(text).toContain("全部");
    expect(text).toContain("待翻译");
    expect(text).toContain("已翻译");
    expect(text).toContain("出错");
    // 20000 / 4 = 5000 条 error
    expect(text).toContain("重试 5000 条失败");
  });

  it("切换到「出错」过滤后只显示错误条目", async () => {
    await render();

    const errorChip = [...container.querySelectorAll("button")].find((btn) =>
      btn.textContent?.startsWith("出错"),
    );
    expect(errorChip).toBeTruthy();

    await act(async () => {
      errorChip!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    expect(container.textContent).toContain("显示 5000/20000 条（已筛选）");
    expect(container.textContent).not.toContain("Entry number 1 ");
  });

  it("搜索后只保留匹配条目", async () => {
    await render();

    const input = container.querySelector<HTMLInputElement>(
      'input[aria-label="搜索条目"]',
    );
    expect(input).toBeTruthy();

    await act(async () => {
      setInputValue(input!, "Entry number 19999");
    });

    expect(container.textContent).toContain("显示 1/20000 条（已筛选）");
  });

  it("过滤后虚拟列表的行与条目一一对应（index 不错位）", async () => {
    await render();

    const errorChip = [...container.querySelectorAll("button")].find((btn) =>
      btn.textContent?.startsWith("出错"),
    );
    await act(async () => {
      errorChip!.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });

    // fixture 的状态按下标轮转：只有 i % 4 === 3 是 error，即 e-3 / e-7 / e-11…
    const rows = [...container.querySelectorAll<HTMLElement>("[data-index]")];
    expect(rows.length).toBeGreaterThan(0);
    rows.forEach((row, position) => {
      const expectedIndex = 4 * position + 3;
      expect(row.textContent).toContain(`Entry number ${expectedIndex}`);
      expect(row.textContent).toContain(`uid-${expectedIndex}`);
    });
    // 第一行是 e-3 而不是原始数组的第 0 条
    expect(rows[0].textContent).toContain("Entry number 3");
  });
});

describe("TranslationTable 空状态", () => {
  it("未选择文件时提示去左侧勾选", async () => {
    useAppStore.getState().setSelectedFiles([]);
    await render();
    expect(container.textContent).toContain("从左侧勾选本地化文件开始翻译");
    expect(renderedRows()).toBe(0);
  });

  it("有条目但无匹配时展示清除筛选按钮", async () => {
    await render();
    const input = container.querySelector<HTMLInputElement>(
      'input[aria-label="搜索条目"]',
    );
    await act(async () => {
      setInputValue(input!, "zzzz-not-exist");
    });
    expect(container.textContent).toContain("没有匹配的条目");
    expect(container.textContent).toContain("清除筛选");
  });
});
