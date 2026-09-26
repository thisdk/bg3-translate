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

3. **保留占位符**：原文里的参数占位符必须**逐字照抄**，一个字符都不能改。本游戏语料里的占位符是 `[数字]` 形态：

   - **方括号和花括号是两套不同的标记，绝对不能互换**：原文写 `[1]` 就必须写 `[1]`（不许改成 `{1}`），原文写 `{1}` 就必须写 `{1}`（不许改成 `[1]`）。换了类型这个值就填不进去，游戏里只会显示成字面量。例：`strike [1] different targets` → 「打击[1]个不同的目标」。
   - 括号一律用半角：`[1]`，不可写成 `【1】` 或 `［1］`。
   - 编号不可改动（`[1]` 不能变成 `[2]`），数量不可增减，顺序可按中文语序调整。

   占位符周围**按中文习惯书写，不要照抄英文的词间空格**：英文靠空格分词、中文不靠，`deal [2] damage` 应译作「造成[2]点伤害」，不是「造成 [2] 点伤害」。

   **唯一例外**：两个占位符直接相邻时必须保留它们之间的分隔——原文 `[1] [2]` 要保留中间的空格，写成 `[1][2]` 会让游戏把两个值渲染成一个数。

4. **保留富文本标签**：原文里的标签（本语料里只有 `<LSTag ...>...</LSTag>` 与 `<br>` 两种）必须原样保留、数量一个不差。标签必须用**字面尖括号**输出，不要写成 `&lt;LSTag ...&gt;` 这类转义形态（游戏不认，会显示成字面标签）：

   - `<LSTag ...>正文</LSTag>`：**只翻译标签对之间的正文**（`Tooltip="CAUSE_FEARED"` 包着的 "Frighten" 要译成「恐惧」）；尖括号里的标签名与属性名**逐字照抄**，不许翻译也不许改大小写（`Type` 不能写成 `类型`）。属性值是游戏内部查表的 key（如 `Type="Spell"`、`Tooltip="HitPoints"`），**不是显示文本，一律逐字照抄**——翻了它游戏就查不到表，tooltip 直接失效。
   - `<br>`：原样保留（语料里从不写 `<br/>`）。
   - 其它白名单标签（`<font>`、`<i>`、`<b>` 之类）同理：只翻标签对之间的正文，标签本身一字不改。

   **标签的位置可以随中文语序调整**：中英语序不同，标签应当跟着它包住的那段正文一起移动，两个标签对整体对调是**正确**的；要求标签停在原位反而是错的。必须保持的是：标签名、属性名、属性值与标签数量逐字不变，开闭标签成对嵌套、不许交叉。

5. **语体**：贴合游戏叙事风格。法术/物品描述用典雅书面语，对话用自然口语，UI 按钮用简洁短语。保持原文的语气和正式程度。

6. **同 MOD 一致性**：同一 MOD 内相同名称、相同专有名词、同系列编号/变体必须使用同一中文译名。编号与字母后缀必须逐字保留（占位符的写法见规则 3）。

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

        // 占位符规则的三条要点都不能被手滑删掉，它们分别对着 fidelity 的三条
        // 校验：`{}` / `[]` 两种括号都要进签名、粘连要报、而「空格」是刻意的盲点。
        // 提示词与校验器必须对齐 —— 提示词放宽的地方校验器不能还在收紧。
        for marker in ["`[1]`", "不要照抄英文的词间空格", "[1] [2]", "[1][2]"] {
            assert!(
                SYSTEM_PROMPT.contains(marker),
                "SYSTEM_PROMPT 的占位符规则缺少要点: {marker}"
            );
        }
        // 真实语料里的占位符只有 `[数字]` 一种形态（303 条 / 417 处，`{...}` 0 次），
        // 点名它能让模型把注意力放在真实见过的东西上。
        assert!(SYSTEM_PROMPT.contains("`[数字]`"));
        // 旧的「位置不可改变」既与代码矛盾（`fidelity` 有专门用例断言顺序可变），
        // 也与官方语料矛盾（官方 `, [1] from [2]` → `，从[2]处取走了[1]` 就是换位），
        // 留着只会把模型推向逐字照抄英文语序。
        assert!(!SYSTEM_PROMPT.contains("位置不可改变"));

        // 两套括号不许互换，是真实报障（`[1]` 被改写成 `{1}`）的根因；
        // 标签位置可随语序调整，对应 fidelity 里刻意不查标签顺序的那条决定。
        // 提示词与校验器两边必须一致，否则模型按提示词做对了、校验器反而报错。
        assert!(
            SYSTEM_PROMPT.contains("方括号和花括号是两套不同的标记"),
            "必须明确禁止 `[1]` ↔ `{{1}}` 互换"
        );
        assert!(
            SYSTEM_PROMPT.contains("标签的位置可以随中文语序调整"),
            "必须明确允许标签随语序换位（校验器已经不查顺序了）"
        );
    }

    /// 规则 4 必须点名**真实语料里存在的**标签形态，并且把校验器的判据说清楚。
    ///
    /// 语料依据（`samples/english.xml`，1971 条）：标签只有 `<LSTag ...>...</LSTag>`
    /// （1103 对）与 `<br>`（410 个，全部不带斜杠）；属性只有 `Tooltip`（1103 次）
    /// 与 `Type`（625 次，取值闭集 Spell / Status / Passive）；`Tooltip` 是查表 key
    /// （`Tooltip="CAUSE_FEARED"` 包着的正文是 "Frighten"）。旧的规则 4 点名的
    /// `<LSTag Tag="...">`、`<font>`、`<i>` 在语料里一个都不存在，而真实存在的
    /// `Type` / `<br>` 反而没被点名 —— 这份提示词是提高一次通过率的主要手段，
    /// 点错形态等于白写。
    #[test]
    fn system_prompt_names_the_real_tag_shapes_first() {
        // 真实形态要排在前面的「其它标签同理」之前
        let real = SYSTEM_PROMPT
            .find("`<LSTag ...>...</LSTag>`")
            .expect("规则 4 必须点名 `<LSTag ...>...</LSTag>`（语料里 1103 对）");
        let other = SYSTEM_PROMPT
            .find("<font>")
            .expect("`<font>` 可作为「其它标签同理」保留");
        assert!(real < other, "真实形态必须写在「其它标签」之前");
        assert!(
            SYSTEM_PROMPT.contains("`<br>`"),
            "必须点名 `<br>`（语料里 410 个，全部不带斜杠，从不写 `<br/>`）"
        );
        // 旧提示词点名了语料里不存在的属性 `Tag="..."`：删掉之后不许再回来
        assert!(
            !SYSTEM_PROMPT.contains("Tag=\""),
            "语料里的属性只有 `Type` 与 `Tooltip`，没有 `Tag`"
        );
    }

    /// 规则 3 / 4 必须与校验器对齐：凡是 `fidelity` 会拦的，提示词都要先讲清楚。
    ///
    /// 逐条对应：占位符多重集（缺失 / 多余）、相邻占位符粘连、标签名与**属性名**
    /// 多重集、交叉嵌套、以及正在加入校验的「key 型属性值必须逐字一致」。
    /// 提示词放宽了而校验器还在收紧，模型就会「按提示词做对了却被打回」。
    #[test]
    fn system_prompt_explains_everything_the_validator_blocks() {
        // 属性值是内部查表 key，逐字照抄 —— 对着 T2 的 AttributeValueChanged 闸门
        assert!(
            SYSTEM_PROMPT.contains("内部查表的 key") && SYSTEM_PROMPT.contains("一律逐字照抄"),
            "必须说明属性值是查表 key、必须逐字照抄"
        );
        assert!(
            SYSTEM_PROMPT.contains("`Type` 不能写成 `类型`"),
            "属性名不许翻译（校验器按大小写敏感比对属性名）"
        );
        assert!(
            SYSTEM_PROMPT.contains("只翻译标签对之间的正文"),
            "标签对**之间**的正文是要翻译的：旧措辞「只翻译标签外的自然语言文本」\
             会把 `<LSTag>Deadly Toxin</LSTag>` 的正文也排除在外"
        );
        assert!(
            SYSTEM_PROMPT.contains("数量一个不差"),
            "标签数量必须一致（MissingTag / ExtraTag）"
        );
        assert!(
            SYSTEM_PROMPT.contains("成对嵌套、不许交叉"),
            "必须禁止交叉嵌套（TagNestingBroken）"
        );
        // 提示词不能诱导模型去改 key：点名 key 的句子里必须带「不许翻」
        assert!(
            SYSTEM_PROMPT.contains("不是显示文本"),
            "要点明属性值不是显示文本，否则模型会把它当正文翻掉"
        );
        // 转义形态（`&lt;LSTag&gt;`）能过结构校验（`restore_entities` 会先还原再比对），
        // 写回时却会被再转义一次、游戏里显示成字面标签 —— 校验器不会拦，所以只能靠
        // 提示词明确要求字面尖括号。提示词绝不能暗示「两种写法都行」。
        assert!(
            SYSTEM_PROMPT.contains("字面尖括号") && SYSTEM_PROMPT.contains("&lt;LSTag ...&gt;"),
            "必须要求用字面尖括号输出标签，禁止 `&lt;LSTag ...&gt;` 转义形态"
        );
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
