//! 翻译条目的读取与写回。
//!
//! 安全前提：`work_dir` 与 `file_name` 都是**前端可控字符串**。命令层必须
//! （1）把 `work_dir` 绑到 [`AppState`] 记录的工作目录上，（2）把 `file_name`
//! 当归档路径校验，之后才能交给 core 读写。缺任何一条都能让前端写到
//! 工作目录之外——见本文件底部的回归测试。

use std::path::{Path, PathBuf};

use bg3_translate_core::types::{PakFileKind, TranslationEntry};
use bg3_translate_core::{AppError, Result, formats, pak};
use tauri::State;

use crate::commands::{blocking, checked_work_root, unpacked_path};
use crate::state::AppState;

/// 读取指定文件的可翻译条目。
#[tauri::command]
pub async fn read_file_entries(
    state: State<'_, AppState>,
    work_dir: String,
    file_name: String,
) -> Result<Vec<TranslationEntry>> {
    let kind = pak::classify_file(&file_name);
    let recorded = state.current_work_dir();
    blocking("读取条目", move || {
        read_entries_checked(recorded.as_deref(), &work_dir, &file_name, kind)
    })
    .await
}

/// 把编辑后的条目写回磁盘（保持原格式）。
#[tauri::command]
pub async fn write_file_entries(
    state: State<'_, AppState>,
    work_dir: String,
    file_name: String,
    entries: Vec<TranslationEntry>,
) -> Result<()> {
    let kind: PakFileKind = pak::classify_file(&file_name);
    let recorded = state.current_work_dir();
    blocking("写回条目", move || {
        write_entries_checked(recorded.as_deref(), &work_dir, &file_name, kind, &entries)
    })
    .await
}

/// 校验工作目录 + 归档路径，返回 `unpacked/` 下的真实路径。
fn checked_path(recorded: Option<&Path>, work_dir: &str, file_name: &str) -> Result<PathBuf> {
    let root = checked_work_root(recorded, work_dir)?;
    unpacked_path(&root, file_name)
}

/// 读取实现（把校验从 Tauri 状态里解耦出来，便于单测直接覆盖）。
fn read_entries_checked(
    recorded: Option<&Path>,
    work_dir: &str,
    file_name: &str,
    kind: PakFileKind,
) -> Result<Vec<TranslationEntry>> {
    let path = checked_path(recorded, work_dir, file_name)?;
    if !path.is_file() {
        return Err(AppError::config(format!("文件不存在: {}", path.display())));
    }
    formats::read_entries_from_path(&path, file_name, kind)
}

/// 写回实现（同上；缺失的子目录会被创建，但一定在 `unpacked/` 内）。
fn write_entries_checked(
    recorded: Option<&Path>,
    work_dir: &str,
    file_name: &str,
    kind: PakFileKind,
    entries: &[TranslationEntry],
) -> Result<()> {
    let path = checked_path(recorded, work_dir, file_name)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    formats::write_entries_to_path(&path, kind, entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    use bg3_translate_core::types::TranslationStatus;

    /// 极简临时目录：src-tauri 没有 tempfile 依赖（也不允许新增）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("bg3-translate-shell-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("创建测试临时目录");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// 准备好 `<tmp>/work/unpacked`，返回 (临时目录, 工作目录字符串)。
    fn work_dir(tag: &str) -> (TempDir, String) {
        let tmp = TempDir::new(tag);
        let work = tmp.path().join("work");
        std::fs::create_dir_all(work.join("unpacked")).expect("建工作目录");
        let work_str = work.to_str().expect("临时路径应为 UTF-8").to_string();
        (tmp, work_str)
    }

    fn translated_entries(source_file: &str) -> Vec<TranslationEntry> {
        let mut entry = TranslationEntry::new(source_file, "c1", "1", "Hello");
        entry.target = "你好".to_string();
        entry.status = TranslationStatus::Translated;
        vec![entry]
    }

    fn write_checked(
        recorded: Option<&Path>,
        work_dir: &str,
        file_name: &str,
        entries: &[TranslationEntry],
    ) -> Result<()> {
        write_entries_checked(
            recorded,
            work_dir,
            file_name,
            pak::classify_file(file_name),
            entries,
        )
    }

    /// 攻击面：`../../x-localization.xml` 不得写到工作目录之外。
    #[test]
    fn write_entries_rejects_parent_dir_escape() {
        let (tmp, work) = work_dir("escape-parent");
        let outside = tmp.path().join("escaped-localization.xml");
        assert!(!outside.exists());

        let entries = translated_entries("Mods/x/localization/english.xml");
        let result = write_checked(
            Some(Path::new(&work)),
            &work,
            "../../escaped-localization.xml",
            &entries,
        );

        assert!(
            result.is_err(),
            "`../..` 越权写盘必须被拒绝，实际: {result:?}"
        );
        assert_eq!(result.unwrap_err().code(), "pak");
        assert!(
            !outside.exists(),
            "工作目录之外不应出现文件: {}",
            outside.display()
        );
    }

    /// 攻击面：绝对路径不得让 `join` 丢掉 work_dir 前缀。
    #[test]
    fn write_entries_rejects_absolute_path() {
        let (tmp, work) = work_dir("escape-absolute");
        let absolute = tmp.path().join("absolute-localization.xml");
        let entries = translated_entries("Mods/x/localization/english.xml");

        let result = write_checked(
            Some(Path::new(&work)),
            &work,
            absolute.to_str().expect("UTF-8"),
            &entries,
        );

        assert!(result.is_err(), "绝对路径必须被拒绝，实际: {result:?}");
        assert!(!absolute.exists(), "绝对路径不应被写出");
    }

    /// 攻击面：Windows 风格反斜杠穿越（`..\..\x`）也必须被拦下。
    #[test]
    fn write_entries_rejects_backslash_escape() {
        let (tmp, work) = work_dir("escape-backslash");
        let outside = tmp.path().join("win-localization.xml");
        let entries = translated_entries("x");

        let result = write_checked(
            Some(Path::new(&work)),
            &work,
            r"..\..\win-localization.xml",
            &entries,
        );

        assert!(result.is_err(), "反斜杠穿越必须被拒绝，实际: {result:?}");
        assert!(!outside.exists());
    }

    /// 攻击面：`work_dir` 本身也被前端控制，必须等于 AppState 记录的那个。
    #[test]
    fn write_entries_rejects_foreign_work_dir() {
        let (tmp, work) = work_dir("foreign");
        let other = tmp.path().join("other");
        std::fs::create_dir_all(other.join("unpacked")).expect("建另一个目录");
        let other_str = other.to_str().expect("UTF-8").to_string();

        let entries = translated_entries("Mods/x/localization/english.xml");
        // recorded 是 work，前端却传 other：拒绝
        let result = write_checked(
            Some(Path::new(&work)),
            &other_str,
            "localization/evil.xml",
            &entries,
        );
        assert!(
            result.is_err(),
            "非当前工作目录必须被拒绝，实际: {result:?}"
        );
        assert!(!other.join("unpacked/localization/evil.xml").exists());

        // AppState 里根本没有工作目录（没打开过 MOD）：同样拒绝
        let result = write_checked(None, &work, "localization/evil.xml", &entries);
        assert!(result.is_err(), "未打开 MOD 时必须拒绝，实际: {result:?}");
    }

    /// 正常路径必须仍然能写：条目写进 `unpacked/<PAK 内路径>`。
    #[test]
    fn write_entries_still_writes_inside_unpacked() {
        let (_tmp, work) = work_dir("happy-path");
        let entries = translated_entries("Mods/x/localization/english.xml");
        let file_name = "Mods/x/Localization/Chinese/english.xml";

        write_checked(Some(Path::new(&work)), &work, file_name, &entries).expect("正常写回应成功");

        let written = Path::new(&work).join("unpacked").join(file_name);
        assert!(written.is_file(), "应写到 {}", written.display());
        let xml = std::fs::read_to_string(&written).expect("读回写出的 XML");
        assert!(xml.contains("你好"), "译文应落盘: {xml}");
        assert!(xml.contains(r#"contentuid="c1""#), "contentuid 应保留");
    }

    /// 正常路径的**正向证据**：`open_mod` 把工作目录记进 AppState → 前端把
    /// `open_mod` 返回的字符串原样回传 `read_file_entries` / `write_file_entries`
    /// → 校验必须通过，并且能正常写回、读回。
    #[test]
    fn frontend_echoed_work_dir_passes_validation() {
        let (_tmp, work) = work_dir("echo-flow");
        let state = AppState::default();
        // 模拟 open_mod：解包完成后记录工作目录
        assert!(state.replace_work_dir(PathBuf::from(&work)).is_none());
        let recorded = state.current_work_dir().expect("AppState 应记录工作目录");
        assert_eq!(recorded, PathBuf::from(&work));

        let file_name = "Mods/x/Localization/Chinese/english.xml";
        let kind = pak::classify_file(file_name);
        let entries = translated_entries(file_name);

        write_entries_checked(Some(&recorded), &work, file_name, kind, &entries)
            .expect("前端原样回传必须通过校验并写回");

        let loaded = read_entries_checked(Some(&recorded), &work, file_name, kind)
            .expect("前端原样回传必须能读回");
        assert_eq!(loaded.len(), 1);
        // 读盘语义：磁盘上的文本会进 source（「待翻译原文」），
        // contentuid / version 必须逐字保留，target 由翻译流程填充
        assert_eq!(loaded[0].source, "你好");
        assert_eq!(loaded[0].contentuid, "c1");
        assert_eq!(loaded[0].version, "1");
        assert!(loaded[0].target.is_empty());
    }

    /// 读取侧同样不能越界（`read_file_entries` 也用同一条校验）。
    #[test]
    fn read_entries_rejects_escape_and_keeps_missing_file_error() {
        let (tmp, work) = work_dir("read");
        let outside = tmp.path().join("outside-localization.xml");
        std::fs::write(&outside, b"<contentList/>").expect("写外部文件");

        let escaped = read_entries_checked(
            Some(Path::new(&work)),
            &work,
            "../outside-localization.xml",
            PakFileKind::LocalizationXml,
        );
        assert!(escaped.is_err(), "越界读取必须被拒绝，实际: {escaped:?}");

        let absolute = read_entries_checked(
            Some(Path::new(&work)),
            &work,
            outside.to_str().expect("UTF-8"),
            PakFileKind::LocalizationXml,
        );
        assert!(absolute.is_err(), "绝对路径读取必须被拒绝");

        // 合法但不存在：仍然报「文件不存在」（错误码保持 config，前端提示不变）
        let missing = read_entries_checked(
            Some(Path::new(&work)),
            &work,
            "Mods/x/localization/none.xml",
            PakFileKind::LocalizationXml,
        )
        .expect_err("文件不存在应报错");
        assert_eq!(missing.code(), "config");
        assert!(missing.to_string().contains("文件不存在"), "{missing}");
    }

    /// `..` 之外的花样：`:`（NTFS 数据流）与 Windows 保留设备名也必须拒绝。
    #[test]
    fn write_entries_rejects_colon_and_reserved_names() {
        let (_tmp, work) = work_dir("reserved");
        let entries = translated_entries("x");

        for file_name in ["unpacked/a:b.lsx", "NUL.lsx", "CON.loca"] {
            let result = write_checked(Some(Path::new(&work)), &work, file_name, &entries);
            assert!(result.is_err(), "{file_name} 应被拒绝，实际: {result:?}");
        }
    }
}
