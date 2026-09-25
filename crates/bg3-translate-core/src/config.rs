//! 配置与数据目录解析。
//!
//! 目录优先级（与 README 的「便携版」承诺一致）：
//! 1. 环境变量 `BG3_TRANSLATE_HOME`（便于测试与高级用户自定义，最高优先级）
//! 2. **便携模式**：可执行文件同级目录的 `config/`（能写就用它，解压即用、方便备份）
//! 3. **系统模式**：平台标准配置目录（`%APPDATA%\bg3-translate`、`~/.config/bg3-translate`）
//!
//! 第 3 步是必要的：安装到 `C:\Program Files` 时 exe 同级目录通常不可写。

use std::path::{Path, PathBuf};

use crate::error::{AppError, Result};
use crate::types::LlmSettings;

/// 环境变量名：直接指定数据目录。
pub const ENV_HOME: &str = "BG3_TRANSLATE_HOME";

/// 数据目录内的子目录名（便携模式下位于 exe 同级）。
pub const PORTABLE_DIR_NAME: &str = "config";

/// 应用标识（用于系统配置目录）。
const APP_QUALIFIER: &str = "";
const APP_ORGANIZATION: &str = "";
const APP_NAME: &str = "bg3-translate";

/// 数据目录的来源，便于日志与 UI 展示。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataDirSource {
    /// 环境变量指定
    EnvOverride,
    /// exe 同级目录（便携）
    Portable,
    /// 系统配置目录
    System,
}

impl DataDirSource {
    pub const fn as_str(self) -> &'static str {
        match self {
            DataDirSource::EnvOverride => "环境变量",
            DataDirSource::Portable => "便携目录",
            DataDirSource::System => "系统配置目录",
        }
    }
}

/// 解析结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataDir {
    pub path: PathBuf,
    pub source: DataDirSource,
}

/// **纯函数**：根据候选路径决定数据目录，不做任何 IO。
///
/// 这是目录策略的唯一真相来源，单元测试直接覆盖它。
pub fn resolve_data_dir(
    env_home: Option<&Path>,
    exe_dir: Option<&Path>,
    system_dir: &Path,
    exe_dir_writable: bool,
) -> DataDir {
    if let Some(dir) = env_home {
        if !dir.as_os_str().is_empty() {
            return DataDir {
                path: dir.to_path_buf(),
                source: DataDirSource::EnvOverride,
            };
        }
    }
    if let Some(dir) = exe_dir {
        if exe_dir_writable {
            return DataDir {
                path: dir.join(PORTABLE_DIR_NAME),
                source: DataDirSource::Portable,
            };
        }
    }
    DataDir {
        path: system_dir.to_path_buf(),
        source: DataDirSource::System,
    }
}

/// 平台标准配置目录（不保证存在）。
pub fn system_data_dir() -> PathBuf {
    directories::ProjectDirs::from(APP_QUALIFIER, APP_ORGANIZATION, APP_NAME)
        .map(|dirs| dirs.config_dir().to_path_buf())
        .unwrap_or_else(|| std::env::temp_dir().join(APP_NAME))
}

/// 探测目录是否可写（不存在则尝试创建）。
fn dir_writable(dir: &Path) -> bool {
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let probe = dir.join(format!(".write-probe-{}", std::process::id()));
    match std::fs::write(&probe, b"") {
        Ok(()) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 当前进程可执行文件所在目录。
fn current_exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
}

/// 解析并创建数据目录。
///
/// 每次调用都会重新探测（要判断 exe 同级目录是否可写），
/// 但调用点很少（读写设置、读写术语表），这点 IO 可以忽略。
pub fn data_dir() -> Result<DataDir> {
    let env_home = std::env::var_os(ENV_HOME).map(PathBuf::from);
    let exe_dir = current_exe_dir();
    let exe_dir_writable = exe_dir.as_deref().is_some_and(dir_writable);
    let system = system_data_dir();

    let resolved = resolve_data_dir(
        env_home.as_deref(),
        exe_dir.as_deref(),
        &system,
        exe_dir_writable,
    );

    // 系统目录也失败时退回临时目录，保证应用还能启动
    if std::fs::create_dir_all(&resolved.path).is_err() {
        let fallback = std::env::temp_dir().join(APP_NAME);
        std::fs::create_dir_all(&fallback)?;
        log::warn!(
            "无法创建数据目录 {}，回退到 {}",
            resolved.path.display(),
            fallback.display()
        );
        return Ok(DataDir {
            path: fallback,
            source: DataDirSource::System,
        });
    }

    Ok(resolved)
}

/// 设置文件绝对路径（不创建目录）。
pub fn settings_path_in(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// 术语表文件绝对路径（不创建目录）。
pub fn glossary_path_in(dir: &Path) -> PathBuf {
    dir.join("glossary.json")
}

/// 从指定目录读取设置；文件不存在或损坏时返回默认值。
pub fn load_from(dir: &Path) -> Result<LlmSettings> {
    let path = settings_path_in(dir);
    if !path.exists() {
        return Ok(LlmSettings::default());
    }
    let content = std::fs::read_to_string(&path)?;
    match serde_json::from_str::<LlmSettings>(&content) {
        Ok(settings) => Ok(settings.normalized()),
        Err(err) => {
            log::warn!("设置文件损坏（{}），已回退默认值: {err}", path.display());
            Ok(LlmSettings::default())
        }
    }
}

/// 把设置写入指定目录。
pub fn save_to(dir: &Path, settings: &LlmSettings) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = settings_path_in(dir);
    let content = serde_json::to_string_pretty(&settings.clone().normalized())
        .map_err(|e| AppError::Config(format!("序列化设置失败: {e}")))?;
    write_atomic(&path, content.as_bytes())?;
    Ok(())
}

/// 读取当前数据目录下的设置。
pub fn load() -> Result<LlmSettings> {
    load_from(&data_dir()?.path)
}

/// 写入当前数据目录下的设置。
pub fn save(settings: &LlmSettings) -> Result<()> {
    save_to(&data_dir()?.path, settings)
}

/// 原子写入：先写临时文件再 rename，避免断电/崩溃留下半个文件。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_file(&tmp);
            Err(AppError::Io(err))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn env_override_wins_over_everything() {
        let resolved = resolve_data_dir(
            Some(Path::new("/custom/home")),
            Some(Path::new("/exe/dir")),
            Path::new("/system/dir"),
            true,
        );
        assert_eq!(resolved.source, DataDirSource::EnvOverride);
        assert_eq!(resolved.path, PathBuf::from("/custom/home"));
    }

    #[test]
    fn blank_env_override_is_ignored() {
        let resolved = resolve_data_dir(
            Some(Path::new("")),
            Some(Path::new("/exe/dir")),
            Path::new("/system/dir"),
            true,
        );
        assert_eq!(resolved.source, DataDirSource::Portable);
    }

    #[test]
    fn writable_exe_dir_means_portable_mode() {
        let resolved = resolve_data_dir(
            None,
            Some(Path::new("/exe/dir")),
            Path::new("/system/dir"),
            true,
        );
        assert_eq!(resolved.source, DataDirSource::Portable);
        assert_eq!(resolved.path, PathBuf::from("/exe/dir/config"));
    }

    #[test]
    fn read_only_exe_dir_falls_back_to_system_dir() {
        let resolved = resolve_data_dir(
            None,
            Some(Path::new("/program files/app")),
            Path::new("/system/dir"),
            false,
        );
        assert_eq!(resolved.source, DataDirSource::System);
        assert_eq!(resolved.path, PathBuf::from("/system/dir"));
    }

    #[test]
    fn missing_exe_dir_falls_back_to_system_dir() {
        let resolved = resolve_data_dir(None, None, Path::new("/system/dir"), true);
        assert_eq!(resolved.source, DataDirSource::System);
    }

    #[test]
    fn dir_writable_detects_writable_and_missing_paths() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(dir_writable(tmp.path()));
        // 新建的子目录也应可写
        assert!(dir_writable(&tmp.path().join("nested/deep")));
        // /proc 下不可写
        #[cfg(target_os = "linux")]
        assert!(!dir_writable(Path::new("/proc/self/nope")));
    }

    #[test]
    fn settings_roundtrip_in_explicit_dir() {
        let tmp = tempfile::tempdir().unwrap();
        // 无文件 -> 默认值
        assert_eq!(load_from(tmp.path()).unwrap(), LlmSettings::default());

        let settings = LlmSettings {
            base_url: "https://example.test/".into(),
            api_key: "sk-1".into(),
            model: " gpt-x ".into(),
            concurrency: 3,
            temperature: 0.55,
        };
        save_to(tmp.path(), &settings).unwrap();

        let loaded = load_from(tmp.path()).unwrap();
        assert_eq!(loaded.base_url, "https://example.test");
        assert_eq!(loaded.model, "gpt-x");
        assert_eq!(loaded.api_key, "sk-1");
        assert_eq!(loaded.concurrency, 3);
        assert_eq!(loaded.temperature, 0.55);
        assert!(settings_path_in(tmp.path()).exists());
    }

    #[test]
    fn corrupted_settings_file_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(settings_path_in(tmp.path()), b"{ not json").unwrap();
        assert_eq!(load_from(tmp.path()).unwrap(), LlmSettings::default());
    }

    #[test]
    fn legacy_settings_without_temperature_still_load() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(
            settings_path_in(tmp.path()),
            br#"{"baseUrl":"https://a.test","apiKey":"k","model":"m","concurrency":4,"batchSize":10}"#,
        )
        .unwrap();
        let loaded = load_from(tmp.path()).unwrap();
        assert_eq!(loaded.concurrency, 4);
        assert_eq!(loaded.temperature, 0.3);
    }

    #[test]
    fn write_atomic_replaces_existing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("out.json");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "two");
        // 不残留临时文件
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");
    }

    #[test]
    fn path_helpers_join_expected_names() {
        let dir = Path::new("/data");
        assert_eq!(settings_path_in(dir), PathBuf::from("/data/settings.json"));
        assert_eq!(glossary_path_in(dir), PathBuf::from("/data/glossary.json"));
        assert!(
            system_data_dir()
                .to_string_lossy()
                .contains("bg3-translate")
        );
    }
}
