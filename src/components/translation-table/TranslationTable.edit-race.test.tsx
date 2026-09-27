/**
 * 「用户手工编辑」与「本轮流式翻译」并发写同一条目的确定性用例。
 *
 * 期望语义（人工成果优先）：
 *   - 用户在流式过程中点「编辑 → 保存」之后，模型后续的 delta / done / error
 *     都不能再覆盖或追加到用户保存的译文上；
 *   - 本轮收尾（取消 / 未完成回滚）也不能把用户已经保存的译文清掉。
 *
 * 违反后果：用户看到自己改好的译文在几秒后被模型文本覆盖 / 拼接，而 `edited`
 * 被改回 `translating` 之后**会被写进 PAK**（status 既不是 error 也不是空 target）。
 *
 * **用 `<StrictMode>` 包裹**（`src/main.tsx` 就是 StrictMode）：这些用例覆盖的是
 * 「工作台挂载 → 卸载 → 重挂载」生命周期最密的路径（流式批处理器、迟到事件、
 * 编辑竞态、切换 MOD）。非 StrictMode 挂载看不见「cleanup 与挂载不一一对应」这类
 * 结构缺陷（R5-02：`useMemo` 的 delta 批处理器被永久 dispose，流式文本全丢），
 * 所以这里刻意用生产/开发实际运行的严格生命周期跑。
 *
 * 跑法：`bunx vitest run src/components/translation-table/TranslationTable.edit-race.test.tsx`
 */
import { StrictMode, act } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { TranslationTable } from "@/components/TranslationTable";
import { useAppStore } from "@/store/app-store";
import { pakFile } from "@/test-utils/fixtures";
import {
  click,
  findButton,
  mountContainer,
  setInputValue,
  stubVirtualScrollLayout,
  waitMs,
  type Mounted,
} from "@/test-utils/dom";
import type { TranslationEntry, TranslationEvent } from "@/lib/types";

const FILE_NAME = "Localization/English/edit.xml";

const backend = vi.hoisted(() => ({
  entries: [] as import("@/lib/types").TranslationEntry[],
  onEvent: null as null | ((event: import("@/lib/types").TranslationEvent) => void),
  finish: null as null | (() => void),
}));

vi.mock("@/lib/tauri", () => ({
  readFileEntries: vi.fn(async () => backend.entries),
  translateEntries: vi.fn(
    async (
      _workDir: string,
      _entries: TranslationEntry[],
      _styleHint: string,
      onEvent: (event: TranslationEvent) => void,
    ) => {
      backend.onEvent = onEvent;
      await new Promise<void>((resolve) => {
        backend.finish = resolve;
      });
    },
  ),
  cancelTranslation: vi.fn(async () => undefined),
}));

let mounted: Mounted;
let restoreLayout: () => void;

function smallEntry(id: string): TranslationEntry {
  return {
    id,
    sourceFile: FILE_NAME,
    source: `Source of ${id}`,
    target: "",
    contentuid: `uid-${id}`,
    version: "1",
    status: "pending",
    error: null,
  };
}

beforeEach(() => {
  restoreLayout = stubVirtualScrollLayout();
  backend.entries = [smallEntry("e-0"), smallEntry("e-1")];
  backend.onEvent = null;
  backend.finish = null;

  const api = useAppStore.getState();
  api.reset();
  api.setModOpened("/tmp/edit.pak", "/tmp/work", [pakFile(FILE_NAME)]);
  api.setSelectedFiles([pakFile(FILE_NAME)]);

  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
  restoreLayout();
});

function storedEntry(id: string): TranslationEntry {
  const list = useAppStore.getState().entriesByFile[FILE_NAME] ?? [];
  const entry = list.find((e) => e.id === id);
  expect(entry).toBeDefined();
  return entry!;
}

function rowFor(index: number): HTMLElement {
  const row = mounted.container.querySelector<HTMLElement>(`[data-index="${index}"]`);
  expect(row).not.toBeNull();
  return row!;
}

/** 走真实 UI：点「编辑」→ 改 textarea → 点「保存」 */
async function editRow(index: number, text: string): Promise<void> {
  const editButton = findButton(rowFor(index), "编辑");
  expect(editButton).not.toBeNull();
  await act(async () => {
    click(editButton!);
  });
  const textarea = rowFor(index).querySelector<HTMLTextAreaElement>(
    'textarea[aria-label="编辑译文"]',
  );
  expect(textarea).not.toBeNull();
  setInputValue(textarea!, text);
  const saveButton = findButton(rowFor(index), "保存");
  expect(saveButton).not.toBeNull();
  await act(async () => {
    click(saveButton!);
  });
}

async function renderTable(): Promise<void> {
  await mounted.render(
    <StrictMode>
      <TranslationTable />
    </StrictMode>,
  );
  await waitMs(0);
}

async function startTranslation(): Promise<(event: TranslationEvent) => void> {
  const button = findButton(mounted.container, "翻译 ");
  expect(button).not.toBeNull();
  await act(async () => {
    click(button!);
  });
  await waitMs(0);
  expect(backend.onEvent).not.toBeNull();
  return backend.onEvent!;
}

/** 让 e-0 / e-1 都进入 translating 并各自落一段半截文本（同步 flush，无计时器） */
async function streamBoth(send: (event: TranslationEvent) => void): Promise<void> {
  await act(async () => {
    send({ type: "progress", entryId: "e-0", status: "translating" });
    send({ type: "delta", entryId: "e-0", text: "半截 0" });
    // e-1 的 progress 会同步 flush 批处理，把 e-0 的半截文本落进 store
    send({ type: "progress", entryId: "e-1", status: "translating" });
    send({ type: "delta", entryId: "e-1", text: "半截 1" });
    // all_done 同样会同步 flush（后端在命令返回前一定会发它）
    send({ type: "all_done", total: 2, failed: 0 });
    await Promise.resolve();
  });
  expect(storedEntry("e-0").target).toBe("半截 0");
  expect(storedEntry("e-1").target).toBe("半截 1");
}

describe("手工编辑与流式翻译并发", () => {
  it("保存之后到达的模型 delta 不得追加到用户译文上", async () => {
    await renderTable();
    const send = await startTranslation();
    await streamBoth(send);
    await editRow(0, "用户手工译文");
    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
    });

    await act(async () => {
      send({ type: "delta", entryId: "e-0", text: "模型又吐的尾巴" });
      send({ type: "all_done", total: 2, failed: 0 });
      await Promise.resolve();
    });

    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
    });
  });

  it("保存之后到达的 done 不得覆盖用户译文", async () => {
    await renderTable();
    const send = await startTranslation();
    await streamBoth(send);
    await editRow(0, "用户手工译文");

    await act(async () => {
      send({ type: "done", entryId: "e-0", text: "模型的权威译文" });
      send({ type: "all_done", total: 2, failed: 0 });
      backend.finish?.();
      backend.finish = null;
      await Promise.resolve();
    });
    await waitMs(0);

    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
    });
  });

  it("重试开始（progress 重发）不得清掉用户已经保存的译文", async () => {
    await renderTable();
    const send = await startTranslation();
    await streamBoth(send);
    await editRow(0, "用户手工译文");
    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
    });

    // 网络退避 / 结构纠错重试：后端对同一条目再发一次 progress(translating)。
    // 这个信号对「正在流式的条目」是清空旧文本，但对人工成果不适用 ——
    // 清掉之后状态会变回 translating，后续 delta 与 done 都能再覆盖它。
    await act(async () => {
      send({ type: "progress", entryId: "e-0", status: "translating" });
      await Promise.resolve();
    });

    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
    });

    // 重试轮的 delta 也不得落到人工译文上（用 e-1 的 progress 同步 flush）
    await act(async () => {
      send({ type: "delta", entryId: "e-0", text: "模型重试文本" });
      send({ type: "progress", entryId: "e-1", status: "translating" });
      await Promise.resolve();
    });

    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
    });
  });

  it("手工保存译文后，条目上不留上一轮的失败诊断", async () => {
    await renderTable();
    const send = await startTranslation();

    // 这一轮以结构校验失败收尾：条目留下诊断文案
    await act(async () => {
      send({ type: "progress", entryId: "e-0", status: "translating" });
      send({ type: "delta", entryId: "e-0", text: "坏译文" });
      send({
        type: "error",
        entryId: "e-0",
        message: "大模型调用错误: 结构校验未通过：占位符 [1] 多出（已重试 1 次）",
      });
      send({ type: "all_done", total: 1, failed: 1 });
      await Promise.resolve();
    });
    expect(storedEntry("e-0").error).toContain("占位符 [1] 多出");

    // 用户手工改好并保存：错误已解决，诊断必须一起清掉
    await editRow(0, "用户手工译文");
    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
      error: null,
    });
  });

  it("取消收尾回滚不得清掉用户已经保存的译文", async () => {
    await renderTable();
    const send = await startTranslation();
    await streamBoth(send);
    await editRow(0, "用户手工译文");

    const cancelButton = findButton(mounted.container, "取消翻译");
    expect(cancelButton).not.toBeNull();
    await act(async () => {
      click(cancelButton!);
    });
    await act(async () => {
      send({ type: "all_done", total: 2, failed: 1 });
      backend.finish?.();
      backend.finish = null;
      await Promise.resolve();
    });
    await waitMs(0);

    // 没被人工改过的条目照常回滚
    expect(storedEntry("e-1")).toMatchObject({ target: "", status: "pending" });
    // 人工成果必须留着（status 保持 edited，写回时不会被后端退回原文）
    expect(storedEntry("e-0")).toMatchObject({
      target: "用户手工译文",
      status: "edited",
    });
  });
});
