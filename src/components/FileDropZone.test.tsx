/**
 * FileDropZone 行为测试。
 *
 * 覆盖真实的拖放路径解析与打开流程；`@/lib/tauri` 与 Tauri webview 事件都用
 * mock 替换，不加载 Tauri runtime。
 */
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { FileDropZone } from "./FileDropZone";
import {
  click,
  findButton,
  mountContainer,
  waitMs,
  type Mounted,
} from "@/test-utils/dom";
import { useAppStore } from "@/store/app-store";
import type { PakFile } from "@/lib/types";

const tauri = vi.hoisted(() => ({
  openMod: vi.fn(),
  extractMod: vi.fn(),
  pickModFile: vi.fn(),
  pickExtractDirectory: vi.fn(),
}));

vi.mock("@/lib/tauri", () => ({
  openMod: tauri.openMod,
  extractMod: tauri.extractMod,
  pickModFile: tauri.pickModFile,
  pickExtractDirectory: tauri.pickExtractDirectory,
}));

/** 记录 webview 拖放事件的订阅者，测试里手动投递事件 */
const dragDrop = vi.hoisted(() => ({
  handlers: [] as ((event: { payload: unknown }) => void)[],
  unlisten: vi.fn(),
}));

vi.mock("@tauri-apps/api/webview", () => ({
  getCurrentWebview: () => ({
    onDragDropEvent: async (handler: (event: { payload: unknown }) => void) => {
      dragDrop.handlers.push(handler);
      return dragDrop.unlisten;
    },
  }),
}));

const pak: PakFile = {
  name: "Localization/English/a.xml",
  size: 10,
  kind: "localization-xml",
  language: "English",
};

let mounted: Mounted;

beforeEach(() => {
  dragDrop.handlers = [];
  tauri.openMod.mockReset();
  tauri.extractMod.mockReset();
  tauri.pickModFile.mockReset();
  tauri.pickExtractDirectory.mockReset();
  useAppStore.getState().reset();
  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

async function render() {
  await mounted.render(<FileDropZone />);
  await waitMs(0);
  expect(dragDrop.handlers).toHaveLength(1);
}

function emitDrop(paths: string[]) {
  dragDrop.handlers[0]({ payload: { type: "drop", paths } });
}

function emit(type: "enter" | "over" | "leave") {
  dragDrop.handlers[0]({ payload: { type } });
}

function dropZone(): HTMLElement {
  return mounted.container.querySelector<HTMLElement>('[aria-label="打开 MOD 文件"]')!;
}

/** 拖拽悬停高亮：只有这一种状态会带上 bg-accent/50 */
function isHighlighted(): boolean {
  return dropZone().className.includes("bg-accent/50");
}

describe("FileDropZone 拖放与打开", () => {
  it("拖入 .pak 路径时调用 openMod 并写入 store", async () => {
    tauri.openMod.mockResolvedValue({ workDir: "/tmp/work", files: [pak] });
    await render();

    await act(async () => {
      emitDrop(["/tmp/readme.txt", "/tmp/My Mod.zip"]);
    });
    await waitMs(0);

    // 多文件拖放时挑出第一个 .pak/.zip
    expect(tauri.openMod).toHaveBeenCalledWith("/tmp/My Mod.zip");
    const state = useAppStore.getState();
    expect(state.stage).toBe("files");
    expect(state.modFilePath).toBe("/tmp/My Mod.zip");
    expect(state.workDir).toBe("/tmp/work");
    expect(state.files).toEqual([pak]);
    expect(state.loading).toBe(false);
    expect(state.error).toBeNull();
  });

  it("拖入非 pak/zip 路径时给出提示且不打开", async () => {
    await render();

    await act(async () => {
      emitDrop(["/tmp/notes.txt"]);
    });

    expect(tauri.openMod).not.toHaveBeenCalled();
    expect(useAppStore.getState().error).toBe("请拖入 .pak 或 .zip 文件");
  });

  it("drag enter/leave 切换高亮，drop 后取消高亮", async () => {
    tauri.openMod.mockResolvedValue({ workDir: "/tmp/work", files: [pak] });
    await render();

    await act(async () => {
      emit("enter");
    });
    expect(isHighlighted()).toBe(true);

    await act(async () => {
      emit("leave");
    });
    expect(isHighlighted()).toBe(false);

    await act(async () => {
      emit("over");
    });
    expect(isHighlighted()).toBe(true);

    await act(async () => {
      emitDrop(["/tmp/a.pak"]);
    });
    await waitMs(0);
    expect(isHighlighted()).toBe(false);
  });

  it("点击「选择文件」走 pickModFile + openMod，取消选择则什么都不做", async () => {
    tauri.pickModFile.mockResolvedValueOnce(null);
    await render();
    await act(async () => {
      click(findButton(mounted.container, "选择文件")!);
    });
    await waitMs(0);
    expect(tauri.openMod).not.toHaveBeenCalled();

    tauri.pickModFile.mockResolvedValueOnce("/tmp/picked.pak");
    tauri.openMod.mockResolvedValue({ workDir: "/tmp/w2", files: [pak] });
    await act(async () => {
      click(findButton(mounted.container, "选择文件")!);
    });
    await waitMs(0);
    expect(tauri.openMod).toHaveBeenCalledWith("/tmp/picked.pak");
    expect(useAppStore.getState().workDir).toBe("/tmp/w2");
  });

  it("打开失败时把错误写进 store 并解除 loading", async () => {
    tauri.openMod.mockRejectedValue(new Error("坏的 pak"));
    await render();

    await act(async () => {
      emitDrop(["/tmp/broken.pak"]);
    });
    await waitMs(0);

    expect(useAppStore.getState().error).toContain("坏的 pak");
    expect(useAppStore.getState().loading).toBe(false);
    expect(useAppStore.getState().stage).toBe("home");
  });

  it("「仅解压」需要同时选到文件和输出目录，成功后展示结果", async () => {
    await render();

    // 只选文件、不选目录 → 不解压
    tauri.pickModFile.mockResolvedValueOnce("/tmp/a.pak");
    tauri.pickExtractDirectory.mockResolvedValueOnce(null);
    await act(async () => {
      click(findButton(mounted.container, "仅解压")!);
    });
    await waitMs(0);
    expect(tauri.extractMod).not.toHaveBeenCalled();

    // 目录也选上 → 解压并展示条数 + 目录
    tauri.pickModFile.mockResolvedValueOnce("/tmp/a.pak");
    tauri.pickExtractDirectory.mockResolvedValueOnce("/tmp/out");
    tauri.extractMod.mockResolvedValue([pak, pak]);
    await act(async () => {
      click(findButton(mounted.container, "仅解压")!);
    });
    await waitMs(0);

    expect(tauri.extractMod).toHaveBeenCalledWith("/tmp/a.pak", "/tmp/out");
    const text = mounted.container.textContent ?? "";
    expect(text).toContain("已解压 2 个文件");
    expect(text).toContain("/tmp/out");
  });

  it("浏览器兜底：HTML5 drop 带 path 的文件也能打开", async () => {
    tauri.openMod.mockResolvedValue({ workDir: "/tmp/work", files: [pak] });
    await render();

    const file = Object.assign(new File(["x"], "mod.pak"), { path: "/tmp/mod.pak" });
    const event = new Event("drop", { bubbles: true, cancelable: true }) as Event & {
      dataTransfer: unknown;
    };
    event.dataTransfer = { files: [file] };

    await act(async () => {
      dropZone().dispatchEvent(event);
    });
    await waitMs(0);

    expect(tauri.openMod).toHaveBeenCalledWith("/tmp/mod.pak");
  });
});
