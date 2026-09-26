/**
 * `useEntryLoading` 的确定性回归用例。
 *
 * 关注点：已加载的文件在**新增勾选**时会不会被重新读一遍。
 * 重新读一遍有两个后果，都是真实的数据损失：
 *   1. 重复 IPC（勾选 N 个文件一共要读 O(N²) 次）；
 *   2. 用磁盘内容整体替换 store 里的条目 —— 用户手工编辑过的译文、
 *      以及本轮已经流式翻译出来的结果会被悄悄丢掉。
 *
 * 用例全部用受控 mock（立即 resolve 的 async fn）+ 显式 flush，不依赖 sleep。
 *
 * 跑法：`bunx vitest run src/components/translation-table/hooks/useEntryLoading.test.tsx`
 */
import { act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useEntryLoading } from "./useEntryLoading";
import { useAppStore } from "@/store/app-store";
import { mountContainer, type Mounted } from "@/test-utils/dom";
import type { PakFile, TranslationEntry } from "@/lib/types";

const FILE_A = "Localization/English/a.xml";
const FILE_B = "Localization/English/b.xml";

const backend = vi.hoisted(() => ({
  readFileEntries: vi.fn(),
}));

vi.mock("@/lib/tauri", () => ({
  readFileEntries: backend.readFileEntries,
}));

function pakFile(name: string): PakFile {
  return { name, size: 1, kind: "localization-xml", language: "English" };
}

function entry(id: string, sourceFile: string): TranslationEntry {
  return {
    id,
    sourceFile,
    source: `Source ${id}`,
    target: "",
    contentuid: `uid-${id}`,
    version: "1",
    status: "pending",
    error: null,
  };
}

/** 只订阅 hook 提供的信息；渲染本身不重要 */
function Probe() {
  const { loading } = useEntryLoading();
  return <span>{loading ? "loading" : "idle"}</span>;
}

let mounted: Mounted;

beforeEach(() => {
  backend.readFileEntries.mockReset();
  backend.readFileEntries.mockImplementation(async (_workDir: string, name: string) => [
    entry(name === FILE_A ? "a-1" : "b-1", name),
  ]);

  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/a.pak", "/tmp/work", [pakFile(FILE_A), pakFile(FILE_B)]);

  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

/** 让 mock 的 Promise 链 + React 状态更新跑完 */
async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve();
    await Promise.resolve();
  });
}

function loadedNames(): string[] {
  return backend.readFileEntries.mock.calls.map((call) => String(call[1]));
}

describe("useEntryLoading 增量加载", () => {
  it("新增勾选第二个文件时不会重读已经加载过的文件", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(FILE_A)]);
    await mounted.render(<Probe />);
    await flush();

    expect(loadedNames()).toEqual([FILE_A]);
    expect(useAppStore.getState().entriesByFile[FILE_A]).toHaveLength(1);

    await act(async () => {
      useAppStore.getState().setSelectedFiles([pakFile(FILE_A), pakFile(FILE_B)]);
    });
    await flush();

    // 只应该多读一个 b.xml；a.xml 已经加载过，不能再读
    expect(loadedNames()).toEqual([FILE_A, FILE_B]);
  });

  it("新增勾选不会用磁盘内容覆盖用户已编辑的译文", async () => {
    useAppStore.getState().setSelectedFiles([pakFile(FILE_A)]);
    await mounted.render(<Probe />);
    await flush();

    // 用户手工编辑（或本轮已经翻译出来）的译文
    useAppStore.getState().updateEntry("a-1", { target: "手工译好的译文", status: "edited" });

    await act(async () => {
      useAppStore.getState().setSelectedFiles([pakFile(FILE_A), pakFile(FILE_B)]);
    });
    await flush();

    expect(useAppStore.getState().entriesByFile[FILE_A][0]).toMatchObject({
      target: "手工译好的译文",
      status: "edited",
    });
  });

  it("快速取消勾选时未完成的加载会被放回待加载集合（不会漏加载）", async () => {
    let resolveA: ((entries: TranslationEntry[]) => void) | null = null;
    backend.readFileEntries.mockImplementation(
      (_workDir: string, name: string) =>
        new Promise<TranslationEntry[]>((resolve) => {
          if (name === FILE_A) resolveA = resolve;
          else resolve([entry("b-1", name)]);
        }),
    );

    useAppStore.getState().setSelectedFiles([pakFile(FILE_A)]);
    await mounted.render(<Probe />);

    // A 还在读，用户取消勾选再重新勾上
    await act(async () => {
      useAppStore.getState().setSelectedFiles([]);
    });
    await act(async () => {
      useAppStore.getState().setSelectedFiles([pakFile(FILE_A)]);
    });
    await flush();

    // 重新勾选必须重新发起读取（第一次请求被取消，条目从未写入 store）
    expect(loadedNames()).toEqual([FILE_A, FILE_A]);

    await act(async () => {
      resolveA?.([entry("a-1", FILE_A)]);
    });
    await flush();

    expect(useAppStore.getState().entriesByFile[FILE_A]).toHaveLength(1);
  });
});
