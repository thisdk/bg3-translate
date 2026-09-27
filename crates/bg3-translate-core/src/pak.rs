//! PAK / ZIP 归档的解包与重打包。
//!
//! 职责边界：
//! - 从 `.pak` 或 Nexus 风格的 `.zip`（内含 `.pak`）解出文件树
//! - 识别每个 PAK 条目的类型与语言，产出前端要的文件列表
//! - 把工作目录重新打包成 BG3 可加载的 `.pak`

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bg3rustpaklib::loca::detect_language_from_path;
use bg3rustpaklib::{Package, PackageBuilder, PackagedFile, get_package_priority};

use crate::error::{AppError, Result};
use crate::types::{PakFile, PakFileKind};

/// 解包后文件树所在子目录名。
pub const UNPACKED_DIR: &str = "unpacked";
/// 工作目录内记录 PAK 元信息的文件名。
pub const META_FILE: &str = "pak_meta.json";

// ─────────────────────────────────────────────────────────────
// 类型识别
// ─────────────────────────────────────────────────────────────

/// 由 PAK 内路径识别文件类型。判定顺序按「可翻译性」优先级排列。
pub fn classify_file(name: &str) -> PakFileKind {
    let lower = name.to_lowercase();
    if lower.ends_with(".xml") && lower.contains("localization") {
        PakFileKind::LocalizationXml
    } else if lower.ends_with(".loca") {
        PakFileKind::LocalizationLoca
    } else if lower.ends_with(".lsx") {
        PakFileKind::MetadataLsx
    } else if lower.ends_with(".lua") {
        PakFileKind::ScriptLua
    } else if lower.ends_with(".txt") {
        PakFileKind::DataTxt
    } else {
        PakFileKind::Other
    }
}

/// PAK 内路径统一用 `/` 分隔（Windows 打包的 PAK 可能是 `\`）。
pub fn normalize_entry_name(name: &str) -> String {
    name.replace('\\', "/")
}

/// 本工具写回中文时使用的目录名（与前端 `src/lib/localization.ts` 的
/// `TARGET_LANGUAGE` 一致）。
const CHINESE_LANGUAGE_DIR: &str = "Chinese";

/// PAK 内路径 → 已知语言名。
///
/// 先走上游的 `detect_language_from_path`（BG3 官方的 14 个语言名），再补一条
/// **本工具自己的约定**：`Localization/Chinese/<file>`。
///
/// 为什么必须补：上游那份清单里只有 `ChineseSimplified` / `ChineseTraditional`，
/// **没有 `Chinese`**；而本工具写回中文用的正是 `Localization/Chinese/`
/// （前端 `TARGET_LANGUAGE = "Chinese"`）。不补的话，上一轮产出的中文文件在下一轮
/// 会被算成「未知语言」，前端 `localizationWritePriority` 里
/// `CHINESE_LANGUAGE_ALIASES` 的 `"Chinese"` 分支永远是死代码，
/// 同一目标路径上英文（优先级 3）会压过中文（本应是 2）。
fn detect_language(name: &str) -> Option<String> {
    if let Some(language) = detect_language_from_path(name) {
        return Some(language.to_string());
    }
    let normalized = normalize_entry_name(name);
    let parts: Vec<&str> = normalized.split('/').collect();
    parts.windows(2).find_map(|window| {
        let is_localization = window[0].eq_ignore_ascii_case("Localization");
        let is_chinese = window[1].eq_ignore_ascii_case(CHINESE_LANGUAGE_DIR);
        (is_localization && is_chinese).then(|| CHINESE_LANGUAGE_DIR.to_string())
    })
}

fn to_pak_file(file: &PackagedFile) -> PakFile {
    let name = normalize_entry_name(file.name());
    PakFile {
        kind: classify_file(&name),
        language: detect_language(&name),
        name,
        size: file.size(),
    }
}

/// 构造传给前端的文件列表（按路径排序，顺序稳定）。
pub fn build_pak_files(pkg: &Package) -> Vec<PakFile> {
    let mut out: Vec<PakFile> = pkg.files().iter().map(to_pak_file).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

// ─────────────────────────────────────────────────────────────
// 工作目录
// ─────────────────────────────────────────────────────────────

static WORK_DIR_SEQ: AtomicU64 = AtomicU64::new(0);

/// 在 `base` 下创建一个唯一的工作目录。
///
/// 名称包含时间戳 + 进程内自增序号，避免同一毫秒内多次打开互相覆盖。
pub fn create_work_dir_in(base: &Path) -> Result<PathBuf> {
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let seq = WORK_DIR_SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = base.join(format!("bg3-translate-{ts}-{seq}"));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 在系统临时目录下创建工作目录。
pub fn create_work_dir() -> Result<PathBuf> {
    create_work_dir_in(&std::env::temp_dir())
}

/// 删除工作目录（打开新 MOD / 退出时调用，避免临时文件无限堆积）。
///
/// 删除失败只记日志，不向上报错——用户不该因为清理失败而被打断。
pub fn remove_work_dir(work_dir: &Path) {
    if !work_dir.exists() {
        return;
    }
    if let Err(err) = std::fs::remove_dir_all(work_dir) {
        log::warn!("清理工作目录失败 {}: {err}", work_dir.display());
    }
}

/// 解包后的文件树根目录。
pub fn unpacked_dir(work_dir: impl AsRef<Path>) -> PathBuf {
    work_dir.as_ref().join(UNPACKED_DIR)
}

/// 给定 PAK 内文件名，得到解包后磁盘上的绝对路径。
pub fn resolve_disk_path(work_dir: impl AsRef<Path>, file_name: &str) -> PathBuf {
    unpacked_dir(work_dir).join(normalize_entry_name(file_name))
}

/// Windows 保留设备名：写这些名字会失败或者变成设备，而不是普通文件。
#[cfg_attr(not(windows), allow(dead_code))]
const WINDOWS_RESERVED_NAMES: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// 某个路径片段是否是 Windows 上不能当文件名用的名字。
fn is_reserved_windows_component(part: &str) -> bool {
    // `NUL.txt` 同样会被解析成设备名，所以只比较第一个 `.` 之前的部分
    let stem = part.split('.').next().unwrap_or(part);
    WINDOWS_RESERVED_NAMES
        .iter()
        .any(|reserved| reserved.eq_ignore_ascii_case(stem))
}

/// 把 PAK 内路径安全地拼到根目录下。
///
/// 这是防 zip-slip / pak-slip 的关键函数，拒绝：
/// - `..`（目录穿越）与绝对路径 / 盘符
/// - Windows 保留设备名（`CON`、`NUL`、`COM1`……）
/// - 含 `:` 的片段（Windows 上 `a:b` 是 NTFS 数据流写法，会写到文件之外）
/// - **尾随的点或空格**：Windows 路径规整会去掉片段结尾的 `.` 与空格，
///   于是 `".. "` 变成 `".."`（经典 zip-slip 绕过）。这类名字在 Windows 上
///   本来就创建不出来，合法 PAK 里不可能有。
/// - 含 NUL 的片段：文件系统层必然失败，早点给出明确错误
pub fn safe_output_path(root: &Path, file_name: &str) -> Result<PathBuf> {
    let mut output = root.to_path_buf();
    for component in Path::new(&normalize_entry_name(file_name)).components() {
        match component {
            Component::Normal(part) => {
                let part = part.to_string_lossy();
                if part.contains(':') {
                    return Err(AppError::pak(format!(
                        "非法归档路径（含冒号）: {file_name}"
                    )));
                }
                if part.contains('\0') {
                    return Err(AppError::pak(format!(
                        "非法归档路径（含 NUL 字节）: {file_name}"
                    )));
                }
                if is_reserved_windows_component(&part) {
                    return Err(AppError::pak(format!(
                        "非法归档路径（Windows 保留设备名）: {file_name}"
                    )));
                }
                if part.ends_with(['.', ' ']) {
                    return Err(AppError::pak(format!(
                        "非法归档路径（片段以点或空格结尾，Windows 上会被规整掉）: {file_name}"
                    )));
                }
                output.push(part.as_ref());
            }
            Component::CurDir => {}
            Component::Prefix(_) | Component::RootDir | Component::ParentDir => {
                return Err(AppError::pak(format!("非法归档路径: {file_name}")));
            }
        }
    }
    Ok(output)
}

/// 递归遍历目录下所有文件。
///
/// **不跟随符号链接**：跟随的话，链接指向的外部文件会被当成目录树里的文件
/// （`pick_largest_pak` 就可能选中解压目录之外的 `.pak`），指向祖先目录的
/// 链接更会让遍历永不结束。用 [`std::fs::DirEntry::file_type`]（不 follow）
/// 判断类型即可。
pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                log::debug!("跳过符号链接: {}", entry.path().display());
                continue;
            }
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// 深度优先找目录树里第一个符号链接（不跟随链接本身）。
fn find_symlink(dir: &Path) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let path = entry.path();
            if file_type.is_symlink() {
                return Some(path);
            }
            if file_type.is_dir() {
                stack.push(path);
            }
        }
    }
    None
}

// ─────────────────────────────────────────────────────────────
// 打开 / 解包
// ─────────────────────────────────────────────────────────────

/// 打开 MOD 文件并解包到临时工作目录。
///
/// 返回 `(工作目录, 文件列表)`；调用方负责在合适的时机 `remove_work_dir`。
pub fn open_and_extract(file_path: &str) -> Result<(PathBuf, Vec<PakFile>)> {
    let base = create_work_dir()?;
    match open_and_extract_in(file_path, &base) {
        Ok(result) => Ok(result),
        Err(err) => {
            // 失败时不要留下半个工作目录
            remove_work_dir(&base);
            Err(err)
        }
    }
}

/// 在指定根目录下解包（便于测试与自定义存放位置）。
pub fn open_and_extract_in(file_path: &str, work_root: &Path) -> Result<(PathBuf, Vec<PakFile>)> {
    let source = Path::new(file_path);
    if !source.is_file() {
        return Err(AppError::config(format!("文件不存在: {file_path}")));
    }

    let work_dir = if is_work_dir(work_root) {
        // 允许直接传入已创建好的工作目录
        work_root.to_path_buf()
    } else {
        create_work_dir_in(work_root)?
    };
    std::fs::create_dir_all(&work_dir)?;

    let pak_path = extract_pak_from(source, &work_dir)?;
    let pkg = Package::open(&pak_path).map_err(|e| AppError::pak(format!("打开 PAK 失败: {e}")))?;

    let extract_dir = unpacked_dir(&work_dir);
    std::fs::create_dir_all(&extract_dir)?;
    let files = extract_package_files(&pkg, &extract_dir)?;

    // 记录 priority 供重打包使用
    let priority = get_package_priority(&pak_path).unwrap_or(0);
    let meta = serde_json::json!({
        "priority": priority,
        "source": file_path,
        "pak": pak_path.file_name().map(|n| n.to_string_lossy().into_owned()),
    });
    std::fs::write(work_dir.join(META_FILE), serde_json::to_vec_pretty(&meta)?)?;

    Ok((work_dir, files))
}

/// 只解包到用户指定目录，不创建工作流。
pub fn extract_to_directory(file_path: &str, output_dir: &str) -> Result<Vec<PakFile>> {
    let source = Path::new(file_path);
    if !source.is_file() {
        return Err(AppError::config(format!("文件不存在: {file_path}")));
    }
    let output = Path::new(output_dir);
    std::fs::create_dir_all(output)?;

    let scratch = create_work_dir()?;
    let result = (|| {
        let pak_path = extract_pak_from(source, &scratch)?;
        let pkg =
            Package::open(&pak_path).map_err(|e| AppError::pak(format!("打开 PAK 失败: {e}")))?;
        extract_package_files(&pkg, output)
    })();
    remove_work_dir(&scratch);
    result
}

/// 工作目录是否已经由本模块创建（避免把用户目录当成临时目录乱建子目录）。
fn is_work_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.starts_with("bg3-translate-"))
}

/// 若入参是 `.zip` 则先解压并找出其中的 `.pak`，否则原样返回。
fn extract_pak_from(file_path: &Path, work_dir: &Path) -> Result<PathBuf> {
    let is_zip = file_path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("zip"));
    if !is_zip {
        return Ok(file_path.to_path_buf());
    }
    extract_zip_to_find_pak(file_path, work_dir)
}

/// 解压 zip 并返回其中体积最大的 `.pak`（Nexus 包里通常还有 README 等杂物）。
fn extract_zip_to_find_pak(zip_path: &Path, work_dir: &Path) -> Result<PathBuf> {
    extract_zip_to_find_pak_limited(zip_path, work_dir, ZipLimits::default())
}

/// zip 解压的防护上限。
///
/// 取值理由：BG3 的 4K 材质 MOD 解包后可能有好几 GB（合法），所以上限必须宽松；
/// 但「65 KB → 64 MiB」（≈1000× 膨胀）这种压缩比炸弹必须在**解压过程中**就被
/// 拦住——`open_and_extract` 的清理跑在解压之后，磁盘已经写满了就来不及了。
#[derive(Debug, Clone, Copy)]
struct ZipLimits {
    /// 单个条目声明的解压后大小上限
    max_entry_bytes: u64,
    /// 所有条目解压后的总字节上限
    max_total_bytes: u64,
    /// 条目数上限（防海量小文件耗尽 inode 与时间）
    max_entries: usize,
}

impl Default for ZipLimits {
    fn default() -> Self {
        Self {
            // 6 GiB：再大的单个文件不可能是 MOD 内容，但留足真实 MOD 的余量
            max_entry_bytes: 6 * 1024 * 1024 * 1024,
            // 16 GiB：几 GB 的 4K 材质 MOD 合法
            max_total_bytes: 16 * 1024 * 1024 * 1024,
            // 20 万条目：真实 MOD 通常几百到几千个文件
            max_entries: 200_000,
        }
    }
}

/// [`extract_zip_to_find_pak`] 的实现（上限可注入，便于用小样本测炸弹防线）。
fn extract_zip_to_find_pak_limited(
    zip_path: &Path,
    work_dir: &Path,
    limits: ZipLimits,
) -> Result<PathBuf> {
    let zip_extract = work_dir.join("zip_contents");
    std::fs::create_dir_all(&zip_extract)?;

    let file = std::fs::File::open(zip_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| AppError::pak(format!("读取 zip 失败: {e}")))?;

    if archive.len() > limits.max_entries {
        return Err(AppError::pak(format!(
            "zip 条目数 {} 超过上限 {}，已中止解压（疑似恶意压缩包，真实 MOD 不会有这么多文件）",
            archive.len(),
            limits.max_entries
        )));
    }

    let mut written_total: u64 = 0;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::pak(format!("读取 zip 条目失败: {e}")))?;

        // ① 先看声明大小：压缩炸弹的声明通常就是几 GB，连文件都不用创建
        let declared = entry.size();
        if declared > limits.max_entry_bytes {
            return Err(AppError::pak(format!(
                "zip 条目 {} 声明解压后 {} 字节，超过单文件上限 {}，已中止解压",
                entry.name(),
                declared,
                limits.max_entry_bytes
            )));
        }
        if written_total.saturating_add(declared) > limits.max_total_bytes {
            return Err(AppError::pak(format!(
                "zip 解压总量将超过上限 {} 字节（已解压 {} 字节），已中止解压",
                limits.max_total_bytes, written_total
            )));
        }

        // enclosed_name() 已经过滤了 `..` 与绝对路径
        let Some(relative) = entry.enclosed_name() else {
            log::warn!("跳过 zip 中的危险路径: {}", entry.name());
            continue;
        };
        let output_path = zip_extract.join(relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&output_path)?;
            continue;
        }
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut output = std::fs::File::create(&output_path)?;

        // ② 再按**实际写入**字节数兜底：声明大小是可以撒谎的。
        // 预算取「剩余总量」与「单文件上限」的较小值 —— 只看总量的话，
        // 一个声明 1 字节的条目能把整份总量预算（默认 16 GiB）写进同一个文件，
        // 单文件上限（默认 6 GiB）形同虚设。
        let total_budget = limits.max_total_bytes.saturating_sub(written_total);
        let entry_budget = limits.max_entry_bytes;
        let budget = total_budget.min(entry_budget);
        let mut limited = std::io::Read::take(&mut entry, budget.saturating_add(1));
        let copied = std::io::copy(&mut limited, &mut output)?;
        drop(output);
        if copied > entry_budget {
            // 及时收尾：失败时不要在工作目录里留下一个已经写了几 GB 的文件
            let _ = std::fs::remove_file(&output_path);
            return Err(AppError::pak(format!(
                "zip 条目 {} 实际解压超过单文件上限 {} 字节，已中止解压（声明大小与内容不符）",
                entry.name(),
                limits.max_entry_bytes
            )));
        }
        if copied > total_budget {
            // 及时收尾：失败时不要在工作目录里留下一个已经写了几 GB 的文件
            let _ = std::fs::remove_file(&output_path);
            return Err(AppError::pak(format!(
                "zip 条目 {} 解压超过总量上限 {} 字节，已中止解压（疑似压缩炸弹）",
                entry.name(),
                limits.max_total_bytes
            )));
        }
        written_total += copied;
    }

    pick_largest_pak(&zip_extract)
        .ok_or_else(|| AppError::pak("zip 内未找到 .pak 文件，请确认这是 BG3 MOD。"))
}

/// 在目录树里选出体积最大的 `.pak`（读不到大小则退回字典序第一个）。
pub fn pick_largest_pak(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(u64, PathBuf)> = None;
    for path in walk_files(dir) {
        let is_pak = path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| e.eq_ignore_ascii_case("pak"));
        if !is_pak {
            continue;
        }
        let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let replace = match &best {
            None => true,
            // 体积优先；同体积时取路径字典序更小的，保证结果确定
            Some((best_size, best_path)) => {
                size > *best_size || (size == *best_size && path < *best_path)
            }
        };
        if replace {
            best = Some((size, path));
        }
    }
    best.map(|(_, path)| path)
}

/// 把 PAK 内所有条目写到磁盘，返回文件列表。
///
/// 同一路径可能有多个版本（PAK 允许重复条目）；优先使用**最后一个可读**的版本，
/// 与游戏加载器行为一致。
fn extract_package_files(pkg: &Package, extract_dir: &Path) -> Result<Vec<PakFile>> {
    extract_package_files_limited(pkg, extract_dir, ExtractLimits::default())
}

/// PAK 解包的防护上限（与 [`ZipLimits`] **对称**：`.pak` 同样是用户下载的不可信输入）。
///
/// 为什么必须有：LZ4 的最大压缩比约 255×，一块 257 KB 的全零条目能解出 64 MiB
/// （实测 43 ms，见 `pak_expansion_is_capped`）。zip 那条路早就有上限，
/// 而 PAK 这条路以前**一个上限都没有** —— 一个百来 MB 的恶意 PAK 就能写出几十 GB、
/// 同时把每个条目的解压结果整份读进内存。
///
/// 取值理由与 [`ZipLimits`] 一致：真实 MOD 的单个 4K 材质文件可达数 GB，
/// 上限必须宽松到不误伤，但「几百 KB → 几十 GB」这种放大一定要在**创建文件之前**
/// 被拦住。
#[derive(Debug, Clone, Copy)]
struct ExtractLimits {
    /// 单个条目解压后的字节上限
    max_entry_bytes: u64,
    /// 所有条目解压后的总字节上限
    max_total_bytes: u64,
    /// 条目数上限（防海量小文件耗尽 inode 与时间）
    max_entries: usize,
}

impl Default for ExtractLimits {
    fn default() -> Self {
        Self {
            // 6 GiB / 16 GiB / 20 万：与 `ZipLimits::default()` 同一组数字
            max_entry_bytes: 6 * 1024 * 1024 * 1024,
            max_total_bytes: 16 * 1024 * 1024 * 1024,
            max_entries: 200_000,
        }
    }
}

/// [`extract_package_files`] 的实现（上限可注入，便于用小样本测炸弹防线）。
fn extract_package_files_limited(
    pkg: &Package,
    extract_dir: &Path,
    limits: ExtractLimits,
) -> Result<Vec<PakFile>> {
    // ① 先按**声明大小**预检：压缩炸弹在创建任何文件之前就该被拦住
    // （`.pak` 的文件表就在归档里，不需要解压任何数据就能读到这些数字）。
    let all_files = pkg.files();
    if all_files.len() > limits.max_entries {
        return Err(AppError::pak(format!(
            "PAK 条目数 {} 超过上限 {}，已中止解包（疑似恶意归档，真实 MOD 不会有这么多文件）",
            all_files.len(),
            limits.max_entries
        )));
    }
    let mut declared_total: u64 = 0;
    for file in all_files {
        let declared = file.size();
        if declared > limits.max_entry_bytes {
            return Err(AppError::pak(format!(
                "PAK 条目 {} 声明解压后 {} 字节，超过单文件上限 {}，已中止解包",
                file.name(),
                declared,
                limits.max_entry_bytes
            )));
        }
        declared_total = declared_total.saturating_add(declared);
        if declared_total > limits.max_total_bytes {
            return Err(AppError::pak(format!(
                "PAK 解压总量将超过上限 {} 字节（已声明 {declared_total} 字节），已中止解包（疑似压缩炸弹）",
                limits.max_total_bytes
            )));
        }
    }

    let mut groups: BTreeMap<String, Vec<&PackagedFile>> = BTreeMap::new();
    for file in all_files {
        groups
            .entry(normalize_entry_name(file.name()))
            .or_default()
            .push(file);
    }

    let mut files = Vec::with_capacity(groups.len());
    let mut written_total: u64 = 0;
    for (name, candidates) in groups {
        // 归档里的「目录条目」：名字以 `/` 结尾，本身不携带内容（LSPK 的文件表
        // 是扁平的，少数打包器会额外写这种条目）。必须特判，否则它会被当成普通
        // 文件写成 `unpacked/Localization`，随后同目录下的
        // `Localization/English/x.xml` 在 `create_dir_all` 处直接失败
        // （`Not a directory`）—— 整个 MOD 一个文件都解不出来，用户只看到一句
        // 与条目无关的 IO 错误。空名字的条目同样没有落盘意义，一并跳过。
        if name.is_empty() || name.ends_with('/') {
            if !name.is_empty() {
                std::fs::create_dir_all(safe_output_path(extract_dir, &name)?)?;
            }
            log::debug!("跳过归档里的目录条目: {name:?}");
            continue;
        }

        let mut selected: Option<(&PackagedFile, Vec<u8>)> = None;
        let mut errors = Vec::new();

        for file in candidates {
            match pkg.read_file(file) {
                Ok(data) => selected = Some((file, data)),
                Err(err) => errors.push(err.to_string()),
            }
        }

        let Some((file, data)) = selected else {
            return Err(AppError::pak(format!(
                "解包失败 {name}: {}",
                errors.join("; ")
            )));
        };
        if !errors.is_empty() {
            log::warn!(
                "PAK 条目 {name} 有 {} 个不可读副本，已使用可读版本",
                errors.len()
            );
        }

        // ② 再按**实际解压**字节数兜底：声明大小是可以撒谎的。
        // 检查放在写盘**之前**：超限的条目一个字节都不该落到磁盘上。
        let written = data.len() as u64;
        if written > limits.max_entry_bytes {
            return Err(AppError::pak(format!(
                "PAK 条目 {name} 实际解压 {} 字节，超过单文件上限 {}，已中止解包（声明大小与内容不符）",
                written, limits.max_entry_bytes
            )));
        }
        written_total = written_total.saturating_add(written);
        if written_total > limits.max_total_bytes {
            return Err(AppError::pak(format!(
                "PAK 解压总量超过上限 {} 字节，已中止解包（疑似压缩炸弹）",
                limits.max_total_bytes
            )));
        }

        let output_path = safe_output_path(extract_dir, &name)?;
        if let Some(parent) = output_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&output_path, &data)?;
        files.push(to_pak_file(file));
    }

    files.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(files)
}

// ─────────────────────────────────────────────────────────────
// 重打包
// ─────────────────────────────────────────────────────────────

/// 读取工作目录记录的原 PAK priority。
pub fn read_priority(work_dir: &Path) -> u8 {
    let raw = std::fs::read_to_string(work_dir.join(META_FILE)).ok();
    raw.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("priority").and_then(|p| p.as_u64()))
        .map(|p| p.min(u8::MAX as u64) as u8)
        .unwrap_or(0)
}

/// 把工作目录的 `unpacked/` 重新打包为 `.pak`。
pub fn repack(work_dir: &str, output_path: &str) -> Result<()> {
    let work_dir = Path::new(work_dir);
    let unpacked = unpacked_dir(work_dir);
    if !unpacked.is_dir() {
        return Err(AppError::config(format!(
            "工作目录无效（缺少 {UNPACKED_DIR} 子目录）: {}",
            work_dir.display()
        )));
    }

    // 底层打包库的 `add_directory` 会跟随符号链接，把链接指向的**工作目录之外**
    // 的文件打进 PAK。解包本身不会产生符号链接，所以这里出现链接只可能来自
    // 本机其它程序——宁可报错也不要把工作目录外的文件写进产物。
    if let Some(link) = find_symlink(&unpacked) {
        return Err(AppError::pak(format!(
            "工作目录里存在符号链接，拒绝打包（它会把工作目录外的文件打进 PAK）: {}",
            link.display()
        )));
    }

    let output = Path::new(output_path);
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let priority = read_priority(work_dir);
    log::info!(
        "开始打包 {} -> {}（priority={priority}）",
        unpacked.display(),
        output.display()
    );

    // 自己走一遍目录（而不是 `add_directory`）只为了能**排除**原子写残留的
    // 临时文件：`add_directory` 只能整目录添加，没法跳过单个文件。
    let mut builder = PackageBuilder::new().priority(priority);
    for (path, archive_path) in collect_pack_files(&unpacked)? {
        builder = builder.add_file(path, archive_path);
    }

    // 先打到**同目录**下的临时文件，成功后再 rename 覆盖目标。
    //
    // `PackageBuilder::build` 的第一步就是 `File::create(output)`（截断），之后才逐个
    // 打开源文件：任何一步失败（源文件读不出来 / 磁盘写满）都会把用户原来放在
    // `output_path` 上的产物清成 0 字节或半截 pak，而「导出覆盖上一次的结果」
    // 是最常见的用法。rename 在同一目录内是原子的；临时文件与目标同目录，
    // 所以不会跨文件系统（跨文件系统 rename 会退化成失败）。
    let tmp = repack_temp_path(output);
    let _ = std::fs::remove_file(&tmp);
    if let Err(err) = builder.build(&tmp) {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::pak(format!("打包失败: {err}")));
    }
    if let Err(err) = std::fs::rename(&tmp, output) {
        let _ = std::fs::remove_file(&tmp);
        return Err(AppError::pak(format!(
            "写入 {} 失败: {err}",
            output.display()
        )));
    }

    Ok(())
}

/// 递归收集待打包的文件：`(磁盘上的路径, PAK 内路径)`。
///
/// # 为什么要自己走目录
///
/// 上游的 `PackageBuilder::add_directory` 只能整目录添加，**没法排除单个文件**。
/// 而 `unpacked/` 里可能残留原子写的临时文件（`english.tmp63760.0`）：
/// `write_atomic` 的失败清理只覆盖「同一次调用内失败」，进程被强杀 / 掉电留下的
/// 那些不会被清掉，直接打进产物就是一条被截断的本地化文件混进了用户的 PAK
/// （独立验证者 R5-01 实测：产物多出两个 `Localization/English/*.tmp*` 垃圾条目）。
///
/// 判定用写入端同一个 [`crate::config::is_atomic_temp_name`]，规则只有一份。
///
/// # 语义与 `add_directory` 对齐
///
/// - 读目录 / 读条目失败**必须报错**：静默跳过等于产物里少文件（静默损坏）；
/// - 符号链接一律拒绝（`repack` 开头已经查过一遍，这里再兜一次）；
/// - 只收普通文件：目录递归下去，FIFO / 设备文件之类跳过（`add_directory` 同样不收）；
/// - 结果按 PAK 内路径排序，同一份工作目录每次打出同样的条目顺序。
fn collect_pack_files(root: &Path) -> Result<Vec<(PathBuf, String)>> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = std::fs::read_dir(&dir)
            .map_err(|err| AppError::pak(format!("读取待打包目录失败 {}: {err}", dir.display())))?;
        for entry in entries {
            let entry = entry.map_err(|err| {
                AppError::pak(format!("读取待打包目录项失败 {}: {err}", dir.display()))
            })?;
            let file_type = entry.file_type().map_err(|err| {
                AppError::pak(format!(
                    "读取待打包条目的类型失败 {}: {err}",
                    entry.path().display()
                ))
            })?;
            if file_type.is_symlink() {
                return Err(AppError::pak(format!(
                    "工作目录里存在符号链接，拒绝打包（它会把工作目录外的文件打进 PAK）: {}",
                    entry.path().display()
                )));
            }
            let path = entry.path();
            if file_type.is_dir() {
                stack.push(path);
                continue;
            }
            if !file_type.is_file() {
                log::warn!("跳过非常规文件（不是普通文件）: {}", path.display());
                continue;
            }
            let relative = path.strip_prefix(root).map_err(|err| {
                AppError::pak(format!("无法计算归档内路径 {}: {err}", path.display()))
            })?;
            let archive_path = normalize_entry_name(&relative.to_string_lossy());
            let file_name = relative
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned);
            // 只在能拿到 UTF-8 文件名时判定：判不出来就照常打包
            // （漏打用户内容是比多打一个临时文件严重得多的错误）。
            if let Some(name) = file_name {
                if crate::config::is_atomic_temp_name(&name) {
                    log::warn!(
                        "跳过原子写残留的临时文件（不打包，可能是上次写回失败/进程被杀留下的）: {}",
                        archive_path
                    );
                    continue;
                }
            }
            out.push((path, archive_path));
        }
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(out)
}

/// 重打包用的临时文件名（与目标同目录，`rename` 才能原子生效）。
///
/// 带进程内自增序号：并发打包同一目标时两次运行不会共用一个临时文件。
/// 前缀是 `.`（隐藏文件），即使进程被强杀留下残骸也不容易被误当成产物。
fn repack_temp_path(output: &Path) -> PathBuf {
    static REPACK_TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = REPACK_TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    output.with_file_name(format!(
        ".{name}.bg3-translate-{}-{seq}.tmp",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_files_by_extension_and_path() {
        assert_eq!(
            classify_file("Localization/English/foo.xml"),
            PakFileKind::LocalizationXml
        );
        assert_eq!(
            classify_file("localization/chinese/x.XML"),
            PakFileKind::LocalizationXml
        );
        assert_eq!(
            classify_file("Localization/English/dialog.loca"),
            PakFileKind::LocalizationLoca
        );
        assert_eq!(classify_file("Mods/Meta.lsx"), PakFileKind::MetadataLsx);
        assert_eq!(classify_file("Scripts/Boot.lua"), PakFileKind::ScriptLua);
        assert_eq!(classify_file("Stats/Weapons.txt"), PakFileKind::DataTxt);
        assert_eq!(classify_file("Mods/foo.pak"), PakFileKind::Other);
        // 名字里带 localization 但扩展名不对，不算本地化 XML
        assert_eq!(
            classify_file("Localization/English/readme.md"),
            PakFileKind::Other
        );
    }

    #[test]
    fn normalizes_backslash_paths() {
        assert_eq!(
            normalize_entry_name(r"Localization\English\a.xml"),
            "Localization/English/a.xml"
        );
    }

    /// `Localization/Chinese/` 必须被认成中文。
    ///
    /// 前端写回中文用的目录名就是 `Chinese`（`TARGET_LANGUAGE`），而上游的
    /// `detect_language_from_path` 只认 BG3 官方 14 个名字、**不含 `Chinese`** ——
    /// 不补这条，上一轮产出的中文文件在下一轮会掉进「未知语言」，
    /// 同一目标路径上英文会压过中文（`localizationWritePriority`）。
    #[test]
    fn chinese_target_directory_is_recognized_as_chinese() {
        for name in [
            "Localization/Chinese/x.xml",
            "localization/chinese/x.xml",
            r"Mods\MyMod\Localization\Chinese\x.loca",
        ] {
            assert_eq!(
                detect_language(name).as_deref(),
                Some("Chinese"),
                "{name} 必须被认成中文"
            );
        }
        // 官方语言名照旧
        assert_eq!(
            detect_language("Localization/Polish/a.xml").as_deref(),
            Some("Polish")
        );
        assert_eq!(
            detect_language("Localization/ChineseSimplified/a.xml").as_deref(),
            Some("ChineseSimplified")
        );
        // 不误伤：只有紧跟在 Localization 后面的那一段才是语言目录
        assert_eq!(
            detect_language("Localization/English/Chinese/x.xml").as_deref(),
            Some("English")
        );
        assert_eq!(detect_language("Mods/Chinese/x.xml"), None);
    }

    /// 端到端：`build_pak_files` 出来的文件列表里，中文目录的语言字段必须是
    /// `Chinese`（变异：把 `to_pak_file` 改回只调上游函数，这条立刻变红）。
    #[test]
    fn build_pak_files_reports_chinese_for_our_target_directory() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src");
        std::fs::create_dir_all(source.join("Localization/Chinese")).unwrap();
        std::fs::write(source.join("Localization/Chinese/x.xml"), b"<contentList/>").unwrap();
        let pak = tmp.path().join("zh.pak");
        PackageBuilder::new()
            .add_directory(&source)
            .unwrap()
            .build(&pak)
            .unwrap();

        let pkg = Package::open(&pak).unwrap();
        let files = build_pak_files(&pkg);
        assert_eq!(files.len(), 1);
        assert_eq!(
            files[0].language.as_deref(),
            Some("Chinese"),
            "文件列表里的语言字段: {files:?}"
        );
    }

    #[test]
    fn safe_output_path_rejects_traversal_and_absolute_paths() {
        let root = Path::new("root");
        assert!(safe_output_path(root, "../evil.txt").is_err());
        assert!(safe_output_path(root, "a/../../evil.txt").is_err());
        assert!(safe_output_path(root, "/evil.txt").is_err());
        assert!(safe_output_path(root, r"..\evil.txt").is_err());
        // Windows 保留设备名与 NTFS 数据流
        assert!(safe_output_path(root, "CON").is_err());
        assert!(safe_output_path(root, "nul.txt").is_err());
        assert!(safe_output_path(root, "Mods/COM1.lsx").is_err());
        assert!(safe_output_path(root, "a:b.txt").is_err());
        // 只是名字里含保留词的不该误伤
        assert!(safe_output_path(root, "CONFIG.lsx").is_ok());
        assert!(safe_output_path(root, "Console.txt").is_ok());
        assert_eq!(
            safe_output_path(root, "Localization/English/test.xml").unwrap(),
            root.join("Localization").join("English").join("test.xml")
        );
        // `./` 应被吞掉而不是报错
        assert_eq!(
            safe_output_path(root, "./a/./b.txt").unwrap(),
            root.join("a").join("b.txt")
        );
    }

    /// Windows 会去掉每个路径片段**结尾**的点与空格，`".. "` 于是变成 `".."`——
    /// 这是经典 zip-slip 绕过（Linux 上 `".. "` 只是个普通文件名，本地测不出来，
    /// 但产物是要在 Windows 上跑的）。这类名字也不可能由合法的 PAK 产生。
    #[test]
    fn safe_output_path_rejects_trailing_dots_and_spaces() {
        let root = Path::new("root");
        for name in [
            ".. ",
            ".. /evil.txt",
            "a. ",
            "COM1 ",
            "NUL. ",
            "...",
            "a/.. . /b.txt",
        ] {
            assert!(
                safe_output_path(root, name).is_err(),
                "{name:?} 在 Windows 上会被规整掉尾随点/空格，必须拒绝"
            );
        }
        // 名字中间的点与空格不受影响
        assert!(safe_output_path(root, "a.b c.txt").is_ok());
        assert!(safe_output_path(root, "Mods/Meta.lsx").is_ok());
    }

    /// 含 NUL 的片段不可能落盘（`fs::write` 会报 `unexpected NUL byte`），
    /// 应该在路径校验阶段就给出明确的 pak 错误，而不是半路抛一个 IO 错误。
    #[test]
    fn safe_output_path_rejects_nul_bytes() {
        let root = Path::new("root");
        assert!(safe_output_path(root, "evil\u{0}.txt").is_err());
        assert!(safe_output_path(root, "a/\u{0}.lsx").is_err());
    }

    /// `walk_files` 不能跟着符号链接走出被遍历的目录树。
    ///
    /// 复现（修复前）：`walk_files` 用 `path.is_dir()`（会跟随链接）判断目录，
    /// 于是链接指向的外部目录里的文件会被当成自己的文件列出来；如果链接指向
    /// 自己的祖先目录，遍历会**永不结束**。`pick_largest_pak` 依赖这个函数，
    /// 也就可能选中解压目录之外的 `.pak`。
    #[cfg(unix)]
    #[test]
    fn walk_files_does_not_follow_symlinked_directories() {
        let base = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("outside.txt"), b"outside").unwrap();
        std::fs::write(base.path().join("inside.txt"), b"inside").unwrap();
        std::os::unix::fs::symlink(outside.path(), base.path().join("link_dir")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("outside.txt"),
            base.path().join("link_file.txt"),
        )
        .unwrap();

        let files = walk_files(base.path());

        assert!(
            files.iter().any(|p| p.ends_with("inside.txt")),
            "本目录里的真实文件必须被列出: {files:?}"
        );
        assert!(
            !files.iter().any(|p| p.ends_with("outside.txt")),
            "符号链接指向的外部文件不该被列出: {files:?}"
        );
        assert!(
            !files
                .iter()
                .any(|p| p.components().any(|c| c.as_os_str() == "link_dir")),
            "符号链接目录不该被递归进去: {files:?}"
        );
    }

    #[test]
    fn work_dirs_are_unique_even_within_the_same_millisecond() {
        let base = tempfile::tempdir().unwrap();
        let a = create_work_dir_in(base.path()).unwrap();
        let b = create_work_dir_in(base.path()).unwrap();
        assert_ne!(a, b);
        assert!(a.is_dir() && b.is_dir());
        assert!(is_work_dir(&a));
        remove_work_dir(&a);
        remove_work_dir(&b);
        assert!(!a.exists() && !b.exists());
    }

    #[test]
    fn remove_work_dir_is_a_noop_on_missing_path() {
        let base = tempfile::tempdir().unwrap();
        remove_work_dir(&base.path().join("does-not-exist"));
    }

    #[test]
    fn resolve_disk_path_joins_under_unpacked() {
        let path = resolve_disk_path("/work", r"Localization\English\a.xml");
        assert_eq!(
            path,
            PathBuf::from("/work/unpacked/Localization/English/a.xml")
        );
    }

    #[test]
    fn walk_files_visits_everything_and_is_sorted() {
        let base = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(base.path().join("b/c")).unwrap();
        std::fs::write(base.path().join("z.txt"), b"1").unwrap();
        std::fs::write(base.path().join("b/a.txt"), b"2").unwrap();
        std::fs::write(base.path().join("b/c/m.txt"), b"3").unwrap();

        let files = walk_files(base.path());
        assert_eq!(files.len(), 3);
        let mut sorted = files.clone();
        sorted.sort();
        assert_eq!(files, sorted);
        assert!(files.iter().any(|p| p.ends_with("c/m.txt")));
    }

    #[test]
    fn picks_the_largest_pak_in_a_zip_tree() {
        let base = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("small.pak"), vec![0u8; 10]).unwrap();
        std::fs::write(base.path().join("big.pak"), vec![0u8; 500]).unwrap();
        std::fs::write(base.path().join("readme.txt"), b"hi").unwrap();
        std::fs::create_dir_all(base.path().join("Mods")).unwrap();
        std::fs::write(base.path().join("Mods/mid.PAK"), vec![0u8; 100]).unwrap();

        let picked = pick_largest_pak(base.path()).unwrap();
        assert_eq!(picked.file_name().unwrap(), "big.pak");
    }

    #[test]
    fn pick_largest_pak_returns_none_without_paks() {
        let base = tempfile::tempdir().unwrap();
        std::fs::write(base.path().join("a.txt"), b"x").unwrap();
        assert!(pick_largest_pak(base.path()).is_none());
    }

    #[test]
    fn read_priority_defaults_to_zero_and_clamps() {
        let base = tempfile::tempdir().unwrap();
        assert_eq!(read_priority(base.path()), 0);

        std::fs::write(base.path().join(META_FILE), br#"{"priority":7}"#).unwrap();
        assert_eq!(read_priority(base.path()), 7);

        std::fs::write(base.path().join(META_FILE), br#"{"priority":9999}"#).unwrap();
        assert_eq!(read_priority(base.path()), u8::MAX);

        std::fs::write(base.path().join(META_FILE), b"not json").unwrap();
        assert_eq!(read_priority(base.path()), 0);
    }

    #[test]
    fn extract_to_directory_rejects_missing_input() {
        let base = tempfile::tempdir().unwrap();
        let err =
            extract_to_directory("/nope/missing.pak", base.path().to_str().unwrap()).unwrap_err();
        assert_eq!(err.code(), "config");
    }

    #[test]
    fn repack_rejects_work_dir_without_unpacked() {
        let base = tempfile::tempdir().unwrap();
        let err = repack(
            base.path().to_str().unwrap(),
            base.path().join("out.pak").to_str().unwrap(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "config");
    }

    /// `repack` 必须拒绝 `unpacked/` 里的符号链接。
    ///
    /// 复现（修复前）：`PackageBuilder::add_directory` 跟随符号链接，
    /// `/tmp/outside/evil.txt`（工作目录之外）被原样打进了 PAK。
    #[cfg(unix)]
    #[test]
    fn repack_refuses_symlinks_inside_unpacked() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let unpacked = work.join(UNPACKED_DIR);
        std::fs::create_dir_all(unpacked.join("Localization/English")).unwrap();
        std::fs::write(
            unpacked.join("Localization/English/a.xml"),
            b"<contentList/>",
        )
        .unwrap();

        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("evil.txt"), b"OUTSIDE SECRET").unwrap();
        std::os::unix::fs::symlink(outside.path(), unpacked.join("link_dir")).unwrap();

        let output = tmp.path().join("out.pak");
        let err = repack(work.to_str().unwrap(), output.to_str().unwrap()).unwrap_err();

        assert_eq!(err.code(), "pak", "必须报 pak 错误: {err}");
        assert!(
            err.to_string().contains("符号链接"),
            "错误信息要说明原因: {err}"
        );
        assert!(!output.exists(), "拒绝打包时不该产出文件");
    }

    /// `repack` 必须跳过原子写残留的临时文件，**且一个合法文件都不能漏打**。
    ///
    /// 复现（修复前）：写回失败（EFBIG/ENOSPC）或进程被强杀会在 `unpacked/` 里
    /// 留下 `english.tmp<pid>.<序号>`，`repack` 返回 `Ok(())` 并把它原样打进
    /// 用户的产物（独立验证者 R5-01 实测：产物多出两个 8 KB 的
    /// `Localization/English/*.tmp*` 垃圾条目，其中内容是**被截断的 XML**）。
    ///
    /// 判定复用写入端同一份规则（`config::is_atomic_temp_name`），所以这里同时钉住
    /// 两侧：只有那条完整形状的名字被跳过，`a.tmp` / `notes.tmp123.txt` /
    /// `a.tmp..1` 这类合法名字必须照常进包（误判 = 静默丢用户内容）。
    #[test]
    fn repack_skips_stale_atomic_write_temp_files() {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let dir = work.join(UNPACKED_DIR).join("Localization/English");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("english.xml"), b"<contentList/>").unwrap();

        // 陈旧的原子写临时文件：形状与写入端一致（`english.xml` → `english.tmp<pid>.<n>`），
        // 内容是**被截断的 XML** —— 打进 PAK 就是一条坏掉的本地化文件
        let stale_name = format!("english.tmp{}.7", std::process::id());
        std::fs::write(
            dir.join(&stale_name),
            b"<contentList><content contentuid=\"h1",
        )
        .unwrap();

        // 合法文件名（都与临时文件形状擦肩而过），一条都不能漏
        let legal = [
            "english.xml",         // 正常目标
            "a.tmp",               // 没有 pid/序号
            "notes.tmp123.txt",    // 结尾不是数字
            "tmp.xml",             // 没有主名
            "a.tmp1.b",            // 结尾是 .b
            "a.tmp..1",            // pid 缺失（空）
            "english.tmp12.3.xml", // 序号后面还有扩展名
        ];
        for name in legal {
            std::fs::write(dir.join(name), b"<contentList/>").unwrap();
        }

        let output = tmp.path().join("Out.pak");
        repack(work.to_str().unwrap(), output.to_str().unwrap()).unwrap();

        let pkg = Package::open(&output).unwrap();
        let names: Vec<String> = build_pak_files(&pkg).into_iter().map(|f| f.name).collect();
        assert!(
            !names.iter().any(|name| name.ends_with(&stale_name)),
            "陈旧临时文件不能进产物: {names:?}"
        );
        assert_eq!(
            names.len(),
            legal.len(),
            "条目数应等于合法文件数: {names:?}"
        );
        for name in legal {
            let expected = format!("Localization/English/{name}");
            assert!(
                names.contains(&expected),
                "{expected} 必须照常打进产物: {names:?}"
            );
        }
    }

    /// 判定要**窄**：只有完整的临时文件形状才跳过，合法名字一律照打。
    ///
    /// 直接对着打包收集器测（比只测 `is_atomic_temp_name` 更靠得住：
    /// 钉的是「打包路径真的用了这份规则、而且只丢该丢的那一个」）。
    #[test]
    fn collect_pack_files_only_drops_the_real_temp_shape() {
        let tmp = tempfile::tempdir().unwrap();
        let unpacked = tmp.path().join(UNPACKED_DIR);
        std::fs::create_dir_all(unpacked.join("Mods")).unwrap();
        for name in [
            "Mods/meta.lsx",
            "Mods/a.tmp",
            "Mods/notes.tmp123.txt",
            "Mods/a.tmp..1",
            "Mods/english.xml.tmp123",
            "Mods/.hidden.tmp9.0", // 唯一的临时文件形状
        ] {
            std::fs::write(unpacked.join(name), b"x").unwrap();
        }

        let collected = collect_pack_files(&unpacked).unwrap();
        let paths: Vec<&str> = collected.iter().map(|(_, path)| path.as_str()).collect();

        assert!(
            !paths.contains(&"Mods/.hidden.tmp9.0"),
            "临时文件形状必须被排除: {paths:?}"
        );
        for kept in [
            "Mods/meta.lsx",
            "Mods/a.tmp",
            "Mods/notes.tmp123.txt",
            "Mods/a.tmp..1",
            "Mods/english.xml.tmp123",
        ] {
            assert!(paths.contains(&kept), "{kept} 不能被丢掉: {paths:?}");
        }
        assert_eq!(paths.len(), 5, "{paths:?}");
        // 条目顺序稳定（同一份工作目录每次打出同样的顺序）
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted, "条目顺序必须稳定: {paths:?}");
    }

    #[test]
    fn open_and_extract_rejects_missing_file() {
        let err = open_and_extract("/nope/missing.pak").unwrap_err();
        assert_eq!(err.code(), "config");
    }

    /// 归档里的「目录条目」（名字以 `/` 结尾）不能把整个 MOD 变成打不开。
    ///
    /// 复现（修复前）：条目 `Localization/` 被当成普通文件写成
    /// `unpacked/Localization`，随后同目录下的 `Localization/English/x.xml`
    /// 在 `create_dir_all` 处直接失败（`Not a directory`），`open_and_extract_in`
    /// 只抛出一句 `文件读写失败: Not a directory (os error 20)` —— 用户不知道
    /// 是哪个条目、也不知道为什么，整个 MOD 一个文件都解不出来。
    ///
    /// 目录条目不携带内容（LSPK 的文件表是扁平的），正确处置是建目录 + 跳过：
    /// 它既不进文件列表，也不该影响同目录下的真实文件。
    #[test]
    fn directory_style_entries_do_not_break_extraction() {
        let tmp = tempfile::tempdir().unwrap();
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("Localization/English")).unwrap();
        std::fs::write(
            src.join("Localization/English/x.xml"),
            br#"<contentList><content contentuid="h1" version="1">A</content></contentList>"#,
        )
        .unwrap();
        // 少数打包器会为目录写一条以 `/` 结尾的空条目
        std::fs::write(tmp.path().join("dummy"), b"").unwrap();
        let pak = tmp.path().join("dirlike.pak");
        PackageBuilder::new()
            .add_directory(&src)
            .unwrap()
            .add_file(tmp.path().join("dummy"), "Localization/")
            .build(&pak)
            .unwrap();

        let work_root = tmp.path().join("work");
        std::fs::create_dir_all(&work_root).unwrap();
        let (work_dir, files) = open_and_extract_in(pak.to_str().unwrap(), &work_root).unwrap();

        assert_eq!(
            files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec!["Localization/English/x.xml"],
            "目录条目不该出现在文件列表里"
        );
        let extracted =
            std::fs::read_to_string(work_dir.join("unpacked/Localization/English/x.xml")).unwrap();
        assert!(extracted.contains("contentuid=\"h1\""), "{extracted}");
        assert!(
            work_dir.join("unpacked/Localization").is_dir(),
            "目录条目应该变成真目录"
        );

        // 重打包也不该被目录条目影响：产物里只有真实文件
        let out = tmp.path().join("out.pak");
        repack(work_dir.to_str().unwrap(), out.to_str().unwrap()).unwrap();
        let pkg = Package::open(&out).unwrap();
        assert_eq!(
            build_pak_files(&pkg)
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Localization/English/x.xml"]
        );
    }

    /// PAK 允许重复条目（同一路径多份数据）；解包必须与**游戏加载器的选择**
    /// 一致：上游 `Package::open` 的 `index` 是「后写覆盖先写」，`get()` 返回
    /// 最后一条，所以解包也必须取**最后一个可读**的版本 —— 取错的话用户
    /// 翻译的是游戏根本不会用到的那份数据。
    #[test]
    fn duplicate_pak_entries_use_the_last_readable_version() {
        let tmp = tempfile::tempdir().unwrap();
        let v1 = tmp.path().join("v1");
        let v2 = tmp.path().join("v2");
        std::fs::create_dir_all(v1.join("Localization/English")).unwrap();
        std::fs::create_dir_all(v2.join("Localization/English")).unwrap();
        let name = "Localization/English/x.xml";
        std::fs::write(
            v1.join(name),
            br#"<contentList><content contentuid="h1" version="1">FIRST</content></contentList>"#,
        )
        .unwrap();
        std::fs::write(
            v2.join(name),
            br#"<contentList><content contentuid="h1" version="1">SECOND</content></contentList>"#,
        )
        .unwrap();

        let pak = tmp.path().join("dup.pak");
        PackageBuilder::new()
            .add_directory(&v1)
            .unwrap()
            .add_file(v2.join(name), name)
            .build(&pak)
            .unwrap();

        let work_root = tmp.path().join("work");
        std::fs::create_dir_all(&work_root).unwrap();
        let (work_dir, files) = open_and_extract_in(pak.to_str().unwrap(), &work_root).unwrap();

        assert_eq!(files.len(), 1, "同名条目只能解出一份文件: {files:?}");
        assert_eq!(files[0].name, name);
        let extracted = std::fs::read_to_string(work_dir.join("unpacked").join(name)).unwrap();
        assert!(
            extracted.contains("SECOND"),
            "必须与游戏加载器取同一份（最后一条）: {extracted}"
        );

        // 与上游的 `get()` 对齐（它就是游戏语义的那份索引）
        let pkg = Package::open(&pak).unwrap();
        assert_eq!(
            pkg.get(name).map(|f| f.size()),
            Some(files[0].size),
            "解包选中的条目必须和 `Package::get` 指向的同一条"
        );
    }

    /// 打包**失败**时不能摧毁 `output_path` 上已有的文件，也不能留下半个 pak。
    ///
    /// `PackageBuilder::build` 的第一步就是 `File::create(output)`（截断），之后才逐个
    /// `File::open` 源文件：任何一个源文件读不出来（磁盘满、权限、文件被中途删掉），
    /// 用户原先放在那个路径上的产物就已经被清空了 —— 而导出覆盖上一次的产物
    /// 恰恰是最常见的用法。
    ///
    /// 复现（修复前）：`EitherExisting.pak` 从 32 字节变成 0 字节，`repack` 返回 Err。
    #[cfg(unix)]
    #[test]
    fn failed_repack_does_not_destroy_an_existing_output_file() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let unpacked = work.join(UNPACKED_DIR);
        std::fs::create_dir_all(unpacked.join("Localization/English")).unwrap();
        std::fs::write(
            unpacked.join("Localization/English/a.xml"),
            b"<contentList/>",
        )
        .unwrap();
        // 这个文件让 `build` 在中途失败（`add_directory` 只 stat，不打开）
        let blocked = unpacked.join("Localization/English/b.xml");
        std::fs::write(&blocked, b"<contentList/>").unwrap();
        let mut perms = std::fs::metadata(&blocked).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&blocked, perms).unwrap();
        if std::fs::File::open(&blocked).is_ok() {
            eprintln!("跳过：当前用户仍可读 chmod 000 的文件（可能以 root 运行）");
            return;
        }

        let output = tmp.path().join("Existing.pak");
        let existing = b"USER EXISTING PAK CONTENT 0123456789";
        std::fs::write(&output, existing).unwrap();

        let err = repack(work.to_str().unwrap(), output.to_str().unwrap()).unwrap_err();
        assert_eq!(err.code(), "pak", "必须报 pak 错误: {err}");
        assert_eq!(
            std::fs::read(&output).unwrap(),
            existing,
            "打包失败不能摧毁 output 路径上已有的文件"
        );

        // 临时文件必须被清理干净（失败路径）
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec!["Existing.pak".to_string(), "work".to_string()],
            "失败路径不能留下临时文件"
        );

        // 恢复权限后成功打包：产物存在且可再次打开（自洽性），也没有临时文件
        let mut perms = std::fs::metadata(&blocked).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&blocked, perms).unwrap();
        let ok_out = tmp.path().join("Out.pak");
        repack(work.to_str().unwrap(), ok_out.to_str().unwrap()).unwrap();
        assert!(ok_out.is_file(), "成功路径必须产出文件");
        let pkg = Package::open(&ok_out).unwrap();
        assert_eq!(build_pak_files(&pkg).len(), 2, "两个文件都要进包");
        let mut names: Vec<String> = std::fs::read_dir(tmp.path())
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            vec![
                "Existing.pak".to_string(),
                "Out.pak".to_string(),
                "work".to_string()
            ],
            "成功路径同样不能留下临时文件"
        );
    }

    /// 压缩炸弹防线：65 KB 的 zip 能解出 64 MiB（≈1000×），必须在解压过程中
    /// 就被拦住，而且**不能**在工作目录里留下已经写出去的大文件。
    ///
    /// 复现（修复前）：`extract_zip_to_find_pak` 对声明大小与实际写入都没有任何
    /// 上限，19 KiB 的 zip 24 ms 就写出 64 MiB（见报告 §2 F-09 的探针输出）。
    #[test]
    fn zip_bomb_is_refused_before_it_fills_the_disk() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("bomb.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("bomb.bin", options).unwrap();
            let chunk = vec![0u8; 1 << 20];
            for _ in 0..4 {
                std::io::Write::write_all(&mut writer, &chunk).unwrap();
            }
            writer.finish().unwrap();
        }
        let zip_size = std::fs::metadata(&zip_path).unwrap().len();
        assert!(zip_size < 64 * 1024, "炸弹样本应远小于解压结果: {zip_size}");

        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        // 只有「单条目声明上限」会触发（总量给了 64 MiB），
        // 这样把单条目检查去掉后炸弹会被真的解压出来，体积断言立刻变红。
        let limits = ZipLimits {
            max_entry_bytes: 1 << 20,  // 1 MiB：4 MiB 的条目必然超
            max_total_bytes: 64 << 20, // 64 MiB：总量检查不会先触发
            max_entries: 16,
        };

        let err = extract_zip_to_find_pak_limited(&zip_path, &work, limits).unwrap_err();
        assert_eq!(err.code(), "pak", "超限必须报 pak 错误: {err}");
        assert!(
            err.to_string().contains("上限") || err.to_string().contains("中止"),
            "错误信息要说清是超限: {err}"
        );

        // 工作目录里不能留下超过单条目上限的文件
        for path in walk_files(&work) {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            assert!(
                size <= (1 << 20),
                "{} 留下了 {size} 字节（超过上限）",
                path.display()
            );
        }
    }

    /// 压缩炸弹防线②：**总量**超限（多个中等条目累加）同样要中止。
    #[test]
    fn zip_total_size_limit_is_enforced_across_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("two.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            let chunk = vec![0u8; 600 * 1024];
            for name in ["a.bin", "b.bin"] {
                writer.start_file(name, options).unwrap();
                std::io::Write::write_all(&mut writer, &chunk).unwrap();
            }
            writer.finish().unwrap();
        }
        // 把两条的「声明大小」都改成 900 KiB（实际各 600 KiB），
        // 于是声明总量 1.8 MiB > 上限 1 MiB，而实际写入总量 1.2 MiB 也不会先触发
        // 单条目上限 —— 只有「声明总量」这一层能拦住它。
        let mut bytes = std::fs::read(&zip_path).unwrap();
        for (signature, offset) in [
            (b"PK\x03\x04".as_slice(), 22usize),
            (b"PK\x01\x02".as_slice(), 24usize),
        ] {
            let mut i = 0usize;
            while let Some(pos) = bytes[i..]
                .windows(4)
                .position(|window| window == signature)
                .map(|p| i + p)
            {
                bytes[pos + offset..pos + offset + 4]
                    .copy_from_slice(&(900u32 * 1024).to_le_bytes());
                i = pos + 4;
            }
        }
        std::fs::write(&zip_path, &bytes).unwrap();

        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let limits = ZipLimits {
            max_entry_bytes: 1 << 20, // 单条目声明 900 KiB 不超
            max_total_bytes: 1 << 20, // 两条声明累加 1.8 MiB 必超
            max_entries: 16,
        };

        let err = extract_zip_to_find_pak_limited(&zip_path, &work, limits).unwrap_err();
        assert_eq!(err.code(), "pak", "总量超限必须报错: {err}");
        assert!(
            err.to_string().contains("将超过上限"),
            "必须是「声明总量」这一层拦下来的（实际写入层是另一条消息，不能算数）: {err}"
        );
    }

    /// 压缩炸弹防线③：声明大小是**可以撒谎的**，实际写入字节数也要兜底。
    ///
    /// 做法：正常写一个 4 MiB 的 deflate 条目，再把本地头（`PK\x03\x04` +22）
    /// 与中央目录（`PK\x01\x02` +24）里的「解压后大小」字段篡改成 1 字节——
    /// 两个声明检查全部放行，只有实际写入计数能拦住它。
    #[test]
    fn zip_with_a_lying_size_header_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("liar.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("liar.bin", options).unwrap();
            std::io::Write::write_all(&mut writer, &vec![0u8; 4 << 20]).unwrap();
            writer.finish().unwrap();
        }

        let mut bytes = std::fs::read(&zip_path).unwrap();
        let mut patched = 0usize;
        for (signature, offset) in [
            (b"PK\x03\x04".as_slice(), 22usize),
            (b"PK\x01\x02".as_slice(), 24usize),
        ] {
            let mut i = 0usize;
            while let Some(pos) = bytes[i..]
                .windows(4)
                .position(|window| window == signature)
                .map(|p| i + p)
            {
                bytes[pos + offset..pos + offset + 4].copy_from_slice(&1u32.to_le_bytes());
                patched += 1;
                i = pos + 4;
            }
        }
        assert!(patched >= 2, "应至少篡改本地头与中央目录各一处: {patched}");
        std::fs::write(&zip_path, &bytes).unwrap();

        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let limits = ZipLimits {
            max_entry_bytes: 1 << 20,
            max_total_bytes: 2 << 20,
            max_entries: 16,
        };

        let err = extract_zip_to_find_pak_limited(&zip_path, &work, limits).unwrap_err();
        assert_eq!(err.code(), "pak", "实际写入超限必须报错: {err}");
        for path in walk_files(&work) {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            assert!(size <= (2 << 20), "{} 留下了 {size} 字节", path.display());
        }
    }

    /// 条目数上限：海量小文件同样要拦（这里用 3 个条目 + 上限 2 来验证）。
    #[test]
    fn zip_entry_count_limit_is_enforced() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("many.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default();
            for i in 0..3 {
                writer.start_file(format!("f{i}.txt"), options).unwrap();
                std::io::Write::write_all(&mut writer, b"x").unwrap();
            }
            writer.finish().unwrap();
        }
        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let limits = ZipLimits {
            max_entry_bytes: 1 << 20,
            max_total_bytes: 1 << 20,
            max_entries: 2,
        };

        let err = extract_zip_to_find_pak_limited(&zip_path, &work, limits).unwrap_err();
        assert_eq!(err.code(), "pak", "必须报 pak 错误: {err}");
        assert!(
            err.to_string().contains("条目数"),
            "必须是「条目数」这一层拦下来的: {err}"
        );
        assert!(
            walk_files(&work).is_empty(),
            "超限时不该解压出任何文件: {:?}",
            walk_files(&work)
        );
    }

    /// 声明大小撒谎时，**单条目**实际写入同样不能突破单文件上限。
    ///
    /// 旧实现的第 ③ 层（实际写入兜底）只按「总量预算」`take`：一个声明 1 字节、
    /// 实际 4 MiB 的条目在总量上限 16 GiB 下会被完整写出来，单文件上限形同虚设
    /// （65 KB 的 zip 仍可写出 16 GiB）。
    ///
    /// 复现（修复前）：本用例解压「成功」（错误信息是「未找到 .pak」），
    /// `bomb.bin` 留下 4 MiB —— 两个断言都红。
    #[test]
    fn zip_entry_that_lies_about_its_size_cannot_exceed_the_per_entry_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let zip_path = tmp.path().join("liar2.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("bomb.bin", options).unwrap();
            std::io::Write::write_all(&mut writer, &vec![0u8; 4 << 20]).unwrap();
            writer.finish().unwrap();
        }

        // 把「解压后大小」篡改成 1 字节：两个声明层检查全部放行
        let mut bytes = std::fs::read(&zip_path).unwrap();
        let mut patched = 0usize;
        for (signature, offset) in [
            (b"PK\x03\x04".as_slice(), 22usize),
            (b"PK\x01\x02".as_slice(), 24usize),
        ] {
            let mut i = 0usize;
            while let Some(pos) = bytes[i..]
                .windows(4)
                .position(|window| window == signature)
                .map(|p| i + p)
            {
                bytes[pos + offset..pos + offset + 4].copy_from_slice(&1u32.to_le_bytes());
                patched += 1;
                i = pos + 4;
            }
        }
        assert!(patched >= 2, "应至少篡改本地头与中央目录各一处: {patched}");
        std::fs::write(&zip_path, &bytes).unwrap();

        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let limits = ZipLimits {
            max_entry_bytes: 1 << 20,  // 1 MiB：实际 4 MiB 必须被拦
            max_total_bytes: 64 << 20, // 总量宽松，只有单条目层能拦住
            max_entries: 16,
        };

        let err = extract_zip_to_find_pak_limited(&zip_path, &work, limits).unwrap_err();
        assert_eq!(err.code(), "pak", "实际写入超限必须报 pak 错误: {err}");
        assert!(
            err.to_string().contains("单文件上限"),
            "必须是「单条目实际写入」这一层拦下来的（「未找到 .pak」不算数）: {err}"
        );
        for path in walk_files(&work) {
            let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            assert!(
                size <= (1 << 20),
                "{} 留下了 {size} 字节（超过单文件上限）",
                path.display()
            );
        }
    }

    /// 真实体量的 zip 照常解压（上限不能误伤正常 MOD）。
    #[test]
    fn normal_zip_still_extracts_under_the_limits() {
        let tmp = tempfile::tempdir().unwrap();
        let source = tmp.path().join("src");
        std::fs::create_dir_all(source.join("Localization/English")).unwrap();
        std::fs::write(source.join("Localization/English/a.xml"), b"<contentList/>").unwrap();
        let inner = tmp.path().join("Inner.pak");
        PackageBuilder::new()
            .priority(1)
            .add_directory(&source)
            .unwrap()
            .build(&inner)
            .unwrap();

        let zip_path = tmp.path().join("Normal.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            writer.start_file("README.txt", options).unwrap();
            std::io::Write::write_all(&mut writer, b"install me").unwrap();
            writer.start_file("Mods/Inner.pak", options).unwrap();
            std::io::Write::write_all(&mut writer, &std::fs::read(&inner).unwrap()).unwrap();
            writer.finish().unwrap();
        }

        let work = tmp.path().join("work");
        std::fs::create_dir_all(&work).unwrap();
        let pak = extract_zip_to_find_pak(&zip_path, &work).unwrap();
        assert!(pak.is_file());
        assert!(pak.ends_with("Inner.pak"));
    }

    /// `ZipLimits::default()` 必须被钉住。
    ///
    /// 上面几条炸弹用例全部**注入**小上限（否则测试要造几 GiB 的归档），所以
    /// 它们拦得住「解压逻辑坏了」，却拦不住「默认值被改成 `u64::MAX`」——
    /// 那等于把整条防线关掉而 `cargo test` 依然全绿。这里直接给默认值设界，
    /// 让任何「关掉防线」的改动立刻变红（独立验证者用变异实验发现过这个缺口）。
    #[test]
    fn zip_limits_default_stays_bounded() {
        let limits = ZipLimits::default();

        // 上界：默认值不能大到等于没有限制
        assert!(
            limits.max_entry_bytes <= 16 * 1024 * 1024 * 1024,
            "单条目默认上限过大，压缩炸弹防线等于失效: {}",
            limits.max_entry_bytes
        );
        assert!(
            limits.max_total_bytes <= 64 * 1024 * 1024 * 1024,
            "总量默认上限过大，压缩炸弹防线等于失效: {}",
            limits.max_total_bytes
        );
        assert!(
            limits.max_entries <= 1_000_000,
            "条目数默认上限过大，海量小文件防线等于失效: {}",
            limits.max_entries
        );

        // 下界：也不能小到误伤真实 MOD（几 GB 的 4K 材质包必须放行）
        assert!(
            limits.max_entry_bytes >= 1024 * 1024 * 1024,
            "单条目默认上限过小，会误伤真实的大 MOD: {}",
            limits.max_entry_bytes
        );
        assert!(
            limits.max_total_bytes >= limits.max_entry_bytes,
            "总量上限不该小于单条目上限: {} < {}",
            limits.max_total_bytes,
            limits.max_entry_bytes
        );
    }

    /// 造一个含指定文件的 PAK（炸弹用例的原材料）。
    fn pak_with_files(tmp: &Path, files: &[(&str, Vec<u8>)]) -> PathBuf {
        let src = tmp.join("payload");
        for (name, data) in files {
            let path = src.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, data).unwrap();
        }
        let pak = tmp.join("payload.pak");
        PackageBuilder::new()
            .add_directory(&src)
            .unwrap()
            .build(&pak)
            .unwrap();
        pak
    }

    /// PAK 解压必须有上限，而且要**对称于 zip 那条路**。
    ///
    /// 复现（修复前）：257 KB 的 PAK（一个 64 MiB 全零条目，LZ4 压缩比 255×）
    /// 43 ms 内被原样解出 67108864 字节，全程没有任何上限；而同样的放大在 zip
    /// 路径上会被 `ZipLimits` 拦住。把上限去掉（或改成 `u64::MAX`）这条用例立刻变红。
    ///
    /// 用注入的小上限跑（否则测试要造几 GiB 的归档），默认值另由
    /// `extract_limits_default_stays_bounded` 钉住。
    #[test]
    fn pak_expansion_is_capped_like_the_zip_path() {
        let tmp = tempfile::tempdir().unwrap();
        // 4 MiB 全零 → 归档只有几 KB
        let pak = pak_with_files(tmp.path(), &[("bomb.bin", vec![0u8; 4 << 20])]);
        let archive_size = std::fs::metadata(&pak).unwrap().len();
        assert!(
            archive_size < (1 << 20),
            "炸弹样本应远小于解压结果: {archive_size}"
        );
        let pkg = Package::open(&pak).unwrap();

        // ① 单条目上限。声明 4 MiB > 注入的 1 MiB，所以拦下它的是**声明预检**这一层
        // （真正「声明撒谎」的那一层由 `lying_declared_size_is_caught_by_the_actual_bytes_layer` 覆盖）。
        let dir = tmp.path().join("out1");
        std::fs::create_dir_all(&dir).unwrap();
        let limits = ExtractLimits {
            max_entry_bytes: 1 << 20,
            max_total_bytes: 64 << 20,
            max_entries: 16,
        };
        let err = extract_package_files_limited(&pkg, &dir, limits).unwrap_err();
        assert_eq!(err.code(), "pak", "{err}");
        assert!(
            err.to_string().contains("声明解压后"),
            "必须是「声明预检」这一层拦下来的: {err}"
        );
        assert!(
            !err.to_string().contains("实际解压"),
            "声明层能拦住就不该走到实际写入层: {err}"
        );
        assert!(
            walk_files(&dir).is_empty(),
            "超限时必须**一个文件都没写**（检查在写盘之前）: {:?}",
            walk_files(&dir)
        );

        // ② 总量上限
        let pak2 = pak_with_files(
            &tmp.path().join("two"),
            &[
                ("a.bin", vec![0u8; 600 << 10]),
                ("b.bin", vec![0u8; 600 << 10]),
            ],
        );
        let pkg2 = Package::open(&pak2).unwrap();
        let dir2 = tmp.path().join("out2");
        std::fs::create_dir_all(&dir2).unwrap();
        let err = extract_package_files_limited(
            &pkg2,
            &dir2,
            ExtractLimits {
                max_entry_bytes: 1 << 20,
                max_total_bytes: 1 << 20,
                max_entries: 16,
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "pak", "{err}");
        assert!(
            err.to_string().contains("总量"),
            "必须是「总量」这一层拦下来的: {err}"
        );
        assert!(walk_files(&dir2).is_empty(), "超限时不该写出任何文件");

        // ③ 条目数上限
        let pak3 = pak_with_files(
            &tmp.path().join("many"),
            &[("a.txt", b"a".to_vec()), ("b.txt", b"b".to_vec())],
        );
        let pkg3 = Package::open(&pak3).unwrap();
        let dir3 = tmp.path().join("out3");
        std::fs::create_dir_all(&dir3).unwrap();
        let err = extract_package_files_limited(
            &pkg3,
            &dir3,
            ExtractLimits {
                max_entry_bytes: 1 << 20,
                max_total_bytes: 1 << 20,
                max_entries: 1,
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "pak", "{err}");
        assert!(err.to_string().contains("条目数"), "{err}");

        // ④ 正向对照：正常体量照常解出（上限不能误伤）
        let dir4 = tmp.path().join("out4");
        std::fs::create_dir_all(&dir4).unwrap();
        let files = extract_package_files_limited(&pkg, &dir4, ExtractLimits::default()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            std::fs::metadata(dir4.join("bomb.bin")).unwrap().len(),
            4 << 20
        );
    }

    /// 默认上限同样必须被钉住（注入小上限的用例拦不住「默认值被改成 `u64::MAX`」）。
    ///
    /// ⚠️ 这条只钉「常量本身有界」，**不能**证明生产入口真的用了它 ——
    /// 「生产入口用了默认限制」由
    /// [`Self::production_entry_enforces_the_default_declared_total_limit`] 证明
    /// （独立验证者 T2 的变异实验：把 `extract_package_files` 改成注入
    /// `u64::MAX` 时，全量 492 条测试仍然全绿，就是缺了那条）。
    #[test]
    fn extract_limits_default_stays_bounded() {
        let limits = ExtractLimits::default();

        // 上界：默认值不能大到等于没有限制
        assert!(
            limits.max_entry_bytes <= 16 * 1024 * 1024 * 1024,
            "单条目默认上限过大，压缩炸弹防线等于失效: {}",
            limits.max_entry_bytes
        );
        assert!(
            limits.max_total_bytes <= 64 * 1024 * 1024 * 1024,
            "总量默认上限过大，压缩炸弹防线等于失效: {}",
            limits.max_total_bytes
        );
        assert!(
            limits.max_entries <= 1_000_000,
            "条目数默认上限过大，海量小文件防线等于失效: {}",
            limits.max_entries
        );

        // 下界：也不能小到误伤真实 MOD
        assert!(
            limits.max_entry_bytes >= 1024 * 1024 * 1024,
            "单条目默认上限过小，会误伤真实的大 MOD: {}",
            limits.max_entry_bytes
        );
        assert!(
            limits.max_total_bytes >= limits.max_entry_bytes,
            "总量上限不该小于单条目上限: {} < {}",
            limits.max_total_bytes,
            limits.max_entry_bytes
        );
    }

    // ─────────────────────────────────────────────────────────────
    // 手工拼的 PAK：用来打「生产入口真的用了默认限制」这条链路
    // ─────────────────────────────────────────────────────────────

    /// V7（legacy）文件表项大小：name[256] + offset u32 + size_on_disk u32
    /// + uncompressed_size u32 + archive_part u32。
    const V7_ENTRY_SIZE: usize = 272;
    /// V7 头部大小：version + data_offset + num_parts + file_list_size
    /// + little_endian(1) + num_files。
    const V7_HEADER_SIZE: usize = 21;

    /// 拼一条 V7 文件表项。
    fn v7_entry(name: &str, offset: u32, size_on_disk: u32, declared: u32) -> Vec<u8> {
        let mut entry = vec![0u8; V7_ENTRY_SIZE];
        let name_bytes = name.as_bytes();
        assert!(name_bytes.len() < 256, "名字必须能放进 256 字节的定长字段");
        entry[..name_bytes.len()].copy_from_slice(name_bytes);
        entry[256..260].copy_from_slice(&offset.to_le_bytes());
        entry[260..264].copy_from_slice(&size_on_disk.to_le_bytes());
        entry[264..268].copy_from_slice(&declared.to_le_bytes());
        entry[268..272].copy_from_slice(&0u32.to_le_bytes()); // archive_part
        entry
    }

    /// 拼一个 V7 PAK：头部 + **未压缩**文件表 + 可选负载。
    ///
    /// 为什么是 V7：`bg3rustpaklib` 的 `has_compressed_file_list()` 是
    /// `version >= V13`，只有 V7/V9/V10 的文件表是明文 —— V13+ 的表是 LZ4 压过的，
    /// 要在测试里伪造声明就得先有一个 LZ4 编码器（不允许新增依赖）。
    ///
    /// 这也是**唯一**能在默认上限下触发声明预检的形态：V7/V18 的单条目
    /// `uncompressed_size` 是 u32（≤ 4 GiB），低于默认单条目上限 6 GiB，
    /// 所以「单条目声明超默认上限」在真实格式里不可达；能达的是**总量**
    /// （多条 u32 声明累加 > 16 GiB）与**条目数**（> 20 万）两个默认值。
    fn forge_v7_pak(entries: &[Vec<u8>], payload: &[u8]) -> Vec<u8> {
        let file_list_len = entries.len() * V7_ENTRY_SIZE;
        let mut pak = Vec::with_capacity(V7_HEADER_SIZE + file_list_len + payload.len());
        pak.extend_from_slice(&7u32.to_le_bytes()); // version = V7
        pak.extend_from_slice(&0u32.to_le_bytes()); // data_offset
        pak.extend_from_slice(&1u32.to_le_bytes()); // num_parts
        pak.extend_from_slice(&(file_list_len as u32).to_le_bytes());
        pak.push(1); // little_endian
        pak.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for entry in entries {
            pak.extend_from_slice(entry);
        }
        pak.extend_from_slice(payload);
        pak
    }

    /// 声明「解压后 4 GiB」的 V7 条目（`uncompressed_size > 0` ⇒ 被当成压缩条目，
    /// 于是 `size()` 返回这个声明值）。
    fn v7_entry_declaring_4gib(name: &str) -> Vec<u8> {
        v7_entry(name, 0, 0, u32::MAX)
    }

    /// DEFLATE **stored**（不压缩）块：`BFINAL|BTYPE=00` + LEN + ~LEN + 原始字节。
    fn deflate_stored(payload: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(payload.len() + 16);
        let chunks: Vec<&[u8]> = payload.chunks(65535).collect();
        if chunks.is_empty() {
            out.push(0x01);
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(&0xFFFFu16.to_le_bytes());
            return out;
        }
        for (index, chunk) in chunks.iter().enumerate() {
            out.push(if index + 1 == chunks.len() {
                0x01
            } else {
                0x00
            });
            let len = chunk.len() as u16;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(&(!len).to_le_bytes());
            out.extend_from_slice(chunk);
        }
        out
    }

    /// Adler-32（zlib 的校验尾）。
    fn adler32(bytes: &[u8]) -> u32 {
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for &byte in bytes {
            a = (a + u32::from(byte)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// 把负载包成合法 zlib 流（2 字节头 + stored DEFLATE + 4 字节 Adler-32）。
    fn zlib_stored(payload: &[u8]) -> Vec<u8> {
        // 0x7801：CM=deflate、FLEVEL=0，且 0x7801 % 31 == 0（zlib 的 FCHECK 约束）
        let mut out = vec![0x78, 0x01];
        out.extend_from_slice(&deflate_stored(payload));
        out.extend_from_slice(&adler32(payload).to_be_bytes());
        out
    }

    /// 拼一个「**声明撒谎**」的 V7 PAK：负载真实解压出 `payload.len()` 字节，
    /// 但文件表里写的是 `declared`。
    ///
    /// 上游 `decompress_zlib` 只把 `uncompressed_size` 当容量提示
    /// （`Vec::with_capacity`），实际返回的是完整解压结果 —— 所以声明小、
    /// 内容大是可能的，这正是「实际写入字节数」那一层存在的理由。
    fn forge_v7_pak_with_lying_entry(name: &str, payload: &[u8], declared: u32) -> Vec<u8> {
        let compressed = zlib_stored(payload);
        let offset = (V7_HEADER_SIZE + V7_ENTRY_SIZE) as u32;
        let entry = v7_entry(name, offset, compressed.len() as u32, declared);
        forge_v7_pak(&[entry], &compressed)
    }

    /// **生产入口**必须真的用默认限制：这是「防线被静默摘掉」的哨兵。
    ///
    /// 独立验证者 T2 的变异实验：把 `extract_package_files` 改成注入
    /// `ExtractLimits { u64::MAX, u64::MAX, usize::MAX }`（等于修复前的无上限），
    /// 全量 492 条测试**全部仍然绿** —— 因为 `extract_limits_default_stays_bounded`
    /// 只钉常量、`pak_expansion_is_capped_like_the_zip_path` 只钉注入上限的机制，
    /// 没有一条断言生产入口真的把默认限制传了下去。
    ///
    /// 这条走 `open_and_extract_in`（不注入任何 limits），用一个 V7 PAK 让 5 个条目
    /// 各声明 4 GiB（合计 20 GiB > 默认 16 GiB）触发**声明预检**：
    /// - 不需要真的解压任何数据、不需要真的占 16 GiB 磁盘（归档只有 1.4 KB）；
    /// - 预检在创建任何文件之前，所以断言「一个文件都没写」。
    ///
    /// 变异实验：把生产入口改成 `u64::MAX` ⇒ 本用例红（解包「成功」）；
    /// 恢复 ⇒ 绿。
    #[test]
    fn production_entry_enforces_the_default_declared_total_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let pak = tmp.path().join("declares-too-much.pak");
        let entries: Vec<Vec<u8>> = (0..5)
            .map(|index| v7_entry_declaring_4gib(&format!("bomb{index}.bin")))
            .collect();
        let bytes = forge_v7_pak(&entries, &[]);
        assert!(
            bytes.len() < 4096,
            "样本必须很小（声明撒谎，不占磁盘）: {} 字节",
            bytes.len()
        );
        std::fs::write(&pak, &bytes).unwrap();

        // 走生产入口，不注入 limits；工作目录名以 bg3-translate- 开头 ⇒ 被直接用
        let work = tmp.path().join("bg3-translate-probe");
        // 不写 `.unwrap_err()`：万一防线被摘掉，`Ok` 里的几百个条目会刷满输出
        let err = match open_and_extract_in(pak.to_str().unwrap(), &work) {
            Ok((_, files)) => panic!("默认声明总量上限没生效：{} 个条目全被解出来了", files.len()),
            Err(err) => err,
        };

        assert_eq!(err.code(), "pak", "必须报 pak 错误: {err}");
        assert!(
            err.to_string().contains("总量"),
            "必须是「声明总量」这一层拦下来的: {err}"
        );
        assert!(
            walk_files(&unpacked_dir(&work)).is_empty(),
            "预检必须发生在写盘之前，实际写了: {:?}",
            walk_files(&unpacked_dir(&work))
        );
    }

    /// 默认的**条目数**上限同样要由生产入口兜住（与总量是两条独立的默认值）。
    ///
    /// 20 万零 1 个条目、每个 272 字节明文表项 ≈ 54 MB 的归档，条目本身都不带数据，
    /// 所以磁盘与时间成本都可接受；断言解包被拒且一个文件都没写。
    #[test]
    fn production_entry_enforces_the_default_entry_count_limit() {
        let tmp = tempfile::tempdir().unwrap();
        let pak = tmp.path().join("too-many-entries.pak");
        let count = ExtractLimits::default().max_entries + 1;
        // 直接拼明文文件表（V7），不必给每个条目真的准备数据
        let mut entries = Vec::with_capacity(count * V7_ENTRY_SIZE);
        for index in 0..count {
            entries.extend_from_slice(&v7_entry(&format!("f{index}.txt"), 0, 0, 0));
        }
        let mut bytes = Vec::with_capacity(V7_HEADER_SIZE + entries.len());
        bytes.extend_from_slice(&7u32.to_le_bytes());
        bytes.extend_from_slice(&0u32.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&((count * V7_ENTRY_SIZE) as u32).to_le_bytes());
        bytes.push(1);
        bytes.extend_from_slice(&(count as u32).to_le_bytes());
        bytes.extend_from_slice(&entries);
        std::fs::write(&pak, &bytes).unwrap();

        let work = tmp.path().join("bg3-translate-probe");
        // 同上：不写 `.unwrap_err()`，否则防线一被摘掉就会打印 20 万条条目
        let err = match open_and_extract_in(pak.to_str().unwrap(), &work) {
            Ok((_, files)) => panic!(
                "默认条目数上限没生效：{} 个条目全被解出来了（应当报错）",
                files.len()
            ),
            Err(err) => err,
        };

        assert_eq!(err.code(), "pak", "必须报 pak 错误: {err}");
        assert!(
            err.to_string().contains("条目数"),
            "必须是「条目数」这一层拦下来的: {err}"
        );
        assert!(
            walk_files(&unpacked_dir(&work)).is_empty(),
            "不该写出任何文件"
        );
    }

    /// 「声明撒谎」必须被**实际解压字节数**那一层拦住。
    ///
    /// 这条覆盖的是兜底层，**不是**生产默认值（生产默认的覆盖见上面两条）。
    /// V7 条目声明只解压 4 KiB、真实内容是 4 MiB：声明层全部放行，只有实际计数能拦住。
    /// 注入小上限是因为默认单条目上限是 6 GiB，真造一份 6 GiB 的样本不现实。
    #[test]
    fn lying_declared_size_is_caught_by_the_actual_bytes_layer() {
        let tmp = tempfile::tempdir().unwrap();
        let payload = vec![b'A'; 4 << 20];
        let declared = 4096u32;
        let pak = tmp.path().join("liar.pak");
        std::fs::write(
            &pak,
            forge_v7_pak_with_lying_entry("liar.bin", &payload, declared),
        )
        .unwrap();

        let pkg = Package::open(&pak).unwrap();
        assert_eq!(
            pkg.files()[0].size(),
            u64::from(declared),
            "夹具的前提：文件表里的声明必须是 4 KiB（小于注入的 1 MiB 上限）"
        );

        // ① 单条目层：声明 4 KiB 放行，实际 4 MiB 超 1 MiB
        let dir = tmp.path().join("out1");
        std::fs::create_dir_all(&dir).unwrap();
        let err = extract_package_files_limited(
            &pkg,
            &dir,
            ExtractLimits {
                max_entry_bytes: 1 << 20,
                max_total_bytes: 64 << 20,
                max_entries: 16,
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "pak", "{err}");
        assert!(
            err.to_string().contains("实际解压") && err.to_string().contains("声明大小与内容不符"),
            "必须是「实际写入」这一层拦下来的: {err}"
        );
        assert!(
            !err.to_string().contains("声明解压后"),
            "声明层本该放行（4 KiB < 1 MiB）: {err}"
        );
        assert!(walk_files(&dir).is_empty(), "超限时一个字节都不该落盘");

        // ② 总量层：单条目 8 MiB 放行、总量 1 MiB 被实际 4 MiB 突破
        let dir2 = tmp.path().join("out2");
        std::fs::create_dir_all(&dir2).unwrap();
        let err = extract_package_files_limited(
            &pkg,
            &dir2,
            ExtractLimits {
                max_entry_bytes: 8 << 20,
                max_total_bytes: 1 << 20,
                max_entries: 16,
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "pak", "{err}");
        assert!(
            err.to_string().contains("PAK 解压总量超过上限"),
            "必须由「实际解压总量」这一层拦下来: {err}"
        );
        assert!(walk_files(&dir2).is_empty(), "超限时一个字节都不该落盘");

        // ③ 正向对照：把上限放开到真实体量之上，内容必须原样解出
        let dir3 = tmp.path().join("out3");
        std::fs::create_dir_all(&dir3).unwrap();
        let files = extract_package_files_limited(&pkg, &dir3, ExtractLimits::default()).unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(
            std::fs::read(dir3.join("liar.bin")).unwrap(),
            payload,
            "夹具必须真的能解出 4 MiB（否则上面的红是假红）"
        );
    }
}
