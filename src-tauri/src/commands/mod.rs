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

use bg3_translate_core::error::AppError;

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
