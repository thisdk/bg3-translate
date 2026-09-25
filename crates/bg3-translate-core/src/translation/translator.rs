//! 「文本 → 译文」抽象与基于 reqwest 的生产实现（OpenAI 兼容 + SSE 流式）。
//!
//! 抽象成 trait 是为了单测：注入假实现后，事件序列、并发、失败统计、
//! 取消行为都能在没有网络的情况下断言。生产实现 [`HttpTranslator`] 只负责
//! 「发请求 → 解析 SSE → 实时推 delta」，重试与调度在 `retry` / `engine` 里。

use std::time::Duration;

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
        let body = serde_json::json!({
            "model": self.settings.model,
            "stream": true,
            "temperature": self.settings.temperature,
            "messages": [
                { "role": "system", "content": SYSTEM_PROMPT },
                { "role": "user", "content": user_prompt }
            ]
        });

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
        Ok(Some(full.trim().to_string()))
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
}
