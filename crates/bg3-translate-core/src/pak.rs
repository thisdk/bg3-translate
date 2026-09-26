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

fn to_pak_file(file: &PackagedFile) -> PakFile {
    let name = normalize_entry_name(file.name());
    PakFile {
        kind: classify_file(&name),
        language: detect_language_from_path(&name).map(String::from),
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

        // ② 再按**实际写入**字节数兜底：声明大小是可以撒谎的
        let budget = limits.max_total_bytes - written_total;
        let mut limited = std::io::Read::take(&mut entry, budget.saturating_add(1));
        let copied = std::io::copy(&mut limited, &mut output)?;
        drop(output);
        if copied > budget {
            // 及时收尾：失败时不要在工作目录里留下一个已经写了几个 GB 的文件
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
    let mut groups: BTreeMap<String, Vec<&PackagedFile>> = BTreeMap::new();
    for file in pkg.files() {
        groups
            .entry(normalize_entry_name(file.name()))
            .or_default()
            .push(file);
    }

    let mut files = Vec::with_capacity(groups.len());
    for (name, candidates) in groups {
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

    let builder = PackageBuilder::new()
        .priority(priority)
        .add_directory(&unpacked)
        .map_err(|e| AppError::pak(format!("读取待打包目录失败: {e}")))?;
    builder
        .build(output)
        .map_err(|e| AppError::pak(format!("打包失败: {e}")))?;

    Ok(())
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

    #[test]
    fn open_and_extract_rejects_missing_file() {
        let err = open_and_extract("/nope/missing.pak").unwrap_err();
        assert_eq!(err.code(), "config");
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
}
