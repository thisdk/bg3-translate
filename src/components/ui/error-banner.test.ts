import { describe, expect, it } from "vitest";
import { splitErrorLines } from "./error-banner";

describe("splitErrorLines", () => {
  it("单行错误原样返回", () => {
    expect(splitErrorLines("未配置 API Key")).toEqual(["未配置 API Key"]);
  });

  it("多行错误按行拆分并去掉空白行", () => {
    expect(splitErrorLines("第一行\n\n  第二行  \n\n第三行")).toEqual([
      "第一行",
      "第二行",
      "第三行",
    ]);
  });

  it("兼容 Windows 换行", () => {
    expect(splitErrorLines("a\r\nb")).toEqual(["a", "b"]);
  });

  it("全是空白时退回原始文本（trim 后）", () => {
    expect(splitErrorLines("   ")).toEqual([""]);
    expect(splitErrorLines("\n\n")).toEqual([""]);
  });

  it("不合并超长单行（交给 UI 折叠）", () => {
    const long = `x`.repeat(500);
    expect(splitErrorLines(long)).toEqual([long]);
  });
});
