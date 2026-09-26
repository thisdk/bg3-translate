/**
 * Progress 的无障碍属性回归用例。
 *
 * 翻译进度是用户在长任务里唯一的进度信号，进度条必须暴露
 * `role="progressbar"` 与 `aria-valuenow/max/min`，否则屏幕阅读器读不出来。
 *
 * 跑法：`bunx vitest run src/components/ui/progress.test.tsx`
 */
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { Progress } from "./progress";
import { mountContainer, type Mounted } from "@/test-utils/dom";

let mounted: Mounted;

beforeEach(() => {
  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

describe("Progress 无障碍", () => {
  it("暴露 progressbar 角色与 aria-value*，并给出 aria-label", async () => {
    await mounted.render(
      <Progress value={3} max={10} aria-label="翻译进度" />,
    );

    const bar = mounted.container.querySelector<HTMLElement>('[role="progressbar"]');
    expect(bar).not.toBeNull();
    expect(bar!.getAttribute("aria-valuenow")).toBe("3");
    expect(bar!.getAttribute("aria-valuemax")).toBe("10");
    expect(bar!.getAttribute("aria-valuemin")).toBe("0");
    expect(bar!.getAttribute("aria-label")).toBe("翻译进度");
  });

  it("填充宽度按 value/max 计算并钳制在 0–100%", async () => {
    await mounted.render(<Progress value={1} max={4} />);
    const fill = mounted.container.querySelector<HTMLElement>("[role=progressbar] > div");
    expect(fill?.style.width).toBe("25%");

    await mounted.render(<Progress value={9} max={4} />);
    const clamped = mounted.container.querySelector<HTMLElement>("[role=progressbar] > div");
    expect(clamped?.style.width).toBe("100%");
  });
});
