//! 应用共享状态。
//!
//! 这里只放「跨命令需要共享的东西」，业务逻辑一律在 `bg3-translate-core`。

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard};

use bg3_translate_core::translation::CancelToken;
use bg3_translate_core::types::LlmSettings;

/// Tauri 托管的全局状态。
pub struct AppState {
    /// LLM 设置的内存缓存（避免每次翻译都读磁盘）
    pub settings: Mutex<Option<LlmSettings>>,
    /// 当前翻译任务的取消令牌
    pub cancel: CancelToken,
    /// 当前 MOD 的工作目录；打开新 MOD 时会清理上一个
    pub work_dir: Mutex<Option<PathBuf>>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            settings: Mutex::new(None),
            cancel: CancelToken::new(),
            work_dir: Mutex::new(None),
        }
    }
}

impl AppState {
    /// 读取设置，必要时加锁；锁中毒时返回一条可读错误而不是 panic。
    pub fn settings_guard(&self) -> Result<std::sync::MutexGuard<'_, Option<LlmSettings>>, String> {
        self.settings
            .lock()
            .map_err(|err| format!("设置锁已损坏: {err}"))
    }

    /// 取工作目录锁；锁中毒（持锁线程 panic）时取回内部值继续用。
    ///
    /// 这里**不能**沿用 `.lock().ok()?` 那种「中毒即返回 None」的写法：`open_mod`
    /// 会照样报成功，但工作目录记不下来 —— 之后每条命令都提示「尚未打开 MOD」，
    /// 重新打开也没用，已解包的临时目录还会越堆越多（再没人记得它们）。
    /// 锁里只有一次 `replace` / `take` / `clone`，不存在被 panic 留在中间态的可能，
    /// 所以取回内部值是安全的（与术语表写锁 `glossary_write_guard` 同一处置）。
    fn work_dir_guard(&self) -> MutexGuard<'_, Option<PathBuf>> {
        match self.work_dir.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    /// 记录当前工作目录，并返回被替换掉的旧目录（供调用方清理）。
    pub fn replace_work_dir(&self, next: PathBuf) -> Option<PathBuf> {
        self.work_dir_guard().replace(next)
    }

    /// 取出并清空当前工作目录。
    pub fn take_work_dir(&self) -> Option<PathBuf> {
        self.work_dir_guard().take()
    }

    /// 读取当前工作目录的快照（不清空），供命令层校验前端传来的 `work_dir`。
    pub fn current_work_dir(&self) -> Option<PathBuf> {
        self.work_dir_guard().clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 锁中毒（持锁线程 panic）不能让工作目录**静默**变成「没打开过 MOD」。
    ///
    /// 旧的 `.lock().ok()?` 在中毒后一律返回 `None`：`open_mod` 照样报成功，但目录
    /// 记不下来 —— 之后每条命令都提示「尚未打开 MOD」，重新打开也没用，解包出来的
    /// 临时目录还会越堆越多（没人再记得它们）。这里按本模块对设置锁的原则处理
    /// （「锁中毒时返回一条可读错误而不是 panic」）：取回内部值继续用。锁里只有
    /// 一次 `replace` / `take` / `clone`，不存在被 panic 留在中间态的可能。
    #[test]
    fn poisoned_work_dir_lock_does_not_silently_forget_the_directory() {
        let state = AppState::default();
        let dir = PathBuf::from("/tmp/bg3-translate-poison-probe");

        // 制造中毒：持锁时 panic（与真实的中毒路径一致）
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = state.work_dir.lock().expect("首次加锁");
            panic!("故意 panic，让 work_dir 的锁中毒");
        }));
        assert!(poisoned.is_err(), "用例前提：panic 必须被捕获");
        assert!(state.work_dir.is_poisoned(), "用例前提：锁必须已中毒");

        // 首次记录：没有旧目录可返回（None 是正常语义，不是中毒的信号）
        assert_eq!(
            state.replace_work_dir(dir.clone()),
            None,
            "首次记录时没有旧目录可返回"
        );
        assert_eq!(
            state.current_work_dir(),
            Some(dir.clone()),
            "中毒不能让已记录的工作目录消失"
        );
        assert_eq!(state.take_work_dir(), Some(dir), "中毒后仍要能取出并清理");
        assert_eq!(state.current_work_dir(), None);
    }
}
