//! 术语命中匹配器：把「每条文本扫全表」变成「一次预筛 + 预编译校验」。
//!
//! 旧实现对每条待翻译文本都遍历全部术语，并对每条术语调用一次
//! `to_lowercase()`，命中时还要 `Regex::new()` 现场编译词边界正则。
//! 2 万条术语下每条文本约 2 万次堆分配，导入后的首次翻译会明显卡顿。
//!
//! 这里的做法：
//! 1. 构造时一次性过滤（`enabled && !ambiguous && source 非空`）、预计算
//!    lowercase、按 `source` 长度降序稳定排序；
//! 2. 用 Aho-Corasick 做**重叠**子串预筛 —— 重叠是必须的：
//!    `Mind Flayer` 与 `Mind` 同时在表里时，非重叠扫描只会报其中一个；
//! 3. 命中后用预编译的 `\b...\b` 正则做词边界校验。

use aho_corasick::{AhoCorasick, AhoCorasickBuilder, MatchKind};
use regex::Regex;

use super::store::Glossary;

/// 命中项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchedTerm {
    pub source: String,
    pub target: String,
}

/// 预处理后的单条术语。
#[derive(Debug)]
struct PreparedTerm {
    source: String,
    target: String,
    case_sensitive: bool,
    whole_word: bool,
    /// 预编译的 `\b...\b`；编译失败时为 `None`（退化为子串命中）。
    boundary: Option<Regex>,
}

/// 术语表匹配器：构造一次，可反复用于任意多条文本。
#[derive(Debug)]
pub struct GlossaryMatcher {
    /// 已按 `source` 长度降序（同长度保持原表顺序）排好的术语。
    terms: Vec<PreparedTerm>,
    /// 大小写不敏感术语的子串预筛（haystack 需要先 lowercase）。
    insensitive: Option<AhoCorasick>,
    /// pattern id → [`Self::terms`] 下标。
    insensitive_map: Vec<usize>,
    /// 大小写敏感术语的子串预筛（直接扫原文）。
    sensitive: Option<AhoCorasick>,
    /// pattern id → [`Self::terms`] 下标。
    sensitive_map: Vec<usize>,
}

impl GlossaryMatcher {
    /// 构造匹配器：一次性完成过滤、lowercase 预计算、排序与正则编译。
    pub fn new(glossary: &Glossary) -> Self {
        let mut terms: Vec<PreparedTerm> = glossary
            .terms
            .iter()
            .filter(|entry| entry.enabled && !entry.ambiguous && !entry.source.trim().is_empty())
            .map(|entry| PreparedTerm {
                boundary: if entry.whole_word {
                    compile_boundary(&needle_for(entry.case_sensitive, &entry.source))
                } else {
                    None
                },
                source: entry.source.clone(),
                target: entry.target.clone(),
                case_sensitive: entry.case_sensitive,
                whole_word: entry.whole_word,
            })
            .collect();

        // 长术语优先（避免短术语覆盖长术语，如 "Mind Flayer" 优先于 "Mind"）。
        // 稳定排序保证同长度时仍按原表顺序输出，与旧实现的排序语义一致。
        terms.sort_by_key(|term| std::cmp::Reverse(term.source.len()));

        let mut insensitive_patterns = Vec::new();
        let mut insensitive_map = Vec::new();
        let mut sensitive_patterns = Vec::new();
        let mut sensitive_map = Vec::new();
        for (index, term) in terms.iter().enumerate() {
            if term.case_sensitive {
                sensitive_patterns.push(term.source.clone());
                sensitive_map.push(index);
            } else {
                insensitive_patterns.push(term.source.to_lowercase());
                insensitive_map.push(index);
            }
        }

        Self {
            insensitive: build_automaton(insensitive_patterns),
            insensitive_map,
            sensitive: build_automaton(sensitive_patterns),
            sensitive_map,
            terms,
        }
    }

    /// 在当前文本里查找命中的术语，结果按「长术语优先」排列。
    pub fn find_matches(&self, text: &str) -> Vec<MatchedTerm> {
        if self.terms.is_empty() {
            return Vec::new();
        }

        let mut hit = vec![false; self.terms.len()];

        // 大小写不敏感：只对**文本**做一次 lowercase（旧实现是对每条术语做一次）。
        let lowered = (!self.insensitive_map.is_empty()).then(|| text.to_lowercase());
        if let (Some(automaton), Some(haystack)) = (&self.insensitive, lowered.as_deref()) {
            mark_hits(automaton, &self.insensitive_map, haystack, &mut hit);
        }
        if let Some(automaton) = &self.sensitive {
            mark_hits(automaton, &self.sensitive_map, text, &mut hit);
        }

        let mut matches = Vec::new();
        for (index, term) in self.terms.iter().enumerate() {
            if !hit[index] {
                continue;
            }
            if term.whole_word {
                let haystack = if term.case_sensitive {
                    text
                } else {
                    lowered.as_deref().unwrap_or(text)
                };
                if !boundary_confirms(term.boundary.as_ref(), haystack) {
                    continue;
                }
            }
            matches.push(MatchedTerm {
                source: term.source.clone(),
                target: term.target.clone(),
            });
        }
        matches
    }

    /// 参与匹配的术语条数（过滤后的）。
    pub fn len(&self) -> usize {
        self.terms.len()
    }

    /// 是否没有任何可参与匹配的术语。
    pub fn is_empty(&self) -> bool {
        self.terms.is_empty()
    }
}

/// 用重叠扫描把所有出现过的 pattern 标记为命中。
///
/// 必须用 `find_overlapping_iter`：`Standard` 语义下它报告所有出现位置，
/// 包括互相重叠的 pattern，这样 `Mind Flayer` 与 `Mind` 才能同时命中。
fn mark_hits(automaton: &AhoCorasick, map: &[usize], haystack: &str, hit: &mut [bool]) {
    for found in automaton.find_overlapping_iter(haystack) {
        if let Some(index) = map.get(found.pattern().as_usize()) {
            hit[*index] = true;
        }
    }
}

/// 需要交给词边界正则的 needle：大小写不敏感时用小写形式。
fn needle_for(case_sensitive: bool, source: &str) -> String {
    if case_sensitive {
        source.to_string()
    } else {
        source.to_lowercase()
    }
}

/// 编译词边界正则；失败时返回 `None`。
///
/// `\b` 只在**紧邻 `\w` 的那一侧**成立。旧实现一律写成 `\b{needle}\b`：
/// 术语首尾是标点时（`+1 Sword`、`{1} damage`），对应那一侧的 `\b` 两侧都是
/// 非单词字符，正则永远不成立 —— 预筛明明命中了，边界校验却把整条术语丢掉，
/// 用户看到的是「这条术语怎么都不生效」。
fn compile_boundary(needle: &str) -> Option<Regex> {
    let left = if needle.chars().next().is_some_and(is_word_char) {
        r"\b"
    } else {
        ""
    };
    let right = if needle.chars().next_back().is_some_and(is_word_char) {
        r"\b"
    } else {
        ""
    };
    Regex::new(&format!("{left}{}{right}", regex::escape(needle))).ok()
}

/// 与 regex 的 `\w` 对齐（Unicode 感知：字母 / 数字 / 下划线）。
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// 词边界校验。
///
/// `None` 表示正则编译失败：此时**退化为子串命中**（子串存在已由 Aho-Corasick
/// 确认），与旧实现 `Err(_) => true` 的退化行为一致。
fn boundary_confirms(boundary: Option<&Regex>, haystack: &str) -> bool {
    match boundary {
        Some(regex) => regex.is_match(haystack),
        None => true,
    }
}

/// 空 pattern 表返回 `None`：Aho-Corasick 不接受空模式集。
fn build_automaton(patterns: Vec<String>) -> Option<AhoCorasick> {
    if patterns.is_empty() {
        return None;
    }
    match AhoCorasickBuilder::new()
        .match_kind(MatchKind::Standard)
        .build(patterns)
    {
        Ok(automaton) => Some(automaton),
        Err(err) => {
            // 预筛索引构建失败（内存不足等）：退化为「无子串预筛」，
            // find_matches 仍能工作，只是命中集合为空。
            log::warn!("术语预筛索引构建失败，本次会话将无法命中术语: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::*;
    use crate::glossary::entry::GlossaryEntry;
    use crate::glossary::store::Glossary;

    fn glossary_of(pairs: &[(&str, &str)]) -> Glossary {
        Glossary {
            terms: pairs
                .iter()
                .map(|(source, target)| GlossaryEntry {
                    source: (*source).to_string(),
                    target: (*target).to_string(),
                    ..GlossaryEntry::default()
                })
                .collect(),
        }
    }

    fn sources(text: &str, glossary: &Glossary) -> Vec<String> {
        GlossaryMatcher::new(glossary)
            .find_matches(text)
            .into_iter()
            .map(|term| term.source)
            .collect()
    }

    #[test]
    fn matches_case_insensitively_by_default() {
        let glossary = glossary_of(&[("Paladin", "圣武士"), ("Divine Smite", "至圣斩")]);
        let found = sources("The PALADIN strikes with divine smite", &glossary);
        assert_eq!(found, vec!["Divine Smite", "Paladin"]);
    }

    #[test]
    fn case_sensitive_terms_only_match_exact_case() {
        let mut glossary = glossary_of(&[("Gate", "大门")]);
        glossary.terms[0].case_sensitive = true;
        assert!(sources("the gate", &glossary).is_empty());
        assert_eq!(sources("Baldur's Gate", &glossary), vec!["Gate"]);
    }

    #[test]
    fn whole_word_requires_word_boundaries() {
        let glossary = glossary_of(&[("Cantrip", "戏法")]);
        assert!(sources("Spelling bee", &glossary).is_empty());
        assert_eq!(sources("A cantrip!", &glossary), vec!["Cantrip"]);
        // 标点/空白都算边界
        assert_eq!(sources("(Cantrip)", &glossary), vec!["Cantrip"]);
    }

    /// 术语首尾是标点时，`\b` 会**永远**匹配不上：整条术语静默失效。
    ///
    /// `\b` 的含义是「一侧是 `\w`、另一侧不是」。`{1} damage` 的开头是 `{`、
    /// `+1 Sword` 的开头是 `+`，它们与前面的空格之间都不是词边界，于是
    /// `\b\{1\} damage\b` 在任何正常句子里都不成立——预筛命中了也会被边界
    /// 校验丢掉。术语表里这类条目（`+1`、`{1}` 开头的句式模板）真实存在。
    #[test]
    fn whole_word_terms_with_punctuation_edges_still_match() {
        let glossary = glossary_of(&[("+1 Sword", "＋1 长剑"), ("{1} damage", "伤害")]);
        let found = sources("Deal {1} damage with a +1 Sword", &glossary);
        assert_eq!(found, vec!["{1} damage", "+1 Sword"]);
    }

    /// 反过来：标点结尾的术语也不能因此变成「任意后缀都算命中」。
    #[test]
    fn punctuation_edged_terms_still_respect_the_word_side() {
        // 结尾是标点 → 右侧不加 `\b`；但左侧仍是词首，`Sword+` 不应在
        // `Swords+` 里命中（左边界靠 `\b` 保住）
        let glossary = glossary_of(&[("Sword+", "剑+")]);
        assert_eq!(sources("a Sword+ here", &glossary), vec!["Sword+"]);
        assert!(sources("Swords+ here", &glossary).is_empty());
    }

    #[test]
    fn non_whole_word_terms_match_inside_words() {
        let mut glossary = glossary_of(&[("Fire", "火")]);
        glossary.terms[0].whole_word = false;
        assert_eq!(sources("Fireball", &glossary), vec!["Fire"]);
    }

    #[test]
    fn longer_terms_are_reported_first() {
        let glossary = glossary_of(&[("Mind", "心灵"), ("Mind Flayer", "夺心魔")]);
        let matches = GlossaryMatcher::new(&glossary).find_matches("A Mind Flayer appears");
        let sources: Vec<_> = matches.iter().map(|m| m.source.as_str()).collect();
        assert_eq!(sources, vec!["Mind Flayer", "Mind"]);
    }

    #[test]
    fn overlapping_terms_are_all_reported() {
        // Aho-Corasick 的非重叠扫描会漏掉其中一个，这里锁死重叠语义
        let glossary = glossary_of(&[("app", "应用"), ("append", "追加"), ("appendage", "附属物")]);
        let found = sources("append the app to the appendage", &glossary);
        assert_eq!(found, vec!["appendage", "append", "app"]);
    }

    #[test]
    fn disabled_and_ambiguous_terms_never_match() {
        let mut glossary = glossary_of(&[
            ("Enabled", "启用"),
            ("Disabled", "禁用"),
            ("Ambiguous", "歧义"),
        ]);
        glossary.terms[1].enabled = false;
        glossary.terms[2].ambiguous = true;
        let found = sources("Enabled Disabled Ambiguous", &glossary);
        assert_eq!(found, vec!["Enabled"]);
    }

    #[test]
    fn blank_sources_are_ignored() {
        let glossary = glossary_of(&[("   ", "空白"), ("Real", "真的")]);
        let matcher = GlossaryMatcher::new(&glossary);
        assert_eq!(matcher.len(), 1);
        assert_eq!(
            matcher.find_matches("Real")[0].source,
            "Real",
            "空白 source 不应进入索引"
        );
    }

    #[test]
    fn empty_inputs_are_safe() {
        let empty = Glossary { terms: Vec::new() };
        let matcher = GlossaryMatcher::new(&empty);
        assert!(matcher.is_empty());
        assert!(matcher.find_matches("anything").is_empty());

        let glossary = glossary_of(&[("Fireball", "火球术")]);
        assert!(sources("", &glossary).is_empty());
        assert!(sources("nothing here", &glossary).is_empty());
    }

    #[test]
    fn duplicates_are_reported_once_per_entry() {
        let glossary = glossary_of(&[("Gale", "盖尔"), ("Gale", "大风")]);
        let matches = GlossaryMatcher::new(&glossary).find_matches("Gale");
        assert_eq!(matches.len(), 2, "同名条目各算一条，与旧实现一致");
        assert_eq!(matches[0].target, "盖尔");
        assert_eq!(matches[1].target, "大风");
    }

    #[test]
    fn regex_compile_failure_degrades_to_substring_hit() {
        // 无法在测试里稳定构造「正则编译失败」，直接锁死退化函数的契约
        assert!(boundary_confirms(None, "任意文本"));
        let regex = Regex::new(r"\bGale\b").unwrap();
        assert!(boundary_confirms(Some(&regex), "Gale"));
        assert!(!boundary_confirms(Some(&regex), "Galen"));
    }

    #[test]
    fn matcher_preprocesses_terms_once() {
        let glossary = glossary_of(&[
            ("Mind Flayer", "夺心魔"),
            ("Mind", "心灵"),
            ("Cantrip", "戏法"),
        ]);
        let matcher = GlossaryMatcher::new(&glossary);
        // 长度降序：Mind Flayer(11) > Cantrip(7) > Mind(4)
        let sources: Vec<_> = matcher.terms.iter().map(|t| t.source.as_str()).collect();
        assert_eq!(sources, vec!["Mind Flayer", "Cantrip", "Mind"]);
        assert_eq!(matcher.insensitive.as_ref().unwrap().patterns_len(), 3);
        assert!(matcher.sensitive.is_none());
        // 反复调用结果稳定（预筛索引不可变）
        for _ in 0..3 {
            assert_eq!(matcher.find_matches("Mind Flayer casts a cantrip").len(), 3);
        }
    }

    /// 真实 20K 官方术语表（`samples/`）。
    ///
    /// 样本随仓库提交且非 Git LFS，缺失 = checkout 不完整：这里**直接失败**
    /// 而不是跳过 —— 跳过会让这条真实数据用例静默变空，而测试依然全绿。
    fn real_glossary() -> Glossary {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crate 应位于 <repo>/crates/bg3-translate-core")
            .join("samples/bg3-official-glossary.json");
        let json = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "真实术语表样本缺失或不可读：{}（{err}）。该文件随仓库提交（非 Git LFS），\
                 缺失说明 checkout 不完整；请执行 `git checkout -- samples/` 或重新 clone。",
                path.display()
            )
        });
        Glossary::from_json(&json).expect("真实术语表样本应能被解析")
    }

    #[test]
    fn real_glossary_matching_is_fast_and_ordered() {
        let glossary = real_glossary();

        let started = Instant::now();
        let matcher = GlossaryMatcher::new(&glossary);
        let build = started.elapsed();
        assert!(matcher.len() > 15_000, "参与匹配的术语过少");
        assert!(
            build < Duration::from_secs(20),
            "真实术语表构造过慢: {build:?}"
        );

        // 大小写不敏感 + 期望命中真实官方译名
        let matches = matcher.find_matches("a mind flayer haunts baldur's gate with astarion");
        let pairs: Vec<(&str, &str)> = matches
            .iter()
            .map(|m| (m.source.as_str(), m.target.as_str()))
            .collect();
        assert!(
            pairs.contains(&("Mind Flayer", "夺心魔")),
            "实际: {pairs:?}"
        );
        assert!(pairs.contains(&("Baldur's Gate", "博德之门")));
        assert!(pairs.contains(&("Astarion", "阿斯代伦")));

        // 长术语优先
        let sources: Vec<&str> = matches.iter().map(|m| m.source.as_str()).collect();
        let longest_first = sources
            .windows(2)
            .all(|pair| pair[0].len() >= pair[1].len());
        assert!(longest_first, "结果未按长度降序: {sources:?}");

        let texts = [
            "The Emperor watches Astarion and Shadowheart cross Baldur's Gate.",
            "A Mind Flayer tadpole grants strange powers.",
            "Cast Fireball for {1} damage on the Sword Coast.",
            "No known term appears in this plain sentence at all.",
        ];
        let started = Instant::now();
        let mut hits = 0usize;
        for _ in 0..50 {
            for text in texts {
                hits += matcher.find_matches(text).len();
            }
        }
        let elapsed = started.elapsed();
        assert!(hits > 0);
        assert!(
            elapsed < Duration::from_secs(2),
            "真实术语表 200 次匹配耗时过长: {elapsed:?}"
        );
    }

    /// 真实术语表回归：`'Brake' Lever` 这类「首字符是引号」的条目
    /// 过去因为 `\b` 永远命中不了（19,524 条可用术语里有 266 条属于这一类）。
    #[test]
    fn real_glossary_punctuation_edged_terms_now_match() {
        let matcher = GlossaryMatcher::new(&real_glossary());
        let matches = matcher.find_matches("Pull the 'Brake' Lever now");
        let sources: Vec<&str> = matches.iter().map(|m| m.source.as_str()).collect();
        assert!(
            sources.contains(&"'Brake' Lever"),
            "真实条目必须能命中，实际: {sources:?}"
        );
    }

    #[test]
    fn twenty_thousand_terms_stay_fast() {
        // 复杂度回归：2 万条术语 + 1000 条文本。旧实现每条文本 2 万次
        // `to_lowercase()` + 命中时现场编译正则，这里必须远低于阈值。
        // 阈值故意放得很松（慢 CI 也不会误报），只拦「退化成 O(n·m)」的回归。
        let terms: Vec<GlossaryEntry> = (0..20_000)
            .map(|i| GlossaryEntry {
                source: format!("SynthTerm{i}"),
                target: format!("合成术语{i}"),
                ..GlossaryEntry::default()
            })
            .collect();
        let glossary = Glossary { terms };

        let started = std::time::Instant::now();
        let matcher = GlossaryMatcher::new(&glossary);
        let build_elapsed = started.elapsed();
        assert_eq!(matcher.len(), 20_000);
        assert!(
            build_elapsed < std::time::Duration::from_secs(20),
            "匹配器构造过慢: {build_elapsed:?}"
        );

        let texts: Vec<String> = (0..1000)
            .map(|i| {
                format!(
                    "Line {i} mentions SynthTerm{} and SynthTerm{} plus the word Mind.",
                    i % 20_000,
                    (i * 7 + 1) % 20_000
                )
            })
            .collect();

        let started = std::time::Instant::now();
        let mut total = 0usize;
        for text in &texts {
            total += matcher.find_matches(text).len();
        }
        let match_elapsed = started.elapsed();
        assert_eq!(total, 2000, "每条文本应命中两条合成术语");
        assert!(
            match_elapsed < std::time::Duration::from_secs(2),
            "2 万条术语 × 1000 条文本匹配耗时过长: {match_elapsed:?}"
        );
    }
}
