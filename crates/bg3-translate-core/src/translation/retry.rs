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
use super::translator::{TextTranslator, TranslateRequest};

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
            "结构校验未通过：{}（已重试 1 次）；纠错重试失败：{err}",
            summarize(&issues)
        ))),
    }
}

/// 网络层面的重试循环（最多 3 次重试：500ms / 1000ms / 2000ms）。
async fn translate_attempts(
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
        if attempt > 0 {
            // 第 2 次及以后的尝试：重发 progress，让前端重置该条目的流式文本
            emit_attempt_progress(sink, stream_entry_ids);
        }
        let request = build_request(job, style_hint, sink, cancel, stream_entry_ids);
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
}
