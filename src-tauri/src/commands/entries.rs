//! 翻译条目的读取与写回。

use bg3_translate_core::types::{PakFileKind, TranslationEntry};
use bg3_translate_core::{Result, formats, pak};
use tauri::State;

use crate::commands::blocking;
use crate::state::AppState;

/// 读取指定文件的可翻译条目。
#[tauri::command]
pub async fn read_file_entries(
    _state: State<'_, AppState>,
    work_dir: String,
    file_name: String,
) -> Result<Vec<TranslationEntry>> {
    let kind = pak::classify_file(&file_name);
    blocking("读取条目", move || {
        formats::read_entries(&work_dir, &file_name, kind)
    })
    .await
}

/// 把编辑后的条目写回磁盘（保持原格式）。
#[tauri::command]
pub async fn write_file_entries(
    work_dir: String,
    file_name: String,
    entries: Vec<TranslationEntry>,
) -> Result<()> {
    let kind: PakFileKind = pak::classify_file(&file_name);
    blocking("写回条目", move || {
        formats::write_entries(&work_dir, &file_name, kind, &entries)
    })
    .await
}
