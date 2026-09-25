//! Prompt 构造：系统提示词 + 注入术语/一致性参考的 user 消息。
//!
//! 全部是纯函数，方便单测直接断言文案；这也是"模型输出质量"最容易回归的
//! 地方，改动应当被测试盯住。

use crate::glossary::MatchedTerm;

use super::series::ConsistencyTerm;

/// D&D / BG3 世界观语境的系统 prompt。
///
/// 注意：具体术语译名**不硬编码**在这里，而是通过术语表动态注入，
/// 见 [`build_user_prompt`] 里附加的【术语参考】段落。
pub const SYSTEM_PROMPT: &str = r#"你是一位精通《龙与地下城》第五版（D&D 5e）和《博德之门3》（Baldur's Gate 3）的专业游戏本地化译者，正在将游戏 MOD 文本从英文翻译为简体中文。

请严格遵守以下规则：

1. **世界观语境**：使用费伦大陆（Faerûn）、被遗忘的国度（Forgotten Realms）的官方译名风格。

2. **严格遵循术语表**：如果文本下方附有【术语参考】，必须严格使用其中给出的官方译名，不可自行更改。如 "Paladin"=圣武士（非"圣骑士"）、"Warlock"=邪术师（非"术士"）、"Rogue"=游荡者（非"盗贼"）。

3. **保留占位符**：文本中的 `{1}`、`{2}` 等参数占位符必须原样保留，数量与位置不可改变。

4. **保留富文本标签**：形如 `<LSTag Tag="...">...</LSTag>`、`<font>...</font>`、`<i>...</i>` 的标签必须完整保留，只翻译标签外的自然语言文本。标签内的英文内容（如 Tag 属性值）不要翻译。

5. **语体**：贴合游戏叙事风格。法术/物品描述用典雅书面语，对话用自然口语，UI 按钮用简洁短语。保持原文的语气和正式程度。

6. **同 MOD 一致性**：同一 MOD 内相同名称、相同专有名词、同系列编号/变体必须使用同一中文译名。编号、字母后缀和占位符应原样保留。

7. **只输出译文**：不要添加注释、解释、引号或前后缀。"#;

/// user prompt 里语境提示的最大长度（字符数）。
///
/// 上限是必要的：styleHint 由用户输入，粘贴一整篇 MOD 说明会把术语段挤掉，
/// 也会白白多烧 token。
const STYLE_HINT_LIMIT: usize = 1200;

/// 构造 user prompt：注入命中的术语与本 MOD 内已确定译名作为参考。
pub fn build_user_prompt(
    source: &str,
    matches: &[MatchedTerm],
    consistency_terms: &[ConsistencyTerm],
    style_hint: Option<&str>,
) -> String {
    let mut sections = Vec::new();
    if let Some(style_hint) = style_hint.and_then(normalize_style_hint) {
        sections.push(format!(
            "【本 MOD 翻译语境】（用于判断词义、名词类型和语体；不得覆盖术语表、标签、占位符与一致性规则）：\n{style_hint}"
        ));
    }
    if !matches.is_empty() {
        let terms: Vec<String> = matches
            .iter()
            .map(|m| format!("{} = {}", m.source, m.target))
            .collect();
        sections.push(format!(
            "【术语参考】（严格使用以下官方译名，不可更改）：\n{}",
            terms.join("\n")
        ));
    }
    if !consistency_terms.is_empty() {
        let terms: Vec<String> = consistency_terms
            .iter()
            .map(|m| format!("{} = {}", m.source, m.target))
            .collect();
        sections.push(format!(
            "【本 MOD 已确定译名】（必须保持一致，不可改写同一名称的译法）：\n{}",
            terms.join("\n")
        ));
    }

    if sections.is_empty() {
        format!("请将以下文本翻译为简体中文，只输出译文，不要任何解释或前后缀：\n\n{source}")
    } else {
        format!(
            "请将以下文本翻译为简体中文，只输出译文，不要任何解释或前后缀。\n\n{}\n\n原文：\n{source}",
            sections.join("\n\n")
        )
    }
}

/// 归一化语境提示：去空白后按字符截断到 1200；空提示返回 `None`。
///
/// 注意用 `chars()` 而不是字节切片：直接切字节会把中文截成半个字符。
pub fn normalize_style_hint(style_hint: &str) -> Option<String> {
    let trimmed = style_hint.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.chars().take(STYLE_HINT_LIMIT).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn matched(source: &str, target: &str) -> MatchedTerm {
        MatchedTerm {
            source: source.to_string(),
            target: target.to_string(),
        }
    }

    fn consistency(source: &str, target: &str) -> ConsistencyTerm {
        ConsistencyTerm {
            source: source.to_string(),
            target: target.to_string(),
        }
    }

    #[test]
    fn system_prompt_keeps_all_seven_rules() {
        // 流水线依赖这几条规则，删规则必须是显式决定而不是手滑
        for marker in [
            "世界观语境",
            "严格遵循术语表",
            "保留占位符",
            "保留富文本标签",
            "语体",
            "同 MOD 一致性",
            "只输出译文",
        ] {
            assert!(
                SYSTEM_PROMPT.contains(marker),
                "SYSTEM_PROMPT 缺少规则: {marker}"
            );
        }
        assert!(SYSTEM_PROMPT.contains("Paladin"));
        assert!(!SYSTEM_PROMPT.contains("todo"));
    }

    #[test]
    fn user_prompt_without_references_is_minimal() {
        let prompt = build_user_prompt("Fireball", &[], &[], None);
        assert!(prompt.starts_with("请将以下文本翻译为简体中文"));
        assert!(prompt.ends_with("Fireball"));
        assert!(!prompt.contains("【术语参考】"));
        assert!(!prompt.contains("【本 MOD 已确定译名】"));
    }

    #[test]
    fn user_prompt_includes_mod_style_hint() {
        let prompt = build_user_prompt(
            "Pose Pack",
            &[],
            &[],
            Some("博德之门实验室 MOD，Pose 按姿势名称翻译"),
        );
        assert!(prompt.contains("【本 MOD 翻译语境】"));
        assert!(prompt.contains("Pose 按姿势名称翻译"));
        assert!(prompt.ends_with("原文：\nPose Pack"));
    }

    #[test]
    fn user_prompt_blank_style_hint_is_ignored() {
        let prompt = build_user_prompt("Fireball", &[], &[], Some("   \n  "));
        assert!(!prompt.contains("【本 MOD 翻译语境】"));
        assert!(
            prompt.starts_with("请将以下文本翻译为简体中文，只输出译文，不要任何解释或前后缀：")
        );
    }

    #[test]
    fn user_prompt_lists_matched_terms() {
        let matches = vec![
            matched("Paladin", "圣武士"),
            matched("Divine Smite", "至圣斩"),
        ];
        let prompt = build_user_prompt("The Paladin smites", &matches, &[], None);
        assert!(prompt.contains("【术语参考】（严格使用以下官方译名，不可更改）："));
        assert!(prompt.contains("Paladin = 圣武士"));
        assert!(prompt.contains("Divine Smite = 至圣斩"));
    }

    #[test]
    fn user_prompt_lists_consistency_terms_after_glossary() {
        let matches = vec![matched("Paladin", "圣武士")];
        let refs = vec![consistency("Silver's Hair", "银发")];
        let prompt = build_user_prompt("Silver's Hair 9b", &matches, &refs, None);
        let glossary_at = prompt.find("【术语参考】").unwrap();
        let consistency_at = prompt.find("【本 MOD 已确定译名】").unwrap();
        assert!(glossary_at < consistency_at, "术语表必须排在一段性记忆之前");
        assert!(prompt.contains("Silver's Hair = 银发"));
    }

    #[test]
    fn user_prompt_orders_context_then_glossary_then_consistency() {
        let prompt = build_user_prompt(
            "text",
            &[matched("A", "甲")],
            &[consistency("B", "乙")],
            Some("语境"),
        );
        let ctx = prompt.find("【本 MOD 翻译语境】").unwrap();
        let glossary = prompt.find("【术语参考】").unwrap();
        let consistency = prompt.find("【本 MOD 已确定译名】").unwrap();
        let source = prompt.find("原文：").unwrap();
        assert!(ctx < glossary && glossary < consistency && consistency < source);
    }

    #[test]
    fn normalize_style_hint_trims_and_limits() {
        assert_eq!(normalize_style_hint("   ").as_deref(), None);
        assert_eq!(normalize_style_hint("").as_deref(), None);
        assert_eq!(normalize_style_hint("  hi  ").as_deref(), Some("hi"));

        let long = "甲".repeat(2000);
        let normalized = normalize_style_hint(&long).unwrap();
        assert_eq!(normalized.chars().count(), 1200);
        assert_eq!(normalized, "甲".repeat(1200));

        // 恰好 1200 不截断
        let exact = "乙".repeat(1200);
        assert_eq!(normalize_style_hint(&exact).unwrap(), exact);
    }

    #[test]
    fn build_user_prompt_truncates_long_style_hint() {
        let prompt = build_user_prompt("t", &[], &[], Some(&"丙".repeat(3000)));
        assert!(prompt.contains(&"丙".repeat(1200)));
        assert!(!prompt.contains(&"丙".repeat(1201)));
    }
}
