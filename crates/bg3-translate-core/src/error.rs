//! 统一错误类型。
//!
//! `AppError` 同时用于核心逻辑与 Tauri 命令返回值：Tauri 要求错误类型实现
//! `Serialize`，这里序列化为一条面向用户的中文字符串（前端直接展示）。

use serde::{Serialize, Serializer};

/// 应用统一错误。
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("文件读写失败: {0}")]
    Io(#[from] std::io::Error),

    #[error("PAK 处理错误: {0}")]
    Pak(String),

    #[error("PAK 库错误: {0}")]
    PakLib(#[from] bg3rustpaklib::PakError),

    #[error("LOCA 处理错误: {0}")]
    Loca(#[from] bg3rustpaklib::loca::LocaError),

    #[error("XML 解析错误: {0}")]
    Xml(String),

    #[error("JSON 处理错误: {0}")]
    Json(#[from] serde_json::Error),

    #[error("大模型调用错误: {0}")]
    Llm(String),

    #[error("配置错误: {0}")]
    Config(String),

    #[error("翻译已取消")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl AppError {
    /// 机器可读的错误类别，便于前端做差异化处理（如「取消」不弹错误）。
    pub const fn code(&self) -> &'static str {
        match self {
            AppError::Io(_) => "io",
            AppError::Pak(_) | AppError::PakLib(_) => "pak",
            AppError::Loca(_) => "loca",
            AppError::Xml(_) => "xml",
            AppError::Json(_) => "json",
            AppError::Llm(_) => "llm",
            AppError::Config(_) => "config",
            AppError::Cancelled => "cancelled",
            AppError::Other(_) => "other",
        }
    }

    /// 该错误是否只是「用户主动取消」。
    pub const fn is_cancelled(&self) -> bool {
        matches!(self, AppError::Cancelled)
    }

    pub fn pak(message: impl Into<String>) -> Self {
        AppError::Pak(message.into())
    }

    pub fn xml(message: impl Into<String>) -> Self {
        AppError::Xml(message.into())
    }

    pub fn llm(message: impl Into<String>) -> Self {
        AppError::Llm(message.into())
    }

    pub fn config(message: impl Into<String>) -> Self {
        AppError::Config(message.into())
    }

    pub fn other(message: impl Into<String>) -> Self {
        AppError::Other(message.into())
    }
}

// Tauri 命令的错误返回值需要 Serialize：序列化成用户可读的字符串。
impl Serialize for AppError {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

pub type Result<T> = std::result::Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages_are_localized_and_prefixed() {
        assert_eq!(
            AppError::Pak("坏文件".into()).to_string(),
            "PAK 处理错误: 坏文件"
        );
        assert_eq!(
            AppError::Config("缺 key".into()).to_string(),
            "配置错误: 缺 key"
        );
        assert_eq!(AppError::Cancelled.to_string(), "翻译已取消");
        assert_eq!(AppError::Other("随便".into()).to_string(), "随便");
    }

    #[test]
    fn io_errors_convert_via_question_mark() {
        fn read_missing() -> Result<String> {
            Ok(std::fs::read_to_string("/definitely/not/here/xyz")?)
        }
        let err = read_missing().unwrap_err();
        assert_eq!(err.code(), "io");
        assert!(err.to_string().starts_with("文件读写失败"));
    }

    #[test]
    fn error_codes_and_cancellation_flag() {
        assert_eq!(AppError::pak("x").code(), "pak");
        assert_eq!(AppError::xml("x").code(), "xml");
        assert_eq!(AppError::llm("x").code(), "llm");
        assert_eq!(AppError::config("x").code(), "config");
        assert_eq!(AppError::other("x").code(), "other");
        assert!(AppError::Cancelled.is_cancelled());
        assert!(!AppError::llm("x").is_cancelled());
    }

    #[test]
    fn serializes_to_plain_string_for_frontend() {
        let json = serde_json::to_string(&AppError::Config("没配 key".into())).unwrap();
        assert_eq!(json, "\"配置错误: 没配 key\"");
    }

    #[test]
    fn json_errors_are_wrapped() {
        let err: AppError = serde_json::from_str::<serde_json::Value>("{oops")
            .unwrap_err()
            .into();
        assert_eq!(err.code(), "json");
    }
}
