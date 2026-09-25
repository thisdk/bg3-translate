//! 三种本地化格式与统一 [`TranslationEntry`] 的互转。
//!
//! 设计铁律（改代码前先读）：
//! - `contentuid` / `version` 是游戏查表的句柄，**绝不能改**，只改文本
//! - 富文本标签（`<LSTag>`、`<font>`、`<i>`…）与占位符（`{1}`、`{2}`）必须原样保留
//! - 写回时若条目没有译文，就保留原文，绝不写空
//!
//! [`TranslationEntry`]: crate::types::TranslationEntry

pub mod content_list;
pub mod loca;
pub mod lsx;

use std::path::Path;

use crate::error::{AppError, Result};
use crate::types::{PakFileKind, TranslationEntry};

/// 按文件类型从磁盘读取可翻译条目。
pub fn read_entries(
    work_dir: &str,
    file_name: &str,
    kind: PakFileKind,
) -> Result<Vec<TranslationEntry>> {
    let path = crate::pak::resolve_disk_path(work_dir, file_name);
    if !path.is_file() {
        return Err(AppError::config(format!("文件不存在: {}", path.display())));
    }
    read_entries_from_path(&path, file_name, kind)
}

/// 按文件类型从指定路径读取条目（便于单测直接给临时文件）。
pub fn read_entries_from_path(
    path: &Path,
    file_name: &str,
    kind: PakFileKind,
) -> Result<Vec<TranslationEntry>> {
    match kind {
        PakFileKind::LocalizationXml => content_list::read(path, file_name),
        PakFileKind::LocalizationLoca => loca::read(path, file_name),
        PakFileKind::MetadataLsx => lsx::read(path, file_name),
        _ => Ok(Vec::new()),
    }
}

/// 按文件类型把条目写回磁盘（保持原格式）。
pub fn write_entries(
    work_dir: &str,
    file_name: &str,
    kind: PakFileKind,
    entries: &[TranslationEntry],
) -> Result<()> {
    let path = crate::pak::resolve_disk_path(work_dir, file_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    write_entries_to_path(&path, kind, entries)
}

/// 按文件类型把条目写到指定路径。
pub fn write_entries_to_path(
    path: &Path,
    kind: PakFileKind,
    entries: &[TranslationEntry],
) -> Result<()> {
    match kind {
        PakFileKind::LocalizationXml => content_list::write(path, entries),
        PakFileKind::LocalizationLoca => loca::write(path, entries),
        PakFileKind::MetadataLsx => lsx::write(path, entries),
        // 其他类型没有可写回的内容，静默跳过（与旧行为一致）
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_translatable_kinds_are_noops() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("x.lua");
        std::fs::write(&path, b"print(1)").unwrap();

        assert!(
            read_entries_from_path(&path, "x.lua", PakFileKind::ScriptLua)
                .unwrap()
                .is_empty()
        );
        write_entries_to_path(&path, PakFileKind::ScriptLua, &[]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "print(1)");
    }

    #[test]
    fn read_entries_reports_missing_file() {
        let err = read_entries("/nope", "a.xml", PakFileKind::LocalizationXml).unwrap_err();
        assert_eq!(err.code(), "config");
    }
}
