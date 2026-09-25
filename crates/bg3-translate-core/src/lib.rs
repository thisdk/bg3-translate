//! # bg3-translate-core
//!
//! 《博德之门 3》MOD 汉化工具的核心逻辑，**不依赖 Tauri / 任何 GUI 系统库**。
//!
//! 拆成独立 crate 的原因：
//! 1. 可以在没有 webkit2gtk / dbus 的机器上 `cargo test`（CI 也能并行跑）
//! 2. 业务逻辑与桌面壳解耦，未来做 CLI 或 Web 版可以直接复用
//! 3. 单元测试不需要启动整个 Tauri 应用
//!
//! 模块职责：
//! - [`error`]：统一错误类型
//! - [`types`]：跨 IPC 边界的数据结构（与 `src/lib/types.ts` 一一对应）
//! - [`config`]：便携/系统数据目录解析与设置持久化
//! - [`pak`]：PAK / ZIP 解包与重打包
//! - [`formats`]：`contentList` XML / LSX / LOCA 三种格式与统一条目的互转
//! - [`glossary`]：BG3 官方与自定义术语表 + 文本命中匹配
//! - [`translation`]：LLM 流式翻译引擎
//!
//! 注意这里**刻意没有**本地化路径改写（`Localization/English/...` →
//! `Localization/Chinese/...`）之类的逻辑：写回目标文件的选择由前端
//! `src/lib/localization.ts` 单独负责（它还要按语言优先级在多个源文件之间择优），
//! 后端只按前端给的文件名写盘。同一份规则只保留一处实现，避免两边慢慢走偏。

pub mod config;
pub mod error;
pub mod formats;
pub mod glossary;
pub mod pak;
pub mod translation;
pub mod types;

pub use error::{AppError, Result};

/// 应用版本（与 `Cargo.toml` / `tauri.conf.json` 保持一致）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_exposed() {
        assert!(!VERSION.is_empty());
    }
}
