//! # bg3-translate-core
//!
//! 《博德之门 3》MOD 汉化工具的核心逻辑，**不依赖 Tauri / 任何 GUI 系统库**。
//!
//! 拆成独立 crate 的原因：
//! 1. 可以在没有 webkit2gtk / dbus 的机器上 `cargo test`（CI 也能并行跑）
//! 2. 业务逻辑与桌面壳解耦，未来做 CLI 或 Web 版可以直接复用
//! 3. 单元测试不需要启动整个 Tauri 应用
//!
//! 模块职责：
//! - [`error`]：统一错误类型
//! - [`types`]：跨 IPC 边界的数据结构（与 `src/lib/types.ts` 一一对应）
//! - [`config`]：便携/系统数据目录解析与设置持久化
//! - [`pak`]：PAK / ZIP 解包与重打包
//! - [`formats`]：`contentList` XML / LSX / LOCA 三种格式与统一条目的互转
//! - [`glossary`]：BG3 官方与自定义术语表 + 文本命中匹配
//! - [`translation`]：LLM 流式翻译引擎

pub mod config;
pub mod error;
pub mod formats;
pub mod pak;
pub mod types;

pub use error::{AppError, Result};

/// 应用版本（与 `Cargo.toml` / `tauri.conf.json` 保持一致）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 翻译目标语言在 PAK 路径中使用的目录名。
pub const TARGET_LANGUAGE: &str = "Chinese";

/// 目标语言的可接受别名（用于判断某个本地化文件是否已经是中文）。
pub const TARGET_LANGUAGE_ALIASES: &[&str] = &["Chinese", "ChineseSimplified", "zh-CN"];

/// 判断某个语言目录名是否属于目标语言。
pub fn is_target_language(language: &str) -> bool {
    TARGET_LANGUAGE_ALIASES
        .iter()
        .any(|alias| alias.eq_ignore_ascii_case(language))
}

/// 把本地化文件路径改写到目标语言目录。
///
/// `Localization/English/foo.xml` → `Localization/Chinese/foo.xml`
///
/// 找不到 `Localization/<lang>/` 结构时原样返回。
pub fn to_target_localization_path(file_name: &str) -> String {
    let normalized = file_name.replace('\\', "/");
    let mut parts: Vec<&str> = normalized.split('/').collect();
    let Some(index) = parts
        .iter()
        .position(|part| part.eq_ignore_ascii_case("Localization"))
    else {
        return normalized;
    };
    if parts.len() <= index + 2 {
        return normalized;
    }
    parts[index + 1] = TARGET_LANGUAGE;
    parts.join("/")
}

/// 写回同一目标文件时的优先级：英文源文件优先，其次是已有中文，最后是其他语言。
pub fn localization_write_priority(language: Option<&str>) -> u8 {
    match language {
        Some(lang) if lang.eq_ignore_ascii_case("English") => 3,
        Some(_) => 1,
        None => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_localization_language_segment() {
        assert_eq!(
            to_target_localization_path("Localization/English/foo.xml"),
            "Localization/Chinese/foo.xml"
        );
        // 大小写不敏感
        assert_eq!(
            to_target_localization_path("localization/english/foo.xml"),
            "localization/Chinese/foo.xml"
        );
        // Windows 分隔符
        assert_eq!(
            to_target_localization_path(r"Mods\X\Localization\Polish\a.loca"),
            "Mods/X/Localization/Chinese/a.loca"
        );
    }

    #[test]
    fn leaves_paths_without_language_segment_alone() {
        assert_eq!(
            to_target_localization_path("Mods/Meta.lsx"),
            "Mods/Meta.lsx"
        );
        assert_eq!(
            to_target_localization_path("Localization/foo.xml"),
            "Localization/foo.xml"
        );
        assert_eq!(to_target_localization_path("Localization"), "Localization");
    }

    #[test]
    fn target_language_detection() {
        assert!(is_target_language("Chinese"));
        assert!(is_target_language("chinesesimplified"));
        assert!(is_target_language("zh-CN"));
        assert!(!is_target_language("English"));
    }

    #[test]
    fn english_sources_win_over_other_languages() {
        assert_eq!(localization_write_priority(Some("English")), 3);
        assert_eq!(localization_write_priority(Some("Chinese")), 1);
        assert_eq!(localization_write_priority(Some("Polish")), 1);
        assert_eq!(localization_write_priority(None), 0);
    }

    #[test]
    fn version_is_exposed() {
        assert!(!VERSION.is_empty());
    }
}
