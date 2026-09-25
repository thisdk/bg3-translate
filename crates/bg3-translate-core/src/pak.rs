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
                if is_reserved_windows_component(&part) {
                    return Err(AppError::pak(format!(
                        "非法归档路径（Windows 保留设备名）: {file_name}"
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
pub fn walk_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out.sort();
    out
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
    let zip_extract = work_dir.join("zip_contents");
    std::fs::create_dir_all(&zip_extract)?;

    let file = std::fs::File::open(zip_path)?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|e| AppError::pak(format!("读取 zip 失败: {e}")))?;

    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| AppError::pak(format!("读取 zip 条目失败: {e}")))?;

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
        std::io::copy(&mut entry, &mut output)?;
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

    #[test]
    fn open_and_extract_rejects_missing_file() {
        let err = open_and_extract("/nope/missing.pak").unwrap_err();
        assert_eq!(err.code(), "config");
    }
}
