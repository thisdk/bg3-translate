//! 应用共享状态。
//!
//! 这里只放「跨命令需要共享的东西」，业务逻辑一律在 `bg3-translate-core`。

use std::path::PathBuf;
use std::sync::Mutex;

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

    /// 记录当前工作目录，并返回被替换掉的旧目录（供调用方清理）。
    pub fn replace_work_dir(&self, next: PathBuf) -> Option<PathBuf> {
        let mut guard = self.work_dir.lock().ok()?;
        guard.replace(next)
    }

    /// 取出并清空当前工作目录。
    pub fn take_work_dir(&self) -> Option<PathBuf> {
        self.work_dir.lock().ok()?.take()
    }

    /// 读取当前工作目录的快照（不清空），供命令层校验前端传来的 `work_dir`。
    pub fn current_work_dir(&self) -> Option<PathBuf> {
        self.work_dir.lock().ok()?.clone()
    }
}
