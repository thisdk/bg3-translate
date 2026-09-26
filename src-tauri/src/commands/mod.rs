//! Tauri 命令层：把前端 `invoke` 映射到 `bg3-translate-core`。
//!
//! 这一层刻意做得很薄——参数归一化、进程内状态、事件桥接，
//! 真正的逻辑都在 core 里（可单独测试、可复用）。

mod app;
mod archive;
mod entries;
mod settings;
mod terminology;
mod translate;

pub use app::*;
pub use archive::*;
pub use entries::*;
pub use settings::*;
pub use terminology::*;
pub use translate::*;

use std::path::{Path, PathBuf};

use bg3_translate_core::error::AppError;
use bg3_translate_core::{Result as CoreResult, pak};

/// 把阻塞型任务丢到 tokio 的阻塞线程池，避免卡住 async 运行时。
pub(crate) async fn blocking<T, F>(what: &str, task: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, AppError> + Send + 'static,
{
    tokio::task::spawn_blocking(task)
        .await
        .map_err(|err| AppError::other(format!("{what}任务异常终止: {err}")))?
}

/// 校验前端回传的 `work_dir`，并返回**记录值**作为读写根目录。
///
/// `work_dir` 是**前端可控字符串**，命令层直接拿它拼路径等于把磁盘根目录交出去：
/// 只校验文件名的话，`work_dir = "/"` 就能让 `file_name = "home/<用户>/…"` 落盘。
/// 因此要求它必须等于 `open_mod` 记录的那个目录。
///
/// 返回记录值而不是请求值是有意的纵深防御：即使 [`is_same_dir`] 出现误判，
/// 落盘根目录也不会变成前端这次指定的目录。
pub(crate) fn checked_work_root(recorded: Option<&Path>, requested: &str) -> CoreResult<PathBuf> {
    let Some(recorded) = recorded else {
        return Err(AppError::config("尚未打开 MOD：请先打开一个 .pak 文件"));
    };
    if !is_same_dir(recorded, Path::new(requested)) {
        return Err(AppError::config(
            "工作目录已失效（可能已经打开或关闭了其它 MOD），请重新打开当前 MOD",
        ));
    }
    Ok(recorded.to_path_buf())
}

/// 把前端传来的「PAK 内路径」解析成 `root/unpacked/` 下的**受校验**磁盘路径。
///
/// `pak::resolve_disk_path` 只是纯 `join`：`../../x.loca`、`/tmp/x.lsx`、
/// `a\..\..\x.lsx` 都会逃出工作目录。读写前必须先过
/// [`pak::safe_output_path`]（拒绝 `..`、绝对路径、盘符、`:`、Windows 保留名）。
pub(crate) fn unpacked_path(root: &Path, file_name: &str) -> CoreResult<PathBuf> {
    pak::safe_output_path(&pak::unpacked_dir(root), file_name)
}

/// 判断两个路径是否指向同一个目录（用于校验前端回传的 `work_dir`）。
///
/// 1. 任一侧为空 → 否；
/// 2. 两侧都真实存在时用 `canonicalize` 比较：`.`/`..`、尾随分隔符、
///    `\` 与 `/`、符号链接、Windows `\\?\` 前缀一次解决；
/// 3. 任一侧不存在时退回词法比较（见 [`lexical_parts`]），保守判否。
///
/// 大小写：Windows 按不敏感比较，其它平台按敏感比较——与各自文件系统的
/// 实际行为一致（Linux 上 `/tmp/Work` 与 `/tmp/work` 是两个目录）。
pub(crate) fn is_same_dir(a: &Path, b: &Path) -> bool {
    if a.as_os_str().is_empty() || b.as_os_str().is_empty() {
        return false;
    }
    if let (Ok(a), Ok(b)) = (a.canonicalize(), b.canonicalize()) {
        return a == b;
    }
    match (lexical_parts(a), lexical_parts(b)) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    }
}

/// 词法归一化：统一 `/` 与 `\`、丢掉空段与 `.`、`..` 做栈式回退，
/// 返回逐段比较用的片段。以下情况返回 `None`（一律判否）：
/// - `..` 越过了根；
/// - 路径含非法 UTF-8（`to_string_lossy` 会产生 `U+FFFD`）——
///   IPC 层本来也只能传合法 UTF-8，这里避免两个不同的非法路径被 lossy 成同一个。
fn lexical_parts(path: &Path) -> Option<Vec<String>> {
    let text = path.to_string_lossy();
    if text.contains('\u{FFFD}') {
        return None;
    }
    let mut parts: Vec<String> = Vec::new();
    if text.starts_with(['/', '\\']) {
        // 保留「绝对路径」这一位，避免相对路径与绝对路径的尾段撞车
        parts.push("/".to_string());
    }
    for raw in text.split(['/', '\\']) {
        match raw {
            "" | "." => continue,
            ".." => {
                parts.pop()?;
            }
            other => parts.push(if cfg!(windows) {
                other.to_lowercase()
            } else {
                other.to_string()
            }),
        }
    }
    Some(parts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "bg3-translate-samedir-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("创建测试目录");
        dir
    }

    /// 前端原样回传 `open_mod` 给的 workDir：必须判为同一个目录。
    #[test]
    fn same_dir_accepts_exact_echo_and_trailing_separator() {
        let dir = temp_dir("echo");
        let canonical = dir.canonicalize().expect("canonicalize");
        assert!(is_same_dir(
            &canonical,
            Path::new(canonical.to_string_lossy().as_ref())
        ));

        // 尾随分隔符 / `./` 片段 / 重复分隔符：都还是同一个目录
        let with_slash = format!("{}/", canonical.display());
        let with_dot = format!("{}/./", canonical.display());
        let with_double = format!("{}//", canonical.display());
        for variant in [&with_slash, &with_dot, &with_double] {
            assert!(
                is_same_dir(&canonical, Path::new(variant)),
                "{variant} 应判为同一目录"
            );
        }

        // 反斜杠分隔符形态（Windows 回传 / 混用）：真机上存在时同样通过
        let backslash = canonical.to_string_lossy().replace('/', "\\");
        assert!(is_same_dir(&canonical, Path::new(&backslash)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_dir_rejects_different_and_empty() {
        let a = temp_dir("a");
        let b = temp_dir("b");
        assert!(!is_same_dir(&a, &b));
        assert!(!is_same_dir(&a, Path::new("")));
        assert!(!is_same_dir(Path::new(""), &b));
        // 子目录不是同一个目录
        let child = a.join("unpacked");
        std::fs::create_dir_all(&child).expect("建子目录");
        assert!(!is_same_dir(&a, &child));
        // 父目录不是同一个目录（`..` 不能把校验绕过去）
        if let Some(parent) = a.parent() {
            assert!(!is_same_dir(&a, parent));
        }
        let _ = std::fs::remove_dir_all(&a);
        let _ = std::fs::remove_dir_all(&b);
    }

    /// 大小写：Linux 敏感（本机行为），Windows 不敏感。
    #[test]
    fn same_dir_case_sensitivity_matches_platform() {
        let dir = temp_dir("case");
        let upper = dir.to_string_lossy().to_uppercase();
        if cfg!(windows) {
            assert!(is_same_dir(&dir, Path::new(&upper)));
        } else {
            assert!(
                !is_same_dir(&dir, Path::new(&upper)),
                "Linux 上大小写不同就是不同目录"
            );
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 不存在的路径：走词法回退，同样能识别尾随分隔符与 `./`。
    #[test]
    fn same_dir_falls_back_to_lexical_for_missing_paths() {
        let ghost = temp_dir("ghost");
        let _ = std::fs::remove_dir_all(&ghost);
        assert!(!ghost.exists(), "用例前提：目录必须不存在");
        assert!(is_same_dir(
            &ghost,
            Path::new(&format!("{}/./", ghost.display()))
        ));
        assert!(!is_same_dir(
            &ghost,
            Path::new(&format!("{}/sub", ghost.display()))
        ));
        // `..` 越过根 → 保守判否
        assert!(!is_same_dir(&ghost, Path::new("/..")));
        // 相对路径与绝对路径不能撞车：把绝对路径的**根**去掉之后必须判否。
        //
        // 不要用 `trim_start_matches('/')` 来构造这个相对路径：Windows 的绝对路径以
        // 盘符开头（`C:\Users\...`），那个调用是**空操作**，两边会退化成同一个字符串，
        // 断言在 Windows 上恒假 —— 2026-09-26 的 Windows CI 就是这么红的（v1.1.0 修）。
        // 用 `components()` 去根，在 Unix（`/tmp/x` → `tmp/x`）与 Windows
        // （`C:\a\x` → `a\x`）上都得到真正的相对路径。
        let without_root = strip_root(&ghost);
        assert!(
            without_root.is_relative(),
            "用例前提：去掉根之后必须是相对路径，实际 {}",
            without_root.display()
        );
        assert!(
            !is_same_dir(&ghost, &without_root),
            "相对路径与绝对路径不能撞车: {} vs {}",
            ghost.display(),
            without_root.display()
        );
    }

    /// 去掉路径的根（Unix 的 `/`、Windows 的 `C:\`），返回相对路径。
    fn strip_root(path: &Path) -> PathBuf {
        let mut out = PathBuf::new();
        for component in path.components() {
            match component {
                std::path::Component::Prefix(_) | std::path::Component::RootDir => continue,
                other => out.push(other),
            }
        }
        out
    }

    /// 「绝对路径 vs 相对路径不能撞车」这条不变量，用**字面量**直接钉住。
    ///
    /// 为什么单独写一条：上面那个用例靠 `std::env::temp_dir()` 造路径，构造出的
    /// 「相对版本」是否真的相对**取决于平台**（Windows 的绝对路径以盘符开头，
    /// 早先那版用 `trim_start_matches('/')` 去根在 Windows 上是空操作，断言恒假）。
    /// 这里改用与宿主无关的输入：`/tmp/x` 在两平台都是绝对路径，`tmp/x` 都是相对路径；
    /// `C:\a\x` 与 `a\x` 的差异靠保留下来的 `C:` 分量区分，同样与宿主无关。
    #[test]
    fn lexical_parts_keep_absolute_and_relative_apart() {
        let cases = [
            ("/tmp/x", "tmp/x", "Unix 式根 `/`"),
            (r"C:\a\x", r"a\x", "Windows 式根 `C:`"),
        ];
        for (absolute, relative, label) in cases {
            let absolute_parts = lexical_parts(Path::new(absolute)).expect("词法解析");
            let relative_parts = lexical_parts(Path::new(relative)).expect("词法解析");
            assert_ne!(
                absolute_parts, relative_parts,
                "{label}：根必须参与比较，否则相对路径能冒充绝对路径"
            );
        }
    }

    /// 非 UTF-8：词法回退必须保守判否，不能把两个不同的非法路径 lossy 成同一个。
    #[cfg(unix)]
    #[test]
    fn same_dir_rejects_non_utf8_paths() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt;

        let a = PathBuf::from(OsStr::from_bytes(b"/tmp/bg3-non-utf8-\xff-a"));
        let b = PathBuf::from(OsStr::from_bytes(b"/tmp/bg3-non-utf8-\xfe-a"));
        assert!(a.to_string_lossy().contains('\u{FFFD}'));
        // 两者都不存在 → 词法回退 → 非法 UTF-8 一律判否（即使 lossy 后看起来相同）
        assert!(!is_same_dir(&a, &b));
        assert!(lexical_parts(&a).is_none());
    }

    /// 校验入口：返回的是**记录值**，不是前端这次传进来的字符串。
    #[test]
    fn checked_work_root_returns_recorded_path() {
        let dir = temp_dir("root");
        let recorded = dir.canonicalize().expect("canonicalize");
        let requested = format!("{}/", recorded.display());
        let root = checked_work_root(Some(&recorded), &requested).expect("回传同一目录应通过");
        assert_eq!(root, recorded);

        assert!(checked_work_root(None, &requested).is_err(), "未打开 MOD");
        let other = temp_dir("root-other");
        assert!(
            checked_work_root(Some(&recorded), &other.to_string_lossy()).is_err(),
            "换了目录必须拒绝"
        );
        assert!(
            checked_work_root(Some(&recorded), "").is_err(),
            "空路径必须拒绝"
        );
        assert!(
            checked_work_root(Some(&recorded), "/").is_err(),
            "根目录必须拒绝"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&other);
    }

    /// `unpacked_path` 只在 `root/unpacked` 内拼路径。
    #[test]
    fn unpacked_path_stays_under_root() {
        let dir = temp_dir("unpacked");
        let root = dir.canonicalize().expect("canonicalize");
        assert_eq!(
            unpacked_path(&root, "Mods/x/localization/a.xml").expect("合法路径"),
            root.join("unpacked/Mods/x/localization/a.xml")
        );
        for bad in [
            "../a.lsx",
            "/tmp/a.lsx",
            r"..\..\a.lsx",
            "NUL.lsx",
            "a:b.lsx",
        ] {
            assert!(unpacked_path(&root, bad).is_err(), "{bad} 应被拒绝");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
