//! LLM 流式翻译引擎：并发调度 + 指数退避重试 + 取消 + 事件推送。
//!
//! 设计要点：
//! - **每条（或每组）一次请求、各自流式**：token 实时推给对应条目，
//!   丢弃旧的"批量 JSON"模式（要整批生成完才有反馈，体验极差）；
//! - **`Semaphore` 控并发**：`settings.concurrency`（已在 `normalized()` 里
//!   clamp 到 1..=64）；
//! - **`CancelToken` 贯穿始终**：请求、流读取、退避等待都 `select!` 取消；
//! - **「文本 → 译文」抽象成 [`TextTranslator`]**：生产实现走 reqwest，
//!   单测注入假实现，不需要真实网络也不需要 mock 框架。
//!
//! 关于「每个 job 一个 `tokio::spawn`」：`RunOptions.sink` 是借用
//! （`&'a dyn EventSink`），而 `tokio::spawn` 要求 `'static`；core 的
//! tokio 也没有开 `rt` feature。因此这里用 [`FuturesUnordered`] 并发轮询
//! 各个 job future —— 网络 I/O 由 tokio 的 reactor 驱动，并发度由
//! `Semaphore` 严格限制，行为与 spawn 版本一致。
//!
//! [`FuturesUnordered`]: futures_util::stream::FuturesUnordered

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::future::BoxFuture;
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;

use crate::error::{AppError, Result};
use crate::glossary::{GlossaryMatcher, MatchedTerm};
use crate::types::{LlmSettings, TranslationEntry, TranslationEvent};

use super::planner::{TranslationJob, TranslationOutput, plan_jobs, planned_entry_count};
use super::prompt::{SYSTEM_PROMPT, build_user_prompt, normalize_style_hint};
use super::series::{ConsistencyTerm, compose_variant_translation};
use super::sse::{SseDecoder, SseEvent, parse_chat_chunk};

/// 单次 HTTP 请求的超时（含流式读完）。
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// 最多重试 3 次（加上首次尝试共 4 次请求）。
const MAX_RETRIES: usize = 3;

/// 失败后的指数退避序列：500ms → 1000ms → 2000ms。
const RETRY_BACKOFF: [Duration; MAX_RETRIES] = [
    Duration::from_millis(500),
    Duration::from_millis(1000),
    Duration::from_millis(2000),
];

/// 单个 job 的总尝试次数。
const MAX_ATTEMPTS: usize = MAX_RETRIES + 1;

/// 取消轮询粒度：与旧实现一致，最多 100ms 延迟生效。
const CANCEL_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// API 错误响应里保留的最大字符数（避免把整个 HTML 错误页塞进 UI）。
const ERROR_BODY_LIMIT: usize = 500;

// ─────────────────────────────────────────────────────────────
// 事件出口与取消令牌
// ─────────────────────────────────────────────────────────────

/// 事件出口：真实运行时是 Tauri Channel，单测里是 [`CollectingSink`]。
///
/// `emit` 不返回错误：底层通道发送失败只记日志（旧实现的 Tauri Channel
/// 失败也是被忽略的），翻译流程不应该因为前端断开就崩掉。
pub trait EventSink: Send + Sync + 'static {
    fn emit(&self, event: TranslationEvent);
}

/// 收集事件的内存 sink，供单测断言事件序列。
#[derive(Debug, Default)]
pub struct CollectingSink {
    events: Mutex<Vec<TranslationEvent>>,
}

impl CollectingSink {
    pub fn new() -> Self {
        Self::default()
    }

    /// 复制一份已收集的事件。
    pub fn events(&self) -> Vec<TranslationEvent> {
        self.lock().clone()
    }

    /// 取出全部事件（消费 sink）。
    pub fn into_events(self) -> Vec<TranslationEvent> {
        self.events
            .into_inner()
            .unwrap_or_else(|err| err.into_inner())
    }

    /// 中毒的锁也要能用：断言失败导致 panic 后仍希望看到已收集的事件。
    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<TranslationEvent>> {
        self.events.lock().unwrap_or_else(|err| err.into_inner())
    }
}

impl EventSink for CollectingSink {
    fn emit(&self, event: TranslationEvent) {
        self.lock().push(event);
    }
}

/// 可克隆的取消令牌（所有克隆共享同一个标志）。
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    /// 请求取消；已在飞行中的请求会尽快返回 `Ok(None)`。
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    /// 清除取消标志，供下一次翻译复用同一个令牌。
    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// 轮询等待取消。
///
/// 用轮询而不是通知，是为了让 `CancelToken` 保持"一个 AtomicBool"这么简单；
/// 代价是取消最多延迟 [`CANCEL_POLL_INTERVAL`] 生效（与旧实现一致）。
async fn wait_until_cancelled(cancel: CancelToken) {
    while !cancel.is_cancelled() {
        tokio::time::sleep(CANCEL_POLL_INTERVAL).await;
    }
}

// ─────────────────────────────────────────────────────────────
// 文本 → 译文 的抽象
// ─────────────────────────────────────────────────────────────

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
}

/// 基于 reqwest 的 OpenAI 兼容实现。
#[derive(Debug)]
struct HttpTranslator {
    client: reqwest::Client,
    settings: LlmSettings,
}

impl TextTranslator for HttpTranslator {
    fn translate<'a>(
        &'a self,
        request: TranslateRequest<'a>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(self.translate_once(request))
    }
}

impl HttpTranslator {
    /// 单次尝试：发起请求 + 解析 SSE 流 + 推送增量。
    async fn translate_once(&self, request: TranslateRequest<'_>) -> Result<Option<String>> {
        if request.cancel.is_cancelled() {
            return Ok(None);
        }

        let user_prompt = build_user_prompt(
            request.source,
            request.matches,
            request.consistency_terms,
            request.style_hint,
        );
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

/// 从 panic payload 里取一句人话（`panic!("...")` 的两种形式）。
fn panic_message(payload: &Box<dyn Any + Send>) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "未知 panic".to_string()
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

// ─────────────────────────────────────────────────────────────
// 引擎
// ─────────────────────────────────────────────────────────────

/// 一次翻译运行的结果。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranslationSummary {
    /// 待翻译条目数（不含已有译文的条目）
    pub total: usize,
    /// 最终拿到译文的条目数（含一致性记忆直接复用的）
    pub translated: usize,
    /// 失败的条目数
    pub failed: usize,
    /// 本次运行是否被取消
    pub cancelled: bool,
}

/// 运行参数。
pub struct RunOptions<'a> {
    /// 用户填写的 MOD 语境提示（会被截断到 1200 字符）
    pub style_hint: &'a str,
    /// 事件出口
    pub sink: &'a dyn EventSink,
    /// 取消令牌
    pub cancel: CancelToken,
}

/// LLM 翻译引擎。
pub struct TranslationEngine {
    settings: LlmSettings,
    translator: Arc<dyn TextTranslator>,
    /// 重试退避（单测会缩短，避免每个失败用例都真等 3.5 秒）
    retry_backoff: [Duration; MAX_RETRIES],
}

impl TranslationEngine {
    /// 用调用方提供的 HTTP 客户端构造（客户端超时/代理等由调用方决定）。
    pub fn new(client: reqwest::Client, settings: LlmSettings) -> Self {
        let settings = settings.normalized();
        let translator = Arc::new(HttpTranslator {
            client,
            settings: settings.clone(),
        });
        Self {
            settings,
            translator,
            retry_backoff: RETRY_BACKOFF,
        }
    }

    /// 用内置默认 HTTP 客户端构造（应用层用这个）。
    ///
    /// 客户端配置：连接超时 15s、整体请求超时 60s、连接池空闲 90s。
    /// 放在 core 里是为了不让 `reqwest` 类型泄漏到 Tauri 壳。
    pub fn with_settings(settings: LlmSettings) -> Result<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(REQUEST_TIMEOUT)
            .pool_idle_timeout(Duration::from_secs(90))
            .build()
            .map_err(|err| AppError::Llm(format!("HTTP 客户端初始化失败: {err}")))?;
        Ok(Self::new(client, settings))
    }

    /// 注入自定义「文本 → 译文」实现（单测用假实现；也可用于自建网关）。
    pub fn with_translator(translator: Arc<dyn TextTranslator>, settings: LlmSettings) -> Self {
        Self {
            settings: settings.normalized(),
            translator,
            retry_backoff: RETRY_BACKOFF,
        }
    }

    /// 归一化后的设置（并发数等）。
    pub fn settings(&self) -> &LlmSettings {
        &self.settings
    }

    /// 批量流式翻译入口。
    ///
    /// - 只翻译 `is_pending_translation()` 的条目；
    /// - 同 MOD 已有译文的条目在规划阶段直接完成（发 `Progress` + `Done`）；
    /// - 事件语义：`Progress` → `Delta`(多次) → `Done`，失败发 `Error`，
    ///   最后一定发一次 `AllDone { total, failed }`（取消也发）。
    pub async fn run(
        &self,
        entries: &[TranslationEntry],
        matcher: &GlossaryMatcher,
        options: RunOptions<'_>,
    ) -> Result<TranslationSummary> {
        let RunOptions {
            style_hint,
            sink,
            cancel,
        } = options;
        let style_hint = normalize_style_hint(style_hint);

        let (jobs, plan) = plan_jobs(entries, matcher, sink);
        log::info!(
            "翻译规划：待翻译 {} 条，一致性直接完成 {} 条，请求 {} 个（参与匹配的术语 {} 条）",
            plan.total,
            plan.skipped,
            plan.jobs,
            matcher.len()
        );
        if plan.skipped + planned_entry_count(&jobs) != plan.total {
            log::warn!(
                "翻译规划覆盖不完整：待翻译 {} 条，实际覆盖 {} 条",
                plan.total,
                plan.skipped + planned_entry_count(&jobs)
            );
        }

        if plan.total == 0 {
            sink.emit(TranslationEvent::AllDone {
                total: 0,
                failed: 0,
            });
            return Ok(TranslationSummary::default());
        }

        let semaphore = Semaphore::new(self.settings.concurrency);
        let mut pending = FuturesUnordered::new();
        for job in jobs {
            let translator = self.translator.as_ref();
            let style_hint = style_hint.as_deref();
            let retry_backoff = &self.retry_backoff;
            // 单个 job panic 不能带走整轮翻译（旧实现靠 JoinHandle 捕获任务 panic，
            // 这里用 catch_unwind 达到同样效果）：客户端会一直等 AllDone。
            pending.push(
                AssertUnwindSafe(run_job(
                    translator,
                    style_hint,
                    sink,
                    &cancel,
                    retry_backoff,
                    &semaphore,
                    job,
                ))
                .catch_unwind(),
            );
        }

        let mut outcome = JobOutcome {
            translated: plan.skipped,
            failed: 0,
        };
        while let Some(result) = pending.next().await {
            match result {
                Ok(Ok(job_outcome)) => {
                    outcome.translated += job_outcome.translated;
                    outcome.failed += job_outcome.failed;
                }
                // job 里只有信号量异常会走到这里，按旧实现记为 1 条失败
                Ok(Err(err)) => {
                    outcome.failed += 1;
                    log::error!("翻译任务失败: {err}");
                }
                Err(panic) => {
                    outcome.failed += 1;
                    log::error!("翻译任务 panic: {}", panic_message(&panic));
                }
            }
        }

        sink.emit(TranslationEvent::AllDone {
            total: plan.total,
            failed: outcome.failed,
        });
        Ok(TranslationSummary {
            total: plan.total,
            translated: outcome.translated,
            failed: outcome.failed,
            cancelled: cancel.is_cancelled(),
        })
    }
}

/// 单个 job 的产出统计（条目粒度）。
#[derive(Debug, Default, Clone, Copy)]
struct JobOutcome {
    translated: usize,
    failed: usize,
}

/// 取并发配额后执行一个 job。
///
/// 配额在 future 内部获取而不是在入队前获取：`FuturesUnordered` 由当前任务
/// 轮询，如果父任务阻塞在 `acquire()` 上就没人推进已在跑的 future 了（死锁）。
async fn run_job(
    translator: &dyn TextTranslator,
    style_hint: Option<&str>,
    sink: &dyn EventSink,
    cancel: &CancelToken,
    retry_backoff: &[Duration; MAX_RETRIES],
    semaphore: &Semaphore,
    job: TranslationJob,
) -> Result<JobOutcome> {
    // 已经取消：连 Progress 都不发（旧实现同样在取配额前后各检查一次）
    if cancel.is_cancelled() {
        return Ok(JobOutcome::default());
    }
    let _permit = tokio::select! {
        permit = semaphore.acquire() => permit
            .map_err(|err| AppError::Llm(format!("并发信号量错误: {err}")))?,
        () = wait_until_cancelled(cancel.clone()) => return Ok(JobOutcome::default()),
    };
    if cancel.is_cancelled() {
        return Ok(JobOutcome::default());
    }

    let target_ids = job.target_ids();
    for entry_id in &target_ids {
        emit_progress(sink, entry_id);
    }

    // 系列组不推 Delta：成员各自拼后缀，中间态没有意义
    let stream_entry_ids: Vec<String> = match &job.output {
        TranslationOutput::Series { .. } => Vec::new(),
        _ => target_ids.clone(),
    };

    let translated = match translate_with_retry(
        translator,
        style_hint,
        sink,
        cancel,
        retry_backoff,
        &job,
        &stream_entry_ids,
    )
    .await
    {
        Ok(Some(text)) => text,
        // 取消：不记失败，让前端保留"未翻译"状态
        Ok(None) => return Ok(JobOutcome::default()),
        Err(err) => {
            let message = err.to_string();
            for entry_id in &target_ids {
                sink.emit(TranslationEvent::error(entry_id, message.clone()));
            }
            return Ok(JobOutcome {
                translated: 0,
                failed: target_ids.len(),
            });
        }
    };

    match job.output {
        TranslationOutput::Single { entry_id } => {
            sink.emit(TranslationEvent::done(&entry_id, translated));
        }
        TranslationOutput::ExactGroup { entry_ids } => {
            for entry_id in entry_ids {
                sink.emit(TranslationEvent::done(&entry_id, translated.clone()));
            }
        }
        TranslationOutput::Series { members } => {
            for member in members {
                sink.emit(TranslationEvent::done(
                    &member.entry_id,
                    compose_variant_translation(&translated, &member.suffix),
                ));
            }
        }
    }

    Ok(JobOutcome {
        translated: target_ids.len(),
        failed: 0,
    })
}

/// 翻译一段文本，带指数退避重试（最多 3 次重试：500ms / 1000ms / 2000ms）。
async fn translate_with_retry(
    translator: &dyn TextTranslator,
    style_hint: Option<&str>,
    sink: &dyn EventSink,
    cancel: &CancelToken,
    retry_backoff: &[Duration; MAX_RETRIES],
    job: &TranslationJob,
    stream_entry_ids: &[String],
) -> Result<Option<String>> {
    let mut last_err = String::new();
    for attempt in 0..MAX_ATTEMPTS {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        let request = TranslateRequest {
            source: &job.source,
            matches: &job.matches,
            consistency_terms: &job.consistency_terms,
            style_hint,
            stream_entry_ids,
            sink,
            cancel,
        };
        match translator.translate(request).await {
            Ok(Some(text)) => return Ok(Some(text)),
            Ok(None) => return Ok(None),
            Err(err) => {
                last_err = err.to_string();
                log::warn!("[{}] 第 {} 次尝试失败: {last_err}", job.source, attempt + 1);
                if let Some(backoff) = retry_backoff.get(attempt) {
                    tokio::select! {
                        () = tokio::time::sleep(*backoff) => {}
                        () = wait_until_cancelled(cancel.clone()) => return Ok(None),
                    }
                }
            }
        }
    }
    Err(AppError::Llm(last_err))
}

/// 单测用：缩短退避，避免每个失败用例都真等 3.5 秒。
#[cfg(test)]
impl TranslationEngine {
    fn with_retry_backoff(mut self, backoff: [Duration; MAX_RETRIES]) -> Self {
        self.retry_backoff = backoff;
        self
    }
}

/// 发「开始翻译」。
pub(crate) fn emit_progress(sink: &dyn EventSink, entry_id: &str) {
    sink.emit(TranslationEvent::progress(entry_id));
}

/// 发「开始翻译 + 完成」（规划阶段的直接完成走这里，与旧 `send_done` 一致）。
pub(crate) fn emit_done(sink: &dyn EventSink, entry_id: &str, text: String) {
    emit_progress(sink, entry_id);
    sink.emit(TranslationEvent::done(entry_id, text));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    use crate::glossary::Glossary;
    use crate::types::TranslationStatus;

    /// 手写假实现：记录调用、统计并发、可控失败与取消。
    #[derive(Debug, Default)]
    struct FakeTranslator {
        /// 固定译文；`None` 表示回显 `译:{source}`
        output: Option<String>,
        /// 每次翻译的耗时（模拟网络）
        delay: Duration,
        /// 还剩几次调用直接失败（测重试）
        failures_left: AtomicUsize,
        /// 一直等到取消（测取消）
        wait_for_cancel: bool,
        /// 是否推送流式增量
        stream_deltas: bool,
        /// 每次调用直接 panic（测 job 级 panic 隔离）
        panic_on_call: bool,
        calls: Mutex<Vec<String>>,
        deltas: Mutex<Vec<(String, String)>>,
        inflight: AtomicUsize,
        max_inflight: AtomicUsize,
    }

    impl FakeTranslator {
        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn deltas(&self) -> Vec<(String, String)> {
            self.deltas.lock().unwrap().clone()
        }

        fn max_inflight(&self) -> usize {
            self.max_inflight.load(Ordering::SeqCst)
        }
    }

    impl TextTranslator for FakeTranslator {
        fn translate<'a>(
            &'a self,
            request: TranslateRequest<'a>,
        ) -> BoxFuture<'a, Result<Option<String>>> {
            Box::pin(async move {
                self.calls.lock().unwrap().push(request.source.to_string());
                if self.panic_on_call {
                    panic!("假实现：故意 panic");
                }
                let now = self.inflight.fetch_add(1, Ordering::SeqCst) + 1;
                self.max_inflight.fetch_max(now, Ordering::SeqCst);

                if self.wait_for_cancel {
                    while !request.cancel.is_cancelled() {
                        tokio::time::sleep(Duration::from_millis(20)).await;
                    }
                    self.inflight.fetch_sub(1, Ordering::SeqCst);
                    return Ok(None);
                }
                if !self.delay.is_zero() {
                    tokio::time::sleep(self.delay).await;
                }
                self.inflight.fetch_sub(1, Ordering::SeqCst);

                if self.failures_left.load(Ordering::SeqCst) > 0 {
                    self.failures_left.fetch_sub(1, Ordering::SeqCst);
                    return Err(AppError::Llm("假实现：请求失败".into()));
                }

                let text = self
                    .output
                    .clone()
                    .unwrap_or_else(|| format!("译:{}", request.source));
                if self.stream_deltas {
                    // 按字符切两半：中文译文直接切字节会 panic
                    let chars: Vec<char> = text.chars().collect();
                    let (head, tail) = chars.split_at(chars.len() / 2);
                    for half in [head.iter().collect::<String>(), tail.iter().collect()] {
                        if half.is_empty() {
                            continue;
                        }
                        for entry_id in request.stream_entry_ids {
                            self.deltas
                                .lock()
                                .unwrap()
                                .push((entry_id.clone(), half.clone()));
                            request
                                .sink
                                .emit(TranslationEvent::delta(entry_id, half.clone()));
                        }
                    }
                }
                Ok(Some(text))
            })
        }
    }

    fn settings(concurrency: usize) -> LlmSettings {
        LlmSettings {
            concurrency,
            ..LlmSettings::default()
        }
    }

    fn engine_with(fake: Arc<FakeTranslator>, concurrency: usize) -> TranslationEngine {
        TranslationEngine::with_translator(fake, settings(concurrency))
    }

    /// 退避缩到 1ms：失败用例不必真等 3.5 秒。
    fn engine_with_fast_retry(fake: Arc<FakeTranslator>, concurrency: usize) -> TranslationEngine {
        engine_with(fake, concurrency).with_retry_backoff([Duration::from_millis(1); MAX_RETRIES])
    }

    fn entry(source: &str, uid: &str) -> TranslationEntry {
        TranslationEntry::new("test.loca", uid, "1", source)
    }

    fn matcher(pairs: &[(&str, &str)]) -> GlossaryMatcher {
        GlossaryMatcher::new(&Glossary {
            terms: pairs
                .iter()
                .map(|(source, target)| crate::glossary::GlossaryEntry {
                    source: (*source).to_string(),
                    target: (*target).to_string(),
                    ..Default::default()
                })
                .collect(),
        })
    }

    fn run_options<'a>(
        sink: &'a CollectingSink,
        cancel: &CancelToken,
        style_hint: &'a str,
    ) -> RunOptions<'a> {
        RunOptions {
            style_hint,
            sink,
            cancel: cancel.clone(),
        }
    }

    fn done_events(events: &[TranslationEvent]) -> Vec<(String, String)> {
        events
            .iter()
            .filter_map(|event| match event {
                TranslationEvent::Done { entry_id, text } => Some((entry_id.clone(), text.clone())),
                _ => None,
            })
            .collect()
    }

    fn error_events(events: &[TranslationEvent]) -> Vec<String> {
        events
            .iter()
            .filter_map(|event| match event {
                TranslationEvent::Error { entry_id, .. } => Some(entry_id.clone()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn retry_backoff_is_exponential_and_bounded() {
        assert_eq!(
            RETRY_BACKOFF,
            [
                Duration::from_millis(500),
                Duration::from_millis(1000),
                Duration::from_millis(2000),
            ]
        );
        assert_eq!(MAX_RETRIES, 3, "最多重试 3 次");
        assert_eq!(MAX_ATTEMPTS, 4, "首次 + 3 次重试");
    }

    #[tokio::test]
    async fn retry_uses_the_configured_backoff_schedule() {
        // 真等一次 500ms，证明生产退避确实接到了重试路径上
        let fake = Arc::new(FakeTranslator {
            failures_left: AtomicUsize::new(1),
            ..FakeTranslator::default()
        });
        let engine = engine_with(fake.clone(), 1);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();

        let started = std::time::Instant::now();
        let summary = engine
            .run(
                &[entry("Fireball", "u1")],
                &matcher(&[]),
                run_options(&sink, &cancel, ""),
            )
            .await
            .unwrap();

        assert_eq!(summary.translated, 1);
        assert_eq!(fake.calls().len(), 2);
        assert!(
            started.elapsed() >= Duration::from_millis(400),
            "第一次重试应等待 500ms 退避"
        );
    }

    #[test]
    fn cancel_token_is_shared_between_clones() {
        let token = CancelToken::new();
        let clone = token.clone();
        assert!(!token.is_cancelled());
        clone.cancel();
        assert!(token.is_cancelled(), "克隆共享同一个标志");
        token.reset();
        assert!(!clone.is_cancelled());
        assert!(!CancelToken::default().is_cancelled());
    }

    #[test]
    fn collecting_sink_records_events_in_order() {
        let sink = CollectingSink::new();
        sink.emit(TranslationEvent::progress("a"));
        sink.emit(TranslationEvent::done("a", "甲"));
        assert_eq!(
            sink.events(),
            vec![
                TranslationEvent::progress("a"),
                TranslationEvent::done("a", "甲")
            ]
        );
        assert_eq!(
            sink.into_events(),
            vec![
                TranslationEvent::progress("a"),
                TranslationEvent::done("a", "甲")
            ]
        );
    }

    #[test]
    fn new_normalizes_settings() {
        let engine = TranslationEngine::new(
            reqwest::Client::new(),
            LlmSettings {
                base_url: " https://x.test/ ".into(),
                api_key: " k ".into(),
                model: " m ".into(),
                concurrency: 999,
                temperature: 9.0,
            },
        );
        let settings = engine.settings();
        assert_eq!(settings.base_url, "https://x.test");
        assert_eq!(settings.api_key, "k");
        assert_eq!(settings.model, "m");
        assert_eq!(settings.concurrency, 64);
        assert_eq!(settings.temperature, 2.0);
        assert_eq!(
            settings.chat_completions_url(),
            "https://x.test/v1/chat/completions"
        );
    }

    #[test]
    fn with_settings_builds_default_client() {
        let engine = TranslationEngine::with_settings(settings(3)).unwrap();
        assert_eq!(engine.settings().concurrency, 3);
    }

    #[tokio::test]
    async fn run_emits_progress_delta_done_then_all_done() {
        let fake = Arc::new(FakeTranslator {
            stream_deltas: true,
            ..FakeTranslator::default()
        });
        let engine = engine_with(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let entries = vec![entry("Fireball", "u1")];

        let summary = engine
            .run(&entries, &matcher(&[]), run_options(&sink, &cancel, "语境"))
            .await
            .unwrap();

        assert_eq!(
            summary,
            TranslationSummary {
                total: 1,
                translated: 1,
                failed: 0,
                cancelled: false,
            }
        );
        let events = sink.events();
        assert_eq!(events[0], TranslationEvent::progress("test.loca#u1"));
        assert_eq!(
            events.last().unwrap(),
            &TranslationEvent::AllDone {
                total: 1,
                failed: 0
            }
        );
        assert_eq!(
            done_events(&events),
            vec![("test.loca#u1".to_string(), "译:Fireball".to_string())]
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, TranslationEvent::Delta { .. }))
        );
        assert_eq!(fake.calls(), vec!["Fireball".to_string()]);
        assert_eq!(fake.deltas().len(), 2, "假实现切了两段增量");
    }

    #[tokio::test]
    async fn duplicate_sources_share_one_request_and_one_translation() {
        let fake = Arc::new(FakeTranslator::default());
        let engine = engine_with(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let entries = vec![entry("Fireball", "u1"), entry("Fireball", "u2")];

        let summary = engine
            .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
            .await
            .unwrap();

        assert_eq!(
            fake.calls(),
            vec!["Fireball".to_string()],
            "重复原文只发一次请求"
        );
        assert_eq!(summary.translated, 2);
        assert_eq!(
            done_events(&sink.events()),
            vec![
                ("test.loca#u1".to_string(), "译:Fireball".to_string()),
                ("test.loca#u2".to_string(), "译:Fireball".to_string()),
            ]
        );
    }

    #[tokio::test]
    async fn series_group_emits_no_delta_and_composes_suffixes() {
        let fake = Arc::new(FakeTranslator {
            output: Some("银发".into()),
            stream_deltas: true,
            ..FakeTranslator::default()
        });
        let engine = engine_with(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let entries = vec![
            entry("Silver's Hair 9b", "u1"),
            entry("Silver's Hair 10", "u2"),
        ];

        let summary = engine
            .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
            .await
            .unwrap();

        assert_eq!(fake.calls(), vec!["Silver's Hair".to_string()], "只翻 base");
        assert_eq!(summary.translated, 2);
        assert_eq!(
            done_events(&sink.events()),
            vec![
                ("test.loca#u1".to_string(), "银发 9b".to_string()),
                ("test.loca#u2".to_string(), "银发 10".to_string()),
            ]
        );
        assert!(
            !sink
                .events()
                .iter()
                .any(|event| matches!(event, TranslationEvent::Delta { .. })),
            "系列组不应推 Delta"
        );
    }

    #[tokio::test]
    async fn memory_completed_entries_count_as_translated() {
        let fake = Arc::new(FakeTranslator::default());
        let engine = engine_with(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();

        let mut translated = entry("Fireball", "t1");
        translated.mark_translated("火球术");
        let entries = vec![translated, entry("Fireball", "u1")];

        let summary = engine
            .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
            .await
            .unwrap();

        assert_eq!(summary.total, 1);
        assert_eq!(summary.translated, 1);
        assert!(fake.calls().is_empty(), "一致性记忆命中不应发请求");
        assert_eq!(
            done_events(&sink.events()),
            vec![("test.loca#u1".to_string(), "火球术".to_string())]
        );
    }

    #[tokio::test]
    async fn empty_input_only_emits_all_done() {
        let fake = Arc::new(FakeTranslator::default());
        let engine = engine_with(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();

        let summary = engine
            .run(&[], &matcher(&[]), run_options(&sink, &cancel, ""))
            .await
            .unwrap();

        assert_eq!(summary, TranslationSummary::default());
        assert_eq!(
            sink.events(),
            vec![TranslationEvent::AllDone {
                total: 0,
                failed: 0
            }]
        );
    }

    #[tokio::test]
    async fn non_pending_entries_are_ignored() {
        let fake = Arc::new(FakeTranslator::default());
        let engine = engine_with(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let mut done = entry("Fireball", "t1");
        done.mark_translated("火球术");
        let mut errored = entry("Magic Missile", "t2");
        errored.target = "魔法飞弹".into();
        errored.mark_error("上次翻译失败，但译文已保留");

        let summary = engine
            .run(
                &[done, errored],
                &matcher(&[]),
                run_options(&sink, &cancel, ""),
            )
            .await
            .unwrap();
        assert_eq!(summary.total, 0);
        assert!(fake.calls().is_empty());
    }

    #[tokio::test]
    async fn failures_are_reported_per_entry() {
        let fake = Arc::new(FakeTranslator {
            failures_left: AtomicUsize::new(usize::MAX),
            ..FakeTranslator::default()
        });
        let engine = engine_with_fast_retry(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let entries = vec![entry("Fireball", "u1"), entry("Fireball", "u2")];

        let summary = engine
            .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
            .await
            .unwrap();

        assert_eq!(fake.calls().len(), MAX_ATTEMPTS, "同一条目组应重试到上限");
        assert_eq!(summary.failed, 2, "失败按条目计数");
        assert_eq!(summary.translated, 0);
        assert_eq!(
            error_events(&sink.events()),
            vec!["test.loca#u1".to_string(), "test.loca#u2".to_string()]
        );
        assert!(done_events(&sink.events()).is_empty());
        assert_eq!(
            sink.events().last().unwrap(),
            &TranslationEvent::AllDone {
                total: 2,
                failed: 2
            }
        );
    }

    #[tokio::test]
    async fn panicking_job_is_counted_as_failure_and_run_finishes() {
        // 这个用例会往 stderr 打一行 panic 信息：那是被 catch_unwind 捕获的预期输出
        let fake = Arc::new(FakeTranslator {
            panic_on_call: true,
            ..FakeTranslator::default()
        });
        let engine = engine_with_fast_retry(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let entries = vec![entry("Fireball", "u1"), entry("Magic Missile", "u2")];

        let summary = engine
            .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
            .await
            .unwrap();

        // 每个 job 的首次尝试就 panic，被算作失败；整轮 run 仍然正常收尾
        assert_eq!(summary.failed, 2);
        assert_eq!(summary.translated, 0);
        assert_eq!(
            sink.events().last().unwrap(),
            &TranslationEvent::AllDone {
                total: 2,
                failed: 2
            }
        );
    }

    #[tokio::test]
    async fn retries_until_success_without_duplicate_done() {
        let fake = Arc::new(FakeTranslator {
            failures_left: AtomicUsize::new(1),
            output: Some("火球术".into()),
            ..FakeTranslator::default()
        });
        let engine = engine_with(fake.clone(), 2);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();

        let summary = engine
            .run(
                &[entry("Fireball", "u1")],
                &matcher(&[]),
                run_options(&sink, &cancel, ""),
            )
            .await
            .unwrap();

        assert_eq!(fake.calls().len(), 2, "第一次失败后应重试");
        assert_eq!(summary.translated, 1);
        assert_eq!(summary.failed, 0);
        assert!(error_events(&sink.events()).is_empty());
        assert_eq!(
            done_events(&sink.events()),
            vec![("test.loca#u1".to_string(), "火球术".to_string())]
        );
    }

    #[tokio::test]
    async fn run_respects_concurrency_limit() {
        let fake = Arc::new(FakeTranslator {
            delay: Duration::from_millis(60),
            ..FakeTranslator::default()
        });
        let engine = engine_with(fake.clone(), 3);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        // 注意不能用 "Text 0" 这种尾巴带数字的原文：那会被识别为系列变体合并成一个 job
        let entries: Vec<_> = (0..10)
            .map(|i| entry(&format!("Unique text {i} here"), &format!("u{i}")))
            .collect();

        let summary = engine
            .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
            .await
            .unwrap();

        assert_eq!(summary.translated, 10);
        assert_eq!(fake.calls().len(), 10);
        assert!(
            fake.max_inflight() <= 3,
            "并发数超过 Semaphore 限制: {}",
            fake.max_inflight()
        );
        assert!(fake.max_inflight() >= 2, "并发没有真正发生");
    }

    #[tokio::test]
    async fn cancel_stops_in_flight_jobs() {
        let fake = Arc::new(FakeTranslator {
            wait_for_cancel: true,
            ..FakeTranslator::default()
        });
        let engine = engine_with(fake.clone(), 4);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        let entries: Vec<_> = (0..4)
            .map(|i| entry(&format!("Unique text {i} here"), &format!("u{i}")))
            .collect();

        let matcher = matcher(&[]);
        let run = engine.run(&entries, &matcher, run_options(&sink, &cancel, ""));
        let canceller = async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            cancel.cancel();
        };
        let (summary, ()) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(run, canceller)
        })
        .await
        .expect("取消后 run 必须在 5 秒内返回");

        assert!(summary.unwrap().cancelled);
        assert!(done_events(&sink.events()).is_empty(), "取消后不应有 Done");
        assert!(
            sink.events()
                .iter()
                .any(|event| matches!(event, TranslationEvent::Progress { .. }))
        );
        assert_eq!(
            sink.events().last().unwrap(),
            &TranslationEvent::AllDone {
                total: 4,
                failed: 0
            },
            "取消也要发 AllDone，且取消不计失败"
        );
    }

    #[tokio::test]
    async fn cancel_before_start_only_emits_all_done() {
        let fake = Arc::new(FakeTranslator::default());
        let engine = engine_with(fake.clone(), 4);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();
        cancel.cancel();

        let summary = engine
            .run(
                &[entry("Fireball", "u1")],
                &matcher(&[]),
                run_options(&sink, &cancel, ""),
            )
            .await
            .unwrap();

        assert!(summary.cancelled);
        assert_eq!(summary.translated, 0);
        assert!(fake.calls().is_empty(), "已取消时不应发起请求");
        assert_eq!(
            sink.events(),
            vec![TranslationEvent::AllDone {
                total: 1,
                failed: 0
            }]
        );
    }

    #[tokio::test]
    async fn glossary_matches_and_style_hint_reach_the_translator() {
        /// 记录最后一次请求内容的假实现。
        struct RecordingTranslator {
            last_prompt_input: Mutex<Option<(Vec<String>, Option<String>)>>,
        }

        impl TextTranslator for RecordingTranslator {
            fn translate<'a>(
                &'a self,
                request: TranslateRequest<'a>,
            ) -> BoxFuture<'a, Result<Option<String>>> {
                Box::pin(async move {
                    *self.last_prompt_input.lock().unwrap() = Some((
                        request
                            .matches
                            .iter()
                            .map(|m| format!("{} = {}", m.source, m.target))
                            .collect(),
                        request.style_hint.map(str::to_string),
                    ));
                    Ok(Some("火球术".into()))
                })
            }
        }

        let recorder = Arc::new(RecordingTranslator {
            last_prompt_input: Mutex::new(None),
        });
        let engine = TranslationEngine::with_translator(recorder.clone(), settings(2));
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();

        engine
            .run(
                &[entry("Cast Fireball", "u1")],
                &matcher(&[("Fireball", "火球术")]),
                run_options(&sink, &cancel, "  MOD 语境  "),
            )
            .await
            .unwrap();

        let recorded = recorder.last_prompt_input.lock().unwrap().clone().unwrap();
        assert_eq!(recorded.0, vec!["Fireball = 火球术".to_string()]);
        assert_eq!(recorded.1.as_deref(), Some("MOD 语境"));
    }

    #[test]
    fn truncate_chars_never_splits_multibyte() {
        assert_eq!(truncate_chars("夺心魔", 10), "夺心魔");
        assert_eq!(truncate_chars("夺心魔", 2), "夺心");
        assert_eq!(truncate_chars("", 3), "");
    }

    #[test]
    fn progress_events_carry_translating_status() {
        let sink = CollectingSink::new();
        emit_done(&sink, "e1", "译文".into());
        assert_eq!(
            sink.events(),
            vec![
                TranslationEvent::Progress {
                    entry_id: "e1".into(),
                    status: TranslationStatus::Translating,
                },
                TranslationEvent::done("e1", "译文"),
            ]
        );
    }
}
