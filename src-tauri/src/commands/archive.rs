//! MOD 归档相关命令：打开、解包到目录、重新打包。

use std::path::PathBuf;

use bg3_translate_core::Result;
use bg3_translate_core::pak;
use bg3_translate_core::types::{ExtractResult, PakFile};
use tauri::State;

use crate::commands::{blocking, checked_work_root};
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

    // 记下新工作目录，并把上一个目录删掉（每次都是新建的唯一目录，
    // 相同只可能出现在极端并发下，所以顺手判一下再删）
    if let Some(previous) = state.replace_work_dir(PathBuf::from(&work_dir)) {
        if previous.as_os_str() != work_dir.as_str() {
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
///
/// `output_path` 是用户通过系统对话框选的保存位置，**合法地可以指向任意目录**
/// （导出到桌面是正常用法），所以不限制它；要校验的是 `work_dir`：它由前端
/// 提供，必须等于 `open_mod` 记录的那个工作目录，否则前端能把任意目录打成
/// pak 写到任意位置。
#[tauri::command]
pub async fn repack_mod(
    state: State<'_, AppState>,
    work_dir: String,
    output_path: String,
) -> Result<()> {
    let recorded = state.current_work_dir();
    blocking("打包", move || {
        let root = checked_work_root(recorded.as_deref(), &work_dir)?;
        pak::repack(&root.to_string_lossy(), &output_path)
    })
    .await
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
