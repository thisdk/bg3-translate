//! 翻译任务规划：把条目切成「一次请求对应一组条目」的 job。
//!
//! 规划阶段要做三件事，全部在**发请求之前**完成：
//! 1. 过滤：只规划 [`TranslationEntry::is_pending_translation`] 的条目；
//! 2. 复用：命中一致性记忆（同 MOD 已有译文）的条目直接产出译文，
//!    通过 sink 发 `Progress` + `Done`，不消耗额度；
//! 3. 合并：完全相同的原文并成一次请求（`ExactGroup`），同一系列 base
//!    且成员 ≥2 的变体并成一次请求（`Series`，只翻 base，后缀原样拼回）。

use std::collections::{HashMap, HashSet};

use crate::glossary::{GlossaryMatcher, MatchedTerm};
use crate::types::TranslationEntry;

use super::engine::{EventSink, emit_done};
use super::series::{
    ConsistencyTerm, build_consistency_memory, build_series_aliases, compose_variant_translation,
    find_consistency_references, normalize_consistency_key, split_series_variant,
};

/// 系列组里的一个成员。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesMember {
    pub entry_id: String,
    /// 变体后缀，如 `9b`
    pub suffix: String,
}

/// 一次请求覆盖的条目集合。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranslationOutput {
    /// 单条
    Single { entry_id: String },
    /// 原文完全相同的多条（共享同一份译文）
    ExactGroup { entry_ids: Vec<String> },
    /// 同一系列 base 的多个变体（只翻 base）
    Series { members: Vec<SeriesMember> },
}

/// 一个翻译任务。
#[derive(Debug, Clone)]
pub struct TranslationJob {
    /// 送去翻译的原文（`Series` 时是 base）
    pub source: String,
    /// 命中的术语，注入 prompt
    pub matches: Vec<MatchedTerm>,
    /// 同 MOD 一致性参考，注入 prompt
    pub consistency_terms: Vec<ConsistencyTerm>,
    /// 结果分发方式
    pub output: TranslationOutput,
}

impl TranslationJob {
    /// 本任务覆盖的全部条目 ID。
    pub fn target_ids(&self) -> Vec<String> {
        match &self.output {
            TranslationOutput::Single { entry_id } => vec![entry_id.clone()],
            TranslationOutput::ExactGroup { entry_ids } => entry_ids.clone(),
            TranslationOutput::Series { members } => {
                members.iter().map(|m| m.entry_id.clone()).collect()
            }
        }
    }
}

/// 规划结果统计，给 engine 汇总与日志用。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TranslationPlan {
    /// 待翻译条目数（`source` 非空且 `target` 为空的条目）
    pub total: usize,
    /// 规划阶段直接命中一致性记忆、无需请求的条目数
    pub skipped: usize,
    /// 需要发起的请求数
    pub jobs: usize,
}

/// 系列分组中间态。
#[derive(Debug)]
struct PendingSeriesGroup {
    base_source: String,
    members: Vec<SeriesMember>,
}

/// 规划任务。规划阶段就会通过 `sink` 发出「直接完成」的事件（与旧行为一致）。
pub fn plan_jobs(
    entries: &[TranslationEntry],
    matcher: &GlossaryMatcher,
    sink: &dyn EventSink,
) -> (Vec<TranslationJob>, TranslationPlan) {
    let pending: Vec<&TranslationEntry> = entries
        .iter()
        .filter(|entry| entry.is_pending_translation())
        .collect();
    let series_aliases = build_series_aliases(entries);
    let memory = build_consistency_memory(entries);
    let mut used_ids: HashSet<String> = HashSet::new();
    let mut skipped = 0usize;
    let mut jobs = Vec::new();

    // ── 系列变体分组 ──
    let mut series_groups: HashMap<String, PendingSeriesGroup> = HashMap::new();
    let mut series_order: Vec<String> = Vec::new();
    for entry in &pending {
        let Some(variant) = split_series_variant(&entry.source) else {
            continue;
        };
        let raw_base_key = normalize_consistency_key(&variant.base);
        let base_key = series_aliases.canonical_key(&raw_base_key);
        let base_source = series_aliases.source_for(&base_key, &variant.base);
        if let Some(term) = memory.get(&base_key) {
            emit_done(
                sink,
                &entry.id,
                compose_variant_translation(&term.target, &variant.suffix),
            );
            used_ids.insert(entry.id.clone());
            skipped += 1;
            continue;
        }
        if !series_groups.contains_key(&base_key) {
            series_order.push(base_key.clone());
        }
        series_groups
            .entry(base_key)
            .or_insert_with(|| PendingSeriesGroup {
                base_source,
                members: Vec::new(),
            })
            .members
            .push(SeriesMember {
                entry_id: entry.id.clone(),
                suffix: variant.suffix,
            });
    }

    // 只有 ≥2 个成员才值得合并：单成员系列合并后反而丢了它自己的 matches
    for key in series_order {
        let Some(group) = series_groups.remove(&key) else {
            continue;
        };
        if group.members.len() < 2 {
            continue;
        }
        for member in &group.members {
            used_ids.insert(member.entry_id.clone());
        }
        jobs.push(TranslationJob {
            matches: matcher.find_matches(&group.base_source),
            consistency_terms: find_consistency_references(&group.base_source, &memory),
            source: group.base_source,
            output: TranslationOutput::Series {
                members: group.members,
            },
        });
    }

    // ── 其余条目：一致性复用 + 相同原文合并 ──
    let mut exact_groups: HashMap<String, Vec<&TranslationEntry>> = HashMap::new();
    let mut exact_order: Vec<String> = Vec::new();
    for entry in &pending {
        if used_ids.contains(&entry.id) {
            continue;
        }
        let key = normalize_consistency_key(&entry.source);
        if let Some(term) = memory.get(&key) {
            emit_done(sink, &entry.id, term.target.clone());
            used_ids.insert(entry.id.clone());
            skipped += 1;
            continue;
        }
        if !exact_groups.contains_key(&key) {
            exact_order.push(key.clone());
        }
        exact_groups.entry(key).or_default().push(entry);
    }

    for key in exact_order {
        let Some(group) = exact_groups.remove(&key) else {
            continue;
        };
        let Some(first) = group.first() else {
            continue;
        };
        let entry_ids: Vec<String> = group.iter().map(|entry| entry.id.clone()).collect();
        let output = if entry_ids.len() == 1 {
            TranslationOutput::Single {
                entry_id: entry_ids[0].clone(),
            }
        } else {
            TranslationOutput::ExactGroup { entry_ids }
        };
        jobs.push(TranslationJob {
            source: first.source.clone(),
            matches: matcher.find_matches(&first.source),
            consistency_terms: find_consistency_references(&first.source, &memory),
            output,
        });
    }

    let plan = TranslationPlan {
        total: pending.len(),
        skipped,
        jobs: jobs.len(),
    };
    (jobs, plan)
}

/// 规划阶段是否有「计划外」的条目被静默丢掉（engine 汇总时校验）。
///
/// 计划覆盖的条目 = `total - skipped`，与各 job 的 `target_ids()` 之和相等；
/// 不等说明分组逻辑漏了条目，这类 bug 在旧实现里很难被发现。
pub(crate) fn planned_entry_count(jobs: &[TranslationJob]) -> usize {
    jobs.iter().map(|job| job.target_ids().len()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::glossary::Glossary;
    use crate::translation::engine::CollectingSink;
    use crate::types::TranslationEvent;

    fn entry(source: &str, uid: &str) -> TranslationEntry {
        TranslationEntry::new("test.loca", uid, "1", source)
    }

    fn translated(source: &str, target: &str, uid: &str) -> TranslationEntry {
        let mut entry = entry(source, uid);
        entry.mark_translated(target);
        entry
    }

    fn empty_matcher() -> GlossaryMatcher {
        GlossaryMatcher::new(&Glossary { terms: Vec::new() })
    }

    fn event_ids(events: &[TranslationEvent]) -> Vec<(String, &'static str)> {
        events
            .iter()
            .filter_map(|event| {
                let id = event.entry_id()?.to_string();
                let kind = match event {
                    TranslationEvent::Progress { .. } => "progress",
                    TranslationEvent::Delta { .. } => "delta",
                    TranslationEvent::Done { .. } => "done",
                    TranslationEvent::Error { .. } => "error",
                    TranslationEvent::AllDone { .. } => "all_done",
                };
                Some((id, kind))
            })
            .collect()
    }

    #[test]
    fn plan_skips_entries_with_existing_target() {
        let sink = CollectingSink::new();
        let entries = vec![
            translated("Done already", "已完成", "u1"),
            entry("Needs work", "u2"),
            entry("   ", "u3"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.total, 1);
        assert_eq!(plan.skipped, 0);
        assert_eq!(plan.jobs, 1);
        assert_eq!(jobs[0].source, "Needs work");
        assert_eq!(
            jobs[0].output,
            TranslationOutput::Single {
                entry_id: "test.loca#u2".into()
            }
        );
        assert!(sink.events().is_empty(), "没有一致性命中就不该发事件");
    }

    #[test]
    fn identical_sources_share_one_request() {
        let sink = CollectingSink::new();
        let entries = vec![
            entry("Fireball", "u1"),
            entry("Fireball", "u2"),
            entry("Magic Missile", "u3"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.total, 3);
        assert_eq!(plan.jobs, 2);

        let group = jobs
            .iter()
            .find(|job| job.source == "Fireball")
            .expect("相同原文应合并");
        assert_eq!(
            group.output,
            TranslationOutput::ExactGroup {
                entry_ids: vec!["test.loca#u1".into(), "test.loca#u2".into()]
            }
        );
        assert_eq!(planned_entry_count(&jobs), 3);
    }

    #[test]
    fn consistency_memory_completes_entries_and_emits_events() {
        let sink = CollectingSink::new();
        let entries = vec![
            translated("Fireball", "火球术", "t1"),
            entry("Fireball", "u1"),
            entry("Cast Fireball now", "u2"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.total, 2);
        assert_eq!(plan.skipped, 1);
        assert_eq!(plan.jobs, 1);

        let events = sink.events();
        assert_eq!(
            event_ids(&events),
            vec![
                ("test.loca#u1".to_string(), "progress"),
                ("test.loca#u1".to_string(), "done"),
            ]
        );
        assert_eq!(events[1], TranslationEvent::done("test.loca#u1", "火球术"));

        // 剩下的那条仍要发请求，并把一致性译名注入 prompt 参考
        assert_eq!(jobs[0].source, "Cast Fireball now");
        assert_eq!(jobs[0].consistency_terms.len(), 1);
        assert_eq!(jobs[0].consistency_terms[0].target, "火球术");
    }

    #[test]
    fn series_members_merge_into_one_job() {
        let sink = CollectingSink::new();
        let entries = vec![
            entry("Silver's Hair 9b", "u1"),
            entry("Silver's Hair 10", "u2"),
            entry("Other Thing", "u3"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.total, 3);
        assert_eq!(plan.jobs, 2);

        let series = jobs
            .iter()
            .find(|job| matches!(job.output, TranslationOutput::Series { .. }))
            .expect("应有系列合并任务");
        assert_eq!(series.source, "Silver's Hair");
        match &series.output {
            TranslationOutput::Series { members } => {
                let suffixes: Vec<_> = members.iter().map(|m| m.suffix.as_str()).collect();
                assert_eq!(suffixes, vec!["9b", "10"]);
            }
            other => panic!("期望 Series，实际 {other:?}"),
        }
        assert_eq!(planned_entry_count(&jobs), 3);
    }

    #[test]
    fn single_member_series_is_not_merged() {
        let sink = CollectingSink::new();
        let entries = vec![entry("Silver's Hair 9b", "u1")];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.jobs, 1);
        assert_eq!(jobs[0].source, "Silver's Hair 9b", "单成员不应丢后缀");
        assert!(matches!(jobs[0].output, TranslationOutput::Single { .. }));
    }

    #[test]
    fn series_completed_from_memory_keeps_suffix() {
        let sink = CollectingSink::new();
        let entries = vec![
            translated("Silver's Hair 9b", "银发 9b", "t1"),
            entry("Silver's Hair 10", "u1"),
            entry("Silver's Hair 11", "u2"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        // 记忆里有 base「silver's hair = 银发」，两条变体都能直接拼出译文
        assert_eq!(plan.total, 2);
        assert_eq!(plan.skipped, 2);
        assert_eq!(plan.jobs, 0);
        let done: Vec<_> = sink
            .events()
            .into_iter()
            .filter_map(|event| match event {
                TranslationEvent::Done { entry_id, text } => Some((entry_id, text)),
                _ => None,
            })
            .collect();
        assert_eq!(
            done,
            vec![
                ("test.loca#u1".to_string(), "银发 10".to_string()),
                ("test.loca#u2".to_string(), "银发 11".to_string()),
            ]
        );
        assert!(jobs.is_empty());
    }

    #[test]
    fn aliased_cjk_series_reuses_latin_memory() {
        let sink = CollectingSink::new();
        // 英文与中文系列通过 contentuid 归一，中文 base 可以复用英文 base 的译名
        let entries = vec![
            translated("Silver's Hair 9b", "银发 9b", "same"),
            entry("银色发型10", "same"),
        ];
        let (_, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.skipped, 1);
        assert_eq!(plan.jobs, 0);
        assert_eq!(
            sink.events().last().unwrap(),
            &TranslationEvent::done("test.loca#same", "银发 10")
        );
    }

    #[test]
    fn jobs_carry_glossary_matches_and_consistency() {
        let glossary = Glossary {
            terms: vec![crate::glossary::GlossaryEntry {
                source: "Fireball".into(),
                target: "火球术".into(),
                ..Default::default()
            }],
        };
        let matcher = GlossaryMatcher::new(&glossary);
        let sink = CollectingSink::new();
        let entries = vec![
            translated("Magic Missile", "魔法飞弹", "t1"),
            entry("Cast Fireball and Magic Missile!", "u1"),
        ];
        let (jobs, _) = plan_jobs(&entries, &matcher, &sink);
        assert_eq!(jobs.len(), 1);
        assert_eq!(
            jobs[0].matches,
            vec![MatchedTerm {
                source: "Fireball".into(),
                target: "火球术".into()
            }]
        );
        assert_eq!(jobs[0].consistency_terms[0].source, "Magic Missile");
    }

    #[test]
    fn empty_input_plans_nothing() {
        let sink = CollectingSink::new();
        let (jobs, plan) = plan_jobs(&[], &empty_matcher(), &sink);
        assert!(jobs.is_empty());
        assert_eq!(plan, TranslationPlan::default());
        assert!(sink.events().is_empty());
    }

    #[test]
    fn target_ids_covers_all_output_kinds() {
        let single = TranslationJob {
            source: "a".into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            output: TranslationOutput::Single {
                entry_id: "e1".into(),
            },
        };
        assert_eq!(single.target_ids(), vec!["e1".to_string()]);

        let group = TranslationJob {
            source: "a".into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            output: TranslationOutput::ExactGroup {
                entry_ids: vec!["e1".into(), "e2".into()],
            },
        };
        assert_eq!(group.target_ids(), vec!["e1".to_string(), "e2".to_string()]);

        let series = TranslationJob {
            source: "a".into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            output: TranslationOutput::Series {
                members: vec![
                    SeriesMember {
                        entry_id: "e1".into(),
                        suffix: "9b".into(),
                    },
                    SeriesMember {
                        entry_id: "e2".into(),
                        suffix: "10".into(),
                    },
                ],
            },
        };
        assert_eq!(
            series.target_ids(),
            vec!["e1".to_string(), "e2".to_string()]
        );
    }
}
