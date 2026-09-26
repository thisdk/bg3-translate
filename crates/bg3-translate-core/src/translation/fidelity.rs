//! 译文结构保真校验：占位符与富文本标签的「结构签名」比对。
//!
//! 系统 prompt 要求模型原样保留 `{1}` 这类占位符与 `<LSTag ...>` 这类富文本
//! 标签，但在此之前**没有任何代码检查过**：模型丢了占位符照样算成功译文，
//! 直到写盘时 `content_list` 才把标签不配对的文本降级成纯文本，用户看不见。
//! 这个模块补上这条防线：只比较**结构**，不比较内容。
//!
//! ## 抓什么
//!
//! - 占位符：`{}` 与 `[]` 两种写法，多重集必须一致
//!   （缺失 / 多余 / 重复都报）；顺序不算问题（`{1} {2}` ↔ `{2} {1}` 保真）。
//!   标签属性值里的占位符同样参与比对：`{1}` 不能丢（key 型属性的值还要求逐字
//!   一致，见下面标签那条）。
//!   - 花括号：`{1}`、`{10}`、`{name}`、`{user_name}`；
//!   - 方括号：`[1]`、`[10]`（游戏替换数值）、`[IE_PanelSelect]`、`[DRUID]`
//!     （游戏替换按键名之类的内部 ID）。判定形态与
//!     `glossary::entry` 里过滤术语噪音用的 `PLACEHOLDER_PATTERN` /
//!     `UI_MARKER_PATTERN` 保持一致 —— 那边用「这是占位符」把句式模板踢出
//!     术语表，这边用同一判断保护翻译，两边对「什么算占位符」不能有分歧。
//!     语料依据：官方术语表 163 条含 `[数字]` 的条目，**官方简中 163/163
//!     全部原样保留**（含 `, [1] from [2]` → `，从[2]处取走了[1]` 这种换位，
//!     说明 `[N]` 与 `{N}` 一样是顺序无关、多重集必须相等）。
//! - 占位符**粘连**：原文里分开的两个占位符被写成 `[1][2]` 时报。这是**内容
//!   缺陷**而非排版问题 —— 游戏分别替换两个值之后中间没有任何分隔，会渲染成
//!   一个数 `12`。反过来给它们之间**加**分隔（`[1] [2]`、`[1]、[2]`）不报：
//!   多一个分隔无害，粘在一起才有害，所以这个判断是**非对称**的。
//! - 标签：`<LSTag ...>`、`</LSTag>`、`<br/>` 的**标签名多重集**必须一致
//!   （开 / 闭 / 空元素三种形态分开算，`<br>` 这类空元素不要求闭合）；
//!   开始标签的**属性名多重集也必须一致**（顺序不计、大小写敏感）；
//!   **已知 key 型属性（[`KEY_ATTRIBUTES`]：`Tooltip` / `Type`）的值在「标识符
//!   形态」时必须逐字一致**（见 [`is_identifier_shaped`]），其余属性值与标签
//!   之间的正文不参与比较。非空元素的 `<x/>` 与 `<x></x>` 视为
//!   同一签名（XML 语义等价），代价是「空标签」与「包住文本的标签对」也分不出来。
//!   **标签顺序完全不参与比对**，只有**交叉嵌套**才报 —— 理由见
//!   [`is_well_nested`] 与 [`FidelityIssue::TagNestingBroken`]。
//!
//! ## 修什么（写法规整，不是内容改写）
//!
//! [`repair_placeholders`] 在**校验之前**把两类可确定的写法差异修回原文形态：
//! 全角方括号 → 半角、括号类型被模型改写（`[1]` ↔ `{1}`）。两类都只在无歧义时
//! 动手，且必须由调用方在校验前调用 —— 只在签名层归一化的话，`【1】` / `{1}`
//! 会被判成保真然后原样写进 PAK，游戏替换不了它们，「报错可见」就变成了
//! 「静默损坏」。
//!
//! 为什么属性名与 key 型属性值要参与比较：写回链路（`formats::content_list` 的
//! `render` → `write_text_fragment`）拿到的是 `entry.effective_text()`，也就是
//! **模型输出的那段文本**，标签会被逐字节写进 PAK，原文的标签写法**不会**被恢复。
//! 于是 `<LSTag Type="Spell">` 一旦被模型写成 `<LSTag 类型="Spell">` 或 `<LSTag>`，
//! 产物仍然是合法 XML、配对检查也过得去，但游戏侧已经读不到这个属性了。
//!
//! 属性值**同样不能一概放宽** —— 这里订正一条曾经写反的结论（旧注释与 README 都
//! 写着「属性值是玩家可见文本（Tooltip 之类），允许翻译」，那是错的）：
//! `Tooltip` / `Type` 的值不是显示文本，而是**查表 key**。翻了它，游戏按 key 去查
//! 表就查不到，tooltip / 链接直接失效，而且产物是合法 XML、写盘一路静默通过。
//! 语料证据（`samples/english.xml`，1971 条官方/真实文本）：
//! - 1103 处 `<LSTag Tooltip="KEY">BODY</LSTag>`，`Tooltip` 的 **279 个取值全部
//!   不含空格**，全是 `HitPoints` 这类驼峰标识符或 `CAUSE_FEARED` 这类内部 ID；
//! - KEY 与它包着的正文明显是两码事：`VENOMOUS_BARBS_CONDITION` 包着
//!   "Deadly Toxin"、`CAUSE_FEARED` 包着 "Frighten"、`Projectile_MagicStoneThrow`
//!   包着 "Throw Magic Stone"、`ID_INSINUATION` 包着 "Incapacitated" —— 正文才是
//!   玩家看到的字，KEY 是查表用的；
//! - `Type` 只有 3 个取值（`Spell` 311 / `Status` 259 / `Passive` 55），是闭集，
//!   游戏据此决定链接样式。
//!
//! 收口范围刻意**又窄又稳**，两道闸门同时成立才要求逐字一致：
//! 属性名必须在 [`KEY_ATTRIBUTES`] 白名单里，且**原文的值是标识符形态**。于是
//! 万一将来某个 MOD 真把属性值当显示文本用（含空格 / 标点的自然语言），照旧放行
//! —— 语料里 `Tooltip` 279 个取值一个空格都没有，含空格的值不属于已知的 key 用法，
//! 判它失败只会把正确译文退回英文（宁可漏报，不可误报）。新增 key 型属性时只改
//! [`KEY_ATTRIBUTES`] 一处。
//!
//! ## 明确不抓什么（防误报）
//!
//! - 形态不良好的尖括号一律当普通文本：`< 5`、`a < b`、`a < b > c`、`<5>`；
//! - 不在写回白名单里的标签名一律当普通文本（`<name>`、`<color>`）：
//!   `content_list::write_text_fragment` 只把白名单标签写成真标签，其余会转义，
//!   所以它们本来就没有「必须配对」的义务；
//! - 属性解析不出来（引号不闭合等）时整段当普通文本，不做半吊子比较；
//! - **没有引号的属性值不比对**（`<LSTag Tooltip=HitPoints>`）：属性值不加引号在
//!   XML 里不合法，写回层 `content_list::parse_single_tag` 也过不了 quick-xml，
//!   会把这个标签当普通文本转义 —— 拿半解析出来的边界去比正是「半吊子比较」；
//! - `&lt;` / `&gt;` 先还原成字面尖括号再比较：解析层已经把 `&lt;` 还原过一次，
//!   模型若把译文里的 `<` 重新转义回去，不算结构变化；
//! - **方括号散文不算占位符**：只认「纯数字」「全大写 ID」「`IE_` 前缀」三类形态，
//!   混合大小写的 `[Note]`、`[Draft]`、`[todo]` 这类方括号正文是要被翻译的，
//!   收进来会把正确译文判失败；
//! - **占位符周围的空格不参与比对**：`deal [2] damage` 译成「造成[2]点伤害」是
//!   正确中文 —— 中文不用空格分词，英文的词间空格不是结构。官方简中里 `[N]`
//!   两侧带空格的比例只有 6.9% / 3.4%，而英文源是 61.3% / 38.7%。保留下来的
//!   那几处都是**载重的分隔符**（`[1] [2]` 两个值之间、`+ [1]` 运算符之后），
//!   而分隔符的种类不限（空格 / 顿号 / 逗号都行），所以只校验「不许粘连」；
//! - **标签顺序不参与比对**：中英语序不同，标签跟着各自包住的正文换位是常态。
//!   真实例子：`Inflicts <A>Deadly Toxin</A> ... fails <B>Saving Throw</B>` 译成
//!   中文后条件从句提前，两个标签必然对调 —— 要求顺序一致会把这种**正确译文**
//!   判失败然后退回英文原文，比不查糟糕得多；
//! - **非 key 型属性的值**变化、标签内文本被翻译、正文整体改写都不报；已知 key 型
//!   属性（`Tooltip` / `Type`）的值**只有标识符形态**才要求逐字一致，含空格的自然
//!   语言值仍然放行（刻意的盲点，理由见上）；唯一会报的其它「非缺失」情形是标签
//!   交叉嵌套（`<a><b></a></b>`，不是合法 XML，写回层会把整条降级成纯文本）与
//!   上面那条占位符粘连。
//!
//! 原则：**宁可漏报，不可误报** —— 误报会把本来正确的译文判失败，代价比偶尔
//! 漏掉一个坏译文高得多。所有规则都有对应的边界单测（见文件末尾）。

use std::sync::LazyLock;

use regex::Regex;

use crate::formats::content_list::is_allowed_inline_tag;

/// 空元素标签：`<br>` / `<br/>` 都不需要闭合标签。
///
/// 与 `formats::content_list` 的 `VOID_TAGS` 保持一致：那边决定写回时怎么处理，
/// 这边决定校验时怎么算，两边对「空元素」的认定必须相同。
const VOID_TAGS: &[&str] = &["br"];

/// 已知的 **key 型属性**：它们的值是查表用的 key，不是给玩家看的文本。
///
/// 只收当前语料（`samples/english.xml`）里真实出现的两个，见模块头部「为什么属性名
/// 与 key 型属性值要参与比较」一节的证据。**新增 key 型属性时只改这里**：
/// [`key_attribute_values`] 与模块文档的判据都跟着这张表走。
///
/// 刻意不收 `Tag` / `color`：语料里没有它们作为查表 key 的证据，收进来只会多一份
/// 误报风险（`Tag="Fire"` 具体是什么用途没有实测过）。
const KEY_ATTRIBUTES: &[&str] = &["Tooltip", "Type"];

/// 属性值是否是「标识符形态」：非空、只含 ASCII 字母 / 数字 / 下划线。
///
/// 这是 key 型属性值必须逐字一致的**第二道闸门**。语料依据：`Tooltip` 的 279 个取值
/// **全部**满足这个形态（`HitPoints` / `CAUSE_FEARED` / `Projectile_MagicStoneThrow`），
/// 一个空格都没有；含空格 / 标点的自然语言值一律放行 —— 真要是有 MOD 把属性值当显示
/// 文本用（`Tooltip="a natural sentence"`），不会因为这条规则把正确译文判失败。
///
/// 这与 [`is_placeholder_token`] 里花括号的形态判定是同一个思路：先认形态，再谈保护。
fn is_identifier_shaped(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 摘要 / 纠错提示里最多列出的问题条数（避免把整段译文塞进错误信息）。
const MAX_LISTED_ISSUES: usize = 3;

/// 方括号对：半角、全角、以及两者**混用**（`【1]`、`[1】`）。
///
/// 只用于 [`repair_full_width_brackets`]，**不参与签名扫描** —— 签名只认文本里
/// 真实存在的半角方括号，否则「原文用全角、译文用半角」会被判成保真，
/// 而写回的是半角形态，等于悄悄改了原文。
///
/// 字符类里排除了全部三种闭括号，所以贪婪匹配不会跨过一个占位符去咬后面的 ——
/// `[1] 造成【2】` 会分别匹配成两段，不会连成一段。
static FULL_WIDTH_BRACKET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\[【［]([^\]】］]*)[\]】］]").expect("方括号正则是常量，不会编译失败")
});

/// 花括号占位符 token —— 只用于 [`repair_swapped_bracket_style`] 的分类重写。
static BRACE_TOKEN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([A-Za-z0-9_]+)\}").expect("花括号正则是常量，不会编译失败"));

/// 半角方括号占位符 token —— 同上。
static BRACKET_TOKEN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[([A-Za-z0-9_]+)\]").expect("方括号 token 正则是常量，不会编译失败")
});

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
    /// 已知 key 型属性（`KEY_ATTRIBUTES`）的值被改写。
    ///
    /// 属性值不全是显示文本：`Tooltip` / `Type` 的值是**查表 key**。
    /// `Tooltip="VENOMOUS_BARBS_CONDITION"` 包着的正文才是玩家看到的 "Deadly Toxin"，
    /// 把 KEY 翻成中文，游戏按 key 查表就查不到，链接 / tooltip 直接失效；
    /// `Type` 更明显，取值只有 `Spell` / `Status` / `Passive`，翻掉它链接样式就没了。
    ///
    /// 只在**原文**的值属于标识符形态时报（`is_identifier_shaped`）：
    /// 含空格 / 标点的自然语言值仍然放行，那是刻意的盲点（宁可漏报，不可误报）。
    /// 比对按**多重集**：标签可以跟着译文语序整体换位，但每个 key 值都必须原样出现。
    /// 属性名多重集对不上时也不报 —— 那种情况值其实没被动过，上面两条
    /// 缺失 / 多出标签已经指出了根因。
    AttributeValueChanged {
        /// 属性名，如 `Tooltip`
        name: String,
        /// 原文里的值（要求译文逐字保留的那个），如 `HitPoints`
        value: String,
        /// 被改写的处数
        count: usize,
    },
    /// 标签**交叉嵌套**（`<LSTag><b></LSTag></b>`），不是合法结构。
    ///
    /// 刻意**不查标签顺序**：中英语序不同，标签跟着各自包住的正文换位是常态。
    /// 典型反例（真实 MOD 里很常见）：
    /// `Inflicts <A>Deadly Toxin</A> ... fails <B>Saving Throw</B>` 译成中文后
    /// 条件从句在前、结果在后，两个标签必然对调 —— 要求顺序一致会把
    /// **正确译文**判失败，然后退回英文原文，比不查糟糕得多。
    TagNestingBroken,
    /// 译文把原文里**分开**的相邻占位符粘在了一起（`[1] [2]` → `[1][2]`）。
    ///
    /// 占位符多重集是一致的，所以上面几条都不会报 —— 但游戏替换完两个值之后
    /// 中间没有任何分隔，玩家看到的是一个数（`12`）而不是两个。属于内容缺陷。
    GluedPlaceholders { count: usize },
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
            FidelityIssue::AttributeValueChanged { name, value, count } => format!(
                "{}标签属性 {name}=\"{value}\" 是查表 key，必须逐字保留、不要翻译",
                count_text(*count)
            ),
            FidelityIssue::TagNestingBroken => {
                "标签开闭没有正确配对（有落单的闭合标签，或不同标签名之间交叉嵌套），\
                 请让每个开始标签与它对应的闭合标签成对嵌套"
                    .to_string()
            }
            FidelityIssue::GluedPlaceholders { count } => format!(
                "有{}相邻占位符粘在一起，请在它们之间保留空格",
                count_text(*count)
            ),
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
            FidelityIssue::AttributeValueChanged { name, value, count } => write!(
                f,
                "属性 {name}=\"{value}\" 的值被改写{}",
                count_text(*count)
            ),
            FidelityIssue::TagNestingBroken => {
                write!(f, "标签开闭不配对，嵌套不合法")
            }
            FidelityIssue::GluedPlaceholders { count } => {
                write!(f, "相邻占位符粘连{}", count_text(*count))
            }
        }
    }
}

/// 比较原文与译文的结构签名；返回空列表表示保真。
pub fn check_fidelity(source: &str, target: &str) -> Vec<FidelityIssue> {
    let source_signature = Signature::of(source);
    let target_signature = Signature::of(target);
    let mut issues = Vec::new();

    for (token, expected) in counted(&source_signature.placeholder_tokens()) {
        let found = target_signature.count_placeholder(token);
        if found < expected {
            issues.push(FidelityIssue::MissingPlaceholder {
                token: token.to_string(),
                count: expected - found,
            });
        }
    }
    for (token, found) in counted(&target_signature.placeholder_tokens()) {
        let expected = source_signature.count_placeholder(token);
        if found > expected {
            issues.push(FidelityIssue::ExtraPlaceholder {
                token: token.to_string(),
                count: found - expected,
            });
        }
    }
    for (tag, expected) in counted(&source_signature.tag_tokens()) {
        let found = target_signature.count_tag(tag);
        if found < expected {
            issues.push(FidelityIssue::MissingTag {
                tag: tag.to_string(),
                count: expected - found,
            });
        }
    }
    for (tag, found) in counted(&target_signature.tag_tokens()) {
        let expected = source_signature.count_tag(tag);
        if found > expected {
            issues.push(FidelityIssue::ExtraTag {
                tag: tag.to_string(),
                count: found - expected,
            });
        }
    }

    // key 型属性（`Tooltip` / `Type`）的值是**查表 key**，必须逐字一致。
    //
    // 只在标签名多重集一致时查：属性名对不上时（`Type` → `类型`）值其实没被动过，
    // 这里报「值被改写」是**错的**，而且上面那两条缺失 / 多出标签已经指出根因。
    // 占位符问题不影响这条：它们互相独立（key 型属性值不可能含 `{1}`）。
    let tag_names_consistent = !issues.iter().any(|issue| {
        matches!(
            issue,
            FidelityIssue::MissingTag { .. } | FidelityIssue::ExtraTag { .. }
        )
    });
    if tag_names_consistent {
        // 判据是**方向性**的：原文里标识符形态的 key 值必须在译文里原样出现。
        // 反向（译文多出某个 key 值）不用单独查 —— 标签名多重集已经相等，
        // 原文的值少一个必然在「缺失」这个方向上先报出来。
        //
        // 按**多重集**比对，与标签顺序自由一致：`Tooltip="VENOMOUS_BARBS_CONDITION"`
        // 那个标签整体换到句首没问题，但值一个都不能改、不能少。
        for ((name, value), expected) in counted_pairs(&source_signature.key_attribute_values) {
            let found = target_signature.count_key_attribute_value(name, value);
            if found < expected {
                issues.push(FidelityIssue::AttributeValueChanged {
                    name: name.to_string(),
                    value: value.to_string(),
                    count: expected - found,
                });
            }
        }
    }

    // 标签**顺序不参与比对**，只查嵌套是否合法。
    //
    // 为什么不能要求顺序一致：中英语序不同，标签跟着各自包住的正文换位是常态。
    // 真实例子（`Inflicts <A>Deadly Toxin</A> ... fails <B>Saving Throw</B>`）
    // 译成中文后条件从句提前，两个标签必然对调 —— 旧实现把这种**正确译文**
    // 判成「标签顺序与原文不一致」，然后退回英文原文，比不查糟糕得多。
    //
    // 真正会把 MOD 弄坏的是**交叉嵌套**：`<LSTag><b></LSTag></b>` 不是合法 XML，
    // 写回层 `content_list::is_tag_balanced` 会把整条降级成纯文本转义，
    // 游戏里看到的是一堆字面标签。所以这里只查嵌套合法性。
    //
    // 门槛是「原文本身合法」：原文自己就交叉嵌套时（MOD 本来就坏），模型无从改好，
    // 不为难它 —— 多重集那条已经足够报出「标签数量对不上」。
    if issues.is_empty()
        && is_well_nested(&source_signature.tags)
        && !is_well_nested(&target_signature.tags)
    {
        issues.push(FidelityIssue::TagNestingBroken);
    }

    // 多重集一致，但译文比原文多粘了一处（`[1] [2]` → `[1][2]`）。
    //
    // 放在多重集检查之后是有意的：占位符本身缺了 / 多了的时候，粘连多半只是
    // 重复的副作用（`{1}` 被写成 `{1}{1}`），再报一条会把真正的原因淹掉。
    // 判据是**非对称**的：只报「译文比原文更粘」，加分隔不算问题。
    if issues.is_empty() {
        let glued = target_signature.glued_pairs();
        let expected = source_signature.glued_pairs();
        if glued > expected {
            issues.push(FidelityIssue::GluedPlaceholders {
                count: glued - expected,
            });
        }
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

/// 一段文本里的一个占位符。
///
/// 刻意**不派生 `PartialEq`**：元素里带着 `glued_to_previous`，一旦有人拿它当
/// 多重集的比对键，就会把「粘着的 `{1}`」和「不粘的 `{1}`」当成两个 token，
/// 重复次数统计不出来（`{1}{1}` 变成两条 `count = 1`），从而**静默漏报**
/// 「译文多吐了一个占位符」。要比 token 就用 [`Signature::placeholder_tokens`]。
#[derive(Debug, Clone)]
struct Placeholder {
    /// 渲染形态，用于多重集比对：`{1}` / `[2]`
    token: String,
    /// 与**前一个**占位符之间是否一个字符都没有（`[1][2]` 里第二个为 `true`）。
    ///
    /// 只看「有没有东西隔着」，不看隔着的是什么 —— 中文里空格、顿号、逗号
    /// 都是合法分隔，只有「什么都没有」才会让游戏把两个值渲染成一个数。
    glued_to_previous: bool,
}

/// 一段文本的结构签名。
#[derive(Debug, Default)]
struct Signature {
    /// 占位符（含重复），按出现顺序
    placeholders: Vec<Placeholder>,
    /// 标签 token（含重复），按出现顺序；`<LSTag>` / `</LSTag>` / `<br/>`
    tags: Vec<String>,
    /// 必须逐字保留的 key 型属性值 `(属性名, 值)`（含重复），按出现顺序。
    ///
    /// 只收 [`KEY_ATTRIBUTES`] 里、且值是标识符形态的属性，见 [`key_attribute_values`]。
    /// 刻意与 `tags` 分开存：`tags` 只承载「标签名 + 属性名」的结构 token，
    /// 值在这里单独按多重集比对。
    key_attribute_values: Vec<(String, String)>,
}

impl Signature {
    fn of(text: &str) -> Self {
        let text = restore_entities(text);
        let scanned = scan_tags(&text);
        let key_attribute_values = scanned
            .iter()
            .flat_map(|tag| tag.key_values.iter().cloned())
            .collect();
        let tags = normalize_self_closing(scanned.into_iter().map(|tag| tag.token).collect());
        let placeholders = scan_placeholders(&text);
        Self {
            placeholders,
            tags,
            key_attribute_values,
        }
    }

    fn count_placeholder(&self, token: &str) -> usize {
        self.placeholders
            .iter()
            .filter(|p| p.token == token)
            .count()
    }

    fn count_tag(&self, tag: &str) -> usize {
        self.tags.iter().filter(|t| *t == tag).count()
    }

    /// 某个 key 型属性值在签名里出现了几次。
    fn count_key_attribute_value(&self, name: &str, value: &str) -> usize {
        self.key_attribute_values
            .iter()
            .filter(|(known_name, known_value)| known_name == name && known_value == value)
            .count()
    }

    /// 占位符的渲染形态（去掉相邻信息）—— 多重集统计用。
    ///
    /// 统计**不能**直接拿 `placeholders` 的元素当比对键：元素里还带着
    /// `glued_to_previous`，直接比会把「粘着的 `{1}`」和「不粘的 `{1}`」当成
    /// 两个不同 token，重复次数就统计不出来（`{1}{1}` 会变成两条 `count = 1`，
    /// 而不是一条 `count = 2`），于是「译文多吐了一个 `{1}`」这条反而漏报。
    fn placeholder_tokens(&self) -> Vec<&str> {
        self.placeholders.iter().map(|p| p.token.as_str()).collect()
    }

    /// 标签 token —— 多重集统计用。
    fn tag_tokens(&self) -> Vec<&str> {
        self.tags.iter().map(String::as_str).collect()
    }

    /// 零间隔相邻的占位符对数。
    fn glued_pairs(&self) -> usize {
        self.placeholders
            .iter()
            .filter(|p| p.glued_to_previous)
            .count()
    }
}

/// 标签序列是否是**合法的嵌套**（每个闭合标签都对上最近一个未闭合的开始标签）。
///
/// 只看嵌套，**不看顺序** —— 顺序由目标语言的语序决定，不是结构属性。
/// 合法的例子：`<a></a><b></b>`、`<b></b><a></a>`（换位，都合法）、`<a><b></b></a>`；
/// 非法：`<a><b></a></b>`（**不同名**标签交叉）、`x</a>`（闭合标签落单，栈下溢）、
/// `<a><b></b>`（少一个闭合，但那条由多重集负责报）。
///
/// **同名标签之间不存在「交叉」**：`<a>x<a></a>y</a>` 就是合法的内层嵌套，
/// `</a>` 的语义是「关掉最内层那个」。所以对当前语料（只有 `LSTag` 一种非空标签、
/// 嵌套深度 1）而言，这条检查实际能抓到的是**落单的闭合标签**，
/// 交叉只有 `LSTag` 与 `<b>` 之类混用时才可能出现。错误文案要同时覆盖这两种。
///
/// token 形态见 [`render_tag`]：开始标签 `<LSTag Type>`、结束标签 `</LSTag>`、
/// 空元素 `<br/>`（不入栈，写回层也不要求它闭合）。
fn is_well_nested(tags: &[String]) -> bool {
    let mut stack: Vec<&str> = Vec::new();
    for tag in tags {
        if let Some(name) = tag.strip_prefix("</") {
            let name = name.strip_suffix('>').unwrap_or(name);
            match stack.pop() {
                Some(open) if open == name => {}
                // 对不上栈顶：交叉嵌套，或者根本没有对应的开始标签
                _ => return false,
            }
        } else if tag.ends_with("/>") {
            // 空元素：不需要闭合，不入栈
        } else {
            let inner = tag
                .strip_prefix('<')
                .and_then(|rest| rest.strip_suffix('>'))
                .unwrap_or(tag);
            stack.push(inner.split_whitespace().next().unwrap_or(""));
        }
    }
    stack.is_empty()
}

/// 把**非空元素**的 `<x/>` 规范化成 `<x>` + `</x>`：XML 里这两种写法等价。
///
/// 只做这一步：`<br/>` 这类空元素（[`VOID_TAGS`]）保持原样，因为写回层也不要求
/// 它闭合。没有这一步，`<i/>` 与 `<i></i>` 这对等价写法会互相判失败，
/// 把本来正确的译文判成 `error`（F-03）。
///
/// token 里可能带属性名（`<LSTag Tooltip Type/>`），所以标签名要单独取出来：
/// 判断空元素看名字，展开出来的闭合标签不能带属性。
fn normalize_self_closing(tags: Vec<String>) -> Vec<String> {
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags {
        match tag
            .strip_prefix('<')
            .and_then(|rest| rest.strip_suffix("/>"))
        {
            Some(inner) => {
                let name = inner.split_whitespace().next().unwrap_or("");
                if VOID_TAGS.contains(&name) {
                    out.push(tag);
                } else {
                    out.push(format!("<{inner}>"));
                    out.push(format!("</{name}>"));
                }
            }
            None => out.push(tag),
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
fn scan_tags(text: &str) -> Vec<TagToken> {
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
fn parse_tag(text: &str, start: usize) -> Option<(TagToken, usize)> {
    let after = text[start..].strip_prefix('<')?;
    let gt = find_tag_end(after)?;
    let end = start + 1 + gt + 1;
    let tag = &text[start..end];
    let inner = &after[..gt];
    // 严格形态检查在前，白名单在后：`< 5`、`<5>`、`<name>` 都要被挡掉
    let token = render_tag(inner)?;
    if !is_allowed_inline_tag(tag) {
        return None;
    }
    Some((token, end))
}

/// 找到标签结束的 `>`，返回它相对 `<` 之后文本的偏移。
///
/// 属性值里的 `>` 不是标签结束（`<LSTag Tooltip="a > b">` 是合法 XML）；
/// 引号不闭合、或 `<` 之后又出现 `<`（`a <b <c>`）都不算标签。
fn find_tag_end(after_lt: &str) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in after_lt.char_indices() {
        match quote {
            Some(open) => {
                if ch == open {
                    quote = None;
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                '>' => return Some(index),
                '<' => return None,
                _ => {}
            },
        }
    }
    None
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

/// 一个标签的结构签名：渲染 token + 必须逐字保留的 key 型属性值。
#[derive(Debug, Clone)]
struct TagToken {
    /// 结构 token，见 [`render_tag`]：`<LSTag>` / `</LSTag>` / `<LSTag Tooltip Type>` / `<br/>`
    token: String,
    /// 这个标签上要逐字保留的 `(属性名, 值)`，见 [`key_attribute_values`]。
    key_values: Vec<(String, String)>,
}

impl TagToken {
    /// 没有属性值的标签（**结束标签**专用）。
    ///
    /// 自闭合的开始标签（`<LSTag Type="Spell"/>`）不走这里：它照样可能带 key 型属性。
    fn bare(token: String) -> Self {
        Self {
            token,
            key_values: Vec::new(),
        }
    }
}

/// 开始标签里的一个属性。
#[derive(Debug, Clone)]
struct Attribute {
    name: String,
    /// **带引号**的值（引号本身不算）。
    ///
    /// 没写 `=`、或值没加引号时为 `None`：边界解析不出来就不参与比较
    /// （不加引号的属性值在 XML 里不合法，写回层也会把这种标签当普通文本，
    /// 见模块头部「明确不抓什么」）。
    quoted_value: Option<String>,
}

/// 挑出必须逐字保留的 key 型属性值：属性名在 [`KEY_ATTRIBUTES`] 里、值是标识符形态。
///
/// 两道闸门**同时**成立才收，见模块头部与 [`is_identifier_shaped`]。
fn key_attribute_values(attrs: &[Attribute]) -> Vec<(String, String)> {
    attrs
        .iter()
        .filter(|attr| KEY_ATTRIBUTES.contains(&attr.name.as_str()))
        .filter_map(|attr| {
            let value = attr.quoted_value.as_deref()?;
            is_identifier_shaped(value).then(|| (attr.name.clone(), value.to_string()))
        })
        .collect()
}

/// 把标签正文渲染成稳定的结构 token：`<LSTag>` / `</LSTag>` / `<br/>`。
///
/// 开始标签的 token **带属性名**（排序后、空格分隔），属性值不在这里：
/// 属性名是结构，写回时会被原样写进 PAK，必须逐个对上（顺序不计 —— 排序后再拼接）；
/// 属性值里只有 [`KEY_ATTRIBUTES`] 的标识符形态值要逐字一致，那些值单独放进
/// [`TagToken::key_values`] 按多重集比对，其余值不参与比较。
fn render_tag(inner: &str) -> Option<TagToken> {
    if let Some(body) = inner.strip_prefix('/') {
        // 结束标签：名字之后只允许空白
        let (name, rest) = tag_name(body)?;
        if !rest.trim().is_empty() {
            return None;
        }
        return Some(TagToken::bare(format!("</{name}>")));
    }
    // 开始标签：`<` 之后必须紧跟字母，所以 `< 5` / `a < b > c` 到不了这里
    let (name, rest) = tag_name(inner)?;
    let (attrs, self_closing) = scan_attributes(rest)?;
    let key_values = key_attribute_values(&attrs);
    let mut names: Vec<String> = attrs.into_iter().map(|attr| attr.name).collect();
    names.sort();
    let head = if names.is_empty() {
        format!("<{name}")
    } else {
        format!("<{name} {}", names.join(" "))
    };
    let token = if self_closing || VOID_TAGS.contains(&name) {
        format!("{head}/>")
    } else {
        format!("{head}>")
    };
    Some(TagToken { token, key_values })
}

/// 解析开始标签里「标签名之后」的部分，返回（属性，是否自闭合）。
///
/// 语法不合法就返回 `None`：调用方会把整段当普通文本，不做半吊子比较
/// （宁可漏报，不可误报 —— 半解析出来的属性名集合会让正确译文互相判失败）。
///
/// 只有**带引号**的值才会被记进 [`Attribute::quoted_value`]；没写 `=` 或值没加
/// 引号时值为 `None`（宽松接受，但不拿它比对）。
fn scan_attributes(rest: &str) -> Option<(Vec<Attribute>, bool)> {
    let mut attrs: Vec<Attribute> = Vec::new();
    let mut cursor = rest;
    loop {
        let trimmed = cursor.trim_start();
        if trimmed.is_empty() {
            return Some((attrs, false));
        }
        if let Some(after_slash) = trimmed.strip_prefix('/') {
            // 自闭合：`/` 之后只允许空白
            return after_slash.trim().is_empty().then_some((attrs, true));
        }
        let name_end = trimmed
            .find(|c: char| c.is_whitespace() || c == '=' || c == '/' || c == '"' || c == '\'')
            .unwrap_or(trimmed.len());
        if name_end == 0 {
            // `"` / `=` / `/` 打头：不是合法属性名
            return None;
        }
        let name = trimmed[..name_end].to_string();
        let after_name = trimmed[name_end..].trim_start();
        let Some(after_eq) = after_name.strip_prefix('=') else {
            // 没有 `=`：宽松接受（XML 要求属性有值，但这里不为难模型）
            attrs.push(Attribute {
                name,
                quoted_value: None,
            });
            cursor = after_name;
            continue;
        };
        let value = after_eq.trim_start();
        let quoted_value = match value.chars().next() {
            Some(quote @ ('"' | '\'')) => {
                // 引号里的内容整体跳过：空格、`/`、`=` 都只是值的一部分
                let body = &value[quote.len_utf8()..];
                let close = body.find(quote)?;
                cursor = &body[close + quote.len_utf8()..];
                Some(body[..close].to_string())
            }
            Some(_) => {
                let end = value.find(char::is_whitespace).unwrap_or(value.len());
                cursor = &value[end..];
                // 没加引号：只跳过、不比对
                None
            }
            // `name=` 后面什么都没有
            None => return None,
        };
        attrs.push(Attribute { name, quoted_value });
    }
}

/// 扫描占位符，返回 `{1}` / `[2]` 这类 token 序列（含重复与相邻信息）。
///
/// 两种括号在**同一遍**里按出现顺序扫：这样才能算出 `[1]{2}` 这种混合写法的
/// 相邻关系，也才不会因为「有一种括号没闭合」就漏掉后面另一种括号。
fn scan_placeholders(text: &str) -> Vec<Placeholder> {
    let mut tokens: Vec<Placeholder> = Vec::new();
    let mut previous_end: Option<usize> = None;
    let mut index = 0usize;

    while index < text.len() {
        let rest = &text[index..];
        let brace = rest.find('{').map(|offset| (index + offset, '{'));
        let bracket = rest.find('[').map(|offset| (index + offset, '['));
        // 取靠前的那个开括号；两种都没有就结束
        let Some((start, open)) = (match (brace, bracket) {
            (Some(b), Some(k)) => Some(if b.0 <= k.0 { b } else { k }),
            (Some(b), None) => Some(b),
            (None, Some(k)) => Some(k),
            (None, None) => None,
        }) else {
            break;
        };
        let closer = if open == '{' { '}' } else { ']' };

        let Some(offset) = text[start + 1..].find(closer) else {
            // 这个开括号没有闭合符：跳过它本身，后面另一种括号仍要参与扫描
            index = start + 1;
            continue;
        };
        let end = start + 1 + offset + 1;
        let inner = &text[start + 1..end - 1];

        if is_placeholder_token(inner, open) {
            tokens.push(Placeholder {
                token: text[start..end].to_string(),
                glued_to_previous: previous_end == Some(start),
            });
            previous_end = Some(end);
            index = end;
        } else {
            // `{{1}}`：外层 `{` 与最近的 `}` 之间是 `{1`，非法；
            // 从下一个字符继续扫，于是内层 `{1}` 仍会被识别。
            index = start + 1;
        }
    }
    tokens
}

/// 是否是认得的占位符形式。
///
/// - **花括号**：非空、只含 ASCII 字母 / 数字 / 下划线。刻意收窄：`{}`、`{ }`、
///   `{a b}`、`{"k": 1}`、`{#FFAA00}` 都不算占位符，免得把普通文本里的花括号
///   当结构来要求译文匹配。
/// - **方括号**：只认 [`is_bracket_placeholder_token`] 的三类形态。
fn is_placeholder_token(inner: &str, open: char) -> bool {
    if inner.is_empty() {
        return false;
    }
    if open == '[' {
        return is_bracket_placeholder_token(inner);
    }
    inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 方括号占位符的形态判定：纯数字 / 全大写内部 ID / `IE_` 前缀，三类之一。
///
/// 与 `glossary::entry` 里过滤术语噪音的三条判据一一对应
/// （`PLACEHOLDER_PATTERN` = `\[\d+\]`、`UI_MARKER_PATTERN` = `^\[[A-Z_]{2,}\]`、
/// `source.contains("[IE_")`）：那边用「这是占位符」把句式模板踢出术语表，
/// 这边用同一判断保护翻译。两边对「什么算占位符」有分歧的话，
/// 会出现「术语表认定是模板、翻译层却不保护」这种漏洞。
///
/// 刻意**不认** `[Note]`、`[Draft]`、`[todo]` 这类方括号散文 —— 它们是要被
/// 翻译的正文，收进来会把正确译文判失败（模块开头「宁可漏报，不可误报」）。
fn is_bracket_placeholder_token(inner: &str) -> bool {
    if inner.is_empty() {
        return false;
    }
    // `[1]`、`[10]`：游戏替换数值
    if inner.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    // `[DRUID]`、`[IE_TOGGLE_SPELLS]`：全大写内部 ID
    if inner.chars().count() >= 2 && inner.chars().all(|c| c.is_ascii_uppercase() || c == '_') {
        return true;
    }
    // `[IE_PanelSelect]`：`IE_` 前缀的驼峰形态
    inner.starts_with("IE_") && inner.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 把译文里被写成全角形态的占位符修回半角：`【1】` / `［1］` → `[1]`。
///
/// 模型很容易按中文排版习惯把 `[1]` 写成 `【1】` —— 这是「格式规范」而不是
/// 「内容丢失」，直接判失败会让本来正确的译文被退回原文，代价比这个瑕疵高。
///
/// **为什么必须在校验之前修、而不能只在签名层归一化**：只在签名层归一化的话，
/// `【1】` 会被判成保真，然后**原样写进 PAK** —— 游戏替换不了全角占位符，
/// 于是「报错可见」被换成了「静默损坏」。所以这一步由调用方
/// （`retry::translate_with_retry`）在拿到模型完整输出后立刻执行一次，
/// 让校验、`done` 事件与最终落盘的文本看到的是同一个版本。
///
/// 只修**内层是合法占位符形态**的方括号，所以译文里作为普通标点使用的
/// `【注意事项】` 不会被误伤。
pub fn repair_full_width_brackets(text: &str) -> String {
    if !text.contains(['【', '】', '［', '］']) {
        return text.to_string();
    }
    FULL_WIDTH_BRACKET
        .replace_all(text, |caps: &regex::Captures<'_>| {
            let inner = &caps[1];
            if is_bracket_placeholder_token(inner) {
                format!("[{inner}]")
            } else {
                // 不是占位符形态：原样保留（中文标点用法）
                caps[0].to_string()
            }
        })
        .into_owned()
}

/// 把译文里可确定的占位符**写法**差异修回原文形态。
///
/// 由 `retry::translate_with_retry` 在**结构校验之前**调用一次，让校验、
/// `Done` 事件与最终落盘的文本看到同一个版本。目前处理两类：
///
/// 1. 全角方括号 → 半角（[`repair_full_width_brackets`]）；
/// 2. 括号类型被模型改写（`[1]` ↔ `{1}`，见 [`repair_swapped_bracket_style`]）。
///
/// 两类都只在**无歧义**时动手；判断不了的留给结构校验报错、让模型重译。
pub fn repair_placeholders(source: &str, target: &str) -> String {
    let target = repair_full_width_brackets(target);
    repair_swapped_bracket_style(source, &target)
}

/// 模型把 `[1]` 写成 `{1}`（或反向）时，按**原文**修回正确的括号类型。
///
/// BG3 里花括号与方括号是**两套不同的替换机制**，换了类型游戏就填不上这个值，
/// 界面上会直接显示字面量。模型有很强的「归一化」倾向 —— 它会把自己更熟悉的
/// 那一种当成标准写法（典型是把少见的 `[1]` 改成 `{1}`），所以这里以原文为准。
///
/// **只在原文只使用了一种括号类型时才动手**：原文同时含 `{1}` 和 `[1]` 时，
/// 译文里的 `{1}` 到底该留着还是该变成 `[1]` 无法判断（两个 token 长得一样，
/// 分不清谁是谁改的），这种情况不猜 —— 交给结构校验报「缺失 / 多出」。
///
/// 只改内层名字在原文里出现过的 token，所以模型凭空多写的 `{2}` 不会被洗白。
fn repair_swapped_bracket_style(source: &str, target: &str) -> String {
    let signature = Signature::of(source);
    let mut brace_names: Vec<&str> = Vec::new();
    let mut bracket_names: Vec<&str> = Vec::new();
    for placeholder in &signature.placeholders {
        if let Some(inner) = placeholder
            .token
            .strip_prefix('{')
            .and_then(|rest| rest.strip_suffix('}'))
        {
            brace_names.push(inner);
        } else if let Some(inner) = placeholder
            .token
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
        {
            bracket_names.push(inner);
        }
    }

    match (brace_names.is_empty(), bracket_names.is_empty()) {
        // 原文只用方括号：译文里的 `{X}` 一定是被改写出来的，修回 `[X]`
        (true, false) => BRACE_TOKEN
            .replace_all(target, |caps: &regex::Captures<'_>| {
                if bracket_names.iter().any(|name| *name == &caps[1]) {
                    format!("[{}]", &caps[1])
                } else {
                    caps[0].to_string()
                }
            })
            .into_owned(),
        // 原文只用花括号：反向
        (false, true) => BRACKET_TOKEN
            .replace_all(target, |caps: &regex::Captures<'_>| {
                if brace_names.iter().any(|name| *name == &caps[1]) {
                    format!("{{{}}}", &caps[1])
                } else {
                    caps[0].to_string()
                }
            })
            .into_owned(),
        // 两种都用（或都没用）：不猜
        _ => target.to_string(),
    }
}

/// 统计 token 多重集，按首次出现顺序返回 `(token, 次数)`。
///
/// 内层 `'a` 与外层借用分开：返回的 `&'a str` 借的是原始字符串，
/// 不是调用方临时拼出来的那层 `Vec<&str>`。
fn counted<'a>(tokens: &[&'a str]) -> Vec<(&'a str, usize)> {
    let mut counted: Vec<(&'a str, usize)> = Vec::new();
    for token in tokens {
        match counted.iter_mut().find(|(known, _)| known == token) {
            Some((_, count)) => *count += 1,
            None => counted.push((token, 1)),
        }
    }
    counted
}

/// 统计 `(属性名, 值)` 多重集，按首次出现顺序返回 `((属性名, 值), 次数)`。
///
/// 与 [`counted`] 同一套写法：键是**整对**，所以 `Tooltip="A"` 与 `Tooltip="B"`
/// 分开计数，重复的 key 也数得对（`Tooltip="A"` 出现两次而译文只剩一次时报 1 处）。
fn counted_pairs<'a>(pairs: &'a [(String, String)]) -> Vec<((&'a str, &'a str), usize)> {
    let mut counted: Vec<((&'a str, &'a str), usize)> = Vec::new();
    for (name, value) in pairs {
        let pair = (name.as_str(), value.as_str());
        match counted.iter_mut().find(|(known, _)| *known == pair) {
            Some((_, count)) => *count += 1,
            None => counted.push((pair, 1)),
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

    // ── 方括号占位符 `[N]` ──

    /// 官方语料正向用例：`[N]` 必须逐字保留，且与 `{N}` 一样**顺序无关**。
    ///
    /// 语料取自 `samples/bg3-official-glossary.json` 里 163 条含 `[数字]` 的真实
    /// 条目。官方简中这 163 条**全部原样保留** `[N]`，其中
    /// `, [1] from [2]` → `，从[2]处取走了[1]` 还换了位 —— 所以判定规则是
    /// 「多重集相等、顺序随意」，与花括号完全一致。
    ///
    /// 注意最后两条把官方译文里的空格去掉了：官方的 `： [2]`（全角冒号后再加空格）
    /// 其实是中文排版错误用法，这里证明**空格不参与比对** —— 去掉它仍然保真。
    #[test]
    fn bracket_placeholders_follow_the_official_corpus() {
        for (source, target) in [
            ("[1] Damage", "[1]伤害"),
            ("[1] Hit Points", "[1]生命值"),
            ("+ [1] Inspiration", "+ [1]激励点"),
            ("-[1] HP", "-[1]生命值"),
            ("[1] feet", "[1]英尺"),
            ("[1] Gold/ [2] Gold", "[1]金币/ [2]金币"),
            ("[IE_PanelSelect] to open", "按[IE_PanelSelect]打开"),
            ("[DRUID] Wild Shape", "[DRUID]荒野形态"),
            ("Base Hit Points: [2]", "基础生命值：[2]"),
            ("In [1] Rounds: [2]", "[1]回合后：[2]"),
            // 官方换位：顺序不算问题
            (", [1] from [2]", "，从[2]处取走了[1]"),
        ] {
            assert!(
                is_faithful(source, target),
                "{source:?} → {target:?} 应保真"
            );
        }
    }

    /// 用户实际报的场景：模型把 `[2]` 连方括号一起吞了。
    #[test]
    fn missing_bracket_placeholder_is_reported() {
        let issues = check_fidelity(
            "If the target is below half its maximum hit points, deal [2] otherwise deal [1].",
            "如果目标当前的生命值低于其最大值的一半，则造成2点伤害，否则造成1点伤害。",
        );
        assert_eq!(
            issues,
            vec![
                FidelityIssue::MissingPlaceholder {
                    token: "[2]".into(),
                    count: 1
                },
                FidelityIssue::MissingPlaceholder {
                    token: "[1]".into(),
                    count: 1
                },
            ]
        );
        assert_eq!(issues[0].to_string(), "占位符 [2] 缺失");
        assert_eq!(issues[0].hint(), "缺少占位符 [2]");

        // 多吐一个也要报，且重复次数要统计对（回归：`counted` 曾经把
        // 「粘着的 `{1}`」与「不粘的 `{1}`」当成两个 token，于是漏报）
        assert_eq!(
            check_fidelity("[1] Damage", "[1][1]伤害")[0],
            FidelityIssue::ExtraPlaceholder {
                token: "[1]".into(),
                count: 1
            }
        );
    }

    /// `{}` 与 `[]` 是两套独立的 token：少一种、换成另一种都要报。
    #[test]
    fn brace_and_bracket_placeholders_are_not_interchangeable() {
        assert!(is_faithful("{1} 与 [2]", "[2] 与 {1}"));
        assert_eq!(missing_placeholders("[1] 和 [2]", "[1] 和"), vec!["[2]"]);
        assert_eq!(missing_placeholders("[1] damage", "{1}伤害"), vec!["[1]"]);
        assert_eq!(extra_placeholders("[1]", "[1]{1}"), vec!["{1}"]);
    }

    /// 方括号散文不算占位符 —— 它们是要被翻译的正文。
    #[test]
    fn bracketed_prose_is_not_a_placeholder() {
        for text in ["[Note]", "[Draft]", "[todo]", "[a]"] {
            assert!(is_faithful(text, text), "{text} 本身不该被当成占位符");
            assert!(
                check_fidelity(text, "译文").is_empty(),
                "{text} 丢了也不该报占位符问题"
            );
        }
        assert!(is_faithful("[Note] see below", "[注] 见下"));
    }

    // ── 占位符粘连 ──

    /// 真缺陷：原文本该分开的两个值被粘成了一个数。
    #[test]
    fn glued_placeholders_are_reported() {
        let issues = check_fidelity("Incompatible with [1] [2]", "与[1][2]不兼容");
        assert_eq!(issues, vec![FidelityIssue::GluedPlaceholders { count: 1 }]);
        assert_eq!(issues[0].to_string(), "相邻占位符粘连");
        assert_eq!(
            issues[0].hint(),
            "有相邻占位符粘在一起，请在它们之间保留空格"
        );

        // 多处粘连：次数要数对
        let issues = check_fidelity("[1] [2] [3]", "[1][2][3]");
        assert_eq!(issues, vec![FidelityIssue::GluedPlaceholders { count: 2 }]);
        assert_eq!(issues[0].to_string(), "相邻占位符粘连 2 处");
        assert_eq!(
            issues[0].hint(),
            "有 2 处相邻占位符粘在一起，请在它们之间保留空格"
        );

        // 花括号混写同样算
        assert_eq!(
            check_fidelity("{1} [2]", "{1}[2]"),
            vec![FidelityIssue::GluedPlaceholders { count: 1 }]
        );
        // 标签插在中间就不算粘连（原文和译文里都插了标签，才只差分隔）
        assert!(is_faithful("[1] <br/>[2]", "[1]<br/>[2]"));
    }

    /// 判据是**非对称**的：给分开的占位符加分隔永远不算问题，而且分隔符不限于
    /// 空格 —— 顿号、逗号、连词都是合法中文写法。多一个分隔无害，粘在一起才有害。
    #[test]
    fn adding_a_separator_is_never_a_problem() {
        for target in [
            "与[1] [2]不兼容",
            "与[1]、[2]不兼容",
            "与[1]，[2]不兼容",
            "与[1]和[2]不兼容",
        ] {
            assert!(
                is_faithful("Incompatible with [1] [2]", target),
                "加分隔不该被判失败: {target}"
            );
        }
        // 原文本来就粘着：照抄不算问题，补上分隔也不算
        assert!(is_faithful("[1][2]", "[1][2]"));
        assert!(is_faithful("[1][2]", "[1] [2]"));
    }

    /// 占位符重复时不该再叠加一条粘连 —— 粘连多半只是重复的副作用，
    /// 报出来会把真正的原因淹掉。
    #[test]
    fn duplication_does_not_also_report_gluing() {
        assert_eq!(
            check_fidelity("Deals {1} damage", "造成 {1}{1} 点伤害"),
            vec![FidelityIssue::ExtraPlaceholder {
                token: "{1}".into(),
                count: 1
            }]
        );
    }

    /// 单个占位符周围的空格**不参与比对**。
    ///
    /// 英文源 `[N]` 两侧带空格的比例是 61.3% / 38.7%，官方简中只剩 6.9% / 3.4%
    /// —— 空格是英文正字法（英文靠空格分词），不是占位符标记的一部分。
    #[test]
    fn spaces_around_a_single_placeholder_do_not_matter() {
        let source = "deal [2], otherwise deal [1].";
        for target in [
            "则造成[2]点伤害，否则造成[1]点伤害。",
            "则造成 [2] 点伤害，否则造成 [1] 点伤害。",
            "则造成[2] 点伤害，否则造成 [1]点伤害。",
        ] {
            assert!(
                is_faithful(source, target),
                "空格不该参与结构比对: {target}"
            );
        }
    }

    // ── 全角方括号修复 ──

    /// 模型按中文排版习惯写全角时，**修回半角**而不是判失败。
    ///
    /// 关键：这必须在**校验之前**执行（由 `retry` 调用），否则 `【1】` 会被判
    /// 保真、然后原样写进 PAK —— 游戏替换不了全角占位符，「报错可见」就变成了
    /// 「静默损坏」。所以这里断言的是「修好的内容」，不是「被判保真」。
    #[test]
    fn full_width_brackets_are_repaired() {
        assert_eq!(
            repair_full_width_brackets("造成【2】点伤害，否则造成［1］点伤害。"),
            "造成[2]点伤害，否则造成[1]点伤害。"
        );
        // 混用写法也修
        assert_eq!(
            repair_full_width_brackets("造成【1]点伤害"),
            "造成[1]点伤害"
        );
        assert_eq!(
            repair_full_width_brackets("造成［1】点伤害"),
            "造成[1]点伤害"
        );
        // 内部 ID 形态同样修
        assert_eq!(
            repair_full_width_brackets("按【IE_PanelSelect】打开"),
            "按[IE_PanelSelect]打开"
        );
        // 中文标点用法**不能**误伤
        assert_eq!(
            repair_full_width_brackets("参见【附录甲】与【注意事项】。"),
            "参见【附录甲】与【注意事项】。"
        );
        // 没有全角括号时原样返回
        assert_eq!(repair_full_width_brackets("Nothing here."), "Nothing here.");
        assert_eq!(repair_full_width_brackets(""), "");
    }

    /// 修复之后必须**真的能过校验**，否则「修了还报错」等于没修。
    #[test]
    fn repaired_full_width_brackets_pass_the_check() {
        let source = "deal [2], otherwise deal [1].";
        let raw = "则造成【2】点伤害，否则造成【1】点伤害。";
        // 不修的话两个占位符都算缺失
        assert_eq!(missing_placeholders(source, raw), vec!["[2]", "[1]"]);
        // 修完保真
        assert!(is_faithful(source, &repair_full_width_brackets(raw)));
    }

    // ── 括号类型被模型改写：`[1]` ↔ `{1}` ──

    /// 模型把少见的 `[1]`「归一化」成它更熟悉的 `{1}`（真实报障场景）。
    ///
    /// 两套括号在 BG3 里是不同的替换机制，换了类型游戏填不上值，界面上显示字面量，
    /// 所以这里按原文修回来 —— 与全角括号同理：这是**写法差异**，不是内容丢失，
    /// 判失败会让本来正确的译文退回英文，代价高得多。
    #[test]
    fn swapped_bracket_style_is_repaired() {
        let source = "Vanish into the wind and strike [1] different targets. Then teleport next to one of them.";
        let raw = "消散于风中并打击 {1} 个不同的目标。随后传送至其中一个目标身旁。";

        // 不修的话：`[1]` 缺失 + `{1}` 多出，模型收到这种提示也未必知道该改成哪种括号
        assert_eq!(missing_placeholders(source, raw), vec!["[1]"]);
        assert_eq!(extra_placeholders(source, raw), vec!["{1}"]);

        let fixed = repair_placeholders(source, raw);
        assert_eq!(
            fixed,
            "消散于风中并打击 [1] 个不同的目标。随后传送至其中一个目标身旁。"
        );
        assert!(is_faithful(source, &fixed));
    }

    /// 反方向也要修：原文用花括号、译文写成方括号。
    #[test]
    fn swapped_bracket_style_is_repaired_in_both_directions() {
        assert_eq!(
            repair_placeholders("Deals {1} damage", "造成 [1] 点伤害"),
            "造成 {1} 点伤害"
        );
        assert_eq!(
            repair_placeholders("Deals [1] damage", "造成 {1} 点伤害"),
            "造成 [1] 点伤害"
        );
        // 内部 ID 形态同样按原文修
        assert_eq!(
            repair_placeholders("[IE_PanelSelect] to open", "按 {IE_PanelSelect} 打开"),
            "按 [IE_PanelSelect] 打开"
        );
    }

    /// **原文两种括号都用时不许猜**：译文里的 `{1}` 到底该留着还是该变成 `[1]`
    /// 分不清（两个 token 长得一样），这种情况交给结构校验报错、让模型重译。
    #[test]
    fn mixed_source_bracket_styles_are_left_alone() {
        let source = r#"<LSTag Tooltip="Deals {1} damage">x</LSTag> strike [1] targets"#;
        let raw = "施放时造成 {1} 点伤害并打击 {1} 个目标";
        // 原文同时含 `{1}` 与 `[1]`，所以一个字都不改
        assert_eq!(repair_placeholders(source, raw), raw);
    }

    /// 模型凭空多写的占位符不能被「洗白」成原文有的那个。
    #[test]
    fn invented_placeholders_are_not_laundered() {
        // 原文只有 `[1]`：模型写的 `{2}` 换了编号，不在原文里，不修
        assert_eq!(
            repair_placeholders("strike [1] targets", "打击 {2} 个目标"),
            "打击 {2} 个目标"
        );
        // 原文根本没有方括号占位符：模型自己加的 `{1}` 同样不修
        assert_eq!(
            repair_placeholders("strike targets", "打击 {1} 个目标"),
            "打击 {1} 个目标"
        );
        // 修完仍然报错，不会被静默放过
        assert!(!is_faithful("strike [1] targets", "打击 {2} 个目标"));
    }

    /// 两类修复串起来跑：全角先变半角，再按原文的括号类型归一。
    #[test]
    fn repair_placeholders_composes_both_repairs() {
        assert_eq!(
            repair_placeholders("strike [1] targets", "打击 【1】 个目标"),
            "打击 [1] 个目标"
        );
        // 原文只用花括号：全角方括号先修成半角，再改成花括号
        assert_eq!(
            repair_placeholders("Deals {1} damage", "造成 【1】 点伤害"),
            "造成 {1} 点伤害"
        );
    }

    // ── 标签 ──

    #[test]
    fn matching_tags_are_faithful() {
        assert!(is_faithful(
            r#"Cast <LSTag Tag="Fire">Fireball</LSTag> now"#,
            r#"现在施放 <LSTag Tag="Fire">火球术</LSTag>"#
        ));
        // 两个兄弟标签换位：标签跟着各自包住的正文走，是合法译文（见
        // `tag_order_is_free_but_crossed_nesting_is_not`）
        assert!(is_faithful(
            "<b>粗</b> 与 <i>斜</i>",
            "<i>斜</i> 与 <b>粗</b>"
        ));
    }

    /// 属性**名**是结构，必须逐字保留；**非 key 型**属性的**值**自由。
    ///
    /// 写回链路（`content_list::render` → `write_text_fragment`）会把模型输出的
    /// 标签文本原样写进 PAK，**不会**恢复原文的属性 —— 所以属性名一旦被模型
    /// 改坏或删掉，产物里的标签对游戏就失效了，必须在这里拦住。
    ///
    /// key 型属性（`Tooltip` / `Type`）的值不自由，见
    /// [`key_attribute_values_must_be_kept_verbatim`]。
    #[test]
    fn attribute_names_must_match_and_non_key_values_are_free() {
        // 非 key 型属性（`color` / `Tag`）的值被翻译 / 改写：保真
        assert!(is_faithful(
            r#"<font color="red">红</font>"#,
            r#"<font color="深红">红字</font>"#
        ));
        assert!(is_faithful(
            r#"<LSTag Tag="Fire">Fireball</LSTag>"#,
            r#"<LSTag Tag="火">火球术</LSTag>"#
        ));
        // 属性被整段删掉：不保真
        assert!(!is_faithful(
            r#"<font color="red">红</font>"#,
            "<font>红字</font>"
        ));
        assert_eq!(
            check_fidelity(
                r#"<LSTag Type="Spell">Fireball</LSTag>"#,
                r#"<LSTag>火球术</LSTag>"#
            ),
            vec![
                FidelityIssue::MissingTag {
                    tag: "<LSTag Type>".into(),
                    count: 1
                },
                FidelityIssue::ExtraTag {
                    tag: "<LSTag>".into(),
                    count: 1
                },
            ]
        );
    }

    /// 真缺陷（旧结论写反的那条）：`Tooltip` / `Type` 的值是**查表 key**，不是显示文本。
    ///
    /// 证据：`Tooltip="VENOMOUS_BARBS_CONDITION"` 包着的正文是 "Deadly Toxin"
    /// —— KEY 与玩家看到的字完全是两回事，翻了 KEY 游戏就查不到表。
    /// 旧实现（以及当时的 README / 注释）写着「属性值是玩家可见文本（Tooltip 之类），
    /// 允许翻译」，于是模型真去翻了、校验器放行、坏 key 静默写进 PAK。
    #[test]
    fn key_attribute_values_must_be_kept_verbatim() {
        // `Type` 的取值是闭集（Spell / Status / Passive），游戏据此决定链接样式
        let issues = check_fidelity(
            r#"<LSTag Type="Spell">Fireball</LSTag>"#,
            r#"<LSTag Type="法术">火球术</LSTag>"#,
        );
        assert_eq!(
            issues,
            vec![FidelityIssue::AttributeValueChanged {
                name: "Type".into(),
                value: "Spell".into(),
                count: 1
            }]
        );
        assert_eq!(issues[0].to_string(), r#"属性 Type="Spell" 的值被改写"#);
        assert_eq!(
            issues[0].hint(),
            r#"标签属性 Type="Spell" 是查表 key，必须逐字保留、不要翻译"#
        );

        // `Tooltip` 同理：`HitPoints` 是查表 key，包着的 "hit points" 才是显示文本
        assert_eq!(
            check_fidelity(
                r#"<LSTag Tooltip="HitPoints">hit points</LSTag>"#,
                r#"<LSTag Tooltip="生命值">生命值</LSTag>"#
            ),
            vec![FidelityIssue::AttributeValueChanged {
                name: "Tooltip".into(),
                value: "HitPoints".into(),
                count: 1
            }]
        );

        // 语料里真实出现的内部 ID 形态：纯大写 / 下划线 / 驼峰，全都要逐字保留
        for (source, target) in [
            (
                r#"<LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">Deadly Toxin</LSTag>"#,
                r#"<LSTag Type="Status" Tooltip="致命毒素">致命毒素</LSTag>"#,
            ),
            (
                r#"<LSTag Tooltip="Projectile_MagicStoneThrow">Throw Magic Stone</LSTag>"#,
                r#"<LSTag Tooltip="投掷魔法石">投掷魔法石</LSTag>"#,
            ),
            (
                r#"<LSTag Tooltip="CAUSE_FEARED">Frighten</LSTag>"#,
                r#"<LSTag Tooltip="恐惧">恐惧</LSTag>"#,
            ),
            (
                r#"<LSTag Type="Passive" Tooltip="ID_INSINUATION">Incapacitated</LSTag>"#,
                r#"<LSTag Type="被动" Tooltip="失能">失能</LSTag>"#,
            ),
        ] {
            assert!(
                !is_faithful(source, target),
                "key 型属性值被翻译必须报: {source:?} → {target:?}"
            );
        }

        // 重复的 key：只有一处被翻也要报出次数
        assert_eq!(
            check_fidelity(
                r#"<LSTag Tooltip="HitPoints">a</LSTag><LSTag Tooltip="HitPoints">b</LSTag>"#,
                r#"<LSTag Tooltip="生命值">甲</LSTag><LSTag Tooltip="HitPoints">乙</LSTag>"#
            ),
            vec![FidelityIssue::AttributeValueChanged {
                name: "Tooltip".into(),
                value: "HitPoints".into(),
                count: 1
            }]
        );
        assert_eq!(
            FidelityIssue::AttributeValueChanged {
                name: "Tooltip".into(),
                value: "HitPoints".into(),
                count: 2
            }
            .to_string(),
            r#"属性 Tooltip="HitPoints" 的值被改写 2 处"#
        );

        // 值逐字保留、只翻正文与换语序 → 保真（这才是正确的译文）
        assert!(is_faithful(
            r#"Inflicts <LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">Deadly Toxin</LSTag> for 2 turns."#,
            r#"使其陷入<LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">致命毒素</LSTag>状态，持续 2 回合。"#
        ));
    }

    /// 刻意的盲点（防误报）：属性值是**含空格的自然语言**时不要求逐字一致。
    ///
    /// 两道闸门同时成立才收口：属性名在 [`KEY_ATTRIBUTES`] 里，且原文的值是
    /// **标识符形态**（[`is_identifier_shaped`]）。语料里 `Tooltip` 的 279 个取值
    /// 一个空格都没有，所以 `Tooltip="a natural sentence"` 不属于已知的 key 用法
    /// —— 万一某个 MOD 真把属性值当显示文本用，照旧放行。
    ///
    /// 代价：非标识符形态的 key 被翻会漏报。但误报要把**正确译文**整条退回英文，
    /// 代价更高（模块开头「宁可漏报，不可误报」）。
    #[test]
    fn natural_language_attribute_values_are_still_free() {
        for (source, target) in [
            (
                r#"<LSTag Tooltip="a natural sentence">a natural sentence</LSTag>"#,
                r#"<LSTag Tooltip="一句自然语言">一句自然语言</LSTag>"#,
            ),
            (
                r#"<LSTag Tooltip="Deals {1} damage">Fireball</LSTag>"#,
                r#"<LSTag Tooltip="造成 {1} 点伤害">火球术</LSTag>"#,
            ),
            (
                r#"<LSTag Tooltip="a > b">x</LSTag>"#,
                r#"<LSTag Tooltip="甲 > 乙">x</LSTag>"#,
            ),
            // 空值 / 带连字符：都不是标识符形态
            (
                r#"<LSTag Tooltip="">x</LSTag>"#,
                r#"<LSTag Tooltip="空">x</LSTag>"#,
            ),
            (
                r#"<LSTag Tooltip="Hit-Points">x</LSTag>"#,
                r#"<LSTag Tooltip="生命值">x</LSTag>"#,
            ),
            // 非 key 型属性（`Tag` / `color`）的值一律自由
            (
                r#"<LSTag Tag="Fire">x</LSTag>"#,
                r#"<LSTag Tag="火">x</LSTag>"#,
            ),
            (
                r#"<font color="red">x</font>"#,
                r#"<font color="红">x</font>"#,
            ),
        ] {
            assert!(
                is_faithful(source, target),
                "非标识符形态的属性值被改写不该报: {source:?} → {target:?}，问题: {:?}",
                check_fidelity(source, target)
            );
        }
    }

    /// 属性名对不上时**不叠加**「值被改写」：那种情况值其实没被动过，报它是错的。
    ///
    /// 根因由「缺失 / 多出标签」两条指出。判据里那道 `tag_names_consistent` 闸门
    /// 就是为此：`Type` → `类型` 时若还报 `属性 Type="Spell" 的值被改写`，
    /// 纠错提示会把模型引向错误的方向（它并没有改值）。
    #[test]
    fn mangled_attribute_names_do_not_also_report_value_changes() {
        assert_eq!(
            check_fidelity(
                r#"<LSTag Type="Spell">Fireball</LSTag>"#,
                r#"<LSTag 类型="法术">火球术</LSTag>"#
            ),
            vec![
                FidelityIssue::MissingTag {
                    tag: "<LSTag Type>".into(),
                    count: 1
                },
                FidelityIssue::ExtraTag {
                    tag: "<LSTag 类型>".into(),
                    count: 1
                },
            ],
            "属性名对不上时只报标签名，不报「值被改写」"
        );
        // 属性名修回来、值仍然被翻 → 报（下一轮就能拿到正确提示）
        assert_eq!(
            check_fidelity(
                r#"<LSTag Type="Spell">Fireball</LSTag>"#,
                r#"<LSTag Type="法术">火球术</LSTag>"#
            )
            .len(),
            1
        );
    }

    /// 属性名是结构签名的一部分：改名、大小写、增删都要报（顺序不算）。
    #[test]
    fn attribute_names_are_part_of_the_signature() {
        let source = r#"<LSTag Type="Spell" Tooltip="Deals {1} damage">Fireball</LSTag>"#;

        // 属性名被翻译（模型把标记也翻了）
        assert!(!is_faithful(source, &source.replace("Type=", "类型=")));
        // 属性名拼错
        assert!(!is_faithful(source, &source.replace("Type=", "Typ=")));
        // 属性名大小写变化：XML 属性名大小写敏感，游戏不认
        assert!(!is_faithful(source, &source.replace("Type=", "type=")));
        // 少一个属性
        assert!(!is_faithful(
            source,
            &source.replace(r#"Type="Spell" "#, "")
        ));
        // 多一个属性
        assert!(!is_faithful(
            source,
            &source.replace(r#"Tooltip="#, r#"Extra="x" Tooltip="#)
        ));

        // 属性顺序调换：不参与比较 → 保真（key 型属性的值逐字保留）
        assert!(is_faithful(
            r#"<LSTag Type="Spell" Tooltip="Fireball">Fireball</LSTag>"#,
            r#"<LSTag Tooltip="Fireball" Type="Spell">火球术</LSTag>"#
        ));
        // 多余空白 / 单引号：保真（key 型属性值逐字保留时，引号风格与空白不计）
        assert!(is_faithful(
            r#"<LSTag Type="Spell" Tooltip="Fireball">Fireball</LSTag>"#,
            "<LSTag   Type='Spell'\tTooltip='Fireball' >火球术</LSTag>"
        ));
        // 同上，但值是非 key 型属性：值本身被改写也保真
        assert!(is_faithful(
            r#"<font color="red">红</font>"#,
            "<font\tcolor='深红' >红字</font>"
        ));
        // 属性值里带 `>` / `/` 也不能让扫描错位（合法 XML，属性值内的 `>` 不是标签结束）
        assert!(is_faithful(
            r#"<LSTag Tooltip="Deals > 10 / hit">Fireball</LSTag>"#,
            r#"<LSTag Tooltip="造成 > 10 / 次伤害">火球术</LSTag>"#
        ));
        // 自闭合写法：属性名一致就保真
        assert!(is_faithful(
            r#"<LSTag Type="Spell"/>"#,
            r#"<LSTag Type='Spell' />"#
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
            r#"<LSTag Type="Spell">火球</LSTag>"#
        ));
        // 但属性名丢了仍然要报：写回写的是模型输出的这段标签文本，
        // `<LSTag>` 落进 PAK 之后游戏就读不到 `Type` 了。
        assert!(!is_faithful(
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

    /// 标签**顺序不参与比对**，只有**交叉嵌套**才报。
    ///
    /// 顺序自由的依据是真实语料：中英语序不同，标签跟着各自包住的正文换位是常态。
    /// `Inflicts <A>Deadly Toxin</A> ... fails <B>Saving Throw</B>` 译成中文后
    /// 条件从句提前，两个标签必然对调 —— 旧实现把这种正确译文判成
    /// 「标签顺序与原文不一致」，然后退回英文原文。
    #[test]
    fn tag_order_is_free_but_crossed_nesting_is_not() {
        // 兄弟标签换位：合法
        assert_eq!(
            check_fidelity("<b>x</b><i>y</i>", "<i>y</i><b>x</b>"),
            vec![]
        );
        // 嵌套顺序对调：仍然合法（两者都合法嵌套）
        assert_eq!(check_fidelity("<b><i>x</i></b>", "<i><b>x</b></i>"), vec![]);
        assert_eq!(check_fidelity("<b><i>x</i></b>", "<b><i>x</i></b>"), vec![]);

        // 交叉嵌套：非法 XML，必须报
        assert_eq!(
            check_fidelity("<LSTag><b>x</b></LSTag>", "<LSTag><b>x</LSTag></b>"),
            vec![FidelityIssue::TagNestingBroken]
        );
        assert_eq!(
            FidelityIssue::TagNestingBroken.to_string(),
            "标签开闭不配对，嵌套不合法"
        );
        assert_eq!(
            FidelityIssue::TagNestingBroken.hint(),
            "标签开闭没有正确配对（有落单的闭合标签，或不同标签名之间交叉嵌套），\
             请让每个开始标签与它对应的闭合标签成对嵌套"
        );

        // 同名标签之间**不存在**「交叉」：`<b>x<b></b>y</b>` 就是合法内层嵌套，
        // `</b>` 的语义是「关掉最内层那个」。所以语料里只有 LSTag 时，
        // 这条检查实际抓到的是**落单的闭合标签**（栈下溢）。
        // 注意必须用白名单标签：`<a>` 会被当成普通文本，根本进不了签名。
        assert!(is_well_nested(&Signature::of("<b>x<b></b>y</b>").tags));
        assert!(!is_well_nested(&Signature::of("x</b><b>").tags));

        // 闭合标签落单：**多重集相等**（各一个 `<b>` / `</b>`），所以缺失/多余都不报，
        // 只有嵌套检查能抓到。
        assert_eq!(
            check_fidelity("<b>x</b>", "x</b><b>"),
            vec![FidelityIssue::TagNestingBroken]
        );
        // 对照：闭标签直接消失了（多重集不等）→ 由缺失方向报，不必叠加嵌套告警
        assert_eq!(
            check_fidelity("<b>x</b>y", "y</b>x"),
            vec![FidelityIssue::MissingTag {
                tag: "<b>".into(),
                count: 1
            }]
        );
    }

    /// 真实场景：条件从句提前导致两个 `LSTag` 对调 —— 这是**正确译文**，不许报错。
    #[test]
    fn reordered_lstag_across_clause_boundary_is_faithful() {
        let source = r#"Inflicts <LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">Deadly Toxin</LSTag> for 2 turns if the target fails a DC 13 Constitution <LSTag Tooltip="SavingThrow">Saving Throw</LSTag>."#;
        let target = r#"若目标未通过 DC 13 体质<LSTag Tooltip="SavingThrow">豁免检定</LSTag>，则使其陷入<LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">致命毒素</LSTag>状态，持续 2 回合。"#;
        assert!(
            is_faithful(source, target),
            "换位是语序导致的，不该报错: {:?}",
            check_fidelity(source, target)
        );
    }

    /// 原文自己就交叉嵌套时（MOD 本来就坏），不为难模型。
    #[test]
    fn broken_source_nesting_does_not_demand_a_fix() {
        // 用白名单标签（`LSTag` / `b`）：白名单之外的 `<a>` 会被当成普通文本，
        // 根本进不了签名，那样的用例通过的原因就不对了。
        let broken = "<LSTag><b>x</LSTag></b>";
        assert!(
            !is_well_nested(&Signature::of(broken).tags),
            "用例前提：原文确实交叉嵌套"
        );
        assert!(is_well_nested(
            &Signature::of("<LSTag><b>x</b></LSTag>").tags
        ));

        // 原文坏、译文照抄：多重集一致，且不因「原文坏」而要求译文变好
        assert_eq!(check_fidelity(broken, broken), vec![]);
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

    /// 端到端证据：被模型改坏的标签属性名会**真的进 PAK**。
    ///
    /// 这条不是重复上面那条单测 —— 它盯的是「为什么必须在这里拦」：
    /// `content_list::render` 拿 `entry.effective_text()`（= 模型输出）里的标签
    /// 逐字节写出去，原文的属性不会被恢复。所以校验漏掉属性名 = 坏标签静默落盘。
    #[test]
    fn mangled_attribute_names_reach_the_written_file_when_not_caught() {
        use crate::formats::content_list::render;
        use crate::types::TranslationEntry;

        let source = r#"<LSTag Type="Spell">Fireball</LSTag>"#;
        let mangled = r#"<LSTag 类型="Spell">火球术</LSTag>"#;
        // 校验先拦下来（引擎会重试 / 标 error，error 条目不写回）
        assert!(
            !is_faithful(source, mangled),
            "属性名被改坏必须判不保真，否则下面的坏标签会落盘"
        );

        let mut entry = TranslationEntry::new("L.xml", "h1", "1", source);
        entry.mark_translated(mangled);
        let out = render(&[entry], &[]).unwrap();
        assert!(
            out.contains(r#"<LSTag 类型="Spell">"#),
            "写回确实会照抄模型输出：{out}"
        );
        // `error` 条目退回原文，所以「校验失败 → 前端置 error」之后盘上是安全的
        let mut rejected = TranslationEntry::new("L.xml", "h1", "1", source);
        rejected.mark_translated(mangled);
        rejected.mark_error("结构校验未通过");
        let out = render(&[rejected], &[]).unwrap();
        assert!(
            out.contains(r#"<LSTag Type="Spell">Fireball</LSTag>"#),
            "被拒条目必须写回原文：{out}"
        );
    }

    #[test]
    fn empty_and_identical_texts_are_faithful() {
        assert!(is_faithful("", ""));
        assert!(is_faithful("", "译文"));
        assert!(is_faithful("Fireball", "火球术"));
    }
}
