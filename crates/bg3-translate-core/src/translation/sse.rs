//! SSE 解析：纯状态机 + OpenAI 兼容 chunk 提取。
//!
//! 旧实现把解析内联在 HTTP 循环里，只能靠真实网络请求间接验证。抽成
//! [`SseDecoder`] 之后，跨 chunk 切断、CRLF、多行 data 这些最容易出错的
//! 分支都能用纯数据驱动测试覆盖。
//!
//! 分工：**SSE 帧解析（[`SseDecoder`]）与流式读取都是本仓库自研**，
//! 只有 chunk 的**协议结构**借自 `async-openai` 的 types-only 特性
//! （见 [`parse_chat_chunk`]），所以这里不带任何 HTTP 客户端依赖。
//!
//! 兼容的现实写法：
//! - `data: {...}` 与 `data:{...}`（有无空格）
//! - `\n` / `\r\n` / 单独 `\r` 三种行结束符
//! - 空行分隔事件；注释行 `:` 与非 data 字段（`event:`/`id:`/`retry:`）忽略
//! - 多行 `data:` 用 `\n` 拼接（SSE 规范）
//! - `[DONE]` 作为独立的 [`SseEvent::Done`]

use std::mem;

use async_openai::types::chat::{CreateChatCompletionStreamResponse, FinishReason};
use serde::Deserialize;

use crate::error::{AppError, Result};

/// 单个 SSE 数据行的字节上限。
///
/// 服务端只要一直不发换行，解码器就会一直攒字节 —— 真实的 delta 分片都是几百
/// 字节级，1 MiB 一行已经离谱；超过就报错让这次请求失败重试，**不静默截断**。
const MAX_LINE_BYTES: usize = 1 << 20;

/// 单个事件（多行 `data:` 拼接后）的字节上限。
const MAX_EVENT_BYTES: usize = 4 << 20;

/// 一条解析出来的 SSE 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseEvent {
    /// 一个事件的 data 负载（多行已用 `\n` 拼接）
    Data(String),
    /// 服务端的 `[DONE]` 结束标记
    Done,
}

impl Default for SseDecoder {
    fn default() -> Self {
        Self::new()
    }
}

/// 增量式 SSE 解码器：喂字节，出事件。
#[derive(Debug)]
pub struct SseDecoder {
    /// 尚未组成完整一行的字节。保留字节而不是 `String`，
    /// 是为了让跨 chunk 切断的多字节 UTF-8 字符也能正确拼回。
    buffer: Vec<u8>,
    /// 当前事件已累积的 data 行。
    data_lines: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            data_lines: Vec::new(),
        }
    }

    /// 喂入一段字节，返回本次能解析出的所有事件。
    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseEvent> {
        self.buffer.extend_from_slice(chunk);
        self.drain_lines()
    }

    /// 带内存保护的 [`Self::push`]：超限返回错误而不是继续攒内存。
    ///
    /// 公开的 [`Self::push`] 保持原语义（无上限、返回 `Vec`），生产路径走这个。
    pub(crate) fn push_checked(&mut self, chunk: &[u8]) -> Result<Vec<SseEvent>> {
        self.buffer.extend_from_slice(chunk);
        if take_line(&self.buffer).is_none() && self.buffer.len() > MAX_LINE_BYTES {
            return Err(AppError::Llm(format!(
                "SSE 数据行已超过 {MAX_LINE_BYTES} 字节仍没有换行（服务端可能没有按 SSE 返回），已中止本次请求"
            )));
        }
        let events = self.drain_lines();
        let pending: usize = self.data_lines.iter().map(String::len).sum();
        if pending > MAX_EVENT_BYTES {
            return Err(AppError::Llm(format!(
                "SSE 单个事件已超过 {MAX_EVENT_BYTES} 字节，已中止本次请求"
            )));
        }
        Ok(events)
    }

    /// 把缓冲区里已经完整的行全部解析出来。
    fn drain_lines(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        while let Some((line, consumed)) = take_line(&self.buffer) {
            self.buffer.drain(..consumed);
            if let Some(event) = self.handle_line(&line) {
                events.push(event);
            }
        }
        events
    }

    /// 流结束时冲刷残留缓冲（最后一行可能没有换行符，也可能没有收尾空行）。
    pub fn finish(&mut self) -> Vec<SseEvent> {
        let mut events = Vec::new();
        let rest = mem::take(&mut self.buffer);
        if !rest.is_empty() {
            let line = String::from_utf8_lossy(&rest);
            let line = line.trim_end_matches(['\r', '\n']);
            if let Some(event) = self.handle_line(line) {
                events.push(event);
            }
        }
        if let Some(event) = self.dispatch() {
            events.push(event);
        }
        events
    }

    /// 带内存保护的 [`Self::finish`]：残留的「没有换行的超长行」同样报错。
    pub(crate) fn finish_checked(&mut self) -> Result<Vec<SseEvent>> {
        if self.buffer.len() > MAX_LINE_BYTES {
            return Err(AppError::Llm(format!(
                "SSE 数据行已超过 {MAX_LINE_BYTES} 字节仍没有换行，已中止本次请求"
            )));
        }
        Ok(self.finish())
    }

    /// 处理一行（不含行结束符）。
    fn handle_line(&mut self, line: &str) -> Option<SseEvent> {
        if line.is_empty() {
            // 空行 = 事件结束
            return self.dispatch();
        }
        if line.starts_with(':') {
            // 注释（有些服务端用它做心跳）
            return None;
        }
        if let Some(value) = line.strip_prefix("data:") {
            // 规范只去掉一个前导空格，保留其它空白（模型输出的缩进有意义）
            let value = value.strip_prefix(' ').unwrap_or(value);
            self.data_lines.push(value.to_string());
            return None;
        }
        if line == "data" {
            // `data` 后无冒号同样表示一个空负载
            self.data_lines.push(String::new());
            return None;
        }
        // event:/id:/retry: 等字段与本次翻译无关
        None
    }

    /// 把累积的 data 行收成一个事件。
    fn dispatch(&mut self) -> Option<SseEvent> {
        if self.data_lines.is_empty() {
            return None;
        }
        let lines = mem::take(&mut self.data_lines);
        if lines.len() == 1 && lines[0].trim() == "[DONE]" {
            return Some(SseEvent::Done);
        }
        Some(SseEvent::Data(lines.join("\n")))
    }
}

/// 从缓冲区头部取出一整行，返回（行内容, 消费的字节数）。
///
/// 行结束符支持 `\n`、`\r\n`、单独 `\r`。若结尾是孤立的 `\r`
/// （可能是 CRLF 被 chunk 从中间切断），返回 `None` 等下一个 chunk。
fn take_line(buffer: &[u8]) -> Option<(String, usize)> {
    for (index, byte) in buffer.iter().enumerate() {
        match *byte {
            b'\n' => {
                let line = String::from_utf8_lossy(&buffer[..index]).into_owned();
                return Some((line, index + 1));
            }
            b'\r' => {
                let next = buffer.get(index + 1)?;
                if *next == b'\n' {
                    let line = String::from_utf8_lossy(&buffer[..index]).into_owned();
                    return Some((line, index + 2));
                }
                let line = String::from_utf8_lossy(&buffer[..index]).into_owned();
                return Some((line, index + 1));
            }
            _ => {}
        }
    }
    None
}

/// 从 OpenAI 兼容的流式响应里取 `choices[0].delta.content`。
///
/// 两级解析：
/// 1. 先按协议标准结构 [`CreateChatCompletionStreamResponse`] 反序列化。类型借自
///    `async-openai` 的 types-only 特性（只有类型，不含 HTTP 客户端），字段名与
///    协议定义同源，协议演进时不用我们跟着改。
/// 2. 标准结构要求 `id` / `index` / `created` / `model` / `object` 齐全，而部分网关
///    只发最小字段（`{"choices":[{"delta":{"content":"…"}}]}`），所以标准解析失败时
///    退回 [`LenientChunk`] 再试一次。
///
/// 两次都失败（心跳、非 JSON、空 choices、错误体）返回 `None`，由调用方跳过。
pub fn parse_chat_chunk(data: &str) -> Option<String> {
    parse_chat_chunk_parts(data)?.content
}

/// 一条流式 chunk 的完整解析结果。
///
/// 比 [`parse_chat_chunk`] 多带一个 `finish_reason`：调用方要能区分
/// 「模型正常收尾」与「撞上长度上限被截断」，后者绝不能当成成功译文。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ChatStreamChunk {
    /// `choices[0].delta.content`
    pub(crate) content: Option<String>,
    /// `choices[0].finish_reason`，保留线上字符串形态（`stop` / `length` / …）
    pub(crate) finish_reason: Option<String>,
}

impl ChatStreamChunk {
    /// 表示「这次响应不完整」的结束原因；`None` 表示服务端自报收尾正常。
    ///
    /// - `length`：撞上输出上限被截断；
    /// - `content_filter`：内容被过滤掉了，同样不是完整译文。
    pub(crate) fn truncation_reason(&self) -> Option<&str> {
        match self.finish_reason.as_deref() {
            Some("length") => Some("length"),
            Some("content_filter") => Some("content_filter"),
            _ => None,
        }
    }
}

/// [`parse_chat_chunk`] 的完整版：同时给出增量文本与结束原因。
///
/// 判定边界与旧行为逐字一致 —— 标准结构解析成功就用它（`choices` 为空 → `None`），
/// 失败才退回宽松结构，两条路径都不再额外放宽。
pub(crate) fn parse_chat_chunk_parts(data: &str) -> Option<ChatStreamChunk> {
    if let Ok(chunk) = serde_json::from_str::<CreateChatCompletionStreamResponse>(data) {
        let choice = chunk.choices.into_iter().next()?;
        return Some(ChatStreamChunk {
            content: choice.delta.content,
            finish_reason: choice.finish_reason.map(finish_reason_text),
        });
    }
    let chunk: LenientChunk = serde_json::from_str(data).ok()?;
    let choice = chunk.choices.into_iter().next()?;
    Some(ChatStreamChunk {
        content: choice.delta.content,
        finish_reason: choice.finish_reason,
    })
}

/// 协议枚举 → 线上字符串（与 serde 的 `snake_case` 表示一致）。
fn finish_reason_text(reason: FinishReason) -> String {
    match reason {
        FinishReason::Stop => "stop",
        FinishReason::Length => "length",
        FinishReason::ToolCalls => "tool_calls",
        FinishReason::ContentFilter => "content_filter",
        FinishReason::FunctionCall => "function_call",
    }
    .to_string()
}

/// 宽松兜底结构：字段全可选，只为兼容「字段不全」的第三方网关。
///
/// 它不再是主解析路径，只是标准结构解析失败后的第二次机会，所以刻意做得最小：
/// 只关心 `choices[0].delta.content` 与 `choices[0].finish_reason`。
#[derive(Debug, Deserialize)]
struct LenientChunk {
    #[serde(default)]
    choices: Vec<LenientChoice>,
}

#[derive(Debug, Deserialize)]
struct LenientChoice {
    #[serde(default)]
    delta: LenientDelta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct LenientDelta {
    #[serde(default)]
    content: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data(payload: &str) -> SseEvent {
        SseEvent::Data(payload.to_string())
    }

    #[test]
    fn parses_single_event_with_space() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: {\"a\":1}\n\n");
        assert_eq!(events, vec![data("{\"a\":1}")]);
    }

    #[test]
    fn parses_event_without_space_after_colon() {
        let mut decoder = SseDecoder::new();
        assert_eq!(decoder.push(b"data:{\"a\":1}\n\n"), vec![data("{\"a\":1}")]);
    }

    #[test]
    fn parses_crlf_events() {
        let mut decoder = SseDecoder::new();
        assert_eq!(
            decoder.push(b"data: one\r\n\r\ndata: two\r\n\r\n"),
            vec![data("one"), data("two")]
        );
    }

    #[test]
    fn parses_lone_cr_terminators() {
        let mut decoder = SseDecoder::new();
        let mut events = decoder.push(b"data: one\r\rdata: two\r\r");
        // 末尾的孤立 `\r` 可能是被切断的 CRLF，必须等 finish 才能确定
        events.extend(decoder.finish());
        assert_eq!(events, vec![data("one"), data("two")]);
    }

    #[test]
    fn detects_done_marker() {
        let mut decoder = SseDecoder::new();
        assert_eq!(
            decoder.push(b"data: {\"x\":1}\n\ndata: [DONE]\n\n"),
            vec![data("{\"x\":1}"), SseEvent::Done]
        );
    }

    #[test]
    fn done_without_trailing_blank_line_is_flushed_by_finish() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: [DONE]").is_empty());
        assert_eq!(decoder.finish(), vec![SseEvent::Done]);
    }

    #[test]
    fn done_marker_with_surrounding_spaces_is_still_done() {
        let mut decoder = SseDecoder::new();
        assert_eq!(decoder.push(b"data:  [DONE] \n\n"), vec![SseEvent::Done]);
    }

    #[test]
    fn joins_multi_line_data_payloads() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: line1\ndata: line2\ndata: line3\n\n");
        assert_eq!(events, vec![data("line1\nline2\nline3")]);
    }

    #[test]
    fn ignores_comments_and_other_fields() {
        let mut decoder = SseDecoder::new();
        let events =
            decoder.push(b": heartbeat\nevent: message\nid: 42\nretry: 100\ndata: payload\n\n");
        assert_eq!(events, vec![data("payload")]);
    }

    #[test]
    fn ignores_blank_lines_without_pending_data() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"\n\n\r\n\r\n").is_empty());
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn empty_data_payload_is_still_an_event() {
        let mut decoder = SseDecoder::new();
        assert_eq!(decoder.push(b"data:\n\n"), vec![data("")]);
        let mut decoder = SseDecoder::new();
        assert_eq!(decoder.push(b"data\n\n"), vec![data("")]);
    }

    #[test]
    fn survives_every_possible_chunk_split() {
        // 关键测试：把两个事件 + [DONE] 的字节流在所有可能的位置切开，
        // 结果必须是同一串事件（跨 chunk 从中间切断是真实网络下的常态）。
        let payload: &[u8] = b"data: {\"choices\":[{\"delta\":{\"content\":\"\xe4\xbd\xa0\"}}]}\r\n\r\ndata: second\n\ndata: [DONE]\n\n";
        let expected = vec![
            data("{\"choices\":[{\"delta\":{\"content\":\"你\"}}]}"),
            data("second"),
            SseEvent::Done,
        ];

        for split in 0..=payload.len() {
            let mut decoder = SseDecoder::new();
            let mut events = Vec::new();
            events.extend(decoder.push(&payload[..split]));
            events.extend(decoder.push(&payload[split..]));
            events.extend(decoder.finish());
            assert_eq!(events, expected, "在 {split} 处切断时解析结果不一致");
        }
    }

    #[test]
    fn survives_byte_by_byte_feeding() {
        let payload = b"data: a\n\ndata: b\n\ndata: [DONE]\n\n".to_vec();
        let mut decoder = SseDecoder::new();
        let mut events = Vec::new();
        for byte in &payload {
            events.extend(decoder.push(std::slice::from_ref(byte)));
        }
        events.extend(decoder.finish());
        assert_eq!(events, vec![data("a"), data("b"), SseEvent::Done]);
    }

    #[test]
    fn keeps_multibyte_characters_split_across_chunks() {
        let text = "夺心魔".as_bytes();
        let mut decoder = SseDecoder::new();
        let mut events = Vec::new();
        events.extend(decoder.push(b"data: "));
        for byte in text {
            events.extend(decoder.push(std::slice::from_ref(byte)));
        }
        events.extend(decoder.push(b"\n\n"));
        assert_eq!(events, vec![data("夺心魔")]);
    }

    #[test]
    fn finish_flushes_unterminated_line() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"data: tail").is_empty());
        assert_eq!(decoder.finish(), vec![data("tail")]);
        // 再调一次不应重复产出
        assert!(decoder.finish().is_empty());
    }

    #[test]
    fn repeated_events_on_one_chunk_are_all_returned() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(b"data: 1\n\ndata: 2\n\ndata: 3\n\n");
        assert_eq!(events, vec![data("1"), data("2"), data("3")]);
    }

    #[test]
    fn trailing_garbage_without_terminator_is_ignored_on_empty_finish() {
        let mut decoder = SseDecoder::new();
        assert!(decoder.finish().is_empty());
        let mut decoder = SseDecoder::new();
        assert!(decoder.push(b"").is_empty());
        assert!(decoder.finish().is_empty());
    }

    // ── parse_chat_chunk ──

    #[test]
    fn parse_chat_chunk_extracts_delta_content() {
        // 字段不全的 chunk：标准协议类型解析不了，靠宽松兜底拿下
        let chunk =
            r#"{"id":"1","choices":[{"index":0,"delta":{"content":"你好"},"finish_reason":null}]}"#;
        assert_eq!(parse_chat_chunk(chunk).as_deref(), Some("你好"));
    }

    #[test]
    fn parse_chat_chunk_accepts_a_complete_protocol_chunk() {
        // 字段齐全：走 async-openai 标准协议类型那条路（类型化解析）
        let chunk = r#"{"id":"chatcmpl-1","object":"chat.completion.chunk","created":1700000000,"model":"deepseek-chat","choices":[{"index":0,"delta":{"role":"assistant","content":"火球"},"finish_reason":null}]}"#;
        assert_eq!(parse_chat_chunk(chunk).as_deref(), Some("火球"));
    }

    #[test]
    fn parse_chat_chunk_agrees_between_typed_and_lenient_paths() {
        // 同一条增量，字段齐全 vs 最小写法，两级解析必须给出同一个结果
        let full = r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{"content":"一致"},"finish_reason":null}]}"#;
        let minimal = r#"{"choices":[{"delta":{"content":"一致"}}]}"#;
        assert_eq!(parse_chat_chunk(full), parse_chat_chunk(minimal));
        assert_eq!(parse_chat_chunk(full).as_deref(), Some("一致"));
    }

    #[test]
    fn parse_chat_chunk_tolerates_missing_pieces() {
        // content 为 null
        assert_eq!(
            parse_chat_chunk(r#"{"choices":[{"delta":{"content":null}}]}"#),
            None
        );
        // 只有 finish_reason 的收尾 chunk
        assert_eq!(
            parse_chat_chunk(r#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#),
            None
        );
        // 没有 choices
        assert_eq!(parse_chat_chunk(r#"{"choices":[]}"#), None);
        assert_eq!(parse_chat_chunk(r#"{}"#), None);
        // 非 JSON（心跳/注释）
        assert_eq!(parse_chat_chunk("ping"), None);
        assert_eq!(parse_chat_chunk(""), None);
    }

    // ── finish_reason（截断信号）──

    #[test]
    fn parts_expose_finish_reason_on_both_parse_paths() {
        let full = r#"{"id":"c1","object":"chat.completion.chunk","created":1,"model":"m","choices":[{"index":0,"delta":{},"finish_reason":"length"}]}"#;
        let minimal = r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#;
        for payload in [full, minimal] {
            let chunk = parse_chat_chunk_parts(payload).expect("应能解析");
            assert_eq!(chunk.finish_reason.as_deref(), Some("length"));
            assert_eq!(chunk.truncation_reason(), Some("length"));
            assert_eq!(chunk.content, None);
        }
        // `stop` / 缺失都不是截断
        for payload in [
            r#"{"choices":[{"delta":{"content":"x"},"finish_reason":"stop"}]}"#,
            r#"{"choices":[{"delta":{"content":"x"}}]}"#,
            r#"{"choices":[{"delta":{"content":"x"},"finish_reason":null}]}"#,
        ] {
            let chunk = parse_chat_chunk_parts(payload).expect("应能解析");
            assert_eq!(chunk.truncation_reason(), None, "payload={payload}");
        }
        // 内容过滤同样算「不完整」
        assert_eq!(
            parse_chat_chunk_parts(
                r#"{"choices":[{"delta":{},"finish_reason":"content_filter"}]}"#
            )
            .unwrap()
            .truncation_reason(),
            Some("content_filter")
        );
        // content 的解析结果与旧 API 逐字一致
        assert_eq!(
            parse_chat_chunk_parts(r#"{"choices":[{"delta":{"content":"你好"}}]}"#)
                .unwrap()
                .content
                .as_deref(),
            parse_chat_chunk(r#"{"choices":[{"delta":{"content":"你好"}}]}"#).as_deref()
        );
    }

    // ── 内存上限（服务端不发换行也不能把内存吃光）──

    #[test]
    fn oversized_line_is_rejected_instead_of_buffered_forever() {
        let mut decoder = SseDecoder::new();
        let mut payload = b"data: ".to_vec();
        payload.resize(MAX_LINE_BYTES + 1, b'a');
        let err = decoder.push_checked(&payload).unwrap_err();
        assert!(err.to_string().contains("没有换行"), "实际: {err}");
        // 公开的 push 不带上限（保持既有语义）：同样的输入不报错
        let mut lenient = SseDecoder::new();
        assert!(lenient.push(&payload).is_empty());
        // 残留缓冲超限时 finish_checked 也要报
        let mut tail = SseDecoder::new();
        let err = tail.finish_checked();
        assert!(err.is_ok(), "空缓冲不该报错");
        let mut leftover = SseDecoder::new();
        let mut payload = b"data: ".to_vec();
        payload.resize(MAX_LINE_BYTES + 1, b'a');
        leftover.push(&payload);
        assert!(leftover.finish_checked().is_err());
    }

    #[test]
    fn long_but_terminated_lines_are_still_fine() {
        // 上限只针对「一直没有换行」的行，正常的长行/多行事件不受影响
        let mut decoder = SseDecoder::new();
        let mut payload = b"data: ".to_vec();
        payload.resize(MAX_LINE_BYTES - 1, b'a');
        payload.extend_from_slice(b"\n\n");
        let events = decoder.push_checked(&payload).unwrap();
        assert_eq!(events.len(), 1);
        assert!(decoder.finish_checked().unwrap().is_empty());
    }

    #[test]
    fn oversized_event_accumulation_is_rejected() {
        let mut decoder = SseDecoder::new();
        let line = {
            let mut line = b"data: ".to_vec();
            line.resize(MAX_LINE_BYTES, b'a');
            line.push(b'\n');
            line
        };
        let mut err = None;
        // 多行 data 之间没有空行 → 事件一直累积
        for _ in 0..8 {
            if let Err(e) = decoder.push_checked(&line) {
                err = Some(e);
                break;
            }
        }
        let err = err.expect("事件累积超过上限必须报错");
        assert!(err.to_string().contains("单个事件"), "实际: {err}");
    }

    #[test]
    fn parse_chat_chunk_returns_empty_string_for_empty_content() {
        // 空增量由调用方过滤（与旧实现一致）
        assert_eq!(
            parse_chat_chunk(r#"{"choices":[{"delta":{"content":""}}]}"#).as_deref(),
            Some("")
        );
    }

    #[test]
    fn decoder_output_feeds_parse_chat_chunk() {
        let mut decoder = SseDecoder::new();
        let events = decoder.push(
            b"data: {\"choices\":[{\"delta\":{\"content\":\"Fire\"}}]}\n\ndata: {\"choices\":[{\"delta\":{\"content\":\"ball\"}}]}\n\ndata: [DONE]\n\n",
        );
        let mut text = String::new();
        for event in events {
            match event {
                SseEvent::Data(payload) => {
                    if let Some(delta) = parse_chat_chunk(&payload) {
                        text.push_str(&delta);
                    }
                }
                SseEvent::Done => break,
            }
        }
        assert_eq!(text, "Fireball");
    }
}
