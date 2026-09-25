/**
 * FileTree 行为测试：只显示本地化文件、分组三态、勾选与全选。
 * 纯受控组件，测试里用一个 harness 持有选中状态，验证真实交互回路。
 */
import { useState } from "react";
import { afterEach, beforeEach, describe, expect, it } from "vitest";
import { FileTree } from "./FileTree";
import {
  click,
  findButton,
  findByLabel,
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
function Harness({ files, initial = [] }: { files: PakFile[]; initial?: PakFile[] }) {
  const [selected, setSelected] = useState<PakFile[]>(initial);
  return (
    <FileTree
      files={files}
      selectedFiles={selected}
      onSelectionChange={setSelected}
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

function rowByTitle(title: string): HTMLElement {
  return mounted.container.querySelector<HTMLElement>(`[title="${title}"]`)!;
}

function selectedNames(): string[] {
  return [...mounted.container.querySelectorAll<HTMLElement>('[aria-pressed="true"]')]
    .map((el) => el.getAttribute("title") ?? "")
    .sort();
}

describe("FileTree 分组与勾选", () => {
  it("只列出本地化文件，隐藏 lsx/lua 等非本地化内容", async () => {
    await mounted.render(<Harness files={FILES} />);

    const text = mounted.container.textContent ?? "";
    expect(text).toContain("3 个文件，已选 0");
    expect(text).toContain("a.xml");
    expect(text).toContain("c.loca");
    expect(text).toContain("b.xml");
    expect(text).not.toContain("meta.lsx");
    expect(text).not.toContain("init.lua");
    // 分组结构：Mods → Foo → Localization → Chinese/English
    expect(text).toContain("Mods");
    expect(text).toContain("Localization");
    expect(text).toContain("Chinese");
    // 语言排序：中文在前
    const order = text.indexOf("b.xml") < text.indexOf("a.xml");
    expect(order).toBe(true);
  });

  it("点击文件行切换选中，再点一次取消", async () => {
    await mounted.render(<Harness files={FILES} />);

    await click(rowByTitle("Mods/Foo/Localization/English/a.xml"));
    expect(selectedNames()).toEqual(["Mods/Foo/Localization/English/a.xml"]);
    expect(mounted.container.textContent).toContain("3 个文件，已选 1");

    await click(rowByTitle("Mods/Foo/Localization/English/a.xml"));
    expect(selectedNames()).toEqual([]);
    expect(mounted.container.textContent).toContain("3 个文件，已选 0");
  });

  it("文件夹按已选数量显示三态，并可整组全选/取消", async () => {
    await mounted.render(
      <Harness files={FILES} initial={[fileByName("Mods/Foo/Localization/English/a.xml")]} />,
    );

    // English 组 2 个文件只选了 1 个 → indeterminate，按钮语义仍是「全选」
    const englishCheckbox = findByLabel(mounted.container, "全选 English");
    expect(englishCheckbox).not.toBeNull();
    expect(englishCheckbox!.getAttribute("aria-checked")).toBe("false");

    await click(englishCheckbox!);
    expect(selectedNames()).toEqual([
      "Mods/Foo/Localization/English/a.xml",
      "Mods/Foo/Localization/English/c.loca",
    ]);
    // 全选后语义变成「取消全选」
    expect(findByLabel(mounted.container, "取消全选 English")).not.toBeNull();

    await click(findByLabel(mounted.container, "取消全选 English")!);
    expect(selectedNames()).toEqual([]);
  });

  it("顶层「全选 / 取消全选」按钮一次选中全部本地化文件", async () => {
    await mounted.render(<Harness files={FILES} />);

    await click(findButton(mounted.container, "全选")!);
    expect(selectedNames()).toEqual([
      "Mods/Foo/Localization/Chinese/b.xml",
      "Mods/Foo/Localization/English/a.xml",
      "Mods/Foo/Localization/English/c.loca",
    ]);

    await click(findButton(mounted.container, "取消全选")!);
    expect(selectedNames()).toEqual([]);
  });

  it("键盘 Enter / 空格也能切换文件选中", async () => {
    await mounted.render(<Harness files={FILES} />);

    pressKey(rowByTitle("Mods/Foo/Localization/Chinese/b.xml"), "Enter");
    expect(selectedNames()).toEqual(["Mods/Foo/Localization/Chinese/b.xml"]);

    pressKey(rowByTitle("Mods/Foo/Localization/Chinese/b.xml"), " ");
    expect(selectedNames()).toEqual([]);
  });

  it("没有本地化文件时展示空状态且不出现全选按钮", async () => {
    await mounted.render(<Harness files={[FILES[3], FILES[4]]} />);

    expect(mounted.container.textContent).toContain("此 MOD 没有本地化文件");
    expect(findButton(mounted.container, "全选")).toBeNull();
  });

  it("勾选顺序决定「已选」列表顺序（追加而非重排）", async () => {
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
    await click(rowByTitle("Mods/Foo/Localization/English/c.loca"));
    await click(rowByTitle("Mods/Foo/Localization/Chinese/b.xml"));

    expect(calls[0]).toEqual([fileByName("Mods/Foo/Localization/English/c.loca")]);
    // 后选的追加在后面，不会被重排成文件名顺序
    expect(calls[1]).toEqual([
      fileByName("Mods/Foo/Localization/English/c.loca"),
      fileByName("Mods/Foo/Localization/Chinese/b.xml"),
    ]);
  });
});
