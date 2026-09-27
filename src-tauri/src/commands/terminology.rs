//! 术语表命令。

use std::path::Path;
use std::sync::{Mutex, MutexGuard};

use bg3_translate_core::Result;
use bg3_translate_core::config::data_dir;
use bg3_translate_core::glossary::{Glossary, GlossaryEntry};

use crate::commands::blocking;

/// 术语表的**进程内写锁**。
///
/// 术语表命令大多是「读盘 → 改 → 写盘」的读改写（`add` / `update` / `delete`），
/// `reset` / `import` 则是整表覆盖。并发执行会互相覆盖：两者都读到旧表，
/// 后保存的把先保存的改动**静默丢掉**。
///
/// 前端并没有把所有入口都串起来：`GlossaryPanel` 的「保存」有 `saving` 守卫，
/// 但删除按钮只按**行**置忙（`busySource === t.source`），所以「先删 A 再删 B」
/// 两次调用可以同时在途；`reset` / `import` 更是没有任何守卫。
/// 命令层是唯一的公共入口，串行化必须放在这里。
static GLOSSARY_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// 取写锁。上一个任务 panic 造成的锁中毒不该让术语表功能永久不可用：
/// 取回内部值继续用（一致性由每次 `load` 重新读盘保证）。
fn glossary_write_guard() -> MutexGuard<'static, ()> {
    match GLOSSARY_WRITE_LOCK.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 所有术语表写命令都是「读盘 → 改 → 写盘」，统一走阻塞线程池 + 写锁。
async fn mutate<F>(what: &str, task: F) -> Result<Glossary>
where
    F: FnOnce(&mut Glossary) -> Result<()> + Send + 'static,
{
    blocking(what, move || {
        let dir = data_dir()?.path;
        mutate_in(&dir, task)
    })
    .await
}

/// [`mutate`] 的实现：目录显式传入，这样单测能直接验证「并发写不丢改动」。
fn mutate_in<F>(dir: &Path, task: F) -> Result<Glossary>
where
    F: FnOnce(&mut Glossary) -> Result<()>,
{
    // 写锁覆盖整个「读盘 → 改 → 写盘」：只锁 save 不够，两个写者会各自读到旧表。
    let _guard = glossary_write_guard();
    let mut glossary = Glossary::load_from(dir)?;
    task(&mut glossary)?;
    glossary.save_to(dir)?;
    Ok(glossary)
}

/// 读取术语表（不存在时用官方种子初始化）。
#[tauri::command]
pub async fn list_glossary() -> Result<Glossary> {
    blocking("读取术语表", Glossary::load).await
}

/// 新增或覆盖一条术语。
#[tauri::command]
pub async fn add_glossary_entry(entry: GlossaryEntry) -> Result<Glossary> {
    mutate("新增术语", move |glossary| glossary.add(entry)).await
}

/// 更新一条术语（按旧原文定位）。
#[tauri::command]
pub async fn update_glossary_entry(old_source: String, entry: GlossaryEntry) -> Result<Glossary> {
    mutate("更新术语", move |glossary| {
        glossary.update(&old_source, entry)
    })
    .await
}

/// 删除一条术语（官方条目不可删除）。
#[tauri::command]
pub async fn delete_glossary_entry(source: String) -> Result<Glossary> {
    mutate("删除术语", move |glossary| glossary.delete(&source)).await
}

/// 重置为内置官方种子。
#[tauri::command]
pub async fn reset_glossary() -> Result<Glossary> {
    // 整表覆盖：与增删改共用同一把写锁，避免「重置」与在途的编辑互相覆盖
    blocking("重置术语表", || {
        let _guard = glossary_write_guard();
        Glossary::reset()
    })
    .await
}

/// 导入游戏提取的完整术语表 JSON（自动清洗噪音条目）。
#[tauri::command]
pub async fn import_glossary(json_str: String) -> Result<Glossary> {
    // 同上：导入是整表覆盖，必须和增删改串行
    blocking("导入术语表", move || {
        let _guard = glossary_write_guard();
        Glossary::import_json(&json_str)
    })
    .await
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use super::*;

    /// 极简临时目录（src-tauri 没有 tempfile 依赖，也不允许新增）。
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("bg3-translate-terms-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("创建测试目录");
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

    fn entry(source: &str) -> GlossaryEntry {
        GlossaryEntry {
            source: source.to_string(),
            target: format!("{source}的译文"),
            ..GlossaryEntry::default()
        }
    }

    /// 两个并发 mutation 必须串行，且都不能丢。
    ///
    /// 这就是前端「先删 A、再删 B」的形状：两次 `delete_glossary_entry`
    /// 同时在途，各自读盘得到同一份旧表，后保存的把先保存的改动静默覆盖 ——
    /// 用户看到删除成功、刷新后术语又回来了。`saving` 只挡住了「保存」，
    /// 删除按钮只按行置忙，所以命令层必须自己串行化。
    #[test]
    fn concurrent_mutations_are_serialized_and_lose_nothing() {
        let dir = TempDir::new("glossary-lock");
        // 先落一份空表：否则两个线程会同时走 `load_from` 的「文件不存在 → 写种子」
        // 分支，测的就不是「读改写会不会互相覆盖」了。
        Glossary { terms: Vec::new() }
            .save_to(dir.path())
            .expect("准备初始术语表");
        let inside = AtomicUsize::new(0);
        let max_inside = AtomicUsize::new(0);
        let barrier = std::sync::Barrier::new(2);

        std::thread::scope(|scope| {
            for source in ["TermA", "TermB"] {
                let inside = &inside;
                let max_inside = &max_inside;
                let barrier = &barrier;
                let dir = dir.path();
                scope.spawn(move || {
                    // 两个写者同时起跑（同步点在锁**外面**：锁里面再互等会死锁）。
                    // 没有写锁时它们会在微秒内先后进入闭包，而闭包要停留 50ms，
                    // 所以「重叠」是必然的，测试不靠运气变红。
                    barrier.wait();
                    mutate_in(dir, |glossary| {
                        let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                        max_inside.fetch_max(now, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(50));
                        inside.fetch_sub(1, Ordering::SeqCst);
                        glossary.add(entry(source))
                    })
                    .expect("并发 mutation 不应报错");
                });
            }
        });

        assert_eq!(
            max_inside.load(Ordering::SeqCst),
            1,
            "「读盘 → 改 → 写盘」必须在写锁内串行，否则会互相覆盖"
        );
        let final_glossary = Glossary::load_from(dir.path()).expect("读回最终术语表");
        for source in ["TermA", "TermB"] {
            assert!(
                final_glossary
                    .terms
                    .iter()
                    .any(|term| term.source == source),
                "并发写把 {source} 丢了：{:?}",
                final_glossary
                    .terms
                    .iter()
                    .map(|term| term.source.as_str())
                    .collect::<Vec<_>>()
            );
        }
    }
}
