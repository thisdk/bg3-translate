//! 应用信息命令。

use bg3_translate_core::config;
use bg3_translate_core::{Result, VERSION};
use serde::Serialize;

use crate::commands::blocking;

/// 运行时信息，供「关于」面板展示与排查问题。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppInfo {
    /// 应用版本
    pub version: String,
    /// 配置与术语表所在目录
    pub data_dir: String,
    /// 目录来源（环境变量 / 便携目录 / 系统配置目录）
    pub data_dir_source: String,
    /// 是否处于便携模式（配置写在 exe 同级）
    pub portable: bool,
}

/// 返回版本与数据目录信息。
#[tauri::command]
pub async fn app_info() -> Result<AppInfo> {
    blocking("读取应用信息", || {
        let dir = config::data_dir()?;
        Ok(AppInfo {
            version: VERSION.to_string(),
            data_dir: dir.path.to_string_lossy().into_owned(),
            data_dir_source: dir.source.as_str().to_string(),
            portable: matches!(dir.source, config::DataDirSource::Portable),
        })
    })
    .await
}
