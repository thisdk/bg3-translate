//! 前后端共享的数据结构。
//!
//! 这些结构通过 serde 直接跨 IPC 边界，字段名即 TypeScript 侧字段名，
//! 因此 **改动字段名等于破坏契约**，必须同步修改 `src/lib/types.ts`。

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

// ─────────────────────────────────────────────────────────────
// PAK 内文件类型
// ─────────────────────────────────────────────────────────────

/// PAK 内文件类型分类。
///
/// serde 表示为 kebab-case 字符串（`"localization-xml"`），前端用同名联合类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PakFileKind {
    /// `<contentList>` 本地化 XML
    LocalizationXml,
    /// `.loca` 二进制本地化
    LocalizationLoca,
    /// `.lsx` 元数据（Description / DisplayName 等字段）
    MetadataLsx,
    /// Lua 脚本
    ScriptLua,
    /// stats / 数据 txt
    DataTxt,
    /// 其他
    Other,
}

impl PakFileKind {
    /// 全部取值，顺序稳定，便于遍历/测试。
    pub const ALL: [PakFileKind; 6] = [
        PakFileKind::LocalizationXml,
        PakFileKind::LocalizationLoca,
        PakFileKind::MetadataLsx,
        PakFileKind::ScriptLua,
        PakFileKind::DataTxt,
        PakFileKind::Other,
    ];

    /// 是否可以从该类型文件中提取可翻译文本。
    pub const fn is_translatable(self) -> bool {
        matches!(
            self,
            PakFileKind::LocalizationXml | PakFileKind::LocalizationLoca | PakFileKind::MetadataLsx
        )
    }

    /// kebab-case 名称（与 serde 输出一致）。
    pub const fn as_str(self) -> &'static str {
        match self {
            PakFileKind::LocalizationXml => "localization-xml",
            PakFileKind::LocalizationLoca => "localization-loca",
            PakFileKind::MetadataLsx => "metadata-lsx",
            PakFileKind::ScriptLua => "script-lua",
            PakFileKind::DataTxt => "data-txt",
            PakFileKind::Other => "other",
        }
    }
}

impl fmt::Display for PakFileKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for PakFileKind {
    type Err = crate::error::AppError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == s)
            .ok_or_else(|| crate::error::AppError::Config(format!("未知文件类型: {s}")))
    }
}

// ─────────────────────────────────────────────────────────────
// PAK 文件元信息
// ─────────────────────────────────────────────────────────────

/// PAK 内单个文件元信息（传给前端）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PakFile {
    /// PAK 内路径，统一使用 `/` 分隔，如 `Localization/English/foo.xml`
    pub name: String,
    /// 解压后字节大小
    pub size: u64,
    /// 文件类型分类
    pub kind: PakFileKind,
    /// 所属 BG3 语言（仅本地化文件有），如 `English` / `Polish`
    pub language: Option<String>,
}

impl PakFile {
    /// 文件名（不含目录）。
    pub fn base_name(&self) -> &str {
        self.name.rsplit('/').next().unwrap_or(&self.name)
    }
}

// ─────────────────────────────────────────────────────────────
// 翻译条目
// ─────────────────────────────────────────────────────────────

/// 条目翻译状态。serde 表示为小写字符串，与前端联合类型一致。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TranslationStatus {
    /// 待翻译
    #[default]
    Pending,
    /// 翻译中
    Translating,
    /// 已由模型翻译
    Translated,
    /// 人工编辑过
    Edited,
    /// 出错
    Error,
}

impl TranslationStatus {
    pub const ALL: [TranslationStatus; 5] = [
        TranslationStatus::Pending,
        TranslationStatus::Translating,
        TranslationStatus::Translated,
        TranslationStatus::Edited,
        TranslationStatus::Error,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            TranslationStatus::Pending => "pending",
            TranslationStatus::Translating => "translating",
            TranslationStatus::Translated => "translated",
            TranslationStatus::Edited => "edited",
            TranslationStatus::Error => "error",
        }
    }
}

impl fmt::Display for TranslationStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 一个可翻译条目（XML / LSX / LOCA 的统一中间表示）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TranslationEntry {
    /// 条目唯一 ID：`{PAK 内路径}#{contentuid}`
    pub id: String,
    /// 所属 PAK 内文件路径
    pub source_file: String,
    /// 原文
    pub source: String,
    /// 译文；未翻译时为空字符串
    #[serde(default)]
    pub target: String,
    /// contentuid / loca key；**重打包时必须原样保留**
    pub contentuid: String,
    /// 版本号；**重打包时必须原样保留**
    #[serde(default)]
    pub version: String,
    /// 条目状态
    #[serde(default)]
    pub status: TranslationStatus,
    /// 错误信息（status == error 时）
    #[serde(default)]
    pub error: Option<String>,
}

impl TranslationEntry {
    /// 构造一个待翻译条目。
    pub fn new(
        source_file: impl Into<String>,
        contentuid: impl Into<String>,
        version: impl Into<String>,
        source: impl Into<String>,
    ) -> Self {
        let source_file = source_file.into();
        let contentuid = contentuid.into();
        Self {
            id: format!("{source_file}#{contentuid}"),
            source_file,
            source: source.into(),
            target: String::new(),
            contentuid,
            version: version.into(),
            status: TranslationStatus::Pending,
            error: None,
        }
    }

    /// 是否「有原文、没译文」，即需要送去翻译。
    pub fn is_pending_translation(&self) -> bool {
        !self.source.trim().is_empty() && self.target.trim().is_empty()
    }

    /// 是否有译文文本（**不代表可以写回**，见 [`Self::has_writable_target`]）。
    pub fn has_target(&self) -> bool {
        !self.target.trim().is_empty()
    }

    /// 是否有「可以写回 PAK」的译文：`target` 非空 **且** 状态不是 [`TranslationStatus::Error`]。
    ///
    /// 为什么必须看状态：结构保真校验失败（或网络中断）时条目会被置为 `error`，
    /// 但 `target` 里仍留着被拒译文 / 半截流式文本（前端要展示给用户看）。
    /// 用户不点「重试失败」直接打包时，如果写回只看「target 非空」，坏译文
    /// （漏 `{1}`、漏标签、两轮拼接）就会照样进 PAK —— 正是保真防线要拦的东西。
    ///
    /// 人工编辑过的条目状态会变成 [`TranslationStatus::Edited`]，不受影响，
    /// 因此「翻译失败 → 手动改好 → 打包」这条路仍然通。
    pub fn has_writable_target(&self) -> bool {
        self.has_target() && self.status != TranslationStatus::Error
    }

    /// 写回时使用的文本：有可写回的译文用译文，否则保留原文。
    ///
    /// `error` 条目一律退回原文 —— 写回链路（content_list / loca / lsx）
    /// 都只看这个方法，所以「坏译文不进 PAK」这条不变量只在这里定义一次。
    pub fn effective_text(&self) -> &str {
        if self.has_writable_target() {
            &self.target
        } else {
            &self.source
        }
    }

    /// 标记为已翻译。
    pub fn mark_translated(&mut self, target: impl Into<String>) {
        self.target = target.into();
        self.status = TranslationStatus::Translated;
        self.error = None;
    }

    /// 标记为出错。
    pub fn mark_error(&mut self, message: impl Into<String>) {
        self.status = TranslationStatus::Error;
        self.error = Some(message.into());
    }

    /// 追加流式增量文本（不清空已有译文）。
    pub fn append_delta(&mut self, delta: &str) {
        self.target.push_str(delta);
        self.status = TranslationStatus::Translating;
    }
}

// ─────────────────────────────────────────────────────────────
// 流式事件
// ─────────────────────────────────────────────────────────────

/// 流式翻译事件，通过 Tauri Channel 推送到前端。
///
/// 注意：`#[serde(tag = "type")]` 只作用于 variant 名，
/// variant 内部字段需要显式 `rename`，这里统一显式标注以防回归。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranslationEvent {
    /// 某条目状态变更（开始翻译）
    Progress {
        #[serde(rename = "entryId")]
        entry_id: String,
        status: TranslationStatus,
    },
    /// 翻译增量文本
    Delta {
        #[serde(rename = "entryId")]
        entry_id: String,
        text: String,
    },
    /// 单条完成
    Done {
        #[serde(rename = "entryId")]
        entry_id: String,
        text: String,
    },
    /// 单条出错
    Error {
        #[serde(rename = "entryId")]
        entry_id: String,
        message: String,
    },
    /// 全部完成
    AllDone { total: usize, failed: usize },
}

impl TranslationEvent {
    pub fn progress(entry_id: impl Into<String>) -> Self {
        TranslationEvent::Progress {
            entry_id: entry_id.into(),
            status: TranslationStatus::Translating,
        }
    }

    pub fn delta(entry_id: impl Into<String>, text: impl Into<String>) -> Self {
        TranslationEvent::Delta {
            entry_id: entry_id.into(),
            text: text.into(),
        }
    }

    pub fn done(entry_id: impl Into<String>, text: impl Into<String>) -> Self {
        TranslationEvent::Done {
            entry_id: entry_id.into(),
            text: text.into(),
        }
    }

    pub fn error(entry_id: impl Into<String>, message: impl Into<String>) -> Self {
        TranslationEvent::Error {
            entry_id: entry_id.into(),
            message: message.into(),
        }
    }

    /// 事件涉及的条目 ID（`AllDone` 没有对应条目）。
    pub fn entry_id(&self) -> Option<&str> {
        match self {
            TranslationEvent::Progress { entry_id, .. }
            | TranslationEvent::Delta { entry_id, .. }
            | TranslationEvent::Done { entry_id, .. }
            | TranslationEvent::Error { entry_id, .. } => Some(entry_id),
            TranslationEvent::AllDone { .. } => None,
        }
    }
}

// ─────────────────────────────────────────────────────────────
// 设置
// ─────────────────────────────────────────────────────────────

fn default_base_url() -> String {
    "https://api.deepseek.com".into()
}

fn default_model() -> String {
    "deepseek-chat".into()
}

fn default_concurrency() -> usize {
    6
}

fn default_temperature() -> f32 {
    0.3
}

/// 大模型连接设置（OpenAI 兼容协议）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LlmSettings {
    /// API base URL，例如 `https://api.deepseek.com`（带不带 `/v1` 都可以，
    /// 见 [`LlmSettings::chat_completions_url`]）
    #[serde(default = "default_base_url")]
    pub base_url: String,
    /// API Key
    #[serde(default)]
    pub api_key: String,
    /// 模型名
    #[serde(default = "default_model")]
    pub model: String,
    /// 并发请求数
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    /// 采样温度：越低越稳定，翻译建议 0.1–0.4
    #[serde(default = "default_temperature")]
    pub temperature: f32,
}

impl Default for LlmSettings {
    fn default() -> Self {
        Self {
            base_url: default_base_url(),
            api_key: String::new(),
            model: default_model(),
            concurrency: default_concurrency(),
            temperature: default_temperature(),
        }
    }
}

impl LlmSettings {
    /// 归一化：去掉首尾空白、裁剪非法取值。
    pub fn normalized(mut self) -> Self {
        self.base_url = self.base_url.trim().trim_end_matches('/').to_string();
        self.api_key = self.api_key.trim().to_string();
        self.model = self.model.trim().to_string();
        self.concurrency = self.concurrency.clamp(1, 64);
        // NaN 与任何数比较都是 false，`clamp` 会把它原样放过去，最后序列化成
        // `"temperature": null`（serde_json 对 NaN 的表示）—— 部分网关会直接 400。
        // NaN 没有「更接近哪一端」可言，退回默认温度。
        self.temperature = if self.temperature.is_nan() {
            default_temperature()
        } else {
            self.temperature.clamp(0.0, 2.0)
        };
        if self.base_url.is_empty() {
            self.base_url = default_base_url();
        }
        if self.model.is_empty() {
            self.model = default_model();
        }
        self
    }

    /// chat/completions 完整地址。
    ///
    /// 用户粘进来的地址花样很多，这里统一收口（尾斜杠在 [`LlmSettings::normalized`]
    /// 里已经去掉）：
    ///
    /// | 填的 baseUrl | 实际请求 |
    /// | --- | --- |
    /// | `https://host` | `https://host/v1/chat/completions` |
    /// | `https://host/v1` | `https://host/v1/chat/completions` |
    /// | `https://host/openai/v1` | `https://host/openai/v1/chat/completions` |
    /// | `https://host/v1/chat/completions` | 原样使用 |
    /// | `https://host/v1?api-version=1` | `https://host/v1/chat/completions?api-version=1` |
    ///
    /// 第二行是必须的：`https://api.deepseek.com/v1` 是最常见的填法之一，
    /// 直接拼 `/v1/chat/completions` 会得到 `/v1/v1/chat/completions` → 404。
    pub fn chat_completions_url(&self) -> String {
        let base = self.base_url.trim();
        // query / fragment 只属于最终地址：`.../v1?api-version=1` 的路径部分是 `/v1`，
        // 直接往后拼会得到 `?api-version=1/v1/chat/completions` 这种废地址。
        let (path, suffix) = match base.find(['?', '#']) {
            Some(index) => (&base[..index], &base[index..]),
            None => (base, ""),
        };
        let path = path.trim_end_matches('/');
        let url = if path.ends_with("/chat/completions") {
            path.to_string()
        } else if path.ends_with("/v1") {
            format!("{path}/chat/completions")
        } else {
            format!("{path}/v1/chat/completions")
        };
        format!("{url}{suffix}")
    }

    /// 是否已配置到可以发起请求。
    pub fn is_configured(&self) -> bool {
        !self.api_key.trim().is_empty()
    }
}

// ─────────────────────────────────────────────────────────────
// 解包结果
// ─────────────────────────────────────────────────────────────

/// 解包结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtractResult {
    /// 解包临时工作目录（后端管理）
    pub work_dir: String,
    /// 文件列表
    pub files: Vec<PakFile>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pak_file_kind_roundtrips_through_strings() {
        for kind in PakFileKind::ALL {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, format!("\"{}\"", kind.as_str()));
            let back: PakFileKind = serde_json::from_str(&json).unwrap();
            assert_eq!(back, kind);
            assert_eq!(kind.as_str().parse::<PakFileKind>().unwrap(), kind);
            assert_eq!(kind.to_string(), kind.as_str());
        }
        assert!("nope".parse::<PakFileKind>().is_err());
    }

    #[test]
    fn only_three_kinds_are_translatable() {
        let translatable: Vec<_> = PakFileKind::ALL
            .into_iter()
            .filter(|k| k.is_translatable())
            .collect();
        assert_eq!(
            translatable,
            vec![
                PakFileKind::LocalizationXml,
                PakFileKind::LocalizationLoca,
                PakFileKind::MetadataLsx
            ]
        );
    }

    #[test]
    fn translation_status_serializes_lowercase() {
        let cases = [
            (TranslationStatus::Pending, "pending"),
            (TranslationStatus::Translating, "translating"),
            (TranslationStatus::Translated, "translated"),
            (TranslationStatus::Edited, "edited"),
            (TranslationStatus::Error, "error"),
        ];
        for (status, expected) in cases {
            assert_eq!(
                serde_json::to_string(&status).unwrap(),
                format!("\"{expected}\"")
            );
        }
        assert_eq!(TranslationStatus::default(), TranslationStatus::Pending);
    }

    #[test]
    fn translation_entry_serializes_camel_case() {
        let entry = TranslationEntry::new("a/b.xml", "h1", "1", "Hello");
        let value: serde_json::Value = serde_json::to_value(&entry).unwrap();
        assert_eq!(value["id"], "a/b.xml#h1");
        assert_eq!(value["sourceFile"], "a/b.xml");
        assert_eq!(value["contentuid"], "h1");
        assert_eq!(value["source"], "Hello");
        assert_eq!(value["target"], "");
        assert_eq!(value["version"], "1");
        assert_eq!(value["status"], "pending");
        assert!(value["error"].is_null());
    }

    #[test]
    fn translation_entry_deserializes_without_optional_fields() {
        // 前端可能只回传一部分字段（例如只发了 contentuid/source），缺省必须可用
        let json = r#"{"id":"f#1","sourceFile":"f","source":"Hi","contentuid":"1"}"#;
        let entry: TranslationEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.target, "");
        assert_eq!(entry.version, "");
        assert_eq!(entry.status, TranslationStatus::Pending);
        assert!(entry.error.is_none());
    }

    #[test]
    fn entry_helpers_track_pending_and_effective_text() {
        let mut entry = TranslationEntry::new("f", "1", "1", "Hello");
        assert!(entry.is_pending_translation());
        assert!(!entry.has_target());
        assert_eq!(entry.effective_text(), "Hello");

        entry.append_delta("你");
        assert_eq!(entry.status, TranslationStatus::Translating);
        assert_eq!(entry.effective_text(), "你");

        entry.mark_translated("你好");
        assert_eq!(entry.status, TranslationStatus::Translated);
        assert_eq!(entry.effective_text(), "你好");
        assert!(!entry.is_pending_translation());

        entry.mark_error("boom");
        assert_eq!(entry.status, TranslationStatus::Error);
        assert_eq!(entry.error.as_deref(), Some("boom"));
        // target 留着给用户看，但已经不算「可写回」了（F-01）
        assert!(entry.has_target());
        assert!(!entry.has_writable_target());
        assert_eq!(entry.effective_text(), "Hello");
    }

    /// F-01：`error` 条目没有「可写回的译文」，写回文本退回原文；
    /// 人工编辑（`edited`）后照常写回。
    #[test]
    fn error_entries_have_no_writable_target() {
        let mut entry = TranslationEntry::new("f.loca", "h1", "1", "Deals {1} damage");
        entry.mark_translated("造成伤害"); // 漏占位符的坏译文
        assert!(entry.has_target());
        assert!(entry.has_writable_target());
        assert_eq!(entry.effective_text(), "造成伤害");

        entry.mark_error("结构校验未通过：占位符 {1} 缺失（已重试 1 次）");
        assert!(entry.has_target(), "target 仍要保留，前端展示被拒译文");
        assert!(!entry.has_writable_target(), "error 条目不得写回");
        assert_eq!(entry.effective_text(), "Deals {1} damage", "写回退回原文");

        // 人工编辑抢救（前端把状态置为 edited）之后可以照常写回
        entry.target = "造成 {1} 点伤害".into();
        entry.status = TranslationStatus::Edited;
        assert!(entry.has_writable_target());
        assert_eq!(entry.effective_text(), "造成 {1} 点伤害");

        // 抢救过的译文再次被判失败，同样不写回
        entry.mark_error("又错了");
        assert_eq!(entry.effective_text(), "Deals {1} damage");
    }

    /// 流式增量（`translating`）与空 target 的既有语义不变。
    #[test]
    fn writable_target_covers_all_statuses() {
        let mut entry = TranslationEntry::new("f.loca", "h1", "1", "Fireball");

        // 没有译文：退回原文
        entry.status = TranslationStatus::Pending;
        assert!(!entry.has_writable_target());
        assert_eq!(entry.effective_text(), "Fireball");

        // 流式中间态：行为与改动前一致（有文本就写）
        entry.append_delta("火球");
        assert_eq!(entry.status, TranslationStatus::Translating);
        assert!(entry.has_writable_target());
        assert_eq!(entry.effective_text(), "火球");

        entry.mark_translated("火球术");
        assert_eq!(entry.status, TranslationStatus::Translated);
        assert_eq!(entry.effective_text(), "火球术");

        // 只有空白字符的译文不算译文
        entry.target = "   ".into();
        assert!(!entry.has_target());
        assert!(!entry.has_writable_target());
        assert_eq!(entry.effective_text(), "Fireball");
    }

    #[test]
    fn blank_source_is_never_pending() {
        let entry = TranslationEntry::new("f", "1", "1", "   ");
        assert!(!entry.is_pending_translation());
    }

    #[test]
    fn translation_event_matches_frontend_contract() {
        let progress = serde_json::to_value(TranslationEvent::progress("a#1")).unwrap();
        assert_eq!(progress["type"], "progress");
        assert_eq!(progress["entryId"], "a#1");
        assert_eq!(progress["status"], "translating");

        let delta = serde_json::to_value(TranslationEvent::delta("a#1", "你")).unwrap();
        assert_eq!(delta["type"], "delta");
        assert_eq!(delta["entryId"], "a#1");
        assert_eq!(delta["text"], "你");

        let done = serde_json::to_value(TranslationEvent::done("a#1", "你好")).unwrap();
        assert_eq!(done["type"], "done");
        assert_eq!(done["text"], "你好");

        let error = serde_json::to_value(TranslationEvent::error("a#1", "超时")).unwrap();
        assert_eq!(error["type"], "error");
        assert_eq!(error["message"], "超时");

        let all_done = serde_json::to_value(TranslationEvent::AllDone {
            total: 3,
            failed: 1,
        })
        .unwrap();
        assert_eq!(all_done["type"], "all_done");
        assert_eq!(all_done["total"], 3);
        assert_eq!(all_done["failed"], 1);

        assert_eq!(TranslationEvent::progress("x").entry_id(), Some("x"));
        assert_eq!(
            TranslationEvent::AllDone {
                total: 0,
                failed: 0
            }
            .entry_id(),
            None
        );
    }

    #[test]
    fn llm_settings_defaults_and_normalization() {
        let defaults = LlmSettings::default();
        assert_eq!(defaults.base_url, "https://api.deepseek.com");
        assert_eq!(defaults.model, "deepseek-chat");
        assert_eq!(defaults.concurrency, 6);
        assert_eq!(defaults.temperature, 0.3);
        assert!(!defaults.is_configured());

        // 旧版本配置文件没有 temperature，必须能读进来
        let legacy =
            r#"{"baseUrl":"https://x.test/","apiKey":" k ","model":" m ","concurrency":999}"#;
        let parsed: LlmSettings = serde_json::from_str(legacy).unwrap();
        assert_eq!(parsed.temperature, 0.3);
        let normalized = parsed.normalized();
        assert_eq!(normalized.base_url, "https://x.test");
        assert_eq!(normalized.api_key, "k");
        assert_eq!(normalized.model, "m");
        assert_eq!(normalized.concurrency, 64);
        assert!(normalized.is_configured());
        assert_eq!(
            normalized.chat_completions_url(),
            "https://x.test/v1/chat/completions"
        );
    }

    /// 用户填的 baseUrl 五花八门，四种常见写法都必须落到同一个端点。
    #[test]
    fn chat_completions_url_accepts_every_common_base_url_shape() {
        let url = |base: &str| {
            LlmSettings {
                base_url: base.into(),
                ..LlmSettings::default()
            }
            .normalized()
            .chat_completions_url()
        };

        // 不带版本前缀 → 补 /v1
        assert_eq!(
            url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // 已经带了 /v1（最常见的坑）→ 不能再补一次
        assert_eq!(
            url("https://api.deepseek.com/v1"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // 尾斜杠
        assert_eq!(
            url("https://api.deepseek.com/v1/"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // 带路径前缀的网关（Azure / 自建反代）
        assert_eq!(
            url("https://gw.corp.test/openai/v1"),
            "https://gw.corp.test/openai/v1/chat/completions"
        );
        // 直接粘完整端点 → 原样使用，不拼出 /v1/v1/chat/completions/chat/completions
        assert_eq!(
            url("https://api.deepseek.com/v1/chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
        // 旧行为不能丢：没写 /v1 也不含端点路径时照样补
        assert_eq!(
            url("http://127.0.0.1:8000"),
            "http://127.0.0.1:8000/v1/chat/completions"
        );
    }

    #[test]
    fn llm_settings_clamps_temperature_and_fills_blanks() {
        let settings = LlmSettings {
            base_url: "   ".into(),
            api_key: String::new(),
            model: String::new(),
            concurrency: 0,
            temperature: 9.0,
        }
        .normalized();
        assert_eq!(settings.base_url, "https://api.deepseek.com");
        assert_eq!(settings.model, "deepseek-chat");
        assert_eq!(settings.concurrency, 1);
        assert_eq!(settings.temperature, 2.0);
    }

    /// NaN 温度必须归一化掉：`clamp` 对 NaN 无效，会一路序列化成 `"temperature": null`。
    #[test]
    fn nan_temperature_falls_back_to_the_default() {
        let settings = LlmSettings {
            temperature: f32::NAN,
            ..LlmSettings::default()
        }
        .normalized();
        assert!(
            settings.temperature.is_finite(),
            "NaN 必须被归一化，实际: {}",
            settings.temperature
        );
        assert_eq!(settings.temperature, 0.3);
        // 序列化出来不能是 null（部分网关会因此 400）。
        // 比字符串而不是 `to_value`：`to_value` 会把 f32 拓宽成 f64，
        // 报出的是永远上不了线的 0.30000001192092896。
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"temperature\":0.3"), "实际序列化: {json}");
        assert!(!json.contains("\"temperature\":null"), "实际序列化: {json}");

        // 其他非有限值仍然按 clamp 语义处理
        assert_eq!(
            LlmSettings {
                temperature: f32::INFINITY,
                ..LlmSettings::default()
            }
            .normalized()
            .temperature,
            2.0
        );
        assert_eq!(
            LlmSettings {
                temperature: f32::NEG_INFINITY,
                ..LlmSettings::default()
            }
            .normalized()
            .temperature,
            0.0
        );
    }

    /// 带 query 的 baseUrl（Azure / 自建网关常见）不能被拼成废地址。
    #[test]
    fn chat_completions_url_keeps_the_query_string_at_the_end() {
        let url = |base: &str| {
            LlmSettings {
                base_url: base.into(),
                ..LlmSettings::default()
            }
            .normalized()
            .chat_completions_url()
        };

        assert_eq!(
            url("https://gw.test/v1?api-version=2024-02-01"),
            "https://gw.test/v1/chat/completions?api-version=2024-02-01"
        );
        assert_eq!(
            url("https://gw.test/openai?x=1"),
            "https://gw.test/openai/v1/chat/completions?x=1"
        );
        assert_eq!(
            url("https://gw.test/v1/chat/completions?api-version=1"),
            "https://gw.test/v1/chat/completions?api-version=1"
        );
        // 尾斜杠 + query 同时出现
        assert_eq!(
            url("https://gw.test/v1/?x=1"),
            "https://gw.test/v1/chat/completions?x=1"
        );
        // 没有 query 的老行为逐字不变
        assert_eq!(
            url("https://api.deepseek.com"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn pak_file_base_name() {
        let file = PakFile {
            name: "Localization/Chinese/foo.xml".into(),
            size: 1,
            kind: PakFileKind::LocalizationXml,
            language: Some("Chinese".into()),
        };
        assert_eq!(file.base_name(), "foo.xml");
    }
}
