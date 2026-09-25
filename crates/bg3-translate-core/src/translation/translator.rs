//! 「文本 → 译文」抽象与基于 reqwest 的生产实现（OpenAI 兼容 + SSE 流式）。
//!
//! 抽象成 trait 是为了单测：注入假实现后，事件序列、并发、失败统计、
//! 取消行为都能在没有网络的情况下断言。生产实现 [`HttpTranslator`] 只负责
//! 「发请求 → 解析 SSE → 实时推 delta」，重试与调度在 `retry` / `engine` 里。
//!
//! 协议边界：**HTTP 客户端、请求发送、SSE 解码、取消都是本仓库自研**；
//! 只有请求体与流式 chunk 的**协议结构**借自 `async-openai` 的 types-only
//! 特性（`default-features = false` + `chat-completion-types`），所以并没有
//! 引入任何第三方 HTTP 客户端。

use std::time::Duration;

use async_openai::types::chat::{
    ChatCompletionRequestMessage, ChatCompletionRequestSystemMessage,
    ChatCompletionRequestSystemMessageContent, ChatCompletionRequestUserMessage,
    ChatCompletionRequestUserMessageContent, CreateChatCompletionRequest,
    CreateChatCompletionRequestArgs,
};
use futures_util::future::BoxFuture;
use futures_util::stream::StreamExt;

use crate::error::{AppError, Result};
use crate::glossary::MatchedTerm;
use crate::types::{LlmSettings, TranslationEvent};

use super::events::{CancelToken, EventSink, wait_until_cancelled};
use super::prompt::{SYSTEM_PROMPT, build_user_prompt};
use super::series::ConsistencyTerm;
use super::sse::{SseDecoder, SseEvent, parse_chat_chunk};

/// 单次 HTTP 请求的超时（含流式读完）。
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// API 错误响应里保留的最大字符数（避免把整个 HTML 错误页塞进 UI）。
const ERROR_BODY_LIMIT: usize = 500;

/// 一次翻译请求的全部输入。
pub struct TranslateRequest<'a> {
    /// 待翻译文本（`Series` 组是 base）
    pub source: &'a str,
    /// 命中的术语，注入 prompt
    pub matches: &'a [MatchedTerm],
    /// 同 MOD 一致性参考，注入 prompt
    pub consistency_terms: &'a [ConsistencyTerm],
    /// 已归一化的语境提示
    pub style_hint: Option<&'a str>,
    /// 需要接收流式增量的条目；`Series` 组为空（成员各自拼后缀）
    pub stream_entry_ids: &'a [String],
    /// 事件出口
    pub sink: &'a dyn EventSink,
    /// 取消令牌
    pub cancel: &'a CancelToken,
}

/// 「文本 → 译文」抽象。
///
/// 生产实现是 [`HttpTranslator`]（reqwest + SSE）；单测注入假实现，
/// 于是事件序列、并发、失败统计、取消行为都能在没有网络的情况下断言。
pub trait TextTranslator: Send + Sync + 'static {
    /// 翻译一段文本。
    ///
    /// - `Ok(Some(text))`：成功；`text` 已 trim
    /// - `Ok(None)`：被取消（调用方不应记为失败）
    /// - `Err(_)`：本次尝试失败，由引擎决定是否重试
    fn translate<'a>(
        &'a self,
        request: TranslateRequest<'a>,
    ) -> BoxFuture<'a, Result<Option<String>>>;

    /// 带结构纠错提示的重试（引擎在译文结构校验失败时调用，只调用一次）。
    ///
    /// 默认实现忽略提示、退回普通翻译：自定义实现（自建网关、单测假实现）
    /// 不需要为了编译而实现它。生产实现把它拼进 user prompt。
    fn translate_with_correction<'a>(
        &'a self,
        request: TranslateRequest<'a>,
        correction: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        let _ = correction;
        self.translate(request)
    }
}

/// 基于 reqwest 的 OpenAI 兼容实现。
#[derive(Debug)]
pub(crate) struct HttpTranslator {
    client: reqwest::Client,
    settings: LlmSettings,
}

impl HttpTranslator {
    pub(crate) fn new(client: reqwest::Client, settings: LlmSettings) -> Self {
        Self { client, settings }
    }

    /// 单次尝试：发起请求 + 解析 SSE 流 + 推送增量。
    async fn translate_once(
        &self,
        request: TranslateRequest<'_>,
        correction: Option<&str>,
    ) -> Result<Option<String>> {
        if request.cancel.is_cancelled() {
            return Ok(None);
        }

        let user_prompt = build_request_prompt(&request, correction);
        let body = build_chat_request(&self.settings, &user_prompt)?;

        let response = tokio::select! {
            response = self
                .client
                .post(self.settings.chat_completions_url())
                .bearer_auth(&self.settings.api_key)
                .json(&body)
                .timeout(REQUEST_TIMEOUT)
                .send() => response.map_err(|e| AppError::Llm(format!("请求失败: {e}")))?,
            () = wait_until_cancelled(request.cancel.clone()) => return Ok(None),
        };

        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            return Err(AppError::Llm(format!(
                "API 返回 {status}: {}",
                truncate_chars(&text, ERROR_BODY_LIMIT)
            )));
        }

        let mut stream = response.bytes_stream();
        let mut decoder = SseDecoder::new();
        let mut full = String::new();

        'stream: loop {
            let next = tokio::select! {
                chunk = stream.next() => chunk,
                () = wait_until_cancelled(request.cancel.clone()) => return Ok(None),
            };
            let events = match next {
                Some(Ok(bytes)) => decoder.push(&bytes),
                Some(Err(err)) => return Err(AppError::Llm(format!("流读取失败: {err}"))),
                None => {
                    let tail = decoder.finish();
                    if tail.is_empty() {
                        break 'stream;
                    }
                    tail
                }
            };

            for event in events {
                match event {
                    SseEvent::Done => break 'stream,
                    SseEvent::Data(payload) => {
                        let Some(delta) = parse_chat_chunk(&payload) else {
                            continue;
                        };
                        if delta.is_empty() {
                            continue;
                        }
                        if request.cancel.is_cancelled() {
                            return Ok(None);
                        }
                        // 实时推送每个 token；系列组不推 delta，只推最终一致译文
                        for entry_id in request.stream_entry_ids {
                            request
                                .sink
                                .emit(TranslationEvent::delta(entry_id, delta.clone()));
                        }
                        full.push_str(&delta);
                    }
                }
            }
        }

        if request.cancel.is_cancelled() {
            return Ok(None);
        }
        let text = full.trim().to_string();
        ensure_stream_produced_text(request.source, &text)?;
        Ok(Some(text))
    }
}

impl TextTranslator for HttpTranslator {
    fn translate<'a>(
        &'a self,
        request: TranslateRequest<'a>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(self.translate_once(request, None))
    }

    fn translate_with_correction<'a>(
        &'a self,
        request: TranslateRequest<'a>,
        correction: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(self.translate_once(request, correction))
    }
}

/// 按 OpenAI 兼容协议构造请求体。
///
/// 类型借自 `async-openai` 的 types-only 特性：字段名、角色枚举与序列化规则都
/// 与协议定义同源，协议加字段时不用我们跟着改。**请求仍由本仓库自己的 reqwest
/// 客户端发出**，不经过任何第三方 HTTP 客户端。
///
/// 顺带修掉手写 `json!` 时的一个隐蔽问题：`json!({"temperature": 0.3f32})` 会先把
/// f32 转成 f64 再序列化，线上实际发出去的是 `0.30000001192092896`；走类型化
/// 序列化才是干净的 `0.3`。
pub(crate) fn build_chat_request(
    settings: &LlmSettings,
    user_prompt: &str,
) -> Result<CreateChatCompletionRequest> {
    CreateChatCompletionRequestArgs::default()
        .model(settings.model.as_str())
        .stream(true)
        .temperature(settings.temperature)
        .messages(vec![
            ChatCompletionRequestMessage::System(ChatCompletionRequestSystemMessage {
                content: ChatCompletionRequestSystemMessageContent::Text(SYSTEM_PROMPT.to_string()),
                name: None,
            }),
            ChatCompletionRequestMessage::User(ChatCompletionRequestUserMessage {
                content: ChatCompletionRequestUserMessageContent::Text(user_prompt.to_string()),
                name: None,
            }),
        ])
        .build()
        .map_err(|err| AppError::Llm(format!("构造请求失败: {err}")))
}

/// 流读完了却一个字都没收到 → 报错。
///
/// 不报错的话会返回空译文：对「原文没有占位符/标签」的条目，空译文能通过结构校验
/// 被当成成功（前端显示「已翻译」但译文空白），只有写回时才因为 target 为空退回
/// 原文。把这种情况变成一次可重试的失败，比让它静默通过更有用 —— 最常见的成因
/// 就是网关把错误包成了 200，或者服务端压根没按 SSE 返回。
///
/// 原文本身为空时不报错：那本来就不该要求模型输出任何东西。
fn ensure_stream_produced_text(source: &str, text: &str) -> Result<()> {
    if text.is_empty() && !source.trim().is_empty() {
        return Err(AppError::Llm(
            "流式响应结束但没有任何文本（网关可能把错误包成了 200，或服务端未按 SSE 返回）"
                .to_string(),
        ));
    }
    Ok(())
}

/// 组装 user prompt；有纠错提示时附在最后。
pub(crate) fn build_request_prompt(
    request: &TranslateRequest<'_>,
    correction: Option<&str>,
) -> String {
    let prompt = build_user_prompt(
        request.source,
        request.matches,
        request.consistency_terms,
        request.style_hint,
    );
    match correction.map(str::trim).filter(|hint| !hint.is_empty()) {
        Some(hint) => format!("{prompt}\n\n【结构校验未通过，请修正】{hint}"),
        None => prompt,
    }
}

/// 按字符截断（不能用字节切片：中文会被切成半个字符导致 panic）。
fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        text.to_string()
    } else {
        text.chars().take(limit).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::translation::engine::CollectingSink;

    /// trait 的默认实现必须能用（自定义实现不必写纠错分支）。
    #[tokio::test]
    async fn default_correction_impl_falls_back_to_plain_translate() {
        struct Echo;

        impl TextTranslator for Echo {
            fn translate<'a>(
                &'a self,
                request: TranslateRequest<'a>,
            ) -> BoxFuture<'a, Result<Option<String>>> {
                Box::pin(async move { Ok(Some(format!("译:{}", request.source))) })
            }
        }

        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let translator = Echo;
        let result = translator
            .translate_with_correction(
                TranslateRequest {
                    source: "Fireball",
                    matches: &[],
                    consistency_terms: &[],
                    style_hint: None,
                    stream_entry_ids: &[],
                    sink: &sink,
                    cancel: &cancel,
                },
                Some("缺少占位符 {1}"),
            )
            .await
            .unwrap();
        assert_eq!(result.as_deref(), Some("译:Fireball"));
    }

    #[test]
    fn request_prompt_carries_correction_hint() {
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let hint = "上一轮译文缺少占位符 {1}，请重新只输出译文。";
        let request = TranslateRequest {
            source: "Deals {1} damage",
            matches: &[],
            consistency_terms: &[],
            style_hint: Some("MOD 语境"),
            stream_entry_ids: &[],
            sink: &sink,
            cancel: &cancel,
        };

        let plain = build_request_prompt(&request, None);
        assert!(plain.ends_with("原文：\nDeals {1} damage"));
        assert!(!plain.contains("结构校验未通过"));

        let corrected = build_request_prompt(&request, Some(hint));
        assert!(
            corrected.contains("【结构校验未通过，请修正】"),
            "实际: {corrected}"
        );
        assert!(corrected.ends_with(hint));

        // 空白提示按「没有提示」处理
        assert_eq!(build_request_prompt(&request, Some("   ")), plain);
        assert_eq!(build_request_prompt(&request, Some("")), plain);
    }

    #[test]
    fn truncate_chars_never_splits_multibyte() {
        assert_eq!(truncate_chars("夺心魔", 10), "夺心魔");
        assert_eq!(truncate_chars("夺心魔", 2), "夺心");
        assert_eq!(truncate_chars("", 3), "");
    }

    // ── 协议类型化（方案 A：types-only 依赖）──

    fn test_settings() -> LlmSettings {
        LlmSettings {
            base_url: "https://api.deepseek.com".into(),
            api_key: "k".into(),
            model: "deepseek-chat".into(),
            concurrency: 2,
            temperature: 0.3,
        }
    }

    /// 类型化请求序列化出来的线上格式，必须与拆分前手写的 `json!` 逐字段一致
    /// （字段名、角色名、内容位置都不许漂移）。
    ///
    /// 注意比对方式：先 `to_string` 再 `from_str` 回 `Value`，而不是直接
    /// `to_value` —— `to_value` 会把 f32 拓宽成 f64，报出的是一个
    /// **永远上不了线**的数字（0.30000001192092896），真实请求走的是
    /// reqwest 的 `to_vec`，即 ryu 对 f32 的格式化结果。
    #[test]
    fn build_chat_request_matches_the_wire_format() {
        let request = build_chat_request(&test_settings(), "原文：\nFireball").unwrap();
        let serialized = serde_json::to_string(&request).unwrap();
        let value: serde_json::Value = serde_json::from_str(&serialized).unwrap();

        assert_eq!(
            value,
            serde_json::json!({
                "model": "deepseek-chat",
                "stream": true,
                "temperature": 0.3,
                "messages": [
                    { "role": "system", "content": SYSTEM_PROMPT },
                    { "role": "user", "content": "原文：\nFireball" }
                ]
            })
        );
        // 恰恰四个字段：async-openai 的请求结构有几十个字段，
        // 不能被填成 null 一起发出去（部分网关对多余字段很挑）
        assert_eq!(value.as_object().unwrap().len(), 4);
    }

    /// 手写 `json!` 会把 0.3f32 发成 0.30000001192092896，类型化后必须是 0.3。
    #[test]
    fn build_chat_request_serializes_temperature_without_f32_noise() {
        let request = build_chat_request(&test_settings(), "x").unwrap();
        let serialized = serde_json::to_string(&request).unwrap();
        assert!(
            serialized.contains("\"temperature\":0.3"),
            "实际序列化: {serialized}"
        );
        assert!(!serialized.contains("0.30000001192092896"));
    }

    // ── 空流防线（200 但没有任何 delta）──

    #[test]
    fn empty_stream_with_non_empty_source_is_an_error() {
        let err = ensure_stream_produced_text("Fireball", "").unwrap_err();
        assert_eq!(err.code(), "llm");
        assert!(err.to_string().contains("没有任何文本"), "实际: {err}");
    }

    #[test]
    fn empty_stream_with_empty_source_is_not_an_error() {
        // 原文本身为空（例如 contentList 里的空 <content/>）不该要求模型输出东西
        ensure_stream_produced_text("", "").unwrap();
        ensure_stream_produced_text("   ", "").unwrap();
    }

    #[test]
    fn non_empty_stream_is_not_an_error() {
        ensure_stream_produced_text("Fireball", "火球").unwrap();
    }
}
