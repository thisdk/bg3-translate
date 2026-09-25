//! 单测共享的假翻译器与断言辅助（只在 `cfg(test)` 下编译）。
//!
//! 引擎、重试、翻译器三个子模块的测试共用这份假实现：它记录每次调用、
//! 统计并发峰值，并且能按需制造网络失败、结构不合格译文、取消与 panic。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use futures_util::future::BoxFuture;

use crate::error::{AppError, Result};
use crate::glossary::{Glossary, GlossaryMatcher};
use crate::translation::engine::{
    CancelToken, CollectingSink, RunOptions, TextTranslator, TranslateRequest, TranslationEngine,
};
use crate::translation::retry::MAX_RETRIES;
use crate::types::{LlmSettings, TranslationEntry, TranslationEvent};

/// 手写假实现：记录调用、统计并发、可控失败与取消。
#[derive(Debug, Default)]
pub(crate) struct FakeTranslator {
    /// 固定译文；`None` 表示回显 `译:{source}`
    pub(crate) output: Option<String>,
    /// 每次翻译的耗时（模拟网络）
    pub(crate) delay: Duration,
    /// 还剩几次调用直接失败（测重试）
    pub(crate) failures_left: AtomicUsize,
    /// 一直等到取消（测取消）
    pub(crate) wait_for_cancel: bool,
    /// 是否推送流式增量
    pub(crate) stream_deltas: bool,
    /// 每次调用直接 panic（测 job 级 panic 隔离）
    pub(crate) panic_on_call: bool,
    /// 还剩几次调用返回"结构不合格"的译文（测结构纠错重试）
    pub(crate) bad_structure_calls_left: AtomicUsize,
    /// 结构不合格时返回什么；`None` 表示 `坏:{source}`
    pub(crate) bad_output: Option<String>,
    /// 返回坏译文时顺手取消（测取消仍然优先）
    pub(crate) cancel_on_bad_structure: bool,
    /// 每次调用收到的原文
    pub(crate) calls: Mutex<Vec<String>>,
    /// 每次调用收到的纠错提示（普通调用记为 `None`）
    pub(crate) corrections: Mutex<Vec<Option<String>>>,
    /// 推送出去的增量
    pub(crate) deltas: Mutex<Vec<(String, String)>>,
    pub(crate) inflight: AtomicUsize,
    pub(crate) max_inflight: AtomicUsize,
}

impl FakeTranslator {
    pub(crate) fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// 每次调用收到的纠错提示；`None` = 普通（非纠错）调用。
    pub(crate) fn corrections(&self) -> Vec<Option<String>> {
        self.corrections.lock().unwrap().clone()
    }

    pub(crate) fn deltas(&self) -> Vec<(String, String)> {
        self.deltas.lock().unwrap().clone()
    }

    pub(crate) fn max_inflight(&self) -> usize {
        self.max_inflight.load(Ordering::SeqCst)
    }

    fn respond<'a>(
        &'a self,
        request: TranslateRequest<'a>,
        correction: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            self.calls.lock().unwrap().push(request.source.to_string());
            self.corrections
                .lock()
                .unwrap()
                .push(correction.map(str::to_string));
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

            let bad_structure = self.bad_structure_calls_left.load(Ordering::SeqCst) > 0;
            let text = if bad_structure {
                self.bad_structure_calls_left.fetch_sub(1, Ordering::SeqCst);
                self.bad_output
                    .clone()
                    .unwrap_or_else(|| format!("坏:{}", request.source))
            } else {
                self.output
                    .clone()
                    .unwrap_or_else(|| format!("译:{}", request.source))
            };
            if bad_structure && self.cancel_on_bad_structure {
                request.cancel.cancel();
            }

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

impl TextTranslator for FakeTranslator {
    fn translate<'a>(
        &'a self,
        request: TranslateRequest<'a>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        self.respond(request, None)
    }

    fn translate_with_correction<'a>(
        &'a self,
        request: TranslateRequest<'a>,
        correction: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Option<String>>> {
        self.respond(request, correction)
    }
}

pub(crate) fn settings(concurrency: usize) -> LlmSettings {
    LlmSettings {
        concurrency,
        ..LlmSettings::default()
    }
}

pub(crate) fn engine_with(fake: Arc<FakeTranslator>, concurrency: usize) -> TranslationEngine {
    TranslationEngine::with_translator(fake, settings(concurrency))
}

/// 退避缩到 1ms：失败用例不必真等 3.5 秒。
pub(crate) fn engine_with_fast_retry(
    fake: Arc<FakeTranslator>,
    concurrency: usize,
) -> TranslationEngine {
    engine_with(fake, concurrency).with_retry_backoff([Duration::from_millis(1); MAX_RETRIES])
}

pub(crate) fn entry(source: &str, uid: &str) -> TranslationEntry {
    TranslationEntry::new("test.loca", uid, "1", source)
}

pub(crate) fn matcher(pairs: &[(&str, &str)]) -> GlossaryMatcher {
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

pub(crate) fn run_options<'a>(
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

pub(crate) fn done_events(events: &[TranslationEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            TranslationEvent::Done { entry_id, text } => Some((entry_id.clone(), text.clone())),
            _ => None,
        })
        .collect()
}

pub(crate) fn error_events(events: &[TranslationEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            TranslationEvent::Error { entry_id, .. } => Some(entry_id.clone()),
            _ => None,
        })
        .collect()
}

/// 取 `(entry_id, message)`，用于断言失败原因写清楚了。
pub(crate) fn error_messages(events: &[TranslationEvent]) -> Vec<(String, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            TranslationEvent::Error { entry_id, message } => {
                Some((entry_id.clone(), message.clone()))
            }
            _ => None,
        })
        .collect()
}

/// 事件流压成短标签序列，便于逐条断言顺序（`progress#id` / `delta#id` / …）。
pub(crate) fn event_flow(events: &[TranslationEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            TranslationEvent::Progress { entry_id, .. } => format!("progress#{entry_id}"),
            TranslationEvent::Delta { entry_id, .. } => format!("delta#{entry_id}"),
            TranslationEvent::Done { entry_id, .. } => format!("done#{entry_id}"),
            TranslationEvent::Error { entry_id, .. } => format!("error#{entry_id}"),
            TranslationEvent::AllDone { total, failed } => format!("all_done({total},{failed})"),
        })
        .collect()
}
