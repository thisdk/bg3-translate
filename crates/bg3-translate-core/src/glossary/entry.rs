//! 术语条目：serde 表示 + 导入时的噪音清洗。
//!
//! serde 表示**冻结**在这里：字段名是前端 TypeScript 字段名，也是用户导入
//! 的真实术语表 JSON 的字段名，任何改动都会静默破坏导入/导出。

use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

/// 术语分类（兼容真实数据，使用字符串以保持灵活）。
pub type Category = String;

/// 单条术语（字段与从游戏提取的真实术语表 JSON 一致）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GlossaryEntry {
    /// 英文术语
    pub source: String,
    /// 中文译名
    pub target: String,
    /// 分类仅用于兼容旧导入数据，界面与保存文件不再展示/输出。
    #[serde(default = "default_category", skip_serializing)]
    pub category: Category,
    /// 来源（official / user）
    #[serde(default, alias = "source_kind")]
    pub source_kind: String,
    /// 是否启用
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 是否有歧义（歧义项默认不参与命中，避免误替换）
    #[serde(default)]
    pub ambiguous: bool,
    /// 是否整词匹配
    #[serde(default = "default_true", alias = "whole_word")]
    pub whole_word: bool,
    /// 是否大小写敏感
    #[serde(default, alias = "case_sensitive")]
    pub case_sensitive: bool,
    /// 在游戏原文中的出现次数（排序参考）
    #[serde(default)]
    pub count: u32,
}

impl Default for GlossaryEntry {
    /// 默认值等价于「用户新加一条空术语」：启用、非歧义、整词、不区分大小写。
    fn default() -> Self {
        Self {
            source: String::new(),
            target: String::new(),
            category: default_category(),
            source_kind: "user".to_string(),
            enabled: true,
            ambiguous: false,
            whole_word: true,
            case_sensitive: false,
            count: 0,
        }
    }
}

fn default_category() -> String {
    "name_or_title".into()
}

fn default_true() -> bool {
    true
}

// ─────────────────────────────────────────────────────────────
// 噪音清洗
// ─────────────────────────────────────────────────────────────

/// 占位符模板：`[1] from [2]`、`+[1] Damage` 这种句式模板不是术语。
static PLACEHOLDER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[\d+\]").expect("占位符正则是常量，不会编译失败"));

/// UI 按键标记：`[DRUID][RANGER]` 这类全大写内部 ID。
static UI_MARKER_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[[A-Z_]{2,}\]").expect("UI 标记正则是常量，不会编译失败"));

/// 纯符号/标点：没有任何字母、数字或汉字的条目。
static SYMBOL_ONLY_PATTERN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[\W_]+$").expect("纯符号正则是常量，不会编译失败"));

/// 过短的常见英文词（of/and/the 等会误匹配正常文本，必须过滤）。
///
/// 表里全是小写：比较前把 source（≤4 字节）转小写即可覆盖 OF/Of/of。
const COMMON_SHORT_WORDS: &[&str] = &[
    "of", "and", "the", "or", "for", "to", "in", "on", "at", "by", "is", "it", "as", "be", "do",
    "no", "if", "an", "my", "we", "he", "up", "so", "tag", "end", "yes", "die", "one", "two",
    "from", "upon", "with", "that", "this", "then", "than", "but", "not", "all", "any", "can",
    "may", "has", "had", "was", "are", "were", "let", "set", "get", "put", "out", "off", "own",
    "new", "old", "big", "low",
];

/// 清洗单条术语，返回 `None` 表示应过滤掉（噪音）。
///
/// 输入是 `&mut`：命中清洗的情况下会把去掉外层引号后的文本写回条目，
/// 调用方拿到的就是最终入库的版本。
///
/// 正则全部走 [`LazyLock`] 预编译：旧实现每次调用都 `Regex::new`，
/// 导入 2 万条术语会编译几万次正则，是导入卡顿的主因。
pub(crate) fn clean_entry(entry: &mut GlossaryEntry) -> Option<GlossaryEntry> {
    let source = entry.source.trim();
    let target = entry.target.trim();

    // 空值过滤
    if source.is_empty() || target.is_empty() {
        return None;
    }

    // ── 噪音过滤（这些不是术语，是从本地化文件机械提取的碎片）──

    // 占位符模板（[1] from [2]、+[1] Damage 这种句式模板）
    if PLACEHOLDER_PATTERN.is_match(source) {
        return None;
    }
    // UI 按键标记（[IE_xxx]、[DRUID] 这种内部 ID）
    if source.contains("[IE_") || UI_MARKER_PATTERN.is_match(source) {
        return None;
    }
    // 破折号破碎文本（-- come ----、---OBEY--- 这种）
    if source.matches('-').count() >= 2 {
        let words: Vec<&str> = source.split_whitespace().collect();
        let non_dash_words = words
            .iter()
            .filter(|w| !w.chars().all(|c| c == '-'))
            .count();
        if non_dash_words <= 2 {
            return None;
        }
    }
    // 纯符号/标点
    if SYMBOL_ONLY_PATTERN.is_match(source) {
        return None;
    }
    // 过短的常见英文词
    if source.len() <= 4 {
        let lower = source.to_lowercase();
        if COMMON_SHORT_WORDS.contains(&lower.as_str()) {
            return None;
        }
    }

    entry.source = strip_full_wrapping_quotes(source);
    entry.target = strip_full_wrapping_quotes(target);
    Some(entry.clone())
}

/// 只去除包住整条文本的成对引号。
///
/// - `"丹瑟隆的飞斧"` → `丹瑟隆的飞斧`
/// - `"制动"拉杆` → `"制动"拉杆`（引号没有包住整条，保留）
pub(crate) fn strip_full_wrapping_quotes(s: &str) -> String {
    let s = s.trim();
    let Some(first) = s.chars().next() else {
        return String::new();
    };
    let Some(expected_last) = matching_quote(first) else {
        return s.to_string();
    };
    if !s.ends_with(expected_last) {
        return s.to_string();
    }

    let start = first.len_utf8();
    let end = s.len() - expected_last.len_utf8();
    if start >= end {
        return s.to_string();
    }

    let inner = s[start..end].trim();
    if inner.is_empty() {
        s.to_string()
    } else {
        inner.to_string()
    }
}

/// 起始引号 → 期望的结束引号；不是引号则返回 `None`。
fn matching_quote(c: char) -> Option<char> {
    match c {
        '\'' => Some('\''),
        '"' => Some('"'),
        '“' => Some('”'),
        '‘' => Some('’'),
        '「' => Some('」'),
        '『' => Some('』'),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(source: &str, target: &str) -> GlossaryEntry {
        GlossaryEntry {
            source: source.to_string(),
            target: target.to_string(),
            ..GlossaryEntry::default()
        }
    }

    #[test]
    fn entry_uses_camel_case_and_aliases() {
        let value = serde_json::to_value(entry("Fireball", "火球术")).unwrap();
        assert_eq!(value["source"], "Fireball");
        assert_eq!(value["target"], "火球术");
        assert_eq!(value["sourceKind"], "user");
        assert_eq!(value["enabled"], true);
        assert_eq!(value["ambiguous"], false);
        assert_eq!(value["wholeWord"], true);
        assert_eq!(value["caseSensitive"], false);
        assert_eq!(value["count"], 0);
        // category 明确不落盘
        assert!(value.get("category").is_none());

        // 旧字段名（snake_case）与 category 必须能读进来
        let parsed: GlossaryEntry = serde_json::from_str(
            r#"{"source":"Paladin","target":"圣武士","category":"class","source_kind":"official","whole_word":false,"case_sensitive":true,"count":7}"#,
        )
        .unwrap();
        assert_eq!(parsed.category, "class");
        assert_eq!(parsed.source_kind, "official");
        assert!(!parsed.whole_word);
        assert!(parsed.case_sensitive);
        assert_eq!(parsed.count, 7);
        assert!(parsed.enabled, "enabled 缺省应为 true");
        assert!(!parsed.ambiguous);
    }

    #[test]
    fn entry_defaults_track_serde_defaults() {
        let parsed: GlossaryEntry = serde_json::from_str(r#"{"source":"a","target":"b"}"#).unwrap();
        let default = GlossaryEntry::default();
        assert_eq!(parsed.category, default.category);
        assert_eq!(parsed.enabled, default.enabled);
        assert_eq!(parsed.whole_word, default.whole_word);
        assert_eq!(parsed.ambiguous, default.ambiguous);
        assert_eq!(parsed.case_sensitive, default.case_sensitive);
        assert_eq!(parsed.count, default.count);
        assert_eq!(parsed.category, "name_or_title");
    }

    #[test]
    fn clean_rejects_blank_entries() {
        assert!(clean_entry(&mut entry("", "空")).is_none());
        assert!(clean_entry(&mut entry("   ", "空")).is_none());
        assert!(clean_entry(&mut entry("Empty", "")).is_none());
        assert!(clean_entry(&mut entry("Empty", "  ")).is_none());
    }

    #[test]
    fn clean_rejects_placeholder_templates() {
        assert!(clean_entry(&mut entry("[1] from [2]", "从[2]处取走[1]")).is_none());
        assert!(clean_entry(&mut entry("+[1] Damage", "额外[1]伤害")).is_none());
        // 没有数字占位符的正常文本不受影响
        let kept = clean_entry(&mut entry("[Not a placeholder]", "保留")).unwrap();
        assert_eq!(kept.source, "[Not a placeholder]");
    }

    #[test]
    fn clean_rejects_ui_markers() {
        assert!(clean_entry(&mut entry("[IE_ContextMenu] Ping", "[IE_ContextMenu]标记")).is_none());
        assert!(clean_entry(&mut entry("[DRUID][RANGER]", "[德鲁伊][游侠]")).is_none());
        // 小写中括号开头的不是 UI 标记
        let kept = clean_entry(&mut entry("[hello] world", "你好世界")).unwrap();
        assert_eq!(kept.source, "[hello] world");
    }

    #[test]
    fn clean_rejects_dash_fragments() {
        assert!(clean_entry(&mut entry("--OBEY--", "--服从--")).is_none());
        assert!(clean_entry(&mut entry("---COME---obey- --yield---", "来吧")).is_none());
        // 破折号多但词数 > 2 时保留（正常句子里的连字符）
        let kept = clean_entry(&mut entry(
            "well-known half-elf name here",
            "知名半精灵名字",
        ))
        .unwrap();
        assert!(kept.source.contains("half-elf"));
    }

    #[test]
    fn clean_rejects_symbols_and_common_short_words() {
        assert!(clean_entry(&mut entry("!!!", "感叹")).is_none());
        assert!(clean_entry(&mut entry("...", "省略")).is_none());
        assert!(clean_entry(&mut entry("of", "之")).is_none());
        assert!(clean_entry(&mut entry("THE", "这")).is_none());
        assert!(clean_entry(&mut entry("The", "这")).is_none());
        // 4 字节以内但不在常见词表里的保留
        let kept = clean_entry(&mut entry("Gale", "盖尔")).unwrap();
        assert_eq!(kept.target, "盖尔");
    }

    #[test]
    fn clean_strips_full_quotes_but_keeps_partial_quotes() {
        let first =
            clean_entry(&mut entry("'Danthelon's Dancing Axe'", "\"丹瑟隆的飞斧\"")).unwrap();
        assert_eq!(first.source, "Danthelon's Dancing Axe");
        assert_eq!(first.target, "丹瑟隆的飞斧");

        let second = clean_entry(&mut entry("'Brake' Lever", "\"制动\"拉杆")).unwrap();
        assert_eq!(second.source, "'Brake' Lever");
        assert_eq!(second.target, "\"制动\"拉杆");
    }

    #[test]
    fn strip_full_wrapping_quotes_handles_unicode_quotes() {
        assert_eq!(strip_full_wrapping_quotes("“你好”"), "你好");
        assert_eq!(strip_full_wrapping_quotes("「你好」"), "你好");
        assert_eq!(strip_full_wrapping_quotes("『你好』"), "你好");
        assert_eq!(strip_full_wrapping_quotes("‘你好’"), "你好");
        assert_eq!(strip_full_wrapping_quotes("\"你好\""), "你好");
        // 不成对 / 单字符 / 空内容一律原样返回
        assert_eq!(strip_full_wrapping_quotes("\"你好"), "\"你好");
        assert_eq!(strip_full_wrapping_quotes("\""), "\"");
        assert_eq!(strip_full_wrapping_quotes("\"\""), "\"\"");
        assert_eq!(strip_full_wrapping_quotes("  "), "");
        assert_eq!(strip_full_wrapping_quotes("plain"), "plain");
    }

    #[test]
    fn clean_trims_whitespace_before_storing() {
        let cleaned = clean_entry(&mut entry("  Fireball  ", "  火球术  ")).unwrap();
        assert_eq!(cleaned.source, "Fireball");
        assert_eq!(cleaned.target, "火球术");
    }

    #[test]
    fn noise_regexes_are_compiled_once() {
        // 行为断言：清洗 2000 条噪音条目不应触发正则重复编译（旧实现会编译 6000 次）。
        // 这里只验证结果稳定，编译次数由 LazyLock 的语义保证。
        for _ in 0..500 {
            assert!(clean_entry(&mut entry("[1] from [2]", "x")).is_none());
            assert!(clean_entry(&mut entry("!!!", "x")).is_none());
            assert!(clean_entry(&mut entry("[DRUID]", "x")).is_none());
        }
        // LazyLock 只初始化一次：取两次引用是同一地址
        let a: &Regex = &PLACEHOLDER_PATTERN;
        let b: &Regex = &PLACEHOLDER_PATTERN;
        assert!(std::ptr::eq(a, b));
    }

    #[test]
    fn cleaned_entry_serializes_without_category() {
        let cleaned = clean_entry(&mut entry("Fireball", "火球术")).unwrap();
        let value = serde_json::to_value(&cleaned).unwrap();
        assert_eq!(
            value,
            json!({
                "source": "Fireball",
                "target": "火球术",
                "sourceKind": "user",
                "enabled": true,
                "ambiguous": false,
                "wholeWord": true,
                "caseSensitive": false,
                "count": 0,
            })
        );
    }
}
