/**
 * GlossaryPanel 行为测试：加载 / 搜索 / 增删改 / 重置 / 导入。
 *
 * mock 说明：术语表的命令走 `@/lib/tauri`；「导入」用的是 Tauri 原生对话框与
 * 文件读取（`@tauri-apps/plugin-dialog` / `@tauri-apps/plugin-fs`），这两个模块
 * 也一并 mock，测试不会加载 Tauri runtime。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { GlossaryPanel } from "./GlossaryPanel";
import {
  click,
  findButton,
  findByLabel,
  mountContainer,
  setInputValue,
  waitMs,
  type Mounted,
} from "@/test-utils/dom";
import type { Glossary, GlossaryEntry } from "@/lib/types";

const tauri = vi.hoisted(() => ({
  listGlossary: vi.fn(),
  addGlossaryEntry: vi.fn(),
  updateGlossaryEntry: vi.fn(),
  deleteGlossaryEntry: vi.fn(),
  resetGlossary: vi.fn(),
  importGlossary: vi.fn(),
}));

vi.mock("@/lib/tauri", () => ({
  listGlossary: tauri.listGlossary,
  addGlossaryEntry: tauri.addGlossaryEntry,
  updateGlossaryEntry: tauri.updateGlossaryEntry,
  deleteGlossaryEntry: tauri.deleteGlossaryEntry,
  resetGlossary: tauri.resetGlossary,
  importGlossary: tauri.importGlossary,
}));

const dialog = vi.hoisted(() => ({ open: vi.fn() }));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: dialog.open }));

const fs = vi.hoisted(() => ({ readTextFile: vi.fn() }));
vi.mock("@tauri-apps/plugin-fs", () => ({ readTextFile: fs.readTextFile }));

const OFFICIAL: GlossaryEntry = {
  source: "Shadowheart",
  target: "影心",
  sourceKind: "official",
  enabled: true,
  ambiguous: false,
  wholeWord: true,
  caseSensitive: false,
  count: 12,
};

const USER: GlossaryEntry = {
  source: "Gith",
  target: "吉斯",
  sourceKind: "user",
  enabled: true,
  ambiguous: false,
  wholeWord: true,
  caseSensitive: false,
  count: 3,
};

const INITIAL: Glossary = { terms: [OFFICIAL, USER] };

let mounted: Mounted;

beforeEach(() => {
  tauri.listGlossary.mockReset().mockResolvedValue(INITIAL);
  tauri.addGlossaryEntry.mockReset();
  tauri.updateGlossaryEntry.mockReset();
  tauri.deleteGlossaryEntry.mockReset();
  tauri.resetGlossary.mockReset();
  tauri.importGlossary.mockReset();
  dialog.open.mockReset();
  fs.readTextFile.mockReset();
  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

async function render() {
  await mounted.render(<GlossaryPanel onClose={() => {}} embedded />);
  await waitMs(0);
}

function rows(): HTMLTableRowElement[] {
  return [...mounted.container.querySelectorAll<HTMLTableRowElement>("tbody tr")];
}

function searchInput(): HTMLInputElement {
  return findByLabel<HTMLInputElement>(mounted.container, "搜索术语")!;
}

function modalTextareas(): HTMLTextAreaElement[] {
  return [...mounted.container.querySelectorAll<HTMLTextAreaElement>("textarea")];
}

describe("GlossaryPanel 列表与搜索", () => {
  it("加载术语表并按行展示英文 / 中文", async () => {
    await render();

    expect(tauri.listGlossary).toHaveBeenCalledTimes(1);
    expect(rows()).toHaveLength(2);
    const text = mounted.container.textContent ?? "";
    expect(text).toContain("Shadowheart");
    expect(text).toContain("影心");
    expect(text).toContain("Gith");
    expect(text).toContain("吉斯");
    expect(text).toContain("2 条术语");
  });

  it("搜索按英文或中文过滤，并显示当前显示条数", async () => {
    await render();

    setInputValue(searchInput(), "shadow");
    expect(rows()).toHaveLength(1);
    expect(rows()[0].textContent).toContain("影心");
    expect(mounted.container.textContent).toContain("当前显示 1");

    setInputValue(searchInput(), "吉斯");
    expect(rows()).toHaveLength(1);
    expect(rows()[0].textContent).toContain("Gith");
  });

  it("搜不到时展示空状态，点「清除搜索」恢复列表", async () => {
    await render();

    setInputValue(searchInput(), "zzzz");
    expect(rows()).toHaveLength(0);
    expect(mounted.container.textContent).toContain("没有匹配的术语");

    click(findButton(mounted.container, "清除搜索")!);
    expect(searchInput().value).toBe("");
    expect(rows()).toHaveLength(2);
  });

  it("官方术语不可删除（没有删除按钮），自定义术语可以", async () => {
    await render();

    expect(findByLabel(mounted.container, "编辑术语 Shadowheart")).not.toBeNull();
    expect(findByLabel(mounted.container, "编辑术语 Gith")).not.toBeNull();
    expect(findByLabel(mounted.container, "删除术语 Shadowheart")).toBeNull();
    expect(findByLabel(mounted.container, "删除术语 Gith")).not.toBeNull();
  });

  it("超过 200 条时只渲染前 200 条，可分批加载", async () => {
    const many: Glossary = {
      terms: Array.from({ length: 205 }, (_, i) => ({
        ...USER,
        source: `term-${i}`,
        target: `词-${i}`,
      })),
    };
    tauri.listGlossary.mockResolvedValue(many);
    await render();

    expect(rows()).toHaveLength(200);
    const more = findButton(mounted.container, "加载更多");
    expect(more).not.toBeNull();
    expect(more!.textContent).toContain("剩余 5 条");

    click(more!);
    expect(rows()).toHaveLength(205);
    expect(findButton(mounted.container, "加载更多")).toBeNull();
  });
});

describe("GlossaryPanel 增删改", () => {
  it("新增术语：先校验非空，再调用后端并刷新列表", async () => {
    await render();

    click(findButton(mounted.container, "新增")!);
    expect(mounted.container.textContent).toContain("新增术语");

    // 空值直接保存 → 报错且不发请求
    click(findButton(mounted.container, "保存")!);
    await waitMs(0);
    expect(mounted.container.textContent).toContain("术语的中英文均不能为空");
    expect(tauri.addGlossaryEntry).not.toHaveBeenCalled();

    const [source, target] = modalTextareas();
    setInputValue(source, "Bhaal");
    setInputValue(target, "巴尔");
    tauri.addGlossaryEntry.mockResolvedValue({
      terms: [...INITIAL.terms, { ...USER, source: "Bhaal", target: "巴尔" }],
    });

    click(findButton(mounted.container, "保存")!);
    await waitMs(0);

    expect(tauri.addGlossaryEntry).toHaveBeenCalledWith({
      source: "Bhaal",
      target: "巴尔",
      sourceKind: "user",
      enabled: true,
      ambiguous: false,
      wholeWord: true,
      caseSensitive: false,
      count: 0,
    });
    // 弹层关闭 + 列表刷新到 3 条
    expect(mounted.container.querySelector("textarea")).toBeNull();
    expect(rows()).toHaveLength(3);
  });

  it("编辑术语：回填原值，保存时带上原始 source", async () => {
    await render();

    click(findByLabel(mounted.container, "编辑术语 Gith")!);
    expect(mounted.container.textContent).toContain("编辑术语");
    const [source, target] = modalTextareas();
    expect(source.value).toBe("Gith");
    expect(target.value).toBe("吉斯");

    setInputValue(target, "吉斯人");
    tauri.updateGlossaryEntry.mockResolvedValue({
      terms: [{ ...OFFICIAL }, { ...USER, target: "吉斯人" }],
    });

    click(findButton(mounted.container, "保存")!);
    await waitMs(0);

    expect(tauri.updateGlossaryEntry).toHaveBeenCalledWith("Gith", {
      ...USER,
      target: "吉斯人",
    });
    expect(mounted.container.textContent).toContain("吉斯人");
  });

  it("删除术语：调用后端并移除该行", async () => {
    await render();

    tauri.deleteGlossaryEntry.mockResolvedValue({ terms: [OFFICIAL] });
    click(findByLabel(mounted.container, "删除术语 Gith")!);
    await waitMs(0);

    expect(tauri.deleteGlossaryEntry).toHaveBeenCalledWith("Gith");
    expect(rows()).toHaveLength(1);
    expect(mounted.container.textContent).not.toContain("吉斯");
  });

  it("删除失败时展示错误并保留原行", async () => {
    await render();

    tauri.deleteGlossaryEntry.mockRejectedValue(new Error("没有权限"));
    click(findByLabel(mounted.container, "删除术语 Gith")!);
    await waitMs(0);

    expect(mounted.container.textContent).toContain("没有权限");
    expect(rows()).toHaveLength(2);
  });

  it("重置术语表：确认后才调用后端", async () => {
    await render();
    const confirmSpy = vi.spyOn(window, "confirm").mockReturnValue(false);

    click(findButton(mounted.container, "重置")!);
    await waitMs(0);
    expect(confirmSpy).toHaveBeenCalled();
    expect(tauri.resetGlossary).not.toHaveBeenCalled();

    confirmSpy.mockReturnValue(true);
    tauri.resetGlossary.mockResolvedValue({ terms: [OFFICIAL] });
    click(findButton(mounted.container, "重置")!);
    await waitMs(0);

    expect(tauri.resetGlossary).toHaveBeenCalledTimes(1);
    expect(rows()).toHaveLength(1);
  });

  it("导入 JSON：读文件内容并交给后端，替换列表", async () => {
    await render();

    dialog.open.mockResolvedValue("/tmp/glossary.json");
    fs.readTextFile.mockResolvedValue('{"terms":[]}');
    tauri.importGlossary.mockResolvedValue({
      terms: [{ ...OFFICIAL, source: "Imported", target: "导入项" }],
    });

    click(findButton(mounted.container, "导入")!);
    await waitMs(0);

    expect(fs.readTextFile).toHaveBeenCalledWith("/tmp/glossary.json");
    expect(tauri.importGlossary).toHaveBeenCalledWith('{"terms":[]}');
    expect(rows()).toHaveLength(1);
    expect(mounted.container.textContent).toContain("导入项");
  });
});
