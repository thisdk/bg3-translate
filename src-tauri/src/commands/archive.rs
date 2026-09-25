//! MOD 归档相关命令：打开、解包到目录、重新打包。

use std::path::PathBuf;

use bg3_translate_core::pak;
use bg3_translate_core::types::{ExtractResult, PakFile};
use bg3_translate_core::{AppError, Result};
use tauri::State;

use crate::commands::blocking;
use crate::state::AppState;

/// 打开 MOD（`.pak` / `.zip`）并解包到临时工作目录。
///
/// 会顺带清理上一个 MOD 的工作目录，避免临时文件无限堆积。
#[tauri::command]
pub async fn open_mod(state: State<'_, AppState>, file_path: String) -> Result<ExtractResult> {
    let (work_dir, files) = blocking("解包", move || {
        let (dir, files) = pak::open_and_extract(&file_path)?;
        Ok((dir.to_string_lossy().into_owned(), files))
    })
    .await?;

    if let Some(previous) = state.replace_work_dir(PathBuf::from(&work_dir)) {
        if previous != PathBuf::from(&work_dir) {
            log::info!("清理上一个工作目录: {}", previous.display());
            pak::remove_work_dir(&previous);
        }
    }

    Ok(ExtractResult { work_dir, files })
}

/// 只解压 MOD 到用户指定目录，不进入翻译流程。
#[tauri::command]
pub async fn extract_mod(file_path: String, output_dir: String) -> Result<Vec<PakFile>> {
    blocking("解压", move || {
        pak::extract_to_directory(&file_path, &output_dir)
    })
    .await
}

/// 重新打包工作目录为 `.pak`。
#[tauri::command]
pub async fn repack_mod(work_dir: String, output_path: String) -> Result<()> {
    blocking("打包", move || pak::repack(&work_dir, &output_path)).await
}

/// 清理当前 MOD 的工作目录（前端「开始新的 MOD」时调用；未调用也会在打开下一个 MOD 时自动清理）。
#[tauri::command]
pub async fn close_mod(state: State<'_, AppState>) -> Result<()> {
    if let Some(dir) = state.take_work_dir() {
        log::info!("释放工作目录: {}", dir.display());
        pak::remove_work_dir(&dir);
    }
    Ok(())
}

/// 校验工作目录是否仍然存在（前端切回旧会话时可用来兜底）。
#[tauri::command]
pub async fn work_dir_alive(work_dir: String) -> Result<bool> {
    let path = PathBuf::from(&work_dir);
    if path.as_os_str().is_empty() {
        return Err(AppError::config("工作目录为空"));
    }
    Ok(pak::unpacked_dir(&path).is_dir())
}
