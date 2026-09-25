//! LLM 流式翻译引擎：并发调度 + 事件推送（对外主入口 [`TranslationEngine`]）。
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
//! 子模块分工（拆分只改了文件位置，**公开路径与签名逐字未变**）：
//! - [`super::events`]：`EventSink` / `CollectingSink` / `CancelToken` / 进度事件
//! - [`super::translator`]：`TranslateRequest` / `TextTranslator` / `HttpTranslator`
//! - [`super::retry`]：网络退避重试 + 结构纠错重试
//! - [`super::fidelity`]：译文结构保真校验（占位符 / 富文本标签）
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
use std::sync::Arc;
use std::time::Duration;

use futures_util::FutureExt;
use futures_util::stream::{FuturesUnordered, StreamExt};
use tokio::sync::Semaphore;

use crate::error::{AppError, Result};
use crate::glossary::GlossaryMatcher;
use crate::types::{LlmSettings, TranslationEntry, TranslationEvent};

use super::events::{emit_progress, wait_until_cancelled};
use super::planner::{TranslationJob, TranslationOutput, plan_jobs, planned_entry_count};
use super::prompt::normalize_style_hint;
use super::retry::{MAX_RETRIES, RETRY_BACKOFF, translate_with_retry};
use super::series::compose_variant_translation;
use super::translator::{HttpTranslator, REQUEST_TIMEOUT};

// 对外路径保持 `translation::engine::{...}`（`translation` 又把它再导出一次）：
// 拆分前这些名字就定义在 engine.rs 里，任何 `use ...::engine::X` 的调用方
// 都不该因为这次拆分而改动。
pub(crate) use super::events::emit_done;
pub use super::events::{CancelToken, CollectingSink, EventSink};
pub use super::translator::{TextTranslator, TranslateRequest};

#[cfg(test)]
mod tests;

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
        let translator = Arc::new(HttpTranslator::new(client, settings.clone()));
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

    // 失败（含结构校验未通过、纠错重试后仍不合格）绝不写 Done：
    // 坏译文不能当成功结果落到条目上。
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

/// 单测用：缩短退避，避免每个失败用例都真等 3.5 秒。
#[cfg(test)]
impl TranslationEngine {
    pub(crate) fn with_retry_backoff(mut self, backoff: [Duration; MAX_RETRIES]) -> Self {
        self.retry_backoff = backoff;
        self
    }
}
