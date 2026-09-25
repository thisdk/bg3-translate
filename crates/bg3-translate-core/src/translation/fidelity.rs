//! 译文结构保真校验：占位符与富文本标签的「结构签名」比对。
//!
//! 系统 prompt 要求模型原样保留 `{1}` 这类占位符与 `<LSTag ...>` 这类富文本
//! 标签，但在此之前**没有任何代码检查过**：模型丢了占位符照样算成功译文，
//! 直到写盘时 `content_list` 才把标签不配对的文本降级成纯文本，用户看不见。
//! 这个模块补上这条防线：只比较**结构**，不比较内容。
//!
//! ## 抓什么
//!
//! - 占位符：`{1}`、`{10}`、`{name}`、`{user_name}` 形式的多重集必须一致
//!   （缺失 / 多余 / 重复都报）；顺序不算问题（`{1} {2}` ↔ `{2} {1}` 保真）。
//!   标签属性值里的占位符同样参与比对：属性值可以改写，但 `{1}` 不能丢。
//! - 标签：`<LSTag ...>`、`</LSTag>`、`<br/>` 的**标签名序列**必须一致
//!   （开 / 闭 / 空元素三种形态分开算，`<br>` 这类空元素不要求闭合）；
//!   属性值、标签之间的正文不参与比较。非空元素的 `<x/>` 与 `<x></x>` 视为
//!   同一签名（XML 语义等价），代价是「空标签」与「包住文本的标签对」也分不出来。
//!
//! ## 明确不抓什么（防误报）
//!
//! - 形态不良好的尖括号一律当普通文本：`< 5`、`a < b`、`a < b > c`、`<5>`；
//! - 不在写回白名单里的标签名一律当普通文本（`<name>`、`<color>`）：
//!   `content_list::write_text_fragment` 只把白名单标签写成真标签，其余会转义，
//!   所以它们本来就没有「必须配对」的义务；
//! - `&lt;` / `&gt;` 先还原成字面尖括号再比较：解析层已经把 `&lt;` 还原过一次，
//!   模型若把译文里的 `<` 重新转义回去，不算结构变化；
//! - 属性值变化、标签内文本被翻译、正文整体改写都不报；唯一会报的「非缺失」
//!   情形是标签的出现顺序 / 嵌套结构与原文不同。
//!
//! 原则：**宁可漏报，不可误报** —— 误报会把本来正确的译文判失败，代价比偶尔
//! 漏掉一个坏译文高得多。所有规则都有对应的边界单测（见文件末尾）。

use crate::formats::content_list::is_allowed_inline_tag;

/// 空元素标签：`<br>` / `<br/>` 都不需要闭合标签。
///
/// 与 `formats::content_list` 的 `VOID_TAGS` 保持一致：那边决定写回时怎么处理，
/// 这边决定校验时怎么算，两边对「空元素」的认定必须相同。
const VOID_TAGS: &[&str] = &["br"];

/// 摘要 / 纠错提示里最多列出的问题条数（避免把整段译文塞进错误信息）。
const MAX_LISTED_ISSUES: usize = 3;

/// 一条结构保真问题。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FidelityIssue {
    /// 原文有、译文缺失（或重复次数不足）的占位符
    MissingPlaceholder { token: String, count: usize },
    /// 译文里多出来（或重复次数过多）的占位符
    ExtraPlaceholder { token: String, count: usize },
    /// 原文有、译文缺失的标签，`tag` 是渲染形态（`<LSTag>` / `</LSTag>` / `<br/>`）
    MissingTag { tag: String, count: usize },
    /// 译文里多出来的标签
    ExtraTag { tag: String, count: usize },
    /// 标签多重集一致，但出现顺序 / 嵌套结构与原文不同
    TagStructureChanged,
}

impl FidelityIssue {
    /// 纠错提示里的说法（拼进重试请求）。
    pub fn hint(&self) -> String {
        match self {
            FidelityIssue::MissingPlaceholder { token, count } => {
                format!("缺少{}占位符 {token}", count_text(*count))
            }
            FidelityIssue::ExtraPlaceholder { token, count } => {
                format!("多出{}占位符 {token}", count_text(*count))
            }
            FidelityIssue::MissingTag { tag, count } => {
                format!("缺少{}标签 {tag}", count_text(*count))
            }
            FidelityIssue::ExtraTag { tag, count } => {
                format!("多出{}标签 {tag}", count_text(*count))
            }
            FidelityIssue::TagStructureChanged => "标签顺序或嵌套与原文不一致".to_string(),
        }
    }
}

/// `count` 处 → `"3 处"`；只有 1 处时不啰嗦。
fn count_text(count: usize) -> String {
    if count > 1 {
        format!(" {count} 处")
    } else {
        String::new()
    }
}

impl std::fmt::Display for FidelityIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FidelityIssue::MissingPlaceholder { token, count } => {
                write!(f, "占位符 {token} 缺失{}", count_text(*count))
            }
            FidelityIssue::ExtraPlaceholder { token, count } => {
                write!(f, "占位符 {token} 多出{}", count_text(*count))
            }
            FidelityIssue::MissingTag { tag, count } => {
                write!(f, "标签 {tag} 缺失{}", count_text(*count))
            }
            FidelityIssue::ExtraTag { tag, count } => {
                write!(f, "标签 {tag} 多出{}", count_text(*count))
            }
            FidelityIssue::TagStructureChanged => write!(f, "标签顺序或嵌套与原文不一致"),
        }
    }
}

/// 比较原文与译文的结构签名；返回空列表表示保真。
pub fn check_fidelity(source: &str, target: &str) -> Vec<FidelityIssue> {
    let source_signature = Signature::of(source);
    let target_signature = Signature::of(target);
    let mut issues = Vec::new();

    for (token, expected) in counted(&source_signature.placeholders) {
        let found = target_signature.count_placeholder(&token);
        if found < expected {
            issues.push(FidelityIssue::MissingPlaceholder {
                token,
                count: expected - found,
            });
        }
    }
    for (token, found) in counted(&target_signature.placeholders) {
        let expected = source_signature.count_placeholder(&token);
        if found > expected {
            issues.push(FidelityIssue::ExtraPlaceholder {
                token,
                count: found - expected,
            });
        }
    }
    for (tag, expected) in counted(&source_signature.tags) {
        let found = target_signature.count_tag(&tag);
        if found < expected {
            issues.push(FidelityIssue::MissingTag {
                tag,
                count: expected - found,
            });
        }
    }
    for (tag, found) in counted(&target_signature.tags) {
        let expected = source_signature.count_tag(&tag);
        if found > expected {
            issues.push(FidelityIssue::ExtraTag {
                tag,
                count: found - expected,
            });
        }
    }

    // 多重集一致但顺序 / 嵌套不同：`<b><i>x</i></b>` 被写成 `<i><b>x</b></i>`
    if issues.is_empty() && source_signature.tags != target_signature.tags {
        issues.push(FidelityIssue::TagStructureChanged);
    }
    issues
}

/// 译文结构是否保真。
pub fn is_faithful(source: &str, target: &str) -> bool {
    check_fidelity(source, target).is_empty()
}

/// 把问题列表压成一句简短中文（Error 事件的 message 用）。
pub fn summarize(issues: &[FidelityIssue]) -> String {
    if issues.is_empty() {
        return "结构校验未通过".to_string();
    }
    let mut parts: Vec<String> = issues
        .iter()
        .take(MAX_LISTED_ISSUES)
        .map(|issue| issue.to_string())
        .collect();
    if issues.len() > MAX_LISTED_ISSUES {
        parts.push(format!("等 {} 项", issues.len()));
    }
    parts.join("；")
}

/// 拼一条纠错提示，附在重试请求里。
pub fn correction_hint(issues: &[FidelityIssue]) -> String {
    let mut parts: Vec<String> = issues
        .iter()
        .take(MAX_LISTED_ISSUES)
        .map(|issue| issue.hint())
        .collect();
    if issues.len() > MAX_LISTED_ISSUES {
        parts.push(format!("等 {} 项结构问题", issues.len()));
    }
    format!(
        "上一轮译文{}，请重新只输出译文，并原样保留原文中的全部占位符与富文本标签。",
        parts.join("；")
    )
}

// ─────────────────────────────────────────────────────────────
// 结构签名
// ─────────────────────────────────────────────────────────────

/// 一段文本的结构签名。
#[derive(Debug, Default, PartialEq, Eq)]
struct Signature {
    /// 占位符（含重复），按出现顺序
    placeholders: Vec<String>,
    /// 标签 token（含重复），按出现顺序；`<LSTag>` / `</LSTag>` / `<br/>`
    tags: Vec<String>,
}

impl Signature {
    fn of(text: &str) -> Self {
        let text = restore_entities(text);
        let tags = normalize_self_closing(scan_tags(&text));
        let placeholders = scan_placeholders(&text);
        Self { placeholders, tags }
    }

    fn count_placeholder(&self, token: &str) -> usize {
        self.placeholders.iter().filter(|t| *t == token).count()
    }

    fn count_tag(&self, tag: &str) -> usize {
        self.tags.iter().filter(|t| *t == tag).count()
    }
}

/// 把**非空元素**的 `<x/>` 规范化成 `<x>` + `</x>`：XML 里这两种写法等价。
///
/// 只做这一步：`<br/>` 这类空元素（[`VOID_TAGS`]）保持原样，因为写回层也不要求
/// 它闭合。没有这一步，`<i/>` 与 `<i></i>` 这对等价写法会互相判失败，
/// 把本来正确的译文判成 `error`（F-03）。
fn normalize_self_closing(tags: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags {
        match tag
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix("/>"))
        {
            Some(name) if !VOID_TAGS.contains(&name) => {
                out.push(format!("<{name}>"));
                out.push(format!("</{name}>"));
            }
            _ => out.push(tag),
        }
    }
    out
}

/// 把 `&lt;` / `&gt;` 还原成字面尖括号（只做这一件事，其余实体不动）。
///
/// 两边都还原，所以「原文写 `<`、模型写 `&lt;`」不会被判成结构不一致。
fn restore_entities(text: &str) -> String {
    if !text.contains("&lt;") && !text.contains("&gt;") {
        return text.to_string();
    }
    text.replace("&lt;", "<").replace("&gt;", ">")
}

/// 扫描形态良好的富文本标签，返回渲染后的 token 序列。
fn scan_tags(text: &str) -> Vec<String> {
    let mut tags = Vec::new();
    let mut index = 0usize;
    while index < text.len() {
        let Some(offset) = text[index..].find('<') else {
            break;
        };
        let start = index + offset;
        match parse_tag(text, start) {
            Some((token, end)) => {
                tags.push(token);
                index = end;
            }
            // 不是标签：只跳过 `<` 本身，后面的 `>` 还要参与扫描
            None => index = start + 1,
        }
    }
    tags
}

/// 尝试把 `text[start..]` 开头的 `<...>` 解析成标签；失败返回 `None`。
fn parse_tag(text: &str, start: usize) -> Option<(String, usize)> {
    let after = text[start..].strip_prefix('<')?;
    let gt = after.find('>')?;
    let tag = &text[start..start + 1 + gt + 1];
    let inner = &after[..gt];
    // `<` 之后又出现 `<`：`a <b <c>` 不是标签
    if inner.contains('<') {
        return None;
    }
    // 严格形态检查在前，白名单在后：`< 5`、`<5>`、`<name>` 都要被挡掉
    let token = render_tag(inner)?;
    if !is_allowed_inline_tag(tag) {
        return None;
    }
    Some((token, start + 1 + gt + 1))
}

/// 标签名必须是 ASCII 字母开头、只含字母数字；返回名字与其后的剩余部分。
fn tag_name(body: &str) -> Option<(&str, &str)> {
    let mut chars = body.char_indices();
    let (_, first) = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    let mut end = first.len_utf8();
    for (idx, ch) in chars {
        if !ch.is_ascii_alphanumeric() {
            break;
        }
        end = idx + ch.len_utf8();
    }
    Some((&body[..end], &body[end..]))
}

/// 把标签正文渲染成稳定的 token：`<LSTag>` / `</LSTag>` / `<br/>`。
fn render_tag(inner: &str) -> Option<String> {
    if let Some(body) = inner.strip_prefix('/') {
        // 结束标签：名字之后只允许空白
        let (name, rest) = tag_name(body)?;
        if !rest.trim().is_empty() {
            return None;
        }
        return Some(format!("</{name}>"));
    }
    // 开始标签：`<` 之后必须紧跟字母，所以 `< 5` / `a < b > c` 到不了这里
    let (name, rest) = tag_name(inner)?;
    let tail = rest.trim_end();
    if tail.ends_with('/') {
        return Some(format!("<{name}/>"));
    }
    if tail.is_empty() || tail.starts_with(char::is_whitespace) {
        if VOID_TAGS.contains(&name) {
            return Some(format!("<{name}/>"));
        }
        return Some(format!("<{name}>"));
    }
    None
}

/// 扫描占位符，返回 `{1}` 这类 token 序列（含重复）。
fn scan_placeholders(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut index = 0usize;
    while index < text.len() {
        let Some(offset) = text[index..].find('{') else {
            break;
        };
        let start = index + offset;
        let Some(close) = text[start..].find('}') else {
            break;
        };
        let end = start + close + 1;
        let inner = &text[start + 1..end - 1];
        if is_placeholder_token(inner) {
            tokens.push(text[start..end].to_string());
            index = end;
        } else {
            // `{{1}}`：外层 `{` 与最近的 `}` 之间是 `{1`，非法；
            // 从下一个字符继续扫，于是内层 `{1}` 仍会被识别。
            index = start + 1;
        }
    }
    tokens
}

/// 是否是认得的占位符形式：非空、只含 ASCII 字母 / 数字 / 下划线。
///
/// 刻意收窄：`{}`、`{ }`、`{a b}`、`{"k": 1}`、`{#FFAA00}` 都不算占位符，
/// 免得把普通文本里的花括号当结构来要求译文匹配。
fn is_placeholder_token(inner: &str) -> bool {
    !inner.is_empty() && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 统计 token 多重集，按首次出现顺序返回 `(token, 次数)`。
fn counted(tokens: &[String]) -> Vec<(String, usize)> {
    let mut counted: Vec<(String, usize)> = Vec::new();
    for token in tokens {
        match counted.iter_mut().find(|(known, _)| known == token) {
            Some((_, count)) => *count += 1,
            None => counted.push((token.clone(), 1)),
        }
    }
    counted
}

#[cfg(test)]
mod tests {
    use super::*;

    fn missing_placeholders(source: &str, target: &str) -> Vec<String> {
        check_fidelity(source, target)
            .into_iter()
            .filter_map(|issue| match issue {
                FidelityIssue::MissingPlaceholder { token, .. } => Some(token),
                _ => None,
            })
            .collect()
    }

    fn extra_placeholders(source: &str, target: &str) -> Vec<String> {
        check_fidelity(source, target)
            .into_iter()
            .filter_map(|issue| match issue {
                FidelityIssue::ExtraPlaceholder { token, .. } => Some(token),
                _ => None,
            })
            .collect()
    }

    // ── 占位符 ──

    #[test]
    fn missing_placeholder_is_reported() {
        let issues = check_fidelity("Deals {1} damage", "造成伤害");
        assert_eq!(
            issues,
            vec![FidelityIssue::MissingPlaceholder {
                token: "{1}".into(),
                count: 1
            }]
        );
        assert_eq!(issues[0].to_string(), "占位符 {1} 缺失");
        assert_eq!(issues[0].hint(), "缺少占位符 {1}");
    }

    #[test]
    fn extra_placeholder_is_reported() {
        assert_eq!(
            check_fidelity("Deals damage", "造成 {1} 点伤害")[0],
            FidelityIssue::ExtraPlaceholder {
                token: "{1}".into(),
                count: 1
            }
        );
        assert_eq!(extra_placeholders("原文", "译文 {2}"), vec!["{2}"]);
    }

    #[test]
    fn duplicated_placeholder_is_reported() {
        let issues = check_fidelity("Deals {1} damage", "造成 {1}{1} 点伤害");
        assert_eq!(
            issues,
            vec![FidelityIssue::ExtraPlaceholder {
                token: "{1}".into(),
                count: 1
            }]
        );
        // 反向：译文少了一处
        let issues = check_fidelity("Deals {1}{1} damage", "造成 {1} 点伤害");
        assert_eq!(
            issues,
            vec![FidelityIssue::MissingPlaceholder {
                token: "{1}".into(),
                count: 1
            }]
        );
        assert_eq!(issues[0].to_string(), "占位符 {1} 缺失");
    }

    #[test]
    fn placeholder_counts_must_match_exactly() {
        assert!(is_faithful("{1}{1}{1}", "甲 {1} 乙 {1} 丙 {1}"));
        assert_eq!(
            check_fidelity("{1}{1}{1}", "{1}").len(),
            1,
            "缺两处只报一条（带次数）"
        );
    }

    #[test]
    fn placeholder_order_is_not_part_of_the_signature() {
        assert!(is_faithful("{1} and {2}", "{2} 与 {1}"));
        assert!(is_faithful("{name} 击中了 {1}", "{1} 被 {name} 击中"));
    }

    #[test]
    fn numeric_and_named_placeholders_are_distinguished() {
        assert_eq!(
            missing_placeholders("{0} {10} {name}", "{0} {name}"),
            vec!["{10}"]
        );
        assert_eq!(missing_placeholders("{name}", "{1}"), vec!["{name}"]);
        assert_eq!(extra_placeholders("{1}", "{10}"), vec!["{10}"]);
        assert!(is_faithful("{user_name} 的 {2}", "{2} 属于 {user_name}"));
    }

    #[test]
    fn doubled_braces_are_scanned_by_the_inner_token() {
        // 边界：`{{1}}` 外层与最近的 `}` 之间是 `{1`（非法），内层 `{1}` 合法。
        // 因此 `{{1}}` 与 `{1}` 视为同一签名 —— 宁可漏报，不可误报。
        assert!(is_faithful("{{1}}", "{{1}}"));
        assert!(is_faithful("{{1}}", "{1}"));
        assert!(is_faithful("{1}", "{{1}}"));
        // 但真的少了占位符仍然要报
        assert_eq!(missing_placeholders("{{1}}", "某物"), vec!["{1}"]);
    }

    #[test]
    fn malformed_braces_are_not_placeholders() {
        for text in [
            "{}",
            "{ }",
            "{a b}",
            "{\"k\": 1}",
            "{a,b}",
            "{#FFAA00}",
            "{-1}",
        ] {
            assert!(
                check_fidelity(text, text).is_empty(),
                "{text} 不该被当成占位符"
            );
            assert!(
                check_fidelity(text, "完全没有括号").is_empty(),
                "{text} 不该被当成占位符"
            );
        }
        assert!(is_faithful("配置 {}", "配置 {任意}"));
    }

    #[test]
    fn placeholders_inside_tag_markup_are_still_compared() {
        // 属性值里的占位符也参与比较：系统 prompt 要求属性值不得翻译，
        // 更不允许把里面的 `{1}` 弄丢（丢了游戏会显示不出参数）。
        assert!(is_faithful(
            r#"<LSTag Tooltip="Deals {1} damage">Fireball</LSTag>"#,
            r#"<LSTag Tooltip="造成 {1} 点伤害">火球术</LSTag>"#
        ));
        assert_eq!(
            missing_placeholders(
                r#"<LSTag Tooltip="Deals {1} damage">Fireball</LSTag>"#,
                r#"<LSTag Tooltip="造成伤害">火球术</LSTag>"#
            ),
            vec!["{1}"]
        );
    }

    // ── 标签 ──

    #[test]
    fn matching_tags_are_faithful() {
        assert!(is_faithful(
            r#"Cast <LSTag Tag="Fire">Fireball</LSTag> now"#,
            r#"现在施放 <LSTag Tag="Fire">火球术</LSTag>"#
        ));
        assert!(!is_faithful(
            "<b>粗</b> 与 <i>斜</i>",
            "<i>斜</i> 与 <b>粗</b>"
        ));
    }

    #[test]
    fn attribute_changes_are_ignored() {
        // 属性值不参与比较：模型改了属性值不该判失败（写回时属性原样保留）
        assert!(is_faithful(
            r#"<LSTag Type="Spell" Tooltip="Fireball">Fireball</LSTag>"#,
            r#"<LSTag Type="法术" Tooltip="火球术">火球术</LSTag>"#
        ));
        assert!(is_faithful(
            "<font color=\"red\">红</font>",
            "<font>红字</font>"
        ));
    }

    #[test]
    fn extra_closing_tag_is_reported() {
        let issues = check_fidelity("<LSTag>x</LSTag>", "<LSTag>译文</LSTag></LSTag>");
        assert_eq!(
            issues,
            vec![FidelityIssue::ExtraTag {
                tag: "</LSTag>".into(),
                count: 1
            }]
        );
        assert_eq!(issues[0].to_string(), "标签 </LSTag> 多出");
        assert_eq!(issues[0].hint(), "多出标签 </LSTag>");
    }

    #[test]
    fn missing_closing_tag_is_reported() {
        let issues = check_fidelity("<LSTag>x</LSTag>", "<LSTag>译文");
        assert_eq!(
            issues,
            vec![FidelityIssue::MissingTag {
                tag: "</LSTag>".into(),
                count: 1
            }]
        );
        assert_eq!(issues[0].to_string(), "标签 </LSTag> 缺失");
    }

    #[test]
    fn missing_opening_tag_is_reported_too() {
        assert_eq!(
            check_fidelity("<b>粗</b>", "粗</b>"),
            vec![FidelityIssue::MissingTag {
                tag: "<b>".into(),
                count: 1
            }]
        );
    }

    #[test]
    fn void_br_needs_no_closing_tag() {
        assert!(is_faithful("a<br/>b", "甲<br/>乙"));
        assert!(is_faithful("a<br>b", "甲<br>乙"));
        // `<br>` / `<br/>` 两种写法等价（写回时都会写成标签，不要求闭合）
        assert!(is_faithful("a<br>b", "甲<br/>乙"));
        assert!(is_faithful("a<br />b", "甲<br/>乙"));
        // 但整个丢了仍要报
        assert_eq!(
            check_fidelity("a<br/>b", "甲乙"),
            vec![FidelityIssue::MissingTag {
                tag: "<br/>".into(),
                count: 1
            }]
        );
    }

    /// F-03：非空元素的 `<x/>` 与 `<x></x>` 在 XML 里等价，不该互相判失败。
    #[test]
    fn non_void_self_closing_equals_an_empty_pair() {
        assert!(is_faithful("<i/>text", "<i></i>文本"));
        assert!(is_faithful("<i></i>文本", "<i/>text"));
        assert!(is_faithful(
            r#"<LSTag Type="Spell"/>"#,
            r#"<LSTag Type="Spell"></LSTag>"#
        ));
        // 代价（刻意的盲点）：一旦承认 `<x/>` 与 `<x></x>` 等价，就再也分不出
        // 「空标签」和「包住文本的标签对」——正文位置本来就不参与结构比对。
        assert!(is_faithful(
            r#"<LSTag Type="Spell"/>"#,
            r#"<LSTag>火球</LSTag>"#
        ));

        // 空元素不受影响：`<br/>` 仍然只算一个 token
        assert!(is_faithful("a<br/>b", "甲<br>乙"));

        // 真的多一个 / 少一个闭合标签仍然要报
        assert_eq!(
            check_fidelity("<i>x</i>", "<i>x</i></i>"),
            vec![FidelityIssue::ExtraTag {
                tag: "</i>".into(),
                count: 1
            }]
        );
        assert_eq!(
            check_fidelity("<i></i>x", "<i>x"),
            vec![FidelityIssue::MissingTag {
                tag: "</i>".into(),
                count: 1
            }]
        );
    }

    #[test]
    fn tag_order_and_nesting_must_match() {
        assert_eq!(
            check_fidelity("<b><i>x</i></b>", "<i><b>x</b></i>"),
            vec![FidelityIssue::TagStructureChanged]
        );
        assert_eq!(
            check_fidelity("<b>x</b><i>y</i>", "<i>y</i><b>x</b>"),
            vec![FidelityIssue::TagStructureChanged]
        );
        assert_eq!(check_fidelity("<b><i>x</i></b>", "<b><i>x</i></b>"), vec![]);
        assert_eq!(
            FidelityIssue::TagStructureChanged.to_string(),
            "标签顺序或嵌套与原文不一致"
        );
    }

    // ── 误报防护：文本里的尖括号 ──

    #[test]
    fn comparison_text_is_not_a_tag() {
        for text in [
            "HP < 5",
            "a < b",
            "a < b > c",
            "<5>",
            "i < 10 and j > 2",
            "3 < 4",
            "x <y", // 没有闭合的 `>`
        ] {
            assert!(
                check_fidelity(text, text).is_empty(),
                "{text} 本身不该被当成标签"
            );
            assert!(
                check_fidelity(text, "译文").is_empty(),
                "{text} 丢了也不该报标签问题"
            );
        }
        assert!(is_faithful("HP < 5%", "生命值 < 5%"));
        assert!(is_faithful("a < b", "甲 < 乙"));
    }

    #[test]
    fn unknown_tag_names_are_plain_text() {
        // 写回白名单之外的 `<...>` 会被转义成普通文本，没有配对义务
        for text in ["<name>", "<color>", "<T>", "<div>"] {
            assert!(is_faithful(text, "普通文本"), "{text} 不该被当成标签");
        }
        assert!(is_faithful("&lt;name&gt;", "<名称>"));
    }

    #[test]
    fn restored_entities_are_not_reported() {
        // 解析层把 `&lt;i&gt;` 还原成了字面 `<i>`：模型再转义回去不算结构变化
        assert!(is_faithful("Use <i> for italic", "使用 &lt;i&gt; 表示斜体"));
        assert!(is_faithful("HP < 50%", "生命值 &lt; 50%"));
        assert!(is_faithful("HP &lt; 50%", "生命值 &lt; 50%"));
        assert!(is_faithful("HP &lt; 50%", "生命值 < 50%"));
        // 真正的标签也不会因为转义写法被判失败
        assert!(is_faithful(
            r#"<LSTag Type="Spell">Fireball</LSTag>"#,
            r#"&lt;LSTag Type="Spell"&gt;火球术&lt;/LSTag&gt;"#
        ));
    }

    #[test]
    fn unbalanced_angle_bracket_is_plain_text() {
        assert!(is_faithful("a <LSTag without end", "甲 <LSTag 没有结束"));
        assert!(is_faithful("a <b <c> d", "甲 <b <c> 乙"));
    }

    // ── 汇总 ──

    #[test]
    fn summarize_and_hint_are_short_chinese() {
        let issues = check_fidelity("Deals {1} <LSTag>x</LSTag>", "造成伤害");
        assert_eq!(
            summarize(&issues),
            "占位符 {1} 缺失；标签 <LSTag> 缺失；标签 </LSTag> 缺失"
        );
        let hint = correction_hint(&issues);
        assert!(
            hint.starts_with("上一轮译文缺少占位符 {1}；缺少标签 <LSTag>；缺少标签 </LSTag>，"),
            "实际: {hint}"
        );
        assert!(hint.ends_with("请重新只输出译文，并原样保留原文中的全部占位符与富文本标签。"));
        assert_eq!(summarize(&[]), "结构校验未通过");
    }

    #[test]
    fn long_issue_lists_are_truncated() {
        let source = "{1}{2}{3}{4}{5}";
        let issues = check_fidelity(source, "什么都没有");
        assert_eq!(issues.len(), 5);
        let summary = summarize(&issues);
        assert!(summary.ends_with("等 5 项"), "实际: {summary}");
        assert_eq!(summary.matches('；').count(), 3, "最多列 3 条");
        assert!(correction_hint(&issues).contains("等 5 项结构问题"));
    }

    #[test]
    fn empty_and_identical_texts_are_faithful() {
        assert!(is_faithful("", ""));
        assert!(is_faithful("", "译文"));
        assert!(is_faithful("Fireball", "火球术"));
    }
}
