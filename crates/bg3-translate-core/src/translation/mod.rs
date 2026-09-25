//! LLM 流式翻译引擎。
//!
//! 子模块分工：
//! - [`engine`]：并发调度、重试、取消、事件推送（对外主入口 [`TranslationEngine`]）
//! - [`planner`]：任务规划（一致性复用、相同原文合并、系列变体合并）
//! - [`prompt`]：系统提示词与 user prompt 构造（纯函数）
//! - [`series`]：系列变体识别与同 MOD 一致性记忆
//! - [`sse`]：SSE 解析状态机与 OpenAI 兼容 chunk 提取
//!
//! 对外只暴露下面这些名字，内部实现细节不导出：
//! `bg3_translate_core::translation::{TranslationEngine, RunOptions, EventSink, ...}`。

pub mod engine;
pub mod planner;
pub mod prompt;
pub mod series;
pub mod sse;

pub use engine::{
    CancelToken, CollectingSink, EventSink, RunOptions, TextTranslator, TranslateRequest,
    TranslationEngine, TranslationSummary,
};
pub use planner::{SeriesMember, TranslationJob, TranslationOutput, TranslationPlan, plan_jobs};
pub use prompt::{SYSTEM_PROMPT, build_user_prompt, normalize_style_hint};
pub use series::{
    ConsistencyTerm, SeriesAliases, SeriesVariant, build_consistency_memory, build_series_aliases,
    compose_variant_translation, find_consistency_references, normalize_consistency_key,
    split_series_variant, strip_target_suffix,
};
pub use sse::{SseDecoder, SseEvent, parse_chat_chunk};
