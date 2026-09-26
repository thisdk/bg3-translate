//! 三种本地化格式与统一 [`TranslationEntry`] 的互转。
//!
//! 设计铁律（改代码前先读）：
//! - `contentuid` / `version` 是游戏查表的句柄，**绝不能改**，只改文本
//! - 富文本标签（`<LSTag>`、`<font>`、`<i>`…）与占位符（`{1}`、`{2}`、`[1]`、`[2]`）
//!   必须原样保留
//! - 写回时若条目没有译文，就保留原文，绝不写空
//!
//! [`TranslationEntry`]: crate::types::TranslationEntry

pub mod content_list;
pub mod loca;
pub mod lsx;

use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};
use crate::types::{PakFileKind, TranslationEntry};

/// 把 `work_dir` + PAK 内 `file_name` 解析成 `unpacked/` 下的**受校验**磁盘路径。
///
/// [`crate::pak::resolve_disk_path`] 只是纯 `join`：`../evil.loca`、`/tmp/x.lsx`、
/// `a\..\..\x.lsx` 都会逃出工作目录（`work_dir/evil.loca` 甚至更外面）。
/// `file_name` 是前端可控字符串，所以读写之前必须先过
/// [`crate::pak::safe_output_path`]。
fn checked_disk_path(work_dir: &str, file_name: &str) -> Result<PathBuf> {
    crate::pak::safe_output_path(&crate::pak::unpacked_dir(work_dir), file_name)
}

/// 按文件类型从磁盘读取可翻译条目。
pub fn read_entries(
    work_dir: &str,
    file_name: &str,
    kind: PakFileKind,
) -> Result<Vec<TranslationEntry>> {
    let path = checked_disk_path(work_dir, file_name)?;
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
    let path = checked_disk_path(work_dir, file_name)?;
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

    /// `file_name` 是前端可控字符串，`resolve_disk_path` 只是纯 `join`：
    /// `../evil.loca` 会写到 `unpacked/` 之外（工作目录、临时目录、任意位置）。
    /// 复现（修复前）：`write_entries(work_dir, "../evil.loca", ..)` 返回 Ok
    /// 且 `work_dir/evil.loca` 真的被创建出来。
    #[test]
    fn write_entries_rejects_file_names_that_escape_unpacked() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(unpacked_style(&work)).unwrap();

        let mut entry = TranslationEntry::new("../evil.loca", "h1", "1", "Hello");
        entry.mark_translated("坏");

        for name in ["../evil.loca", "../../evil.loca", "a/../../evil.loca"] {
            let err = write_entries(
                work.to_str().unwrap(),
                name,
                PakFileKind::LocalizationLoca,
                std::slice::from_ref(&entry),
            )
            .unwrap_err();
            assert_eq!(err.code(), "pak", "{name} 必须被拒绝");
        }
        assert!(
            !work.join("evil.loca").exists() && !dir.path().join("evil.loca").exists(),
            "越界路径不得落盘"
        );

        // 正常的 PAK 内路径（含需要新建的中文目录）不受影响
        let mut ok = TranslationEntry::new("Localization/Chinese/x.loca", "h1", "1", "Hello");
        ok.mark_translated("你好");
        write_entries(
            work.to_str().unwrap(),
            "Localization/Chinese/x.loca",
            PakFileKind::LocalizationLoca,
            &[ok],
        )
        .unwrap();
        assert!(work.join("unpacked/Localization/Chinese/x.loca").is_file());
    }

    /// 读路径同样要校验：否则前端可以拿 `file_name` 去读工作目录外的文件。
    #[test]
    fn read_entries_rejects_file_names_that_escape_unpacked() {
        let dir = tempfile::tempdir().unwrap();
        let work = dir.path().join("work");
        std::fs::create_dir_all(unpacked_style(&work)).unwrap();
        // 把「机密」文件放在工作目录里（unpacked/ 之外）
        std::fs::write(
            work.join("secret.xml"),
            br#"<contentList><content contentuid="h1" version="1">SECRET</content></contentList>"#,
        )
        .unwrap();

        let err = read_entries(
            work.to_str().unwrap(),
            "../secret.xml",
            PakFileKind::LocalizationXml,
        )
        .unwrap_err();
        assert_eq!(err.code(), "pak", "越界读取必须被拒绝");
    }

    /// 只为让测试目录结构与 `unpacked_dir` 保持一致（避免测试里再 import 一次 pak）。
    fn unpacked_style(work: &std::path::Path) -> std::path::PathBuf {
        crate::pak::unpacked_dir(work)
    }
}
