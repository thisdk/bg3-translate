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

use async_openai::types::chat::CreateChatCompletionStreamResponse;
use serde::Deserialize;

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
    match serde_json::from_str::<CreateChatCompletionStreamResponse>(data) {
        Ok(chunk) => chunk
            .choices
            .into_iter()
            .next()
            .and_then(|choice| choice.delta.content),
        Err(_) => {
            let chunk: LenientChunk = serde_json::from_str(data).ok()?;
            chunk
                .choices
                .into_iter()
                .next()
                .and_then(|choice| choice.delta.content)
        }
    }
}

/// 宽松兜底结构：字段全可选，只为兼容「字段不全」的第三方网关。
///
/// 它不再是主解析路径，只是标准结构解析失败后的第二次机会，所以刻意做得最小：
/// 只关心 `choices[0].delta.content`。
#[derive(Debug, Deserialize)]
struct LenientChunk {
    #[serde(default)]
    choices: Vec<LenientChoice>,
}

#[derive(Debug, Deserialize)]
struct LenientChoice {
    #[serde(default)]
    delta: LenientDelta,
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
