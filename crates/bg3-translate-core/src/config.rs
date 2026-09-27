//! 配置与数据目录解析。
//!
//! 目录优先级（与 README 的「便携版」承诺一致）：
//! 1. 环境变量 `BG3_TRANSLATE_HOME`（便于测试与高级用户自定义，最高优先级）
//! 2. **便携模式**：可执行文件同级目录的 `config/`（能写就用它，解压即用、方便备份）
//! 3. **系统模式**：平台标准配置目录（`%APPDATA%\bg3-translate\config`、`~/.config/bg3-translate`）
//!
//! 第 3 步是必要的：安装到 `C:\Program Files` 时 exe 同级目录通常不可写。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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
        // 只写了空格的环境变量按「没设置」处理：否则会在当前工作目录下
        // 建一个名字是空格的目录，配置写进去后用户根本找不到。
        if !dir.as_os_str().is_empty() && !dir.to_string_lossy().trim().is_empty() {
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
    ensure_dir(resolved)
}

/// 确保数据目录真的可用；创建不了就退回系统临时目录。
///
/// 会走到这里的真实场景：`BG3_TRANSLATE_HOME` 指向的是一个**文件**而不是目录
/// （`create_dir_all` 返回 `AlreadyExists`）、目标目录只读、路径不存在且父目录
/// 不可写。此时宁可把配置放到临时目录，也不能让应用起不来。
fn ensure_dir(resolved: DataDir) -> Result<DataDir> {
    match std::fs::create_dir_all(&resolved.path) {
        Ok(()) => Ok(resolved),
        Err(err) => {
            let fallback = std::env::temp_dir().join(APP_NAME);
            std::fs::create_dir_all(&fallback)?;
            log::warn!(
                "无法创建数据目录 {}（{err}），回退到 {}",
                resolved.path.display(),
                fallback.display()
            );
            Ok(DataDir {
                path: fallback,
                source: DataDirSource::System,
            })
        }
    }
}

/// 设置文件绝对路径（不创建目录）。
pub fn settings_path_in(dir: &Path) -> PathBuf {
    dir.join("settings.json")
}

/// 术语表文件绝对路径（不创建目录）。
pub fn glossary_path_in(dir: &Path) -> PathBuf {
    dir.join("glossary.json")
}

/// 从指定目录读取设置；文件不存在、读不出来或损坏时返回默认值。
///
/// 三种「坏」的处置必须一致（**回退默认值 + 留备份**）：
/// 1. JSON 语法坏了；
/// 2. 文件不是合法 UTF-8（中文 Windows 编辑器另存为 GBK 就会这样）；
/// 3. 文件读不出来（权限、路径被目录占用……）。
///
/// 第 2、3 种以前直接 `?` 往上抛，用户在界面上没有任何自救手段；
/// 而三种都会把用户的 API Key 弄丢，所以损坏文件一律先[隔离备份](quarantine_corrupt_file)
/// 再回默认值 —— 不备份的话，用户重填一次就会把原文件静默覆盖掉。
pub fn load_from(dir: &Path) -> Result<LlmSettings> {
    let path = settings_path_in(dir);
    if !path.exists() {
        return Ok(LlmSettings::default());
    }
    let content = match std::fs::read_to_string(&path) {
        Ok(content) => content,
        Err(err) => {
            log::warn!(
                "设置文件无法读取（{}），已回退默认值: {err}",
                path.display()
            );
            quarantine_corrupt_file(&path);
            return Ok(LlmSettings::default());
        }
    };
    match serde_json::from_str::<LlmSettings>(&content) {
        Ok(settings) => Ok(settings.normalized()),
        Err(err) => {
            log::warn!("设置文件损坏（{}），已回退默认值: {err}", path.display());
            quarantine_corrupt_file(&path);
            Ok(LlmSettings::default())
        }
    }
}

/// 把损坏的配置文件挪到一边（**改名而不是删除**），返回备份路径。
///
/// 为什么必须留备份：损坏的文件里常常还留着用户的 API Key / 2 万条术语表，
/// 而调用方接下来就会用默认值重新写盘 —— 不备份等于静默丢数据。
/// 改名失败（权限、跨设备）只记日志：备份是尽力而为，不能因此让应用起不来。
pub(crate) fn quarantine_corrupt_file(path: &Path) -> Option<PathBuf> {
    let backup = quarantine_path(path);
    match std::fs::rename(path, &backup) {
        Ok(()) => {
            log::warn!(
                "损坏的配置文件已备份为 {}（原文件保留内容，可手工恢复）",
                backup.display()
            );
            Some(backup)
        }
        Err(err) => {
            log::warn!("备份损坏的配置文件失败（{}）: {err}", path.display());
            None
        }
    }
}

/// 备份路径：`settings.json` → `settings.json.corrupt`；
/// 已存在时依次尝试 `.corrupt.2` … `.corrupt.9`，绝不顶掉上一份备份。
fn quarantine_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let candidate = |suffix: &str| path.with_file_name(format!("{name}{suffix}"));
    let first = candidate(".corrupt");
    if !first.exists() {
        return first;
    }
    for index in 2..=9 {
        let next = candidate(&format!(".corrupt.{index}"));
        if !next.exists() {
            return next;
        }
    }
    // 10 份备份都存在（极端情况）：退回第一份的路径，让 rename 覆盖它，
    // 至少不会把文件留在原地被后续写盘静默覆盖。
    first
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
///
/// **所有失败路径都必须清掉临时文件**：它与目标同目录，写失败留下的半截文件会
/// 被下一步打包当成 MOD 内容（实测：磁盘写满时 `english.tmp<pid>.<n>` 残留，
/// repack 把 `Localization/English/*.tmp*` 一起打进产物 PAK）。
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = temp_path_for(path);
    // 用守卫而不是在每个 `?` 后面手写清理：写失败、rename 失败都要走同一条
    // 清理逻辑，漏掉一条分支就会留下半截文件（旧实现只在 rename 失败时清理）。
    let mut cleanup = TempFileCleanup {
        path: &tmp,
        handed_over: false,
    };
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).map_err(AppError::Io)?;
    // rename 成功：临时文件已经被移走，不需要再删
    cleanup.handed_over = true;
    Ok(())
}

/// 临时文件的清理守卫：没成功「交接」（rename）出去就删掉它。
struct TempFileCleanup<'a> {
    path: &'a Path,
    handed_over: bool,
}

impl Drop for TempFileCleanup<'_> {
    fn drop(&mut self) {
        if self.handed_over {
            return;
        }
        // 删不掉只记日志：这里已经在错误路径上，清理失败不能顶掉原始错误
        match std::fs::remove_file(self.path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                log::debug!("清理原子写临时文件失败（{}）: {err}", self.path.display());
            }
        }
    }
}

/// 进程内递增序号，保证每次原子写入都拿到**不同的**临时文件名。
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// 原子写临时文件扩展名的固定中缀。
///
/// **写入端（[`temp_path_for`]）与判定端（[`is_atomic_temp_name`]）共用这一处格式**：
/// 打包侧要靠判定端挡住陈旧的临时文件，两边各写一份规则的话，命名一改就会悄悄失效。
const TEMP_INFIX: &str = "tmp";

/// 组装临时文件扩展名：`tmp<pid>.<序号>`。
fn temp_extension(pid: u32, sequence: u64) -> String {
    format!("{TEMP_INFIX}{pid}.{sequence}")
}

/// 原子写的临时文件路径：与目标同目录（跨目录 `rename` 不是原子操作，还可能跨设备失败）、
/// 保留主名便于排查残留。
///
/// 为什么不能只用 `tmp<pid>`：同一进程里两次并发写会算出**同一个**名字 ——
/// 不只是同一个文件被写两次，`a.xml` / `a.loca` / `a` 这类同主名不同扩展名的
/// 目标也会撞在一起。撞名的后果有两个：两个写者互相截断（rename 之后对方还在
/// 往这个 inode 里写，原子性失效），以及先完成的一方把临时文件改名走之后，
/// 另一方 `rename` 直接 ENOENT 报错。
fn temp_path_for(path: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    path.with_extension(temp_extension(std::process::id(), sequence))
}

/// 文件名是否是原子写留下的临时文件（`<主名>.tmp<pid>.<序号>`）。
///
/// 给打包侧用：正常失败路径由 [`TempFileCleanup`] 清掉，但进程被杀 / 掉电时
/// `unpacked/` 里可能留着上一次的临时文件，它不该被当成 MOD 内容打进产物 PAK。
///
/// 判定只认**完整形状**（最后一段全是数字、`tmp` 前必须紧跟 `.`、`tmp` 后全是数字），
/// 免得把正常文件误判成临时文件而**静默丢掉用户的内容**：`english.xml`、`a.tmp`、
/// `notes.tmp123.txt` 都不是。
pub fn is_atomic_temp_name(name: &str) -> bool {
    let Some((rest, sequence)) = name.rsplit_once('.') else {
        return false;
    };
    if !is_ascii_digits(sequence) {
        return false;
    }
    let Some((stem, pid)) = rest.rsplit_once(TEMP_INFIX) else {
        return false;
    };
    is_ascii_digits(pid) && stem.ends_with('.')
}

/// 非空且全是 ASCII 数字。
fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
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

    /// 纯空白的环境变量不是「用户指定了目录」，而是手滑多打了空格。
    ///
    /// 旧行为会把它当合法路径：在当前工作目录下建一个名字是空格的目录，
    /// 配置与术语表写到那里，用户完全找不到。空串被忽略、空白串不被忽略，
    /// 这种不一致本身就是缺陷。
    #[test]
    fn whitespace_only_env_override_is_ignored() {
        let resolved = resolve_data_dir(
            Some(Path::new("   ")),
            Some(Path::new("/exe/dir")),
            Path::new("/system/dir"),
            true,
        );
        assert_eq!(resolved.source, DataDirSource::Portable);
        assert_eq!(resolved.path, PathBuf::from("/exe/dir/config"));
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

    /// `BG3_TRANSLATE_HOME` 写**相对路径**时原样接受，落点 = 进程当前目录。
    ///
    /// 这条是**有意保留**的行为（不是遗漏）：环境变量是给高级用户/脚本用的最高优先
    /// 覆盖入口，「相对路径就忽略」会偷偷改变配置落点，对已经在用相对路径的人更糟。
    /// 代价是落点取决于启动方式（Windows 快捷方式 / 安装器的 CWD 未必是 exe 目录），
    /// 所以 README 里明确写了「建议写绝对路径」。这条用例把这个语义钉住：
    /// 谁要改成「相对路径判否」，它会立刻变红。
    #[test]
    fn relative_env_override_is_used_verbatim() {
        let resolved = resolve_data_dir(
            Some(Path::new("relative/config")),
            Some(Path::new("/exe/dir")),
            Path::new("/system/dir"),
            true,
        );
        assert_eq!(resolved.source, DataDirSource::EnvOverride);
        assert_eq!(resolved.path, PathBuf::from("relative/config"));
        assert!(!resolved.path.is_absolute());
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

    /// `BG3_TRANSLATE_HOME` 指向**文件**而不是目录时：不回退就等于应用起不来，
    /// 必须落回临时目录，并且明确告诉 UI 来源已经变成「系统配置目录」。
    #[test]
    fn data_dir_that_is_actually_a_file_falls_back_to_temp() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("home-is-a-file");
        fs::write(&file, b"not a directory").unwrap();

        let resolved = DataDir {
            path: file.clone(),
            source: DataDirSource::EnvOverride,
        };
        let out = ensure_dir(resolved).expect("必须能回退，而不是把错误抛给调用方");

        assert_ne!(out.path, file, "不能把文件当成数据目录");
        assert_eq!(out.source, DataDirSource::System);
        assert!(out.path.is_dir(), "回退目录必须真实存在");
        assert!(out.path.to_string_lossy().contains(APP_NAME));
    }

    /// 正常目录原样返回，来源保持不变。
    #[test]
    fn ensure_dir_keeps_a_usable_directory_untouched() {
        let tmp = tempfile::tempdir().unwrap();
        let wanted = tmp.path().join("portable/config");
        let resolved = DataDir {
            path: wanted.clone(),
            source: DataDirSource::Portable,
        };
        let out = ensure_dir(resolved).expect("可创建目录不应回退");
        assert_eq!(out.path, wanted);
        assert_eq!(out.source, DataDirSource::Portable);
        assert!(wanted.is_dir());
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

    /// 损坏的设置文件必须**留一份备份**，而不是等下一次保存把它覆盖掉。
    ///
    /// 旧行为只是打一条 warn 日志然后回默认值：用户看到设置面板一片空白，
    /// 重新填一遍 API Key 并保存 —— 原来的 `settings.json`（可能只是手改坏了
    /// 一个引号）就被静默覆盖，用户再也拿不回自己的 key。
    #[test]
    fn corrupted_settings_file_is_kept_as_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = settings_path_in(tmp.path());
        let original =
            br#"{"baseUrl":"https://a.test","apiKey":"sk-secret","model":"m","concurrency":4}"#
                .as_slice();
        fs::write(&path, original).unwrap();
        // 只坏掉一个字符：JSON 里多了一个尾逗号
        fs::write(
            &path,
            br#"{"baseUrl":"https://a.test","apiKey":"sk-secret",}"#,
        )
        .unwrap();

        assert_eq!(load_from(tmp.path()).unwrap(), LlmSettings::default());

        let backup = tmp.path().join("settings.json.corrupt");
        assert!(
            backup.is_file(),
            "损坏的文件应被备份到 {}",
            backup.display()
        );
        assert_eq!(
            fs::read(&backup).unwrap(),
            br#"{"baseUrl":"https://a.test","apiKey":"sk-secret",}"#,
            "备份必须是原始字节，用户要能手工捞回 API Key"
        );
        assert!(!path.exists(), "损坏文件已被挪走（不是复制）");
    }

    /// 同一个目录里第二次出现损坏文件时，不能把上一份备份顶掉。
    #[test]
    fn quarantine_does_not_overwrite_an_existing_backup() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");

        fs::write(&path, b"first-corrupt").unwrap();
        assert_eq!(load_from(tmp.path()).unwrap(), LlmSettings::default());
        fs::write(&path, b"second-corrupt").unwrap();
        assert_eq!(load_from(tmp.path()).unwrap(), LlmSettings::default());

        assert_eq!(
            fs::read(tmp.path().join("settings.json.corrupt")).unwrap(),
            b"first-corrupt"
        );
        assert_eq!(
            fs::read(tmp.path().join("settings.json.corrupt.2")).unwrap(),
            b"second-corrupt"
        );
    }

    /// 非 UTF-8 的设置文件（中文 Windows 编辑器存成 GBK 就会这样）与
    /// 「JSON 语法坏了」是同一类损坏，必须一样能回退。
    ///
    /// 旧行为是 `read_to_string` 直接报 `Io` 错误往上抛：设置面板打不开、
    /// 翻译命令也起不来，而用户在界面上没有任何办法自救。
    #[test]
    fn non_utf8_settings_file_falls_back_to_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let path = settings_path_in(tmp.path());
        // GBK 编码的 `{"apiKey":"测试"}`（0xB2 0xE2 0xCA 0xD4 = "测试"），非合法 UTF-8
        fs::write(&path, b"{\"apiKey\":\"\xb2\xe2\xca\xd4\"}").unwrap();

        let loaded = load_from(tmp.path()).expect("非 UTF-8 不应让设置加载直接失败");
        assert_eq!(loaded, LlmSettings::default());
        assert!(
            tmp.path().join("settings.json.corrupt").is_file(),
            "原文件应被备份而不是丢掉"
        );
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

    /// 「连临时文件都建不出来」时的边界覆盖 —— **不是**能区分 R5-01 修复前后的回归。
    ///
    /// `/proc` 不支持新建文件（`dir_writable` 的既有用例也靠这一点），
    /// `fs::write` 在 `File::create` 这一步就返回 `NotFound`，**临时文件从未被创建**：
    /// 修复前的实现在这条路径上同样不会留下任何东西，所以 `leftovers.is_empty()`
    /// 恒真、用它做变异实验恒绿。它只钉住「这种失败不能把错误类型吞掉」。
    /// 真正能区分修复前后的是
    /// [`real_write_failure_after_the_temp_file_exists_leaves_no_residue`]
    /// （临时文件**已写出**、随后写失败）。
    #[test]
    #[cfg(target_os = "linux")]
    fn write_failure_leaves_no_temp_file() {
        let err = write_atomic(Path::new("/proc/self/residue-check.json"), b"payload").unwrap_err();
        assert_eq!(err.code(), "io", "写失败必须原样报 Io: {err}");

        let leftovers: Vec<String> = fs::read_dir("/proc/self")
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| is_atomic_temp_name(name))
            .collect();
        assert!(
            leftovers.is_empty(),
            "失败路径残留了临时文件: {leftovers:?}"
        );
    }

    /// 原子写失败后目录里不许残留临时文件（rename 失败分支）。
    ///
    /// 残留物与目标同目录，会被下一步打包当成 MOD 内容（实测 EFBIG 之后
    /// `english.tmp<pid>.<n>` 被 repack 打进 PAK）。这里用「目标是个目录」
    /// 让 rename 必然失败：临时文件那时**已经写出来了**，正是需要清理的形态。
    ///
    /// 注意它**不是**能区分 R5-01 修复前后的回归：修复前实现在 rename 失败分支里
    /// 本来就有 `remove_file`，所以这条在变异实验下恒绿。补它是为了钉住那条分支的
    /// 既有行为；真正区分前后的是
    /// [`real_write_failure_after_the_temp_file_exists_leaves_no_residue`]。
    #[test]
    fn failed_atomic_write_leaves_no_temp_file() {
        let tmp = tempfile::tempdir().unwrap();
        let target = tmp.path().join("english.xml");
        fs::create_dir(&target).unwrap();

        let err = write_atomic(&target, b"payload").unwrap_err();
        assert_eq!(err.code(), "io", "rename 失败必须原样报 Io: {err}");

        let leftovers: Vec<String> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .filter(|name| is_atomic_temp_name(name))
            .collect();
        assert!(
            leftovers.is_empty(),
            "失败路径必须清掉临时文件，实际残留: {leftovers:?}"
        );
        // 目标目录本身不该被动过
        assert!(target.is_dir());
    }

    /// R5-01 的真回归：临时文件**已经写出**、随后写失败（磁盘满 / RLIMIT_FSIZE）。
    ///
    /// 这是唯一能区分修复前后的形态。修复前 `fs::write(&tmp, bytes)?` 直接早退、
    /// **不清理**，半截 `<主名>.tmp<pid>.<n>` 留在与目标同目录的位置，下一步 repack
    /// 就会把它打进产物 PAK（验证者的 `pakjunk` 探针实测产物里出现
    /// `Localization/English/*.tmp63760.*`）。
    ///
    /// 为什么不用 `/proc` 或「目标是目录」：那两条一个在 `File::create` 就失败
    /// （临时文件从未创建）、一个走 rename 分支（修复前本来就清理），撤掉修复后
    /// 依然全绿 —— 等于没测。这里用**真实 EFBIG**（`ulimit -f`）触发：临时文件被
    /// `File::create` 真正建出来、`write_all` 写到上限时失败，形状与真实磁盘写满完全
    /// 一致；**修复前实现下它会因为残留而变红**。
    ///
    /// 为什么要 re-exec 自己：`ulimit -f` 只能设在**子进程**（设在父进程会污染其他
    /// 用例，而且默认的 SIGXFSZ 会直接杀掉进程），所以这里用 `sh` 起一个子进程、
    /// 只跑这一条用例（`--exact`）。`trap '' XFSZ` 让内核把超限变成 `write` 返回
    /// EFBIG 而不是投递致命信号；被忽略的信号处置会跨 `exec` 保留。
    #[test]
    #[cfg(target_os = "linux")]
    fn real_write_failure_after_the_temp_file_exists_leaves_no_residue() {
        const CHILD_ENV: &str = "BG3_TRANSLATE_A2_EFBIG_CHILD";
        const TEST_NAME: &str =
            "config::tests::real_write_failure_after_the_temp_file_exists_leaves_no_residue";
        /// 8 个 512 字节块 = 4 KiB（`ulimit -f` 的单位是 512 字节）。
        const CHILD_FILE_LIMIT_BLOCKS: usize = 8;

        // ── 子进程分支：在 RLIMIT_FSIZE 下真正跑一次 write_atomic ──
        if std::env::var_os(CHILD_ENV).is_some() {
            let dir = tempfile::tempdir().unwrap();
            let target = dir.path().join("english.xml");
            // 远大于 4 KiB：确保 write_all 写到一半才失败（临时文件已存在）
            let payload = vec![b'x'; 64 * 1024];

            let err = write_atomic(&target, &payload).expect_err("EFBIG 下写入必须失败");
            assert_eq!(err.code(), "io", "失败必须原样报 Io: {err}");
            assert!(!target.exists(), "失败时不该产出目标文件");

            // 修复前实现会在这里留下 `english.tmp<pid>.<n>`（残留本身就是
            // 「临时文件确实被创建过」的证据）；修复后目录必须是干净的。
            let leftovers: Vec<String> = fs::read_dir(dir.path())
                .unwrap()
                .filter_map(|entry| entry.ok())
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| is_atomic_temp_name(name))
                .collect();
            assert!(
                leftovers.is_empty(),
                "写失败后残留了临时文件: {leftovers:?}"
            );
            return;
        }

        // ── 父进程分支：设好限制后只重跑上面那条用例 ──
        let exe = std::env::current_exe().expect("能取到当前测试二进制路径");
        let script = format!(
            "trap '' XFSZ; ulimit -f {CHILD_FILE_LIMIT_BLOCKS}; exec '{}' --exact '{TEST_NAME}' --quiet",
            exe.display()
        );
        let output = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env(CHILD_ENV, "1")
            .output()
            .expect("能在受限子进程里重跑测试二进制");
        assert!(
            output.status.success(),
            "子进程（ulimit -f {CHILD_FILE_LIMIT_BLOCKS}）里的用例失败: status={:?}\nstdout={}\nstderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );
    }

    /// 清理守卫自身的契约：没「交接」出去的文件必须删掉，交接出去的不许动。
    ///
    /// 与上一条双保险：上一条盯「真实失败路径真的被清理」，这条盯「守卫实现本身
    /// 没写反」（例如 handed_over 判断反了会把刚 rename 好的目标删掉）。
    #[test]
    fn temp_file_cleanup_guard_deletes_only_unhanded_files() {
        let dir = tempfile::tempdir().unwrap();
        let tmp = dir.path().join("english.tmp1234.0");

        // 没交接：Drop 时删除
        fs::write(&tmp, b"partial").unwrap();
        {
            let _cleanup = TempFileCleanup {
                path: &tmp,
                handed_over: false,
            };
        }
        assert!(!tmp.exists(), "未交接的临时文件必须被 Drop 清掉");

        // 已交接（rename 已成功）：不许删，那已经是目标文件了
        fs::write(&tmp, b"done").unwrap();
        {
            let _cleanup = TempFileCleanup {
                path: &tmp,
                handed_over: true,
            };
        }
        assert!(tmp.exists(), "已交接的文件不能被 Drop 删掉");
        assert_eq!(fs::read(&tmp).unwrap(), b"done");
    }

    /// 判定函数只认**完整**的临时文件形状。
    ///
    /// 打包侧要用它挡陈旧临时文件，误判的代价是把用户真实内容静默丢掉，
    /// 所以三个反例（正常文件 / 半截名字 / 主名里带 tmp）必须判否。
    #[test]
    fn atomic_temp_name_predicate_only_accepts_the_real_shape() {
        for name in [
            "english.tmp63760.0",
            "a.tmp63760.1",
            "notes.tmp1.23",
            ".hidden.tmp9.0",
            "mytmp.tmp12.7",
        ] {
            assert!(is_atomic_temp_name(name), "{name} 应判为临时文件");
        }
        for name in [
            "english.xml",
            "a.tmp",
            "notes.tmp123.txt",
            "a.loca",
            "tmp63760.0",      // 没有主名
            "xtmp123.4",       // `tmp` 前没有 `.`
            "english.tmp.0",   // pid 缺失
            "english.tmp123.", // 序号缺失
            "english.xml.tmp123",
            "",
            "tmp",
            ".",
        ] {
            assert!(!is_atomic_temp_name(name), "{name} 不应判为临时文件");
        }
    }

    /// 判定函数与写入端共用同一份形状：写入端生成的每个名字都必须是判定的正例。
    ///
    /// 这条把两端钉在一起 —— 谁单方面改了命名格式（Round-4 的并发唯一性依赖它），
    /// 打包侧的拦截规则就会跟着失效，这里会立刻变红。
    #[test]
    fn generated_temp_names_are_recognized_by_the_predicate() {
        for target in [
            "/data/settings.json",
            "/data/a.loca",
            "/data/a",
            "/data/x.y.xml",
        ] {
            let temp = temp_path_for(Path::new(target));
            let name = temp.file_name().unwrap().to_string_lossy().into_owned();
            assert!(
                is_atomic_temp_name(&name),
                "写入端生成的名字必须能被判定函数认出来: {name}"
            );
        }
    }

    /// 临时文件名必须**每次调用都不同**。
    ///
    /// 旧实现是 `with_extension("tmp<pid>")`：同一进程里两次原子写算出同一个
    /// 临时文件，连 `a.xml` / `a.loca` / `a.lsx` / `a` 这类同主名不同扩展名的
    /// 目标都会撞名。撞名后两个写者互相截断（rename 之后一方还在往那个 inode
    /// 里写），先改名走的一方还会让另一方的 rename 直接 ENOENT。
    /// 这条用例不需要真的并发：名字不唯一就是缺陷本身。
    #[test]
    fn atomic_write_temp_path_is_unique_per_call() {
        let target = Path::new("/data/settings.json");
        let first = temp_path_for(target);
        let second = temp_path_for(target);
        assert_ne!(
            first, second,
            "同一次进程内的两次写入不能共用同一个临时文件"
        );
        // 不同扩展名、同主名的目标也不能撞
        assert_ne!(
            temp_path_for(Path::new("/data/a.xml")),
            temp_path_for(Path::new("/data/a.loca"))
        );
        // 必须与目标同目录（rename 才可能是原子的），且不是目标本身
        assert_eq!(first.parent(), target.parent());
        assert_ne!(first, target);
        assert!(
            first.to_string_lossy().contains("settings"),
            "保留主名便于排查残留: {}",
            first.display()
        );
    }

    /// 并发原子写同一个文件：两个写者都必须成功，最终内容必须是其中一份完整数据。
    ///
    /// 撞名的临时文件会让其中一个写者 rename 失败（或写出撕裂内容），
    /// 所以这条用例锁住的是「原子写在自己的并发下也成立」。
    #[test]
    fn concurrent_atomic_writes_all_succeed_with_whole_content() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        let payloads: Vec<Vec<u8>> = (0..8)
            .map(|index| format!("payload-{index}-{}", "x".repeat(4096)).into_bytes())
            .collect();

        std::thread::scope(|scope| {
            for payload in &payloads {
                let path = path.clone();
                scope.spawn(move || {
                    write_atomic(&path, payload).expect("并发原子写不应失败");
                });
            }
        });

        let written = fs::read(&path).unwrap();
        assert!(
            payloads.contains(&written),
            "最终内容必须是某一份完整数据（不能是两次写入的混合）"
        );
        let leftovers: Vec<_> = fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "不应残留临时文件: {leftovers:?}");
    }

    /// 目标文件**只读**时的跨平台差异（已知差异，刻意不统一）。
    ///
    /// - Linux：`rename` 只需要**目录**写权限，覆盖成功，新文件带的是临时文件的权限，
    ///   原文件的只读位因此丢失（下面断言的就是这个当前行为）；
    /// - Windows：`MoveFileEx` 到只读目标返回 ACCESS_DENIED，保存会报错。
    ///
    /// 没有统一它是因为两个方向都要付出代价：让 Linux 也失败 = 用户明明能保存却被拦；
    /// 让 Windows 也成功 = 要额外清只读位（等于替用户改文件属性）。
    /// 影响面仅限「有人手工把 settings.json 设成只读」，所以记录差异而不是改行为。
    /// **Windows 分支本机跑不了**（本机是 Linux），只按代码路径写出来。
    #[test]
    fn write_atomic_over_read_only_target_differs_by_platform() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("settings.json");
        fs::write(&path, b"old").unwrap();
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_readonly(true);
        fs::set_permissions(&path, perms).unwrap();

        let result = write_atomic(&path, b"new");
        if cfg!(windows) {
            assert!(
                result.is_err(),
                "Windows 上 rename 到只读目标应失败: {result:?}"
            );
        } else {
            assert!(result.is_ok(), "Linux 上 rename 只看目录权限: {result:?}");
            assert_eq!(fs::read(&path).unwrap(), b"new");
            assert!(
                !fs::metadata(&path).unwrap().permissions().readonly(),
                "Linux 会丢掉原文件的只读位（已知跨平台差异）"
            );
        }
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

    /// 系统配置目录的**实际形状**（README / ARCHITECTURE / 本文件头部注释都按它写）。
    ///
    /// Linux：`~/.config/bg3-translate`（或 `$XDG_CONFIG_HOME/bg3-translate`）；
    /// Windows：`%APPDATA%\bg3-translate\config` —— `directories` 在 Windows 上
    /// 会**多拼一层 `config`**（v6 的 `ProjectDirs::config_dir` 文档与 `src/win.rs`
    /// 都这么写），所以文档里只写 `%APPDATA%\bg3-translate` 是错的：
    /// 用户按那个路径去找 `settings.json` 会找不到。
    /// 这条断言把两个平台的实际形状都钉住，避免文档再次腐烂。
    #[test]
    fn system_data_dir_shape_matches_documented_paths() {
        let dir = system_data_dir();
        assert!(
            dir.is_absolute(),
            "系统配置目录必须是绝对路径: {}",
            dir.display()
        );
        let name = dir.file_name().and_then(|name| name.to_str());
        if cfg!(windows) {
            assert_eq!(name, Some("config"), "Windows: {}", dir.display());
            assert_eq!(
                dir.parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str()),
                Some(APP_NAME)
            );
        } else {
            assert_eq!(name, Some(APP_NAME), "Unix: {}", dir.display());
        }
    }
}
