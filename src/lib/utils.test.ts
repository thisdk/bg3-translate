import { describe, expect, it } from "vitest";
import { cn } from "./utils";

describe("cn", () => {
  it("拼接多个类名", () => {
    expect(cn("a", "b")).toBe("a b");
  });

  it("忽略 falsy 值", () => {
    expect(cn("a", false, null, undefined, "", "b")).toBe("a b");
  });

  it("支持对象与数组写法（clsx 语义）", () => {
    expect(cn("a", { b: true, c: false }, ["d", { e: true }])).toBe("a b d e");
  });

  it("后者覆盖同组 Tailwind 类（tailwind-merge 语义）", () => {
    expect(cn("p-2", "p-4")).toBe("p-4");
    expect(cn("text-sm", "text-lg")).toBe("text-lg");
  });

  it("保留不同组的类", () => {
    expect(cn("p-2", "m-2")).toBe("p-2 m-2");
  });

  it("条件类为 false 时不会覆盖已有类", () => {
    expect(cn("p-2", false && "p-4")).toBe("p-2");
  });

  it("支持自定义 border 颜色顺序覆盖（v4 语义色）", () => {
    expect(cn("border-input", "border-primary")).toBe("border-primary");
  });

  it("空调用返回空串", () => {
    expect(cn()).toBe("");
  });
});
