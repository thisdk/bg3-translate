//! 流式翻译命令。

use bg3_translate_core::config;
use bg3_translate_core::glossary::Glossary;
use bg3_translate_core::translation::{
    EventSink, RunOptions, TranslationEngine, TranslationEvent, TranslationSummary,
};
use bg3_translate_core::types::{LlmSettings, TranslationEntry};
use bg3_translate_core::{AppError, Result};
use tauri::State;
use tauri::ipc::Channel;

use crate::state::AppState;

/// 把 core 的事件桥接到 Tauri Channel。
///
/// 推送失败只记 debug 日志：前端窗口关掉之后 `send` 会失败，
/// 但这不应该让翻译任务本身报错。
struct ChannelSink {
    channel: Channel<TranslationEvent>,
}

impl EventSink for ChannelSink {
    fn emit(&self, event: TranslationEvent) {
        if let Err(err) = self.channel.send(event) {
            log::debug!("翻译事件推送失败（前端可能已关闭）: {err}");
        }
    }
}

/// 流式翻译条目。
///
/// - 只翻译 `source` 非空且 `target` 为空的条目（其余直接跳过）
/// - 事件通过 `onEvent` Channel 实时推送
/// - 期间可以用 `cancel_translation` 取消
#[tauri::command]
pub async fn translate_entries(
    state: State<'_, AppState>,
    _work_dir: String,
    entries: Vec<TranslationEntry>,
    style_hint: Option<String>,
    on_event: Channel<TranslationEvent>,
) -> Result<TranslationSummary> {
    let settings = resolve_settings(&state).await?;
    if !settings.is_configured() {
        return Err(AppError::config(
            "未配置 API Key，请先在设置中填写大模型连接信息",
        ));
    }

    let glossary = tokio::task::spawn_blocking(Glossary::load)
        .await
        .map_err(|err| AppError::other(format!("读取术语表任务异常终止: {err}")))??;
    let matcher = glossary.matcher();

    state.cancel.reset();
    let sink = ChannelSink { channel: on_event };

    let engine = TranslationEngine::with_settings(settings)?;
    let summary = engine
        .run(
            &entries,
            &matcher,
            RunOptions {
                style_hint: style_hint.as_deref().unwrap_or_default(),
                sink: &sink,
                cancel: state.cancel.clone(),
            },
        )
        .await?;

    Ok(summary)
}

/// 请求取消当前翻译任务。
#[tauri::command]
pub async fn cancel_translation(state: State<'_, AppState>) -> Result<()> {
    log::info!("收到取消翻译请求");
    state.cancel.cancel();
    Ok(())
}

/// 设置来源：内存缓存优先，其次磁盘，最后默认值。
async fn resolve_settings(state: &State<'_, AppState>) -> Result<LlmSettings> {
    if let Some(cached) = state.settings_guard().map_err(AppError::config)?.clone() {
        return Ok(cached);
    }
    let loaded = tokio::task::spawn_blocking(config::load)
        .await
        .map_err(|err| AppError::other(format!("读取设置任务异常终止: {err}")))??;
    *state.settings_guard().map_err(AppError::config)? = Some(loaded.clone());
    Ok(loaded)
}
