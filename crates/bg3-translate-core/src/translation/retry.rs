//! 重试策略：网络失败的指数退避重试 + 译文结构不合格时的一次纠错重试。
//!
//! 两条重试线路刻意分开：
//! - **网络失败**（请求失败 / 流读取失败 / API 报错）：按 500ms → 1000ms → 2000ms
//!   退避，最多重试 [`MAX_RETRIES`] 次，与旧实现完全一致；
//! - **结构不合格**（请求成功，但译文丢了占位符 / 标签）：只在首次成功后追加
//!   **一次** 带纠错提示的请求。它不占用网络重试额度，也不改变网络重试的语义。
//!
//! 两条线路都遵守取消令牌：退避等待、纠错请求之前都会检查 `CancelToken`，
//! 请求本身由实现（[`super::translator::HttpTranslator`]）`select!` 取消。
//! 并发上限由调用方持有的 `Semaphore` 许可保证 —— 纠错重试发生在同一个 job 内，
//! 不会多占配额，也不会放开并发。
//!
//! **每一次新的尝试开始时都会给流式条目重发一次 `Progress(translating)`**
//! （见 [`emit_attempt_progress`]）：前端据此知道「上一轮作废、重新累积」，
//! 否则两轮 delta 在 UI 上无法区分，会拼成 `坏译文 + 好译文`。
//! `Series` 组不推 delta，因此不发这个信号（保持既有事件序列不变）。

use std::time::Duration;

use crate::error::{AppError, Result};

use super::events::{CancelToken, EventSink, emit_progress, wait_until_cancelled};
use super::fidelity::{FidelityIssue, check_fidelity, correction_hint, summarize};
use super::planner::{TranslationJob, TranslationOutput};
use super::series::compose_variant_translation;
use super::translator::{API_STATUS_PREFIX, TextTranslator, TranslateRequest};

/// 最多重试 3 次（加上首次尝试共 4 次请求）。
pub(crate) const MAX_RETRIES: usize = 3;

/// 失败后的指数退避序列：500ms → 1000ms → 2000ms。
pub(crate) const RETRY_BACKOFF: [Duration; MAX_RETRIES] = [
    Duration::from_millis(500),
    Duration::from_millis(1000),
    Duration::from_millis(2000),
];

/// 单个 job 的总尝试次数。
pub(crate) const MAX_ATTEMPTS: usize = MAX_RETRIES + 1;

/// 翻译一个 job，直到拿到**结构保真**的译文。
///
/// 返回 `Ok(None)` 表示被取消（调用方不应记为失败）；`Err` 表示失败，
/// 消息里会写清楚是网络问题还是结构校验问题。
pub(crate) async fn translate_with_retry(
    translator: &dyn TextTranslator,
    style_hint: Option<&str>,
    sink: &dyn EventSink,
    cancel: &CancelToken,
    retry_backoff: &[Duration; MAX_RETRIES],
    job: &TranslationJob,
    stream_entry_ids: &[String],
) -> Result<Option<String>> {
    let Some(translated) = translate_attempts(
        translator,
        style_hint,
        sink,
        cancel,
        retry_backoff,
        job,
        stream_entry_ids,
    )
    .await?
    else {
        return Ok(None);
    };

    let issues = job_fidelity_issues(job, &translated);
    if issues.is_empty() {
        return Ok(Some(translated));
    }

    // 结构不合格：带纠错提示再请求一次（取消优先，取消后不再发请求）
    if cancel.is_cancelled() {
        return Ok(None);
    }
    log::warn!(
        "[{}] 译文结构校验未通过（{}），带纠错提示重试一次",
        job.source,
        summarize(&issues)
    );
    // 新一轮尝试开始：先发 progress，前端才会丢掉上一轮（被拒）的流式文本
    emit_attempt_progress(sink, stream_entry_ids);
    let hint = correction_hint(&issues);
    let request = build_request(job, style_hint, sink, cancel, stream_entry_ids);
    match translator
        .translate_with_correction(request, Some(&hint))
        .await
    {
        Ok(Some(corrected)) => {
            let remaining = job_fidelity_issues(job, &corrected);
            if remaining.is_empty() {
                Ok(Some(corrected))
            } else {
                log::warn!(
                    "[{}] 纠错重试后结构仍不合格：{}",
                    job.source,
                    summarize(&remaining)
                );
                Err(AppError::Llm(format!(
                    "结构校验未通过：{}（已重试 1 次）",
                    summarize(&remaining)
                )))
            }
        }
        // 取消：不记失败
        Ok(None) => Ok(None),
        Err(err) => Err(AppError::Llm(format!(
            "结构校验未通过：{}（已重试 1 次）；纠错重试失败：{}",
            summarize(&issues),
            error_message(&err)
        ))),
    }
}

/// 网络层面的重试循环（最多 3 次重试：500ms / 1000ms / 2000ms）。
///
/// **确定性失败不重试**：`401` 密钥错、`403` 无权限、`404` 路径错这类错误重试
/// 4 次只是让用户白等 3.5 秒再看到同一个错误（见 [`is_retryable_failure`]）。
async fn translate_attempts(
    translator: &dyn TextTranslator,
    style_hint: Option<&str>,
    sink: &dyn EventSink,
    cancel: &CancelToken,
    retry_backoff: &[Duration; MAX_RETRIES],
    job: &TranslationJob,
    stream_entry_ids: &[String],
) -> Result<Option<String>> {
    let mut last_err: Option<AppError> = None;
    for attempt in 0..MAX_ATTEMPTS {
        if cancel.is_cancelled() {
            return Ok(None);
        }
        if attempt > 0 {
            // 第 2 次及以后的尝试：重发 progress，让前端重置该条目的流式文本
            emit_attempt_progress(sink, stream_entry_ids);
        }
        let request = build_request(job, style_hint, sink, cancel, stream_entry_ids);
        match translator.translate(request).await {
            Ok(Some(text)) => return Ok(Some(text)),
            Ok(None) => return Ok(None),
            Err(err) => {
                log::warn!("[{}] 第 {} 次尝试失败: {err}", job.source, attempt + 1);
                if !is_retryable_failure(&err) {
                    log::warn!("[{}] 该错误重试也不会成功，不再重试", job.source);
                    return Err(err);
                }
                last_err = Some(err);
                if let Some(backoff) = retry_backoff.get(attempt) {
                    tokio::select! {
                        () = tokio::time::sleep(*backoff) => {}
                        () = wait_until_cancelled(cancel.clone()) => return Ok(None),
                    }
                }
            }
        }
    }
    // 原样抛出最后一次的错误：再包一层 `AppError::Llm` 会让用户看到
    // 「大模型调用错误: 大模型调用错误: …」，也会把非 LLM 的错误类别吞掉。
    match last_err {
        Some(err) => Err(err),
        None => Err(AppError::Llm(
            "翻译失败：没有任何一次尝试真正发起".to_string(),
        )),
    }
}

/// 取错误里不带类别前缀的原始消息，供拼接用。
fn error_message(err: &AppError) -> String {
    match err {
        AppError::Llm(message) => message.clone(),
        other => other.to_string(),
    }
}

/// 这个失败值不值得重试。
///
/// 只有**能确定重试没有意义**的错误才返回 `false`：HTTP `API 返回 {status}` 形态里
/// 的确定性 4xx。认不出来的一律当作可重试 —— 把「本来能靠重试恢复」的网络抖动
/// 变成不可重试，代价比多等几次大得多。
pub(crate) fn is_retryable_failure(err: &AppError) -> bool {
    match http_status_of(err) {
        Some(status) => is_retryable_status(status),
        None => true,
    }
}

/// 从 `API 返回 {status}: …` 形态的消息里取状态码。
fn http_status_of(err: &AppError) -> Option<u16> {
    let AppError::Llm(message) = err else {
        return None;
    };
    let rest = message.strip_prefix(API_STATUS_PREFIX)?;
    let digits: String = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .take(3)
        .collect();
    digits.parse().ok()
}

/// `408`（请求超时）、`425`（太早，RFC 8470 要求稍后重试）、`429`（限流）
/// 与全部 5xx 值得重试；其余 4xx 是确定性失败。
///
/// `429` 目前不读 `Retry-After`（已知取舍，见 ARCHITECTURE 的已知限制）。
fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429) || (500..600).contains(&status)
}

fn build_request<'a>(
    job: &'a TranslationJob,
    style_hint: Option<&'a str>,
    sink: &'a dyn EventSink,
    cancel: &'a CancelToken,
    stream_entry_ids: &'a [String],
) -> TranslateRequest<'a> {
    TranslateRequest {
        source: &job.source,
        matches: &job.matches,
        consistency_terms: &job.consistency_terms,
        style_hint,
        stream_entry_ids,
        sink,
        cancel,
    }
}

/// 每一次**新的尝试**开始时，给正在流式的条目重发一次 `Progress`。
///
/// 契约：`progress` 到达 = 该条目开始新一轮累积，上一轮流式文本作废。
/// 前端（`useTranslationRun.ts`）据此在重试时清空旧文本；没有这个信号，
/// 网络重试 / 结构纠错的两轮 delta 会拼成 `坏译文 + 好译文`。
///
/// `stream_entry_ids` 为空（`Series` 组）时不发：系列组本来就不推 delta，
/// 多发一个 progress 只会平白改变既有事件序列。
fn emit_attempt_progress(sink: &dyn EventSink, stream_entry_ids: &[String]) {
    for entry_id in stream_entry_ids {
        emit_progress(sink, entry_id);
    }
}

/// 校验一个 job 的译文结构。
///
/// `Single` / `ExactGroup` 直接比对整条原文；`Series` 组只翻了 base，
/// 所以必须在**合成后的完整译文**上逐成员校验 —— 后缀里也可能带占位符
/// （`Silver's Hair {1}` 会被拆成 base + 后缀 `{1}`），只在 base 上校验会漏。
pub(crate) fn job_fidelity_issues(job: &TranslationJob, translated: &str) -> Vec<FidelityIssue> {
    match &job.output {
        TranslationOutput::Single { .. } | TranslationOutput::ExactGroup { .. } => {
            check_fidelity(&job.source, translated)
        }
        TranslationOutput::Series { members } => {
            let mut issues: Vec<FidelityIssue> = Vec::new();
            for member in members {
                let source = member_source(&job.source, &member.suffix);
                let target = compose_variant_translation(translated, &member.suffix);
                for issue in check_fidelity(&source, &target) {
                    // 各成员共享 base 译文，问题通常重复；同一问题只报一次
                    if !issues.contains(&issue) {
                        issues.push(issue);
                    }
                }
            }
            issues
        }
    }
}

/// 还原系列成员在原文里的完整写法（拆分是"最后一个空白处切开"，这里拼回去）。
///
/// 只用于结构比对：空白怎么写不影响占位符 / 标签签名。
fn member_source(base: &str, suffix: &str) -> String {
    format!("{} {suffix}", base.trim_end())
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    use crate::translation::engine::{CancelToken, CollectingSink, TranslationEngine};
    use crate::translation::test_support::{
        FakeTranslator, done_events, engine_with, engine_with_fast_retry, entry, matcher,
        run_options,
    };

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
            failures_left: std::sync::atomic::AtomicUsize::new(1),
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
        assert!(done_events(&sink.events()).len() == 1);
    }

    #[test]
    fn single_jobs_check_the_whole_source() {
        let job = single_job("Deals {1} damage");
        assert!(job_fidelity_issues(&job, "造成 {1} 点伤害").is_empty());
        assert_eq!(
            job_fidelity_issues(&job, "造成伤害"),
            vec![FidelityIssue::MissingPlaceholder {
                token: "{1}".into(),
                count: 1,
            }]
        );
    }

    #[test]
    fn series_jobs_are_checked_on_the_composed_translation() {
        let job = TranslationJob {
            source: "Silver's Hair".into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            output: TranslationOutput::Series {
                members: vec![
                    crate::translation::planner::SeriesMember {
                        entry_id: "e1".into(),
                        suffix: "{1}".into(),
                    },
                    crate::translation::planner::SeriesMember {
                        entry_id: "e2".into(),
                        suffix: "{2}".into(),
                    },
                ],
            },
        };

        // base 译文本身没有结构问题，合成后每个成员各自带着自己的占位符
        assert!(job_fidelity_issues(&job, "银发").is_empty());
        // base 译文自己多吐了一个 `{1}`：合成后 e1 变成 `银发{1}{1}`，比原文多一处。
        // 只在 base 上校验会漏掉这个错误，所以必须在合成后的完整译文上查。
        assert_eq!(
            job_fidelity_issues(&job, "银发{1}"),
            vec![FidelityIssue::ExtraPlaceholder {
                token: "{1}".into(),
                count: 1,
            }]
        );
    }

    #[test]
    fn series_member_issues_are_deduplicated() {
        let job = TranslationJob {
            source: "Silver's Hair".into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            output: TranslationOutput::Series {
                members: vec![
                    crate::translation::planner::SeriesMember {
                        entry_id: "e1".into(),
                        suffix: "9b".into(),
                    },
                    crate::translation::planner::SeriesMember {
                        entry_id: "e2".into(),
                        suffix: "10".into(),
                    },
                ],
            },
        };
        // 两个成员的完整译文都会多出同一个 `<b>`（base 原文里没有标签），只报一次
        let issues = job_fidelity_issues(&job, "银发 <b>粗");
        assert_eq!(issues.len(), 1, "实际: {issues:?}");
        assert_eq!(
            issues[0],
            FidelityIssue::ExtraTag {
                tag: "<b>".into(),
                count: 1
            }
        );
    }

    fn single_job(source: &str) -> TranslationJob {
        TranslationJob {
            source: source.into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            output: TranslationOutput::Single {
                entry_id: "e1".into(),
            },
        }
    }

    /// 结构纠错重试不该动网络退避额度：一直失败时仍然只重试 MAX_ATTEMPTS 次。
    #[tokio::test]
    async fn structural_retry_does_not_change_network_retry_budget() {
        let fake = Arc::new(FakeTranslator {
            failures_left: std::sync::atomic::AtomicUsize::new(usize::MAX),
            bad_structure_calls_left: std::sync::atomic::AtomicUsize::new(usize::MAX),
            ..FakeTranslator::default()
        });
        let engine = engine_with_fast_retry(fake.clone(), 1);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();

        let summary = engine
            .run(
                &[entry("Deals {1} damage", "u1")],
                &matcher(&[]),
                run_options(&sink, &cancel, ""),
            )
            .await
            .unwrap();

        // 网络一直失败 → 只走网络重试线路，不会再有纠错重试
        assert_eq!(fake.calls().len(), MAX_ATTEMPTS);
        assert_eq!(summary.failed, 1);
        assert!(
            !sink
                .events()
                .iter()
                .any(|event| matches!(event, crate::types::TranslationEvent::Done { .. })),
            "网络失败不该有 Done"
        );
    }

    #[test]
    fn engine_type_is_reachable_from_engine_module() {
        // 公开 API 的路径不能被拆分改掉（文档承诺 + 现有调用方依赖）
        let engine: TranslationEngine =
            TranslationEngine::with_settings(crate::types::LlmSettings {
                concurrency: 2,
                ..Default::default()
            })
            .unwrap();
        assert_eq!(engine.settings().concurrency, 2);
    }

    // ── 重试分类（确定性失败不重试）──

    fn status_err(status: u16, reason: &str) -> AppError {
        AppError::Llm(format!("{API_STATUS_PREFIX}{status} {reason}: body"))
    }

    /// 4xx 里只有 408 / 429 值得重试；其余确定性失败一重试就白等 3.5 秒。
    #[test]
    fn http_status_codes_are_classified_for_retry() {
        for status in [400, 401, 403, 404, 405, 413, 422] {
            assert!(
                !is_retryable_failure(&status_err(status, "Deterministic")),
                "{status} 是确定性失败，不该重试"
            );
        }
        for status in [408, 425, 429, 500, 502, 503, 504] {
            assert!(
                is_retryable_failure(&status_err(status, "Transient")),
                "{status} 应该重试"
            );
        }
        // 302 之类既不是成功也不是 4xx：保守起见不重试
        assert!(!is_retryable_failure(&status_err(302, "Found")));
    }

    /// 认不出来的错误一律当可重试 —— 别把网络抖动变成不可重试。
    #[test]
    fn unrecognized_errors_stay_retryable() {
        assert!(is_retryable_failure(&AppError::Llm(
            "请求失败: connection reset".into()
        )));
        assert!(is_retryable_failure(&AppError::Llm(
            "流读取失败: unexpected EOF".into()
        )));
        assert!(is_retryable_failure(&AppError::Io(std::io::Error::other(
            "boom"
        ))));
        assert!(is_retryable_failure(&AppError::Cancelled));
        // 消息里出现状态码但不是我们的格式：不能误判
        assert!(is_retryable_failure(&AppError::Llm(
            "网关说：API 返回 401 是密钥问题".into()
        )));
    }

    #[test]
    fn http_status_is_parsed_from_our_own_message_shape() {
        assert_eq!(http_status_of(&status_err(404, "Not Found")), Some(404));
        assert_eq!(http_status_of(&status_err(503, "Unavailable")), Some(503));
        assert_eq!(http_status_of(&AppError::Llm("API 返回 ".into())), None);
        assert_eq!(
            http_status_of(&AppError::Llm("API 返回 abc: x".into())),
            None
        );
        assert_eq!(
            http_status_of(&AppError::Other("API 返回 500".into())),
            None
        );
    }

    /// 401 不该重试：一次调用、一条 Error、失败计数 1（端到端接线）。
    #[tokio::test]
    async fn unauthorized_fails_after_a_single_attempt_without_double_prefix() {
        struct Unauthorized;

        impl crate::translation::engine::TextTranslator for Unauthorized {
            fn translate<'a>(
                &'a self,
                _request: TranslateRequest<'a>,
            ) -> futures_util::future::BoxFuture<'a, Result<Option<String>>> {
                Box::pin(async { Err(status_err(401, "Unauthorized")) })
            }
        }

        let engine = TranslationEngine::with_translator(
            std::sync::Arc::new(Unauthorized),
            crate::types::LlmSettings {
                concurrency: 1,
                ..Default::default()
            },
        )
        .with_retry_backoff([Duration::from_millis(1); MAX_RETRIES]);
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

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.translated, 0);
        let messages = crate::translation::test_support::error_messages(&sink.events());
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].1.matches("大模型调用错误").count(), 1);
        assert!(messages[0].1.contains("401"), "实际: {}", messages[0].1);
        assert!(
            !sink
                .events()
                .iter()
                .any(|event| matches!(event, crate::types::TranslationEvent::Done { .. })),
            "失败不该有 Done"
        );
    }

    /// 429 仍然重试到上限（限流是暂时性的）。
    #[tokio::test]
    async fn rate_limited_requests_are_still_retried() {
        struct RateLimited;

        impl crate::translation::engine::TextTranslator for RateLimited {
            fn translate<'a>(
                &'a self,
                _request: TranslateRequest<'a>,
            ) -> futures_util::future::BoxFuture<'a, Result<Option<String>>> {
                Box::pin(async { Err(status_err(429, "Too Many Requests")) })
            }
        }

        let engine = TranslationEngine::with_translator(
            std::sync::Arc::new(RateLimited),
            crate::types::LlmSettings {
                concurrency: 1,
                ..Default::default()
            },
        )
        .with_retry_backoff([Duration::from_millis(1); MAX_RETRIES]);
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

        assert_eq!(summary.failed, 1);
        // progress 每次尝试都发一次 → MAX_ATTEMPTS 次尝试
        let progress = sink
            .events()
            .iter()
            .filter(|event| matches!(event, crate::types::TranslationEvent::Progress { .. }))
            .count();
        assert_eq!(progress, MAX_ATTEMPTS, "429 应重试到上限");
    }

    /// 截断类失败必须是**可重试**的，且最终态是 `Error`、`failed` 计数、绝不 `Done`。
    ///
    /// 这是红队 R-01 的验收口径：截断的风险在于「半句译文被当成成品」，
    /// 所以失败必须一路走到 Error，让条目写回时退回原文。
    #[tokio::test]
    async fn truncation_failures_are_retryable_and_end_as_error_not_done() {
        struct Truncated;

        impl crate::translation::engine::TextTranslator for Truncated {
            fn translate<'a>(
                &'a self,
                _request: TranslateRequest<'a>,
            ) -> futures_util::future::BoxFuture<'a, Result<Option<String>>> {
                Box::pin(async {
                    Err(AppError::Llm(
                        "模型输出不完整（finish_reason=length），已丢弃这次半截译文".to_string(),
                    ))
                })
            }
        }

        let engine = TranslationEngine::with_translator(
            std::sync::Arc::new(Truncated),
            crate::types::LlmSettings {
                concurrency: 1,
                ..Default::default()
            },
        )
        .with_retry_backoff([Duration::from_millis(1); MAX_RETRIES]);
        let sink = CollectingSink::new();
        let cancel = CancelToken::new();

        let summary = engine
            .run(
                &[entry("Deals {1} damage", "u1")],
                &matcher(&[]),
                run_options(&sink, &cancel, ""),
            )
            .await
            .unwrap();

        assert_eq!(summary.failed, 1);
        assert_eq!(summary.translated, 0);
        assert!(!summary.cancelled);
        let events = sink.events();
        assert_eq!(
            crate::translation::test_support::error_events(&events).len(),
            1
        );
        assert!(
            crate::translation::test_support::done_events(&events).is_empty(),
            "截断绝不能产生 Done（半截译文会被写回）"
        );
        assert_eq!(
            events.last().unwrap(),
            &crate::types::TranslationEvent::AllDone {
                total: 1,
                failed: 1
            }
        );
        // 可重试：progress 每次尝试都发一次
        let progress = events
            .iter()
            .filter(|event| matches!(event, crate::types::TranslationEvent::Progress { .. }))
            .count();
        assert_eq!(progress, MAX_ATTEMPTS, "截断应走完既有重试额度");
    }
}
