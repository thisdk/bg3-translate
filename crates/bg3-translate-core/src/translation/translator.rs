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
use super::sse::{SseDecoder, SseEvent, parse_chat_chunk_parts};

/// 单次 HTTP 请求的超时（含流式读完）。
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// API 错误响应里保留的最大字符数（避免把整个 HTML 错误页塞进 UI）。
const ERROR_BODY_LIMIT: usize = 500;

/// HTTP 状态码错误的消息前缀。
///
/// `retry` 靠它把状态码认回来做「值不值得重试」的分类 —— 两端都在本 crate 内，
/// 格式由 [`api_status_error`] 单点产出，改动必须同时过 `retry` 的分类测试。
pub(crate) const API_STATUS_PREFIX: &str = "API 返回 ";

/// 构造 HTTP 状态码错误（供 `retry` 分类识别）。
pub(crate) fn api_status_error(status: reqwest::StatusCode, body: &str) -> AppError {
    AppError::Llm(format!(
        "{API_STATUS_PREFIX}{status}: {}",
        truncate_chars(body, ERROR_BODY_LIMIT)
    ))
}

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
            return Err(api_status_error(status, &text));
        }

        let mut stream = response.bytes_stream();
        consume_chat_stream(&mut stream, &request).await
    }
}

/// 消费 OpenAI 兼容的 SSE 字节流：解码 → 取增量 → 实时推送 → 完整性校验。
///
/// 抽成独立函数是为了能直接喂字节流做测试（跨分片切断、服务端不发 `[DONE]`
/// 直接断流、被长度上限截断、流中报错），不必起真实网络或 mock 框架。
async fn consume_chat_stream<S, B, E>(
    stream: S,
    request: &TranslateRequest<'_>,
) -> Result<Option<String>>
where
    S: futures_util::Stream<Item = std::result::Result<B, E>>,
    B: AsRef<[u8]>,
    E: std::fmt::Display,
{
    let mut stream = std::pin::pin!(stream);
    let mut decoder = SseDecoder::new();
    let mut full = String::new();
    // 服务端自报「这次响应不完整」的结束原因（length / content_filter）
    let mut truncation: Option<String> = None;
    // 「正常收尾」的两个证据：`[DONE]` 与 `finish_reason`
    let mut saw_done = false;
    let mut saw_finish_reason = false;

    'stream: loop {
        let next = tokio::select! {
            chunk = stream.next() => chunk,
            () = wait_until_cancelled(request.cancel.clone()) => return Ok(None),
        };
        let events = match next {
            Some(Ok(bytes)) => decoder.push_checked(bytes.as_ref())?,
            Some(Err(err)) => return Err(AppError::Llm(format!("流读取失败: {err}"))),
            None => {
                let tail = decoder.finish_checked()?;
                if tail.is_empty() {
                    break 'stream;
                }
                tail
            }
        };

        for event in events {
            match event {
                SseEvent::Done => {
                    saw_done = true;
                    break 'stream;
                }
                SseEvent::Data(payload) => {
                    let Some(chunk) = parse_chat_chunk_parts(&payload) else {
                        continue;
                    };
                    saw_finish_reason |= chunk.finish_reason.is_some();
                    if let Some(reason) = chunk.truncation_reason() {
                        truncation = Some(reason.to_string());
                    }
                    let Some(delta) = chunk.content.filter(|text| !text.is_empty()) else {
                        continue;
                    };
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
    // 先判「响应是否完整」：被截断 / 被过滤时这条消息比「没有任何文本」有用得多
    ensure_stream_complete(truncation.as_deref(), saw_done, saw_finish_reason)?;
    ensure_stream_produced_text(request.source, &text)?;
    Ok(Some(text))
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

/// 流式响应必须给出「正常收尾」的证据，否则这次尝试算失败。
///
/// 两条判据：
/// 1. **服务端自报不完整**（`finish_reason` 是 `length` / `content_filter`）：
///    半截译文会一路通过结构校验（原文没有占位符/标签时更是什么都拦不住）
///    被写进 PAK，用户看到的是模型讲到一半的句子，界面上还显示「已翻译」；
/// 2. **既没有 `[DONE]` 也没有任何 `finish_reason`**：那就是一条被中途掐断的连接，
///    拿不到「输出完整」的任何保证。
///
/// 报成可重试的失败更有用：重试会重新生成，重试完仍失败则条目落到 `error`
/// 状态（`has_writable_target()` 为 false，写回退回原文），用户看得见、能手工补，
/// 而不是把坏文本当成成品打进 PAK。
///
/// 代价说清楚：**完全不用 `[DONE]`、也不发 `finish_reason` 的网关会被判失败**。
/// 这类网关无法与「连接被掐断」区分，只能按后者处理。
fn ensure_stream_complete(
    truncation: Option<&str>,
    saw_done: bool,
    saw_finish_reason: bool,
) -> Result<()> {
    if let Some(reason) = truncation {
        return Err(AppError::Llm(format!(
            "模型输出不完整（finish_reason={reason}），已丢弃这次半截译文；\
             可减小单条文本长度或改用输出上限更高的模型后重试"
        )));
    }
    if !saw_done && !saw_finish_reason {
        return Err(AppError::Llm(
            "流式响应结束，但既没有收到 [DONE] 也没有 finish_reason，无法确认输出完整\
             （连接可能被中途掐断）；若在自定义网关上出现，请确认它按 OpenAI 协议发送 \
             [DONE] 或 finish_reason，否则换用标准端点后重试"
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

    // ── 完整 SSE 流（直接喂字节，不需要网络）──

    use crate::translation::events::CancelToken as TestCancelToken;

    /// 把若干字节分片喂给 [`consume_chat_stream`]，返回（结果，推送出去的增量）。
    async fn feed(chunks: Vec<Vec<u8>>, source: &str) -> (Result<Option<String>>, Vec<String>) {
        let sink = CollectingSink::new();
        let cancel = TestCancelToken::new();
        let request = TranslateRequest {
            source,
            matches: &[],
            consistency_terms: &[],
            style_hint: None,
            stream_entry_ids: &["e1".to_string()],
            sink: &sink,
            cancel: &cancel,
        };
        let stream = futures_util::stream::iter(chunks.into_iter().map(Ok::<Vec<u8>, AppError>));
        let result = consume_chat_stream(stream, &request).await;
        let deltas = sink
            .events()
            .into_iter()
            .filter_map(|event| match event {
                TranslationEvent::Delta { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        (result, deltas)
    }

    fn chunk_json(content: &str, finish_reason: &str) -> String {
        format!(
            r#"data: {{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{{"index":0,"delta":{{"content":"{content}"}},"finish_reason":{finish_reason}}}]}}"#
        )
    }

    #[tokio::test]
    async fn consume_stream_assembles_deltas_and_stops_at_done() {
        let body = format!(
            "{}\n\n{}\n\ndata: [DONE]\n\n",
            chunk_json("火球", "null"),
            chunk_json("术", "null")
        );
        let (result, deltas) = feed(vec![body.into_bytes()], "Fireball").await;
        assert_eq!(result.unwrap().as_deref(), Some("火球术"));
        assert_eq!(deltas, vec!["火球".to_string(), "术".to_string()]);
    }

    /// 跨分片切断（含 UTF-8 字符中间）：字节级喂入也必须拼回同一个译文。
    #[tokio::test]
    async fn consume_stream_survives_split_chunks() {
        let body = format!("{}\n\ndata: [DONE]\n\n", chunk_json("夺心魔", "null"));
        let bytes = body.into_bytes();
        let chunks: Vec<Vec<u8>> = bytes.iter().map(|byte| vec![*byte]).collect();
        let (result, deltas) = feed(chunks, "Mind flayer").await;
        assert_eq!(result.unwrap().as_deref(), Some("夺心魔"));
        assert_eq!(deltas, vec!["夺心魔".to_string()]);
    }

    /// 服务端不发 `[DONE]`，但给了 `finish_reason: "stop"`：同样是完整响应。
    #[tokio::test]
    async fn consume_stream_accepts_finish_reason_without_done_marker() {
        let body = format!(
            "{}\n\n{}\n\n",
            chunk_json("火球术", "null"),
            chunk_json("", "\"stop\"")
        );
        let (result, _) = feed(vec![body.into_bytes()], "Fireball").await;
        assert_eq!(result.unwrap().as_deref(), Some("火球术"));
    }

    /// 既没有 `[DONE]` 也没有 `finish_reason`：连接被掐断，不能当成功。
    #[tokio::test]
    async fn consume_stream_rejects_a_stream_that_never_terminates() {
        let body = format!("{}\n\n", chunk_json("半截译文", "null"));
        let (result, deltas) = feed(vec![body.into_bytes()], "A long text").await;
        assert_eq!(deltas, vec!["半截译文".to_string()], "增量照常推送");
        let err = result.expect_err("没有收尾证据的流必须报错");
        assert!(err.to_string().contains("无法确认输出完整"), "实际: {err}");
    }

    #[tokio::test]
    async fn consume_stream_reports_mid_stream_errors() {
        let stream = futures_util::stream::iter(vec![Err::<Vec<u8>, AppError>(AppError::Llm(
            "连接被重置".into(),
        ))]);
        let sink = CollectingSink::new();
        let cancel = TestCancelToken::new();
        let request = TranslateRequest {
            source: "Fireball",
            matches: &[],
            consistency_terms: &[],
            style_hint: None,
            stream_entry_ids: &[],
            sink: &sink,
            cancel: &cancel,
        };
        let err = consume_chat_stream(stream, &request).await.unwrap_err();
        assert!(err.to_string().contains("流读取失败"), "实际: {err}");
    }

    /// 被长度上限截断的响应不能当成功：半截译文会一路写进 PAK。
    ///
    /// 红队 R-01 的形态：服务端发一半 delta + `finish_reason:"length"` 后**直接断流**
    /// （不发 `[DONE]`）。旧实现把它当成功，`Done` 里是原文的前半句。
    #[tokio::test]
    async fn truncated_stream_is_rejected_instead_of_written_back() {
        // 注意：这里刻意不发 `[DONE]`，复现「截断 + 直接断流」
        let body = format!(
            "{}\n\n{}\n\n",
            chunk_json("造成", "null"),
            chunk_json("", "\"length\"")
        );
        let (result, deltas) = feed(vec![body.into_bytes()], "Deals {1} damage").await;
        assert_eq!(deltas, vec!["造成".to_string()], "增量照常推送");
        let err = result.expect_err("截断的流必须报错，不能返回半截译文");
        assert_eq!(err.code(), "llm");
        assert!(
            err.to_string().contains("finish_reason=length"),
            "实际: {err}"
        );
    }

    /// 内容被过滤（`content_filter`）同样不是完整译文。
    #[tokio::test]
    async fn content_filtered_stream_is_rejected() {
        let body = format!(
            "{}\n\ndata: [DONE]\n\n",
            chunk_json("", "\"content_filter\"")
        );
        let (result, _) = feed(vec![body.into_bytes()], "A long text").await;
        let err = result.expect_err("被内容过滤的响应必须报错");
        assert!(err.to_string().contains("content_filter"), "实际: {err}");
    }

    /// 收尾 chunk 是 `stop` 时不受影响（防误报：正常结束必须仍然成功）。
    #[tokio::test]
    async fn stopped_stream_is_still_a_success() {
        let body = format!(
            "{}\n\n{}\n\ndata: [DONE]\n\n",
            chunk_json("火球术", "null"),
            chunk_json("", "\"stop\"")
        );
        let (result, _) = feed(vec![body.into_bytes()], "Fireball").await;
        assert_eq!(result.unwrap().as_deref(), Some("火球术"));
    }

    /// 网关只发最小字段（无 id/created/model）时，`finish_reason` 也要能看到。
    #[tokio::test]
    async fn lenient_gateway_truncation_is_also_rejected() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"半截\"}}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"length\"}]}\n\ndata: [DONE]\n\n";
        let (result, _) = feed(vec![body.as_bytes().to_vec()], "A long text").await;
        assert!(result.is_err(), "实际: {result:?}");
    }

    /// 网关把错误包成 200：HTML / 错误 JSON 体都不能当成功空译文。
    #[tokio::test]
    async fn http_200_with_a_non_sse_body_is_an_error() {
        for body in [
            "<html><body>502 Bad Gateway</body></html>",
            r#"{"error":{"message":"invalid api key","type":"invalid_request_error"}}"#,
            // 带着 [DONE] 的错误体：收尾证据齐了，但一个字都没有
            "data: {\"error\":{\"message\":\"rate limited\"}}\n\ndata: [DONE]\n\n",
        ] {
            let (result, deltas) = feed(vec![body.as_bytes().to_vec()], "Fireball").await;
            let err = result.expect_err("错误体不能被当成空译文成功");
            assert_eq!(err.code(), "llm");
            assert!(deltas.is_empty());
        }
    }

    /// 一次流里出现多个 choices：取第一个（我们从没设过 `n>1`）。
    #[tokio::test]
    async fn multiple_choices_take_the_first_one() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"甲\"}},{\"delta\":{\"content\":\"乙\"}}]}\n\ndata: [DONE]\n\n";
        let (result, _) = feed(vec![body.as_bytes().to_vec()], "Fireball").await;
        assert_eq!(result.unwrap().as_deref(), Some("甲"));
    }

    /// 未知的 finish_reason（网关自造收尾标记）按「服务端自报收尾」处理：
    /// 只把 length / content_filter 当作不完整，避免误判正常网关。
    #[tokio::test]
    async fn unknown_finish_reason_counts_as_a_terminator() {
        let body = "data: {\"choices\":[{\"delta\":{\"content\":\"火球\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"eos\"}]}\n\n";
        let (result, _) = feed(vec![body.as_bytes().to_vec()], "Fireball").await;
        assert_eq!(result.unwrap().as_deref(), Some("火球"));
    }

    #[test]
    fn completeness_helper_covers_both_truncation_signals() {
        // 正常收尾：`[DONE]` 或 `finish_reason` 有一个就够
        ensure_stream_complete(None, true, false).unwrap();
        ensure_stream_complete(None, false, true).unwrap();
        ensure_stream_complete(None, true, true).unwrap();
        // 服务端自报不完整
        let err = ensure_stream_complete(Some("length"), true, true).unwrap_err();
        assert!(
            err.to_string().contains("finish_reason=length"),
            "实际: {err}"
        );
        let err = ensure_stream_complete(Some("content_filter"), true, true).unwrap_err();
        assert!(err.to_string().contains("content_filter"), "实际: {err}");
        // 两个收尾证据都没有
        let err = ensure_stream_complete(None, false, false).unwrap_err();
        assert!(err.to_string().contains("无法确认输出完整"), "实际: {err}");
    }

    /// `API 返回 {status}` 的消息格式是 `retry` 分类的依据，单点产出。
    #[test]
    fn api_status_error_carries_the_shared_prefix() {
        let err = api_status_error(reqwest::StatusCode::UNAUTHORIZED, "bad key");
        assert_eq!(
            err.to_string(),
            "大模型调用错误: API 返回 401 Unauthorized: bad key"
        );
        assert!(
            err.to_string().contains(API_STATUS_PREFIX.trim_end()),
            "前缀必须与 retry 的分类约定一致"
        );
    }
}
