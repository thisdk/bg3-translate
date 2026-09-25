import { useEffect, useState } from "react";
import { Gauge, RotateCcw, Save, Settings2, Thermometer } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { NumberInput } from "@/components/ui/number-input";
import { ErrorBanner } from "@/components/ui/error-banner";
import { loadLlmSettings, saveLlmSettings } from "@/lib/tauri";
import { DEFAULT_SETTINGS, useAppStore } from "@/store/app-store";
import { cn } from "@/lib/utils";
import type { LlmSettings } from "@/lib/types";

/** 采样温度预设 */
const TEMPERATURE_PRESETS = [
  { value: 0.1, label: "严谨" },
  { value: 0.3, label: "默认" },
  { value: 0.7, label: "灵活" },
  { value: 1, label: "发散" },
];

export function SettingsPanel({ compact = false }: { compact?: boolean }) {
  const settings = useAppStore((s) => s.settings);
  const setSettings = useAppStore((s) => s.setSettings);
  const [form, setForm] = useState<LlmSettings>(settings);
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [error, setError] = useState<string | null>(null);

  // 首次加载已保存设置
  useEffect(() => {
    loadLlmSettings()
      .then((s) => {
        setForm(s);
        setSettings(s);
      })
      .catch(() => {
        /* 用默认值 */
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onSave = async () => {
    const concurrency = Number.isFinite(form.concurrency)
      ? form.concurrency
      : 1;
    const temperature = Number.isFinite(form.temperature)
      ? form.temperature
      : DEFAULT_SETTINGS.temperature;
    const normalizedForm: LlmSettings = {
      ...form,
      concurrency: Math.round(Math.min(100, Math.max(1, concurrency))),
      temperature: Math.min(1, Math.max(0, temperature)),
    };
    setSaving(true);
    setError(null);
    try {
      await saveLlmSettings(normalizedForm);
      setForm(normalizedForm);
      setSettings(normalizedForm);
      setSaved(true);
      setTimeout(() => setSaved(false), 1500);
    } catch (e) {
      setError(`保存配置失败：${String(e)}`);
    } finally {
      setSaving(false);
    }
  };

  const onResetDefaults = () => {
    setForm({ ...DEFAULT_SETTINGS, apiKey: form.apiKey });
    setError(null);
  };

  const dirty =
    form.baseUrl !== settings.baseUrl ||
    form.apiKey !== settings.apiKey ||
    form.model !== settings.model ||
    form.concurrency !== settings.concurrency ||
    form.temperature !== settings.temperature;

  return (
    <Card
      className={
        compact ? "rounded-none border-0 bg-transparent shadow-none" : ""
      }
    >
      <CardHeader className={compact ? "p-0 pb-5" : ""}>
        <CardTitle className="flex items-center gap-2 text-base">
          <Settings2 className="h-4 w-4" />
          大模型配置
        </CardTitle>
        <CardDescription>
          使用 OpenAI 兼容协议，支持 DeepSeek、智谱、Kimi、OpenAI、本地 Ollama 等
        </CardDescription>
      </CardHeader>
      <CardContent className={compact ? "space-y-4 p-0" : "space-y-4"}>
        {error && (
          <ErrorBanner
            message={error}
            onClose={() => setError(null)}
            compact
            className="rounded-md border"
          />
        )}

        <div className="space-y-2">
          <Label htmlFor="baseUrl">API Base URL</Label>
          <Input
            id="baseUrl"
            placeholder="https://api.deepseek.com"
            value={form.baseUrl}
            onChange={(e) => setForm({ ...form, baseUrl: e.target.value })}
          />
        </div>

        <div className="space-y-2">
          <Label htmlFor="apiKey">API Key</Label>
          <Input
            id="apiKey"
            type="password"
            placeholder="sk-..."
            value={form.apiKey}
            onChange={(e) => setForm({ ...form, apiKey: e.target.value })}
          />
          <p className="text-xs text-muted-foreground">
            密钥仅保存在本地配置文件，不会上传。
          </p>
        </div>

        <div className="space-y-2">
          <Label htmlFor="model">模型名称</Label>
          <Input
            id="model"
            placeholder="deepseek-chat"
            value={form.model}
            onChange={(e) => setForm({ ...form, model: e.target.value })}
          />
        </div>

        <div className="space-y-2">
          <Label htmlFor="concurrency" className="flex items-center gap-1.5">
            <Gauge className="h-3.5 w-3.5 text-muted-foreground" />
            并发数（同时翻译的条目数）
          </Label>
          <NumberInput
            id="concurrency"
            min={1}
            max={100}
            value={form.concurrency}
            onValueChange={(concurrency) => setForm({ ...form, concurrency })}
          />
          <p className="text-xs text-muted-foreground">
            数值越大翻译越快，但会增加 API 并发请求量。建议 4-8，最高 100。
          </p>
        </div>

        {/* 采样温度 */}
        <div className="space-y-2">
          <div className="flex items-center justify-between gap-2">
            <Label htmlFor="temperature" className="flex items-center gap-1.5">
              <Thermometer className="h-3.5 w-3.5 text-muted-foreground" />
              采样温度
            </Label>
            <span className="tabular-nums text-xs font-medium text-muted-foreground">
              {form.temperature.toFixed(2)}
            </span>
          </div>
          <input
            id="temperature"
            type="range"
            min={0}
            max={1}
            step={0.05}
            value={form.temperature}
            onChange={(e) =>
              setForm({ ...form, temperature: Number(e.target.value) })
            }
            className={cn(
              "h-1.5 w-full cursor-pointer appearance-none rounded-full bg-secondary accent-primary",
              "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background",
            )}
            aria-label="采样温度"
          />
          <div className="flex flex-wrap items-center gap-1">
            {TEMPERATURE_PRESETS.map((preset) => (
              <button
                key={preset.value}
                type="button"
                onClick={() => setForm({ ...form, temperature: preset.value })}
                className={cn(
                  "rounded-full border px-2 py-0.5 text-[11px] transition-colors",
                  "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
                  form.temperature === preset.value
                    ? "border-transparent bg-primary text-primary-foreground"
                    : "border-input text-muted-foreground hover:bg-accent hover:text-accent-foreground",
                )}
              >
                {preset.label} {preset.value}
              </button>
            ))}
          </div>
          <p className="text-xs text-muted-foreground">
            采样温度 0–1，越低越稳定，默认 0.3。翻译任务建议 0.1–0.4，过高会自由发挥。
          </p>
        </div>

        <div className="flex items-center gap-2 pt-1">
          <Button onClick={onSave} loading={saving} className="flex-1">
            <Save className="h-4 w-4" />
            {saving ? "保存中…" : saved ? "已保存" : "保存配置"}
          </Button>
          <Button
            type="button"
            variant="outline"
            onClick={onResetDefaults}
            title="恢复默认（保留 API Key）"
            className="shrink-0"
          >
            <RotateCcw className="h-4 w-4" />
            默认
          </Button>
        </div>
        {dirty && !saving && (
          <p className="text-center text-[11px] text-amber-600 dark:text-amber-400">
            有未保存的修改
          </p>
        )}
      </CardContent>
    </Card>
  );
}
