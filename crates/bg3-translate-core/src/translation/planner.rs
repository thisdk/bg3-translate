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
use super::fidelity::previous_failure_reason;
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
    /// 上一次翻译失败的结构校验诊断（条目上的 `error`），注入重试 prompt。
    ///
    /// 只在本组条目里**任意一条**还留着结构诊断时才有值 —— 那是「用户点了重试」
    /// 的信号。见 [`previous_failure_of`]。
    pub previous_failure: Option<String>,
}

/// 取这一组条目里「上一次结构校验失败的原因」，用于重试 prompt。
///
/// 用户点「重试」时，前端会把条目上留着的上一轮错误文案一起发回来（前端只清
/// 界面上的文案，payload 里保留）。带上它，模型才知道上一次**具体**错在哪；
/// 不带的话它往往原样再错一次。
///
/// 只取 [`previous_failure_reason`] 认得出来的结构诊断：网络错误与网关报错的
/// 文案不进 prompt（既是注入面，也帮不上「怎么改译文」）。
fn previous_failure_of<'a>(entries: impl Iterator<Item = &'a TranslationEntry>) -> Option<String> {
    entries
        .filter_map(|entry| entry.error.as_deref())
        .find_map(previous_failure_reason)
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
    /// 组内第一条带结构诊断的条目给出的「上一次失败原因」（见 [`previous_failure_of`]）
    previous_failure: Option<String>,
}

/// 一致性记忆的复用判据：原文只差**空白与大小写**时才允许直接搬用已有译文。
///
/// 记忆的键（[`normalize_consistency_key`]）会折掉首尾 ASCII 标点，但标点本身
/// 带语义：`Are you sure you want to delete this save?` 与 `…save!` 归一后是同一个
/// 键，直接复用等于把疑问句的译文静默贴到感叹句条目上（结构与占位符完全一致，
/// 校验拦不住）。这与「相同原文合并」改用原文做键是同一条理由，只是那条只管
/// 合并、这条管复用。
///
/// 判失败（不满足）的代价只是多发起一次请求，所以判据可以收紧；反过来放宽
/// 就是把错误译文写进 PAK。
fn memory_reuse_matches(term: &ConsistencyTerm, source: &str) -> bool {
    fold_for_reuse(&term.source) == fold_for_reuse(source)
}

/// 折空白 + 折大小写（**刻意不折标点**，用途见 [`memory_reuse_matches`]）。
fn fold_for_reuse(text: &str) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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
        if let Some(term) = memory
            .get(&base_key)
            .filter(|term| memory_reuse_matches(term, &base_source))
        {
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
        let group = series_groups
            .entry(base_key)
            .or_insert_with(|| PendingSeriesGroup {
                base_source,
                members: Vec::new(),
                previous_failure: None,
            });
        if group.previous_failure.is_none() {
            group.previous_failure = previous_failure_of(std::iter::once(*entry));
        }
        group.members.push(SeriesMember {
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
            previous_failure: group.previous_failure,
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
        if let Some(term) = memory
            .get(&key)
            .filter(|term| memory_reuse_matches(term, &entry.source))
        {
            emit_done(sink, &entry.id, term.target.clone());
            used_ids.insert(entry.id.clone());
            skipped += 1;
            continue;
        }
        // 分组键是**原文本身**，不是一致性 key。
        //
        // 一致性 key 会折掉大小写与首尾 ASCII 标点，用它分组会把
        // `Delete this save?` 与 `Delete this save!` 并成一次请求、共享同一份
        // 译文 —— 后者的译文就是前者那一句，疑问句被静默写成陈述句。结构校验
        // 拦不住这种改写（占位符 / 标签完全一致），所以只能在分组这里不让它发生。
        // 「完全相同的原文才共享一份译文」也是模块文档写明的契约。
        //
        // 一致性 key 继续用在**已确定译名复用**上（那是它的设计用途：同一 MOD
        // 内已翻过的名字 / 专名，`Silver Hair` 与 `silver hair` 视为同一个）。
        let group_key = entry.source.clone();
        if !exact_groups.contains_key(&group_key) {
            exact_order.push(group_key.clone());
        }
        exact_groups.entry(group_key).or_default().push(entry);
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
            previous_failure: previous_failure_of(group.iter().copied()),
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

    /// 带「上一次结构校验失败」诊断的条目：模拟用户点「重试」时前端发回来的形态
    /// （`status` 已被重置为 pending，但 `error` 还留着）。
    fn failed_entry(source: &str, uid: &str, reason: &str) -> TranslationEntry {
        use crate::translation::fidelity::FIDELITY_FAILURE_MARKER;

        let mut entry = entry(source, uid);
        entry.status = crate::types::TranslationStatus::Error;
        entry.error = Some(format!(
            "大模型调用错误: {FIDELITY_FAILURE_MARKER}{reason}（已重试 1 次）"
        ));
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

    /// 重试时 job 必须带上「上一次失败原因」，把结构诊断喂回给模型。
    ///
    /// 这里覆盖三条路径：单条、相同原文合并（取组内第一条带诊断的）、以及
    /// 首次翻译（`error` 为空 → 不注入，行为与改动前逐字一致）。
    #[test]
    fn retry_jobs_carry_the_previous_failure_reason() {
        let sink = CollectingSink::new();

        // ① 首次翻译：没有任何诊断
        let (jobs, _) = plan_jobs(
            &[entry("Fireball", "u1")],
            &empty_matcher(),
            &CollectingSink::new(),
        );
        assert_eq!(jobs[0].previous_failure, None, "首次翻译不该注入原因");

        // ② 单条重试：诊断被取出来（剥掉前缀与「（已重试 1 次）」尾巴）
        let (jobs, _) = plan_jobs(
            &[failed_entry("Fireball", "u1", "占位符 [1] 多出")],
            &empty_matcher(),
            &sink,
        );
        assert_eq!(jobs[0].previous_failure.as_deref(), Some("占位符 [1] 多出"));

        // ③ 相同原文合并：组内任一成员带诊断就要带上（否则那一条又白错一轮）
        let (jobs, _) = plan_jobs(
            &[
                entry("Fireball", "u1"),
                failed_entry("Fireball", "u2", "缺少标签 <LSTag>"),
            ],
            &empty_matcher(),
            &sink,
        );
        assert_eq!(jobs.len(), 1, "相同原文仍然只发一次请求");
        assert_eq!(
            jobs[0].previous_failure.as_deref(),
            Some("缺少标签 <LSTag>")
        );

        // ④ 网络类错误不注入：它既是注入面，也帮不上「怎么改译文」
        let mut network_failure = entry("Fireball", "u1");
        network_failure.status = crate::types::TranslationStatus::Error;
        network_failure.error = Some("大模型调用错误: 连接超时（60 秒）".into());
        let (jobs, _) = plan_jobs(&[network_failure], &empty_matcher(), &sink);
        assert_eq!(jobs[0].previous_failure, None, "网络错误不该进 prompt");
    }

    /// 系列组成员的诊断同样要带回去。
    #[test]
    fn series_retry_jobs_carry_the_previous_failure_reason() {
        let sink = CollectingSink::new();
        let (jobs, _) = plan_jobs(
            &[
                failed_entry("Silver's Hair 9b", "u1", "占位符 [1] 多出"),
                entry("Silver's Hair 10b", "u2"),
            ],
            &empty_matcher(),
            &sink,
        );
        let series = jobs
            .iter()
            .find(|job| matches!(job.output, TranslationOutput::Series { .. }))
            .expect("两条变体应合成系列组");
        assert_eq!(series.previous_failure.as_deref(), Some("占位符 [1] 多出"));
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

    /// 只差句末标点的两条原文**不能**共享一份译文。
    ///
    /// 「相同原文合并」原本用的是**一致性 key**（折空白 / 去首尾 ASCII 标点 /
    /// 转小写），它比「同一段原文」宽：`…save?` 与 `…save!` 会归到一组，于是
    /// 后者的译文就是前者那一句 —— 疑问句被静默写成陈述句。结构校验拦不住
    /// （占位符 / 标签一模一样），玩家看到的是「标点与语气被改掉」。
    #[test]
    fn punctuation_only_differences_do_not_share_one_translation() {
        let sink = CollectingSink::new();
        let entries = vec![
            entry("Are you sure you want to delete this save?", "u1"),
            entry("Are you sure you want to delete this save!", "u2"),
            // 大小写也只是「看起来像」：不能拿一份译文贴到另一条上
            entry("DELETE SAVE", "u3"),
            entry("Delete save", "u4"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.total, 4);
        assert_eq!(plan.jobs, 4, "原文不同的条目必须各自成 job: {jobs:?}");
        assert_eq!(planned_entry_count(&jobs), 4, "覆盖数不能少");
        for job in &jobs {
            assert!(
                matches!(job.output, TranslationOutput::Single { .. }),
                "不该再出现跨原文的 ExactGroup: {job:?}"
            );
        }
        // 逐字相同的两条仍然合并（既有行为不许退化）
        let sink = CollectingSink::new();
        let (jobs, plan) = plan_jobs(
            &[entry("Delete save", "u1"), entry("Delete save", "u2")],
            &empty_matcher(),
            &sink,
        );
        assert_eq!(plan.jobs, 1);
        assert_eq!(
            jobs[0].output,
            TranslationOutput::ExactGroup {
                entry_ids: vec!["test.loca#u1".into(), "test.loca#u2".into()]
            }
        );
    }

    /// 一致性记忆的复用同样不能跨标点：`…save?` 的旧译文不能贴到 `…save!` 上。
    ///
    /// 上面那条把「相同原文合并」改用原文做键，但**记忆复用**仍走一致性 key
    /// （折空白 / 去首尾 ASCII 标点 / 转小写）。于是「已有译文 → 新条目」这条
    /// 路径上，疑问句的译文会被静默贴到感叹句条目上，还直接发 `Done`
    /// —— 玩家看到的标点与语气被改掉，写回时没有任何提示。
    #[test]
    fn punctuation_only_differences_do_not_reuse_one_translation() {
        let sink = CollectingSink::new();
        let entries = vec![
            translated(
                "Are you sure you want to delete this save?",
                "确定要删除此存档吗？",
                "t1",
            ),
            entry("Are you sure you want to delete this save!", "u1"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);

        assert_eq!(plan.total, 1);
        assert_eq!(
            plan.skipped,
            0,
            "标点不同的原文不能直接复用旧译文，事件: {:?}",
            sink.events()
        );
        assert_eq!(plan.jobs, 1, "必须重新发一次请求: {jobs:?}");
        assert!(
            !sink
                .events()
                .iter()
                .any(|event| matches!(event, TranslationEvent::Done { .. })),
            "不能静默产出 Done: {:?}",
            sink.events()
        );

        // 只差大小写 / 空白的仍然复用（既有行为不许退化）
        let sink = CollectingSink::new();
        let entries = vec![
            translated("Fireball", "火球术", "t1"),
            entry("fireball", "u1"),
            entry("  Fireball  ", "u2"),
        ];
        let (_, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.skipped, 2, "大小写与空白差异仍应命中记忆");
        assert_eq!(
            sink.events()
                .iter()
                .filter(|event| matches!(event, TranslationEvent::Done { .. }))
                .count(),
            2
        );
    }

    /// 撞号的两个系列不能互相串味：另一条中文系列的译文不能来自本系列。
    ///
    /// 系列别名靠「后缀重叠 ≥2」把中文系列归到拉丁系列上。同一 MOD 里另一个
    /// 只是部分编号撞上的中文系列（与它毫无关系）也会满足这条，于是整组拿到
    /// 英文系列的译文，且全程没有任何提示。
    #[test]
    fn unrelated_series_do_not_share_one_translation() {
        let sink = CollectingSink::new();
        let entries = vec![
            translated("Silver's Hair 1", "银发 1", "e1"),
            translated("Silver's Hair 2", "银发 2", "e2"),
            translated("Silver's Hair 3", "银发 3", "e3"),
            entry("银白长发1", "z1"),
            entry("银白长发2", "z2"),
        ];
        let (jobs, plan) = plan_jobs(&entries, &empty_matcher(), &sink);
        assert_eq!(plan.total, 2);
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
            Vec::<(String, String)>::new(),
            "只是编号撞上的系列不该直接复用别人的译文"
        );
        // 两条自己成一组（同一个 base），送去翻译的是**它们自己的** base
        assert_eq!(plan.jobs, 1, "只有这一组该发请求: {jobs:?}");
        assert_eq!(jobs[0].source, "银白长发");
        assert_eq!(
            jobs[0].target_ids(),
            vec!["test.loca#z1".to_string(), "test.loca#z2".to_string()]
        );
    }

    /// 被拒译文不能通过记忆复用落到别的条目上。
    ///
    /// `error` 条目的 `target` 里留着被结构校验拒掉的文本（`has_writable_target()`
    /// 明说它不能写回）。它一旦进记忆，另一条同原文的待翻译条目就会**直接拿到这段
    /// 坏文本**并发出 `Done` —— 用户既看不到错误，也不知道这是别人的半截译文。
    #[test]
    fn rejected_targets_are_not_reused_for_other_entries() {
        let sink = CollectingSink::new();
        let mut rejected = translated("Fireball", "火球", "u1");
        rejected.mark_error("大模型调用错误: 结构校验未通过：占位符 {1} 缺失（已重试 1 次）");

        let (jobs, plan) = plan_jobs(
            &[rejected, entry("fireball", "u2")],
            &empty_matcher(),
            &sink,
        );
        assert_eq!(plan.total, 1);
        assert_eq!(plan.skipped, 0, "被拒译文不能算「已确定译名」");
        assert!(
            !sink
                .events()
                .iter()
                .any(|event| matches!(event, TranslationEvent::Done { .. })),
            "不该把被拒译文当结果发出去: {:?}",
            sink.events()
        );
        assert_eq!(plan.jobs, 1, "应重新翻译: {jobs:?}");
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
            previous_failure: None,
            output: TranslationOutput::Single {
                entry_id: "e1".into(),
            },
        };
        assert_eq!(single.target_ids(), vec!["e1".to_string()]);

        let group = TranslationJob {
            source: "a".into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            previous_failure: None,
            output: TranslationOutput::ExactGroup {
                entry_ids: vec!["e1".into(), "e2".into()],
            },
        };
        assert_eq!(group.target_ids(), vec!["e1".to_string(), "e2".to_string()]);

        let series = TranslationJob {
            source: "a".into(),
            matches: Vec::new(),
            consistency_terms: Vec::new(),
            previous_failure: None,
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
