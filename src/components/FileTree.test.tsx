/**
 * FileTree 行为测试：只显示本地化文件、英文优先排序、单选切换与翻译中禁用状态。
 * 纯受控组件，测试里用一个 harness 持有选中状态，验证真实交互回路。
 */
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { FileTree } from "./FileTree";
import {
  click,
  findButton,
  mountContainer,
  pressKey,
  type Mounted,
} from "@/test-utils/dom";
import type { PakFile } from "@/lib/types";

const FILES: PakFile[] = [
  {
    name: "Mods/Foo/Localization/English/a.xml",
    size: 1,
    kind: "localization-xml",
    language: "English",
  },
  {
    name: "Mods/Foo/Localization/English/c.loca",
    size: 1,
    kind: "localization-loca",
    language: "English",
  },
  {
    name: "Mods/Foo/Localization/Chinese/b.xml",
    size: 1,
    kind: "localization-xml",
    language: "Chinese",
  },
  { name: "Mods/Foo/meta.lsx", size: 1, kind: "metadata-lsx", language: null },
  { name: "Mods/Foo/scripts/init.lua", size: 1, kind: "script-lua", language: null },
];

function fileByName(name: string): PakFile {
  return FILES.find((f) => f.name === name)!;
}

/** 受控宿主：把 onSelectionChange 接回 selectedFiles */
function Harness({
  files,
  initial = [],
  disabled = false,
}: {
  files: PakFile[];
  initial?: PakFile[];
  disabled?: boolean;
}) {
  const [selected, setSelected] = useState<PakFile[]>(initial);
  return (
    <FileTree
      files={files}
      selectedFiles={selected}
      onSelectionChange={setSelected}
      disabled={disabled}
    />
  );
}

let mounted: Mounted;

beforeEach(() => {
  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

function rowByPath(path: string): HTMLElement {
  return mounted.container.querySelector<HTMLElement>(`[data-file-path="${path}"]`)!;
}

function selectedNames(): string[] {
  return [...mounted.container.querySelectorAll<HTMLElement>('[role="radio"][aria-checked="true"]')]
    .map((el) => el.getAttribute("data-file-path") ?? "")
    .sort();
}

describe("FileTree 单选与禁用状态", () => {
  it("只列出本地化文件（隐藏 lsx/lua），且英文排在其它语言前面", async () => {
    await mounted.render(<Harness files={FILES} />);

    const text = mounted.container.textContent ?? "";
    expect(text).toContain("3 个文件，已选 0");
    expect(text).toContain("a.xml");
    expect(text).toContain("c.loca");
    expect(text).toContain("b.xml");
    expect(text).not.toContain("meta.lsx");
    expect(text).not.toContain("init.lua");
    // 分组结构：Mods → Foo → Localization → English/Chinese
    expect(text).toContain("Mods");
    expect(text).toContain("Localization");
    expect(text).toContain("English");
    expect(text).toContain("Chinese");
    // 语言排序：英文优先于中文
    expect(text.indexOf("a.xml")).toBeLessThan(text.indexOf("b.xml"));
    // 单选模式：不再提供顶层「全选」按钮或文件夹复选框
    expect(findButton(mounted.container, "全选")).toBeNull();
    expect(mounted.container.querySelectorAll('[role="checkbox"]')).toHaveLength(0);
  });

  it("点击不同文件直接单选切换（替换前一个文件，而非累加多选）", async () => {
    const calls: PakFile[][] = [];
    function RecordingHarness() {
      const [selected, setSelected] = useState<PakFile[]>([]);
      return (
        <FileTree
          files={FILES}
          selectedFiles={selected}
          onSelectionChange={(next) => {
            calls.push(next);
            setSelected(next);
          }}
        />
      );
    }

    await mounted.render(<RecordingHarness />);

    await click(rowByPath("Mods/Foo/Localization/English/a.xml"));
    expect(selectedNames()).toEqual(["Mods/Foo/Localization/English/a.xml"]);
    expect(mounted.container.textContent).toContain("3 个文件，已选 1");

    // 再点另一个语言文件 → 直接切换为新文件，绝不同时选中两个
    await click(rowByPath("Mods/Foo/Localization/Chinese/b.xml"));
    expect(selectedNames()).toEqual(["Mods/Foo/Localization/Chinese/b.xml"]);
    expect(mounted.container.textContent).toContain("3 个文件，已选 1");

    expect(calls[0]).toEqual([fileByName("Mods/Foo/Localization/English/a.xml")]);
    expect(calls[1]).toEqual([fileByName("Mods/Foo/Localization/Chinese/b.xml")]);

    // 再次点击当前已选中的同一个文件 → 取消选中
    await click(rowByPath("Mods/Foo/Localization/Chinese/b.xml"));
    expect(selectedNames()).toEqual([]);
    expect(mounted.container.textContent).toContain("3 个文件，已选 0");
  });

  it("键盘 Enter / 空格也能单选切换文件", async () => {
    await mounted.render(<Harness files={FILES} />);

    pressKey(rowByPath("Mods/Foo/Localization/English/a.xml"), "Enter");
    expect(selectedNames()).toEqual(["Mods/Foo/Localization/English/a.xml"]);

    pressKey(rowByPath("Mods/Foo/Localization/Chinese/b.xml"), " ");
    expect(selectedNames()).toEqual(["Mods/Foo/Localization/Chinese/b.xml"]);

    pressKey(rowByPath("Mods/Foo/Localization/Chinese/b.xml"), " ");
    expect(selectedNames()).toEqual([]);
  });

  it("disabled=true（翻译进行中）时所有文件行置灰且点击/按键均不触发切换", async () => {
    await mounted.render(
      <Harness
        files={FILES}
        initial={[fileByName("Mods/Foo/Localization/English/a.xml")]}
        disabled
      />,
    );

    const rowA = rowByPath("Mods/Foo/Localization/English/a.xml");
    const rowB = rowByPath("Mods/Foo/Localization/Chinese/b.xml");
    expect(rowA.getAttribute("aria-disabled")).toBe("true");
    expect(rowB.getAttribute("aria-disabled")).toBe("true");
    expect(rowB.tabIndex).toBe(-1);

    await click(rowB);
    pressKey(rowB, "Enter");
    pressKey(rowA, " ");

    // 选中项保持不变
    expect(selectedNames()).toEqual(["Mods/Foo/Localization/English/a.xml"]);
  });

  it("没有本地化文件时展示空状态", async () => {
    await mounted.render(<Harness files={[FILES[3], FILES[4]]} />);

    expect(mounted.container.textContent).toContain("此 MOD 没有本地化文件");
    expect(findButton(mounted.container, "全选")).toBeNull();
  });
});
