//! 引擎级测试：事件序列、并发、取消、失败统计，以及结构保真校验接进
//! 主流程后的行为（先坏后好 / 一直坏 / 合法不重试 / 合成后的系列译文）。

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

use futures_util::future::BoxFuture;

use super::*;

use crate::error::Result;
use crate::translation::test_support::{
    FakeTranslator, done_events, engine_with, engine_with_fast_retry, entry, error_events,
    error_messages, event_flow, matcher, run_options,
};

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
    let engine =
        TranslationEngine::with_settings(crate::translation::test_support::settings(3)).unwrap();
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

    assert_eq!(
        fake.calls().len(),
        crate::translation::retry::MAX_ATTEMPTS,
        "同一条目组应重试到上限"
    );
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
    let engine = TranslationEngine::with_translator(
        recorder.clone(),
        crate::translation::test_support::settings(2),
    );
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

// ─────────────────────────────────────────────────────────────
// 结构保真校验接进主流程
// ─────────────────────────────────────────────────────────────

#[tokio::test]
async fn structural_failure_is_retried_once_with_a_correction_hint() {
    let fake = Arc::new(FakeTranslator {
        bad_structure_calls_left: AtomicUsize::new(1),
        // 第一次丢占位符，第二次正确
        bad_output: Some("造成火焰伤害".into()),
        output: Some("造成 {1} 点火焰伤害".into()),
        stream_deltas: true,
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();

    let summary = engine
        .run(
            &[entry("Deals {1} fire damage", "u1")],
            &matcher(&[]),
            run_options(&sink, &cancel, ""),
        )
        .await
        .unwrap();

    assert_eq!(fake.calls().len(), 2, "结构不合格后应恰好重试一次");
    let corrections = fake.corrections();
    assert_eq!(corrections.len(), 2);
    assert_eq!(corrections[0], None, "首次尝试不带纠错提示");
    let hint = corrections[1]
        .as_deref()
        .expect("纠错重试必须把问题拼进请求");
    assert!(
        hint.contains("缺少占位符 {1}"),
        "提示要指出具体问题: {hint}"
    );
    assert!(
        hint.contains("请重新只输出译文"),
        "提示要说明怎么修: {hint}"
    );

    assert_eq!(summary.translated, 1);
    assert_eq!(summary.failed, 0);
    assert!(error_events(&sink.events()).is_empty());
    assert_eq!(
        done_events(&sink.events()),
        vec![(
            "test.loca#u1".to_string(),
            "造成 {1} 点火焰伤害".to_string()
        )],
        "Done 只能有一条，且是修好之后的译文"
    );
    // 事件流里要有纠错重试的边界：两轮 delta 之间夹一个 progress
    assert_eq!(
        event_flow(&sink.events()),
        vec![
            "progress#test.loca#u1",
            "delta#test.loca#u1", // 第一轮：被拒译文
            "delta#test.loca#u1",
            "progress#test.loca#u1", // 纠错重试边界
            "delta#test.loca#u1",    // 第二轮：修好的译文
            "delta#test.loca#u1",
            "done#test.loca#u1",
            "all_done(1,0)",
        ]
    );
    assert_eq!(
        sink.events().last().unwrap(),
        &TranslationEvent::AllDone {
            total: 1,
            failed: 0
        }
    );
}

#[tokio::test]
async fn structural_failure_twice_is_reported_as_failed_without_done() {
    let fake = Arc::new(FakeTranslator {
        bad_structure_calls_left: AtomicUsize::new(usize::MAX),
        bad_output: Some("造成伤害".into()),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
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

    assert_eq!(
        fake.calls().len(),
        2,
        "结构纠错只重试一次（不是网络重试的 4 次）"
    );
    assert_eq!(summary.failed, 1);
    assert_eq!(summary.translated, 0);
    assert!(
        done_events(&sink.events()).is_empty(),
        "结构不合格的译文绝不能发 Done"
    );

    let messages = error_messages(&sink.events());
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, "test.loca#u1");
    let message = &messages[0].1;
    assert!(message.contains("结构校验未通过"), "实际: {message}");
    assert!(message.contains("占位符 {1} 缺失"), "实际: {message}");
    assert!(message.contains("已重试 1 次"), "实际: {message}");
    assert_eq!(
        sink.events().last().unwrap(),
        &TranslationEvent::AllDone {
            total: 1,
            failed: 1
        }
    );
}

#[tokio::test]
async fn dropped_closing_tag_is_retried_and_reported_in_chinese() {
    let fake = Arc::new(FakeTranslator {
        bad_structure_calls_left: AtomicUsize::new(usize::MAX),
        bad_output: Some(r#"<LSTag Tag="Fire">火球术"#.into()),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();

    let summary = engine
        .run(
            &[entry(r#"<LSTag Tag="Fire">Fireball</LSTag>"#, "u1")],
            &matcher(&[]),
            run_options(&sink, &cancel, ""),
        )
        .await
        .unwrap();

    assert_eq!(fake.calls().len(), 2);
    assert_eq!(summary.failed, 1);
    let message = &error_messages(&sink.events())[0].1;
    assert!(message.contains("标签 </LSTag> 缺失"), "实际: {message}");
    assert!(message.contains("已重试 1 次"), "实际: {message}");
}

#[tokio::test]
async fn faithful_translation_is_not_retried() {
    let source = r#"Cast <LSTag Tag="Fire">Fireball</LSTag> for {1} damage"#;
    let fake = Arc::new(FakeTranslator {
        output: Some(r#"施放 <LSTag Tag="Fire">火球术</LSTag>，造成 {1} 点伤害"#.into()),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();

    let summary = engine
        .run(
            &[entry(source, "u1")],
            &matcher(&[]),
            run_options(&sink, &cancel, ""),
        )
        .await
        .unwrap();

    assert_eq!(fake.calls().len(), 1, "合法译文不该触发任何重试");
    assert_eq!(fake.corrections(), vec![None]);
    assert_eq!(summary.translated, 1);
    assert_eq!(summary.failed, 0);
    assert_eq!(done_events(&sink.events()).len(), 1);
}

#[tokio::test]
async fn structural_retry_stops_when_cancelled() {
    let fake = Arc::new(FakeTranslator {
        bad_structure_calls_left: AtomicUsize::new(usize::MAX),
        bad_output: Some("造成伤害".into()),
        cancel_on_bad_structure: true,
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
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

    assert!(summary.cancelled);
    assert_eq!(fake.calls().len(), 1, "取消后不再发纠错请求");
    assert!(done_events(&sink.events()).is_empty());
    assert!(error_events(&sink.events()).is_empty(), "取消不算失败");
    // 取消后连纠错重试的 progress 边界都不许发
    assert_eq!(
        event_flow(&sink.events()),
        vec!["progress#test.loca#u1", "all_done(1,0)"]
    );
    assert_eq!(
        sink.events().last().unwrap(),
        &TranslationEvent::AllDone {
            total: 1,
            failed: 0
        }
    );
}

#[tokio::test]
async fn structural_retries_stay_within_the_concurrency_limit() {
    let fake = Arc::new(FakeTranslator {
        bad_structure_calls_left: AtomicUsize::new(usize::MAX),
        bad_output: Some("没有占位符".into()),
        delay: Duration::from_millis(30),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();
    let entries: Vec<_> = (0..6)
        .map(|i| entry(&format!("Deals {{1}} damage {i} times"), &format!("u{i}")))
        .collect();

    let summary = engine
        .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
        .await
        .unwrap();

    assert_eq!(summary.failed, 6);
    assert_eq!(fake.calls().len(), 12, "每个 job 各发一次纠错重试");
    assert!(
        fake.max_inflight() <= 2,
        "纠错重试不能突破并发上限: {}",
        fake.max_inflight()
    );
    assert!(fake.max_inflight() >= 2, "并发没有真正发生");
}

#[tokio::test]
async fn series_fidelity_is_checked_on_the_composed_translation() {
    // base 是 `Silver's Hair`，两个成员的后缀各自带占位符。
    // 假实现给 base 译文多吐了一个 `{1}`：合成后第一个成员会变成 `银发{1}{1}`，
    // 只有在校验「合成后的完整译文」时才能发现。
    let fake = Arc::new(FakeTranslator {
        output: Some("银发{1}".into()),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();
    let entries = vec![
        entry("Silver's Hair {1}", "u1"),
        entry("Silver's Hair {2}", "u2"),
    ];

    let summary = engine
        .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
        .await
        .unwrap();

    assert_eq!(fake.calls().len(), 2, "合成后不合格 → 纠错重试一次");
    assert_eq!(summary.failed, 2, "系列组失败按条目计数");
    assert_eq!(summary.translated, 0);
    assert!(done_events(&sink.events()).is_empty());
    let messages = error_messages(&sink.events());
    assert_eq!(messages.len(), 2);
    for (_, message) in &messages {
        assert!(message.contains("结构校验未通过"), "实际: {message}");
        assert!(message.contains("占位符 {1} 多出"), "实际: {message}");
    }
}

#[tokio::test]
async fn series_with_placeholder_suffixes_passes_and_composes() {
    let fake = Arc::new(FakeTranslator {
        output: Some("银发".into()),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();
    let entries = vec![
        entry("Silver's Hair {1}", "u1"),
        entry("Silver's Hair {2}", "u2"),
    ];

    let summary = engine
        .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
        .await
        .unwrap();

    assert_eq!(fake.calls().len(), 1, "合成后结构一致，不该重试");
    assert_eq!(summary.translated, 2);
    assert_eq!(summary.failed, 0);
    assert_eq!(
        done_events(&sink.events()),
        vec![
            ("test.loca#u1".to_string(), "银发{1}".to_string()),
            ("test.loca#u2".to_string(), "银发{2}".to_string()),
        ]
    );
}

// ─────────────────────────────────────────────────────────────
// 尝试边界：每次新尝试开始都重发 progress（F-09 契约）
// ─────────────────────────────────────────────────────────────

/// 网络重试：第 1 次失败、第 2 次成功 → 两次 progress 都在 delta 之前，Done 恰好一次。
#[tokio::test]
async fn network_retry_re_emits_progress_before_the_second_attempt() {
    let fake = Arc::new(FakeTranslator {
        failures_left: AtomicUsize::new(1),
        output: Some("火球术".into()),
        stream_deltas: true,
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
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

    assert_eq!(
        event_flow(&sink.events()),
        vec![
            "progress#test.loca#u1",
            // 第二次尝试的边界：前端收到它就丢掉上一轮的流式文本
            "progress#test.loca#u1",
            "delta#test.loca#u1",
            "delta#test.loca#u1",
            "done#test.loca#u1",
            "all_done(1,0)",
        ]
    );
    assert_eq!(summary.translated, 1);
    assert_eq!(done_events(&sink.events()).len(), 1, "Done 只发一次");
}

/// 结构纠错重试：被拒译文先流出去，纠错请求之前必须再发一次 progress。
#[tokio::test]
async fn structural_retry_re_emits_progress_before_the_correction_attempt() {
    let fake = Arc::new(FakeTranslator {
        bad_structure_calls_left: AtomicUsize::new(1),
        bad_output: Some("造成伤害".into()),
        output: Some("造成 {1} 点伤害".into()),
        stream_deltas: true,
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
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

    let flow = event_flow(&sink.events());
    // 前端视角的真实序列：两轮 delta 由第二个 progress 隔开（`--nocapture` 可看）
    println!("两轮尝试的事件序列: {flow:?}");
    assert_eq!(
        flow,
        vec![
            "progress#test.loca#u1",
            "delta#test.loca#u1", // 第一轮：被拒译文
            "delta#test.loca#u1",
            "progress#test.loca#u1", // 纠错边界
            "delta#test.loca#u1",    // 第二轮：合格译文
            "delta#test.loca#u1",
            "done#test.loca#u1",
            "all_done(1,0)",
        ]
    );
    assert_eq!(
        done_events(&sink.events()),
        vec![("test.loca#u1".to_string(), "造成 {1} 点伤害".to_string())]
    );
    assert_eq!(summary.failed, 0);
}

/// 一直不合格 → 每轮尝试一个 progress，最后 error，失败计入 failed。
#[tokio::test]
async fn every_failed_attempt_re_emits_progress_then_error() {
    let fake = Arc::new(FakeTranslator {
        bad_structure_calls_left: AtomicUsize::new(usize::MAX),
        bad_output: Some("造成伤害".into()),
        stream_deltas: true,
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(fake.clone(), 2);
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

    assert_eq!(
        event_flow(&sink.events()),
        vec![
            "progress#test.loca#u1",
            "delta#test.loca#u1",
            "delta#test.loca#u1",
            "progress#test.loca#u1",
            "delta#test.loca#u1",
            "delta#test.loca#u1",
            "error#test.loca#u1",
            "all_done(1,1)",
        ]
    );
    assert_eq!(summary.failed, 1);
    assert!(done_events(&sink.events()).is_empty());
}

/// Series 组不推 delta，也就不需要「重新累积」信号：重试不许多发 progress。
#[tokio::test]
async fn series_retries_do_not_emit_extra_progress() {
    // ① 结构纠错重试
    let structural = Arc::new(FakeTranslator {
        output: Some("银发{1}".into()),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(structural.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();
    let entries = vec![
        entry("Silver's Hair {1}", "u1"),
        entry("Silver's Hair {2}", "u2"),
    ];

    let summary = engine
        .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
        .await
        .unwrap();

    assert_eq!(structural.calls().len(), 2, "确实发生了纠错重试");
    assert_eq!(
        event_flow(&sink.events()),
        vec![
            // job 开始时各发一次（每个成员），重试不再补发
            "progress#test.loca#u1",
            "progress#test.loca#u2",
            "error#test.loca#u1",
            "error#test.loca#u2",
            "all_done(2,2)",
        ]
    );
    assert_eq!(summary.failed, 2);

    // ② 网络重试同样不补发
    let network = Arc::new(FakeTranslator {
        failures_left: AtomicUsize::new(1),
        output: Some("银发".into()),
        ..FakeTranslator::default()
    });
    let engine = engine_with_fast_retry(network.clone(), 2);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();

    let summary = engine
        .run(&entries, &matcher(&[]), run_options(&sink, &cancel, ""))
        .await
        .unwrap();

    assert_eq!(network.calls().len(), 2, "确实发生了网络重试");
    assert_eq!(
        event_flow(&sink.events()),
        vec![
            "progress#test.loca#u1",
            "progress#test.loca#u2",
            "done#test.loca#u1",
            "done#test.loca#u2",
            "all_done(2,0)",
        ]
    );
    assert_eq!(summary.translated, 2);
}

/// 取消发生在退避期间：不得再发 progress / delta / error。
#[tokio::test]
async fn cancel_during_backoff_emits_no_further_events() {
    let fake = Arc::new(FakeTranslator {
        failures_left: AtomicUsize::new(usize::MAX),
        ..FakeTranslator::default()
    });
    // 用生产退避（500ms），好在退避窗口里取消
    let engine = engine_with(fake.clone(), 1);
    let sink = CollectingSink::new();
    let cancel = CancelToken::new();

    let entries = vec![entry("Fireball", "u1")];
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
    assert_eq!(
        event_flow(&sink.events()),
        vec!["progress#test.loca#u1", "all_done(1,0)"],
        "退避期间取消后不得再有 progress / delta / error"
    );
    assert_eq!(fake.calls().len(), 1, "只发了第一次请求");
}
