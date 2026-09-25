/**
 * SettingsPanel 行为测试：加载已保存配置、脏标记、保存时夹取、恢复默认、
 * 保存失败提示、运行信息展示。`@/lib/tauri` 全部 mock，不加载 Tauri runtime。
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { SettingsPanel } from "./SettingsPanel";
import {
  click,
  findButton,
  findByTitle,
  mountContainer,
  setInputValue,
  waitMs,
  type Mounted,
} from "@/test-utils/dom";
import { DEFAULT_SETTINGS, useAppStore } from "@/store/app-store";
import type { AppInfo, LlmSettings } from "@/lib/types";

const tauri = vi.hoisted(() => ({
  loadLlmSettings: vi.fn(),
  saveLlmSettings: vi.fn(),
  getAppInfo: vi.fn(),
}));

vi.mock("@/lib/tauri", () => ({
  loadLlmSettings: tauri.loadLlmSettings,
  saveLlmSettings: tauri.saveLlmSettings,
  getAppInfo: tauri.getAppInfo,
}));

const SAVED: LlmSettings = {
  baseUrl: "https://api.example.com",
  apiKey: "sk-saved",
  model: "saved-model",
  concurrency: 8,
  temperature: 0.7,
};

const INFO: AppInfo = {
  version: "0.2.0",
  dataDir: "/home/u/.config/bg3-translate",
  dataDirSource: "系统配置目录",
  portable: false,
};

let mounted: Mounted;

beforeEach(() => {
  tauri.loadLlmSettings.mockReset().mockResolvedValue(SAVED);
  tauri.saveLlmSettings.mockReset().mockResolvedValue(undefined);
  tauri.getAppInfo.mockReset().mockResolvedValue(INFO);
  useAppStore.getState().reset();
  // reset 会保留用户偏好；测试之间需要显式把设置退回默认值
  useAppStore.setState({ settings: DEFAULT_SETTINGS, settingsLoaded: false });
  mounted = mountContainer();
});

afterEach(() => {
  mounted.unmount();
});

async function render() {
  await mounted.render(<SettingsPanel />);
  await waitMs(0);
}

function input(selector: string): HTMLInputElement {
  const el = mounted.container.querySelector<HTMLInputElement>(selector);
  expect(el).not.toBeNull();
  return el!;
}

describe("SettingsPanel 加载与回填", () => {
  it("挂载时读取后端设置并回填表单与 store", async () => {
    await render();

    expect(tauri.loadLlmSettings).toHaveBeenCalledTimes(1);
    expect(input("#baseUrl").value).toBe(SAVED.baseUrl);
    expect(input("#apiKey").value).toBe("sk-saved");
    expect(input("#model").value).toBe("saved-model");
    expect(input("#concurrency").value).toBe("8");
    expect(input("#temperature").value).toBe("0.7");
    expect(mounted.container.textContent).toContain("0.70");
    // 同步写回 store，供翻译任务使用
    expect(useAppStore.getState().settings).toEqual(SAVED);
    expect(useAppStore.getState().settingsLoaded).toBe(true);
  });

  it("展示运行信息（版本 / 配置目录 / 来源）", async () => {
    await render();
    const text = mounted.container.textContent ?? "";
    expect(text).toContain("运行信息");
    expect(text).toContain("0.2.0");
    expect(text).toContain(INFO.dataDir);
    expect(text).toContain("系统配置目录");
    // 非便携模式不展示便携提示
    expect(text).not.toContain("便携模式");
  });

  it("读取设置失败时保留默认值", async () => {
    tauri.loadLlmSettings.mockRejectedValue(new Error("io"));
    await render();
    expect(input("#baseUrl").value).toBe(DEFAULT_SETTINGS.baseUrl);
    expect(input("#concurrency").value).toBe(String(DEFAULT_SETTINGS.concurrency));
  });
});

describe("SettingsPanel 校验与保存", () => {
  it("修改后出现未保存提示，保存成功后提示消失并显示「已保存」", async () => {
    await render();

    setInputValue(input("#baseUrl"), "https://changed.example.com");
    expect(mounted.container.textContent).toContain("有未保存的修改");

    click(findButton(mounted.container, "保存配置")!);
    await waitMs(0);

    expect(tauri.saveLlmSettings).toHaveBeenCalledWith({
      ...SAVED,
      baseUrl: "https://changed.example.com",
    });
    expect(mounted.container.textContent).toContain("已保存");
    expect(mounted.container.textContent).not.toContain("有未保存的修改");
    expect(useAppStore.getState().settings.baseUrl).toBe(
      "https://changed.example.com",
    );
  });

  it("并发数超出范围时被夹取到 1–64（输入与保存都以夹取后的值为准）", async () => {
    await render();

    setInputValue(input("#concurrency"), "999");
    expect(input("#concurrency").value).toBe("64");

    setInputValue(input("#concurrency"), "0");
    expect(input("#concurrency").value).toBe("1");

    // 小数在保存时四舍五入
    setInputValue(input("#concurrency"), "6.4");
    expect(input("#concurrency").value).toBe("6.4");
    click(findButton(mounted.container, "保存配置")!);
    await waitMs(0);
    expect(tauri.saveLlmSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ concurrency: 6 }),
    );
  });

  it("温度预设按钮写回数值并参与保存", async () => {
    await render();

    click(findButton(mounted.container, "发散")!);
    expect(input("#temperature").value).toBe("1");
    expect(mounted.container.textContent).toContain("1.00");

    click(findButton(mounted.container, "保存配置")!);
    await waitMs(0);
    expect(tauri.saveLlmSettings).toHaveBeenLastCalledWith(
      expect.objectContaining({ temperature: 1 }),
    );
  });

  it("保存失败时给出错误提示且不写入 store", async () => {
    tauri.saveLlmSettings.mockRejectedValue(new Error("磁盘满了"));
    await render();

    setInputValue(input("#model"), "next-model");
    click(findButton(mounted.container, "保存配置")!);
    await waitMs(0);

    expect(mounted.container.textContent).toContain("保存配置失败");
    expect(mounted.container.textContent).toContain("磁盘满了");
    // 表单仍是用户输入的值，store 里也没有被写脏
    expect(input("#model").value).toBe("next-model");
    expect(useAppStore.getState().settings.model).toBe(SAVED.model);
  });

  it("「默认」按钮恢复默认值但保留 API Key", async () => {
    await render();

    // 注意：温度预设里也有「默认 0.3」，这里按 title 精确取「恢复默认」按钮
    click(findByTitle(mounted.container, "恢复默认（保留 API Key）")!);

    expect(input("#baseUrl").value).toBe(DEFAULT_SETTINGS.baseUrl);
    expect(input("#model").value).toBe(DEFAULT_SETTINGS.model);
    expect(input("#concurrency").value).toBe(String(DEFAULT_SETTINGS.concurrency));
    expect(input("#temperature").value).toBe(String(DEFAULT_SETTINGS.temperature));
    expect(input("#apiKey").value).toBe("sk-saved");
    expect(mounted.container.textContent).toContain("有未保存的修改");
  });
});
