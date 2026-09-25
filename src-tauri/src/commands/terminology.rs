//! 术语表命令。

use bg3_translate_core::glossary::{Glossary, GlossaryEntry};
use bg3_translate_core::{AppError, Result};

use crate::commands::blocking;

/// 所有术语表命令都是「读盘 → 改 → 写盘」，统一走阻塞线程池。
async fn mutate<F>(what: &str, task: F) -> Result<Glossary>
where
    F: FnOnce(&mut Glossary) -> Result<()> + Send + 'static,
{
    blocking(what, move || {
        let mut glossary = Glossary::load()?;
        task(&mut glossary)?;
        glossary.save()?;
        Ok(glossary)
    })
    .await
}

/// 读取术语表（不存在时用官方种子初始化）。
#[tauri::command]
pub async fn list_glossary() -> Result<Glossary> {
    blocking("读取术语表", Glossary::load).await
}

/// 新增或覆盖一条术语。
#[tauri::command]
pub async fn add_glossary_entry(entry: GlossaryEntry) -> Result<Glossary> {
    mutate("新增术语", move |glossary| glossary.add(entry)).await
}

/// 更新一条术语（按旧原文定位）。
#[tauri::command]
pub async fn update_glossary_entry(old_source: String, entry: GlossaryEntry) -> Result<Glossary> {
    mutate("更新术语", move |glossary| {
        glossary.update(&old_source, entry)
    })
    .await
}

/// 删除一条术语（官方条目不可删除）。
#[tauri::command]
pub async fn delete_glossary_entry(source: String) -> Result<Glossary> {
    mutate("删除术语", move |glossary| glossary.delete(&source)).await
}

/// 重置为内置官方种子。
#[tauri::command]
pub async fn reset_glossary() -> Result<Glossary> {
    blocking("重置术语表", Glossary::reset).await
}

/// 导入游戏提取的完整术语表 JSON（自动清洗噪音条目）。
#[tauri::command]
pub async fn import_glossary(json_str: String) -> Result<Glossary> {
    blocking("导入术语表", move || {
        Glossary::import_json(&json_str).map_err(AppError::from)
    })
    .await
}
