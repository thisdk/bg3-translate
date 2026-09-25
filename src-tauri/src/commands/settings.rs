//! LLM 设置的读写命令。

use bg3_translate_core::config;
use bg3_translate_core::types::LlmSettings;
use bg3_translate_core::{AppError, Result};
use tauri::State;

use crate::state::AppState;

/// 保存设置（写盘 + 更新内存缓存）。
#[tauri::command]
pub async fn save_llm_settings(state: State<'_, AppState>, settings: LlmSettings) -> Result<()> {
    let normalized = settings.normalized();
    let to_store = normalized.clone();
    tokio::task::spawn_blocking(move || config::save(&to_store))
        .await
        .map_err(|err| AppError::other(format!("保存设置任务异常终止: {err}")))??;

    *state.settings_guard().map_err(AppError::config)? = Some(normalized);
    Ok(())
}

/// 读取设置（内存缓存优先，其次磁盘）。
#[tauri::command]
pub async fn load_llm_settings(state: State<'_, AppState>) -> Result<LlmSettings> {
    if let Some(cached) = state.settings_guard().map_err(AppError::config)?.clone() {
        return Ok(cached);
    }

    let loaded = tokio::task::spawn_blocking(config::load)
        .await
        .map_err(|err| AppError::other(format!("读取设置任务异常终止: {err}")))??;

    *state.settings_guard().map_err(AppError::config)? = Some(loaded.clone());
    Ok(loaded)
}
