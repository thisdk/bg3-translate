//! 真实语料回归 + 变异测试台：结构保真层（`translation::fidelity`）的防线。
//!
//! 语料是 `samples/english.xml` —— 1971 条**真实** BG3 `contentList` 条目
//! （官方文本风格：`<LSTag Tooltip="...">`、`<br>`、`[1]` 占位符），**只读**，
//! 测试只用 `formats::read_entries_from_path` 读它。
//!
//! 为什么需要这个文件：仓库原有的「真实样本」是个 41 条的捏脸 MOD，`{`、`[`、`<`
//! 一个都没有，结构校验这层从没被真实数据验证过。这里补上两组断言：
//!
//! **(A) 零误报** —— 每条语料都构造若干种**合法译文变体**，全部必须 `is_faithful`：
//! 自反、标记逐字节照抄只换正文、尖括号重新转义、两个 `LSTag` 块整体换位
//! （真实语料里最常见的语序调整，288 条）、占位符周围空格增删。
//!
//! **(B) 零漏报** —— 对含标记的条目逐条施加变异（丢标签块 / 改属性名 / 丢改增占位符 /
//! `[1]`→`{1}` / 占位符粘连 / 丢 `<br>` / 交叉嵌套），每种都必须 `!is_faithful`，
//! 而且 `check_fidelity` 报出的 **issue 类型要对得上**（不是只看 `false`）。
//!
//! ## 两条实现纪律
//!
//! 1. **朴素扫描器自己写**（[`scan_tags`] / [`scan_placeholders`]），刻意不复用
//!    `fidelity` 的内部扫描逻辑：语料统计、合法译文构造、变异构造全部走这份实现，
//!    fidelity 出问题时两边不会一起错 —— 这才叫互相验证。
//! 2. **样本缺失必须响亮失败**，不静默跳过：跳过会让这 9000+ 个用例静默变空，
//!    而 `cargo test` 依然全绿（仓库既有约定见 `tests/real_mod_sample.rs`）。
//!
//! ## 用例矩阵（变体数 × 条数，每个 `#[test]` 里都有 `assert!(checked >= N)` 兜底）
//!
//! | 组 | 变体 | 条数 |
//! |----|------|------|
//! | A  | 自反 | 1971 |
//! | A  | 保留标记换正文 | 1971 |
//! | A  | 尖括号转义 | 1971 |
//! | A  | 两个 LSTag 块换位 | 288 |
//! | A  | 占位符空格增删 | 303 |
//! | B  | 丢掉一个 LSTag 块 | 552 |
//! | B  | 属性名改坏 | 349 |
//! | B  | 丢 / 改号 / 凭空加占位符 | 303 × 3 |
//! | B  | `[1]` → `{1}` | 303 |
//! | B  | 相邻占位符粘连 | 54 |
//! | B  | 丢掉 `<br>` | 129 |
//! | B  | 交叉嵌套 | 288 |
//!
//! 合计 9088 个用例。

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use bg3_translate_core::formats;
use bg3_translate_core::translation::{FidelityIssue, check_fidelity, is_faithful};
use bg3_translate_core::types::PakFileKind;

const SAMPLE_XML: &str = "samples/english.xml";
const SAMPLE_NAME: &str = "english.xml";

// ── 语料漂移哨兵：这些数字是 2026-xx 实测出来的快照 ──
//
// 语料是用户提供的固定输入，正常情况下不会变；一旦这些断言红了，说明语料被换掉或
// 改小了 —— 那么下面的「零误报 / 零漏报」结论就不能再当作对当前语料的结论，
// 必须先更新快照、复核新语料，再谈回归全绿。
const CORPUS_ENTRIES: usize = 1971;
/// 读入后所有条目的总字符数（平均约 80 字符）。
///
/// 原始 XML 里还有 26 条条目的首尾带空白（原始侧合计 158032），
/// `content_list::parse` 读进来时会 `trim()`，这里记的是**读入后**的口径。
const CORPUS_TOTAL_CHARS: usize = 158_006;
const CORPUS_MAX_CHARS: usize = 581;
/// 一条标签都没有的条目（可以有 `[1]` 占位符）。
const CORPUS_NO_TAG_ENTRIES: usize = 1401;
const CORPUS_LSTAG_OPEN: usize = 1103;
const CORPUS_LSTAG_CLOSE: usize = 1103;
/// 语料里 `<br>` 出现 410 次，**全部**是不自闭合的 `<br>`，一个 `<br/>` 都没有。
const CORPUS_BR: usize = 410;
const CORPUS_TOOLTIP_ATTRS: usize = 1103;
const CORPUS_TYPE_ATTRS: usize = 625;
const CORPUS_TOOLTIP_DISTINCT: usize = 279;
/// 含 `LSTag` 的条目（552 条里 111 条同时含 `<br>`）。
const CORPUS_ENTRIES_WITH_LSTAG: usize = 552;
/// 只含 `LSTag`、不含 `<br>` 的条目 —— 任务描述里写的「含 LSTag 441」实际是这一档。
const CORPUS_ENTRIES_LSTAG_ONLY: usize = 441;
/// 含 ≥2 个 `LSTag` 块的条目：换位变体与交叉嵌套变体的基数。
const CORPUS_ENTRIES_GE2_LSTAG: usize = 288;
const CORPUS_ENTRIES_WITH_BR: usize = 129;
/// 含**连续两个** `<br>`（零间隔 `<br><br>`）的条目。
const CORPUS_ENTRIES_TWO_ADJACENT_BR: usize = 127;
/// 带 `Type` 属性的条目（属性名改坏变体的基数）。
const CORPUS_ENTRIES_WITH_TYPE: usize = 349;
const CORPUS_PLACEHOLDER_ENTRIES: usize = 303;
const CORPUS_PLACEHOLDER_COUNT: usize = 417;
const CORPUS_PH_WITH_TAGS: usize = 183;
const CORPUS_PH_WITHOUT_TAGS: usize = 120;
/// 有两个**可分**（中间隔着非空、且不含 `<` 的文本）相邻占位符的条目：粘连变体的基数。
const CORPUS_SEPARABLE_PLACEHOLDER_PAIRS: usize = 54;

// ─────────────────────────────────────────────────────────────
// 语料读取
// ─────────────────────────────────────────────────────────────

/// 语料路径；缺失即 panic（附可操作提示），绝不静默跳过。
fn sample_path() -> PathBuf {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate 应位于 <repo>/crates/bg3-translate-core");
    sample_path_at(repo_root)
}

/// 在指定仓库根目录下找语料（抽出来是为了能测「缺失时必须炸」这条防线本身）。
fn sample_path_at(repo_root: &Path) -> PathBuf {
    let path = repo_root.join(SAMPLE_XML);
    assert!(
        path.is_file(),
        "真实语料缺失：{}。该文件随仓库提交（非 Git LFS），缺失说明 checkout 不完整。\
         注意 `git checkout -- samples/english.xml` **不管用** —— 在本仓库里它是一条\
         `pathspec did not match` 报错（该文件曾经未被跟踪）。正确做法：重新 clone，\
         或从提供方取回该文件放到 `samples/english.xml` 后重跑 \
         `cargo test -p bg3-translate-core --test corpus_regression`。",
        path.display()
    );
    path
}

/// 读入全部真实语料原文；样本缺失 / 解析失败 / 读出 0 条都**直接 panic**。
fn load_sources() -> Vec<String> {
    let path = sample_path();
    let entries = formats::read_entries_from_path(&path, SAMPLE_NAME, PakFileKind::LocalizationXml)
        .unwrap_or_else(|err| {
            panic!(
                "真实语料 {} 解析失败: {err}。它是只读输入，解析失败说明语料被改坏、\
                     或读取链路出现回归 —— 不要跳过这条测试。",
                path.display()
            )
        });
    let sources: Vec<String> = entries.into_iter().map(|entry| entry.source).collect();
    assert!(
        !sources.is_empty(),
        "真实语料 {} 读出来是 0 条：下面的用例会静默变空，必须当成失败。",
        path.display()
    );
    sources
}

/// 这条测试是「测试防线」的防线：语料缺失必须炸，而不是静默跳过。
///
/// 这个用例会往 stderr 打一行 panic 信息：那是被 `catch_unwind` 捕获的预期输出。
#[test]
fn missing_corpus_fails_loudly_instead_of_skipping() {
    let panic = std::panic::catch_unwind(|| sample_path_at(Path::new("/definitely/not/a/repo")))
        .expect_err("语料缺失必须 panic");
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default();
    assert!(message.contains("真实语料缺失"), "实际: {message}");
    // 提示必须可操作。曾经这里写的是 `git checkout -- samples/english.xml`，
    // 而那个文件当时还没入库，照做只会得到 `pathspec did not match` ——
    // 独立验证（docs/CORPUS-AUDIT.md D2）实测过，所以这条断言钉的是「重新 clone /
    // 取回文件」而不是那条无效命令。
    assert!(
        message.contains("重新 clone") && message.contains("git checkout"),
        "提示必须可操作、且说明那条 checkout 命令不管用: {message}"
    );
}

// ─────────────────────────────────────────────────────────────
// 朴素扫描器（自己写，不复用 fidelity 内部实现）
// ─────────────────────────────────────────────────────────────

/// 朴素扫描出来的一个标签。
#[derive(Debug, Clone)]
struct Tag {
    /// 标签在原文里的字节区间 `start..end`
    start: usize,
    end: usize,
    /// 标签名（不含 `/`）
    name: String,
    /// 是否结束标签 `</x>`
    closing: bool,
    /// 是否自闭合 `<x/>`（`<br>` 这类空元素不算）
    self_closing: bool,
    /// 属性（按出现顺序；无值属性 `value` 为空串）
    attrs: Vec<Attr>,
}

/// 标签属性。
#[derive(Debug, Clone)]
struct Attr {
    name: String,
    value: String,
    /// 值是否带引号（用来验证「语料里没有无引号属性值」）
    quoted: bool,
}

/// 朴素占位符：`[1]` / `{name}` 的字节区间与原始 token。
#[derive(Debug, Clone)]
struct Placeholder {
    start: usize,
    end: usize,
    token: String,
}

/// 一段字节区间（LSTag 块用）。
#[derive(Debug, Clone, Copy)]
struct Span {
    start: usize,
    end: usize,
}

/// 朴素标签扫描：只认 `<name ...>` / `</name>` / `<name .../>` 这种形态良好的标签。
///
/// 刻意不复用 `fidelity` 的扫描器 —— 这里要的是「另一双眼睛」：语料统计、合法译文
/// 构造、变异构造全部走这份实现，fidelity 出问题时两边不会一起错。
fn scan_tags(text: &str) -> Vec<Tag> {
    let bytes = text.as_bytes();
    let mut tags = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] != b'<' {
            index += 1;
            continue;
        }
        match parse_tag(text, index) {
            Some(tag) => {
                index = tag.end;
                tags.push(tag);
            }
            None => index += 1,
        }
    }
    tags
}

/// 尝试把 `text[start..]` 开头解析成一个标签；形态不良好返回 `None`。
fn parse_tag(text: &str, start: usize) -> Option<Tag> {
    let bytes = text.as_bytes();
    let mut index = start + 1;
    let closing = *bytes.get(index)? == b'/';
    if closing {
        index += 1;
    }
    let name_start = index;
    while index < bytes.len() && bytes[index].is_ascii_alphanumeric() {
        index += 1;
    }
    if index == name_start {
        return None;
    }
    let name = text[name_start..index].to_string();

    let mut self_closing = false;
    let mut attrs = Vec::new();
    loop {
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if index >= bytes.len() {
            return None;
        }
        if bytes[index] == b'>' {
            index += 1;
            break;
        }
        if bytes[index] == b'/' {
            if bytes.get(index + 1) == Some(&b'>') {
                self_closing = true;
                index += 2;
                break;
            }
            return None;
        }
        let name_from = index;
        while index < bytes.len()
            && !matches!(bytes[index], b'=' | b'>' | b'/' | b'"' | b'\'')
            && !bytes[index].is_ascii_whitespace()
        {
            index += 1;
        }
        if index == name_from {
            return None;
        }
        let attr_name = text[name_from..index].to_string();

        let mut cursor = index;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() || bytes[cursor] != b'=' {
            // 无值属性（`disabled` 这种写法）
            attrs.push(Attr {
                name: attr_name,
                value: String::new(),
                quoted: false,
            });
            index = cursor;
            continue;
        }
        cursor += 1;
        while cursor < bytes.len() && bytes[cursor].is_ascii_whitespace() {
            cursor += 1;
        }
        if cursor >= bytes.len() {
            return None;
        }
        if bytes[cursor] == b'"' || bytes[cursor] == b'\'' {
            let quote = bytes[cursor];
            let value_from = cursor + 1;
            let value_len = bytes[value_from..].iter().position(|byte| *byte == quote)?;
            attrs.push(Attr {
                name: attr_name,
                value: text[value_from..value_from + value_len].to_string(),
                quoted: true,
            });
            index = value_from + value_len + 1;
        } else {
            let value_from = cursor;
            while cursor < bytes.len()
                && !bytes[cursor].is_ascii_whitespace()
                && bytes[cursor] != b'>'
            {
                cursor += 1;
            }
            if cursor == value_from {
                return None;
            }
            attrs.push(Attr {
                name: attr_name,
                value: text[value_from..cursor].to_string(),
                quoted: false,
            });
            index = cursor;
        }
    }
    Some(Tag {
        start,
        end: index,
        name,
        closing,
        self_closing,
        attrs,
    })
}

/// 朴素占位符扫描：只认 `[纯数字]`（语料里的形态）与 `{字母数字下划线}`。
///
/// 与 `fidelity` 的判定刻意保持「同形态、不同实现」：两边独立，才能互相验证。
fn scan_placeholders(text: &str) -> Vec<Placeholder> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut index = 0usize;
    while index < bytes.len() {
        let open = bytes[index];
        if open != b'[' && open != b'{' {
            index += 1;
            continue;
        }
        let closer = if open == b'[' { b']' } else { b'}' };
        let Some(offset) = bytes[index + 1..].iter().position(|byte| *byte == closer) else {
            index += 1;
            continue;
        };
        let end = index + 1 + offset;
        let inner = &text[index + 1..end];
        let digit_bracket =
            open == b'[' && !inner.is_empty() && inner.bytes().all(|byte| byte.is_ascii_digit());
        let brace = open == b'{'
            && !inner.is_empty()
            && inner
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_');
        if digit_bracket || brace {
            out.push(Placeholder {
                start: index,
                end: end + 1,
                token: text[index..end + 1].to_string(),
            });
            index = end + 1;
        } else {
            index += 1;
        }
    }
    out
}

/// 标记出 `text` 里所有落在标签内部的字节。
fn tag_coverage(text: &str) -> Vec<bool> {
    let mut covered = vec![false; text.len()];
    for tag in scan_tags(text) {
        for slot in covered.iter_mut().take(tag.end).skip(tag.start) {
            *slot = true;
        }
    }
    covered
}

/// 只取**标签外**（正文里）的占位符。
///
/// 语料里恰好一个「标签内部的占位符」都没有（见 [`corpus_shape_sentinel_placeholders`]），
/// 但这个过滤是必要的：属性值里的占位符属于属性值，动它就不是「正文结构」变异了。
fn placeholders_outside_tags(text: &str) -> Vec<Placeholder> {
    let covered = tag_coverage(text);
    scan_placeholders(text)
        .into_iter()
        .filter(|placeholder| !covered[placeholder.start])
        .collect()
}

/// 按出现顺序配对出全部 `<LSTag ...>...</LSTag>` 块（语料里 LSTag 从不嵌套）。
fn lstag_blocks(text: &str) -> Vec<Span> {
    let mut blocks = Vec::new();
    let mut open: Option<usize> = None;
    for tag in scan_tags(text) {
        if tag.name != "LSTag" || tag.self_closing {
            continue;
        }
        if tag.closing {
            if let Some(start) = open.take() {
                blocks.push(Span {
                    start,
                    end: tag.end,
                });
            }
        } else if open.is_none() {
            open = Some(tag.start);
        }
    }
    blocks
}

// ─────────────────────────────────────────────────────────────
// 断言辅助：失败时把原文 / 译文 / 问题一起打出来
// ─────────────────────────────────────────────────────────────

fn assert_faithful(label: &str, source: &str, target: &str) {
    if !is_faithful(source, target) {
        let issues = check_fidelity(source, target);
        panic!(
            "[{label}] 合法译文被判为不保真（误报），问题: {issues:?}\n\
             原文: {source:?}\n译文: {target:?}"
        );
    }
}

fn assert_not_faithful(label: &str, source: &str, target: &str) -> Vec<FidelityIssue> {
    let issues = check_fidelity(source, target);
    assert!(
        !issues.is_empty(),
        "[{label}] 变异必须被拦下，却判成了保真（漏报）\n原文: {source:?}\n变异: {target:?}"
    );
    issues
}

fn has_issue(issues: &[FidelityIssue], matches: impl Fn(&FidelityIssue) -> bool) -> bool {
    issues.iter().any(matches)
}

/// 用例计数兜底：语料被改小 / 变异构造静默跳过时，这条防线会变空，必须炸。
fn assert_enough(checked: usize, minimum: usize, label: &str) {
    assert!(
        checked >= minimum,
        "[{label}] 实际只跑了 {checked} 条，少于应有的 {minimum} 条：\
         语料被改小、或变异构造在静默跳过，这条防线已经变空。"
    );
}

// ─────────────────────────────────────────────────────────────
// 合法译文构造（组 A 用）
// ─────────────────────────────────────────────────────────────

/// 朴素「保留标记、翻译正文」：`<...>`、`[...]`、`{...}` 逐字节照抄，其余压成 `译`。
///
/// 这是最典型的模型输出形态（标签原样、正文换成中文）。刻意不复用任何生产代码。
fn translate_body_keep_markup(source: &str) -> String {
    let mut out = String::new();
    let mut rest = source;
    while let Some(ch) = rest.chars().next() {
        let closer = match ch {
            '<' => Some('>'),
            '[' => Some(']'),
            '{' => Some('}'),
            _ => None,
        };
        match closer {
            Some(closer) => match rest[ch.len_utf8()..].find(closer) {
                Some(offset) => {
                    let end = ch.len_utf8() + offset + closer.len_utf8();
                    out.push_str(&rest[..end]);
                    rest = &rest[end..];
                }
                None => {
                    out.push(ch);
                    rest = &rest[ch.len_utf8()..];
                }
            },
            None => {
                // 一整段正文只留一个中文占位字符，避免译文过长
                if !out.ends_with('译') {
                    out.push('译');
                }
                rest = &rest[ch.len_utf8()..];
            }
        }
    }
    out
}

/// 把译文里的尖括号重新转义成实体（模型偶尔会这么干）。
///
/// 解析层本来就把 `&lt;` 还原过一次，所以这不改变语义，结构校验不该报。
fn escape_angle_brackets(text: &str) -> String {
    text.replace('<', "&lt;").replace('>', "&gt;")
}

/// 把前两个 `<LSTag ...>...</LSTag>` 块**整体**交换，其余字节逐字保留。
///
/// 这是真实语料里最常见的语序调整形态：块跟着自己包住的正文走。
fn swap_first_two_lstag_blocks(source: &str) -> Option<String> {
    let blocks = lstag_blocks(source);
    let first = blocks.first()?;
    let second = blocks.get(1)?;
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..first.start]);
    out.push_str(&source[second.start..second.end]);
    out.push_str(&source[first.end..second.start]);
    out.push_str(&source[first.start..first.end]);
    out.push_str(&source[second.end..]);
    Some(out)
}

/// 给每个正文占位符的前后各加一个空格（`[1]` → ` [1] `）。
///
/// 中文不用空格分词，占位符两侧的空格不是结构；加分隔只会更安全。
fn add_spaces_around_placeholders(source: &str) -> Option<String> {
    let placeholders = placeholders_outside_tags(source);
    if placeholders.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(source.len() + placeholders.len() * 2);
    let mut cursor = 0usize;
    for placeholder in &placeholders {
        out.push_str(&source[cursor..placeholder.start]);
        out.push(' ');
        out.push_str(&placeholder.token);
        out.push(' ');
        cursor = placeholder.end;
    }
    out.push_str(&source[cursor..]);
    Some(out)
}

// ─────────────────────────────────────────────────────────────
// 变异构造（组 B 用）
// ─────────────────────────────────────────────────────────────

/// 丢掉第一个完整的 `<LSTag ...>...</LSTag>` 块。
fn drop_first_lstag_block(source: &str) -> Option<String> {
    let blocks = lstag_blocks(source);
    let block = blocks.first()?;
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..block.start]);
    out.push_str(&source[block.end..]);
    Some(out)
}

/// 把第一个 `Type=` 属性**名**改成 `类型=`（模拟模型把标记也翻译了）。
///
/// 只改属性名，属性值一个字节不动 —— 「属性值被翻译」是合法的，不该混进这条用例。
fn rename_first_type_attribute(source: &str) -> Option<String> {
    for tag in scan_tags(source) {
        if tag.closing || !tag.attrs.iter().any(|attr| attr.name == "Type") {
            continue;
        }
        let raw = &source[tag.start..tag.end];
        for (offset, _) in raw.match_indices("Type=") {
            let at = tag.start + offset;
            let before_is_space = at > tag.start && source.as_bytes()[at - 1].is_ascii_whitespace();
            let value_follows = source[at + "Type=".len()..].starts_with('"');
            if !before_is_space || !value_follows {
                continue;
            }
            let mut out = String::with_capacity(source.len() + 4);
            out.push_str(&source[..at]);
            out.push_str("类型=");
            out.push_str(&source[at + "Type=".len()..]);
            return Some(out);
        }
    }
    None
}

/// 丢掉第一个正文占位符。
fn drop_first_placeholder(source: &str) -> Option<String> {
    let placeholder = placeholders_outside_tags(source).into_iter().next()?;
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..placeholder.start]);
    out.push_str(&source[placeholder.end..]);
    Some(out)
}

/// 把第一个正文占位符的编号改大 100000（`[1]` → `[100001]`）。
fn renumber_first_placeholder(source: &str) -> Option<String> {
    let placeholder = placeholders_outside_tags(source).into_iter().next()?;
    let digits: String = placeholder
        .token
        .chars()
        .filter(char::is_ascii_digit)
        .collect();
    let number: u64 = digits.parse().ok()?;
    let replacement = format!("[{}]", number + 100_000);
    assert!(
        !source.contains(&replacement),
        "构造出来的占位符编号不该和语料撞车: {replacement}"
    );
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..placeholder.start]);
    out.push_str(&replacement);
    out.push_str(&source[placeholder.end..]);
    Some(out)
}

/// 凭空加一个占位符。
fn invent_placeholder(source: &str) -> Option<String> {
    if placeholders_outside_tags(source).is_empty() {
        return None;
    }
    Some(format!("{source} [900001]"))
}

/// 把第一个 `[N]` 正文占位符写成 `{N}`（括号类型被模型改写）。
fn brace_first_placeholder(source: &str) -> Option<String> {
    let placeholder = placeholders_outside_tags(source).into_iter().next()?;
    if !placeholder.token.starts_with('[') {
        return None;
    }
    let inner = &placeholder.token[1..placeholder.token.len() - 1];
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..placeholder.start]);
    out.push('{');
    out.push_str(inner);
    out.push('}');
    out.push_str(&source[placeholder.end..]);
    Some(out)
}

/// 把两个相邻占位符之间**载重的分隔**整个删掉，制造粘连（`[1] [2]` → `[1][2]`）。
///
/// 只挑中间不含 `<` 的相邻对：删掉带标签的间隔会变成「丢标签」，issue 类型就不是粘连了。
fn glue_first_placeholder_pair(source: &str) -> Option<String> {
    let placeholders = placeholders_outside_tags(source);
    for pair in placeholders.windows(2) {
        let (left, right) = (&pair[0], &pair[1]);
        let gap = &source[left.end..right.start];
        if gap.is_empty() || gap.contains('<') {
            continue;
        }
        let mut out = String::with_capacity(source.len() - gap.len());
        out.push_str(&source[..left.end]);
        out.push_str(&source[right.start..]);
        return Some(out);
    }
    None
}

/// 丢掉第一个 `<br>`。
fn drop_first_br(source: &str) -> Option<String> {
    let tag = scan_tags(source)
        .into_iter()
        .find(|tag| tag.name == "br" && !tag.closing)?;
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..tag.start]);
    out.push_str(&source[tag.end..]);
    Some(out)
}

/// 把第二个 LSTag 块的 `</LSTag>` 挪到第一个 `<LSTag>` **之前**，制造开闭不配对：
/// `P<A>x</A> <B>y</B>S` → `P</B><A>x</A> <B>yS`。标签多重集与占位符一字不动，
/// 只有嵌套关系坏掉（闭标签出现在任何开标签之前）。
///
/// 为什么不用「教科书交叉」`<A>x<B>y</A></B>`：语料里所有标签都叫 `LSTag`、
/// 闭合标签不带属性，`</LSTag>` 在 XML 里的含义就是「关掉最内层的 LSTag」——
/// 那种写法**本身就是合法 XML**（等价于 `<A>x<B></B>y</A>`，`xml.etree` 能正常解析），
/// 名字栈根本无从分辨，也不是「会把文件弄成非法 XML」的结构。真正会让
/// `content_list::write_text_fragment` 降级成纯文本的，是这里的「闭标签先于开标签」。
fn unbalance_first_two_lstag_blocks(source: &str) -> Option<String> {
    let tags = scan_tags(source);
    let blocks = lstag_blocks(source);
    let first = blocks.first()?;
    let second = blocks.get(1)?;
    let second_close = tags
        .iter()
        .find(|tag| tag.name == "LSTag" && tag.closing && tag.end == second.end)?;
    let mut out = String::with_capacity(source.len());
    out.push_str(&source[..first.start]);
    out.push_str(&source[second_close.start..second_close.end]);
    out.push_str(&source[first.start..second_close.start]);
    out.push_str(&source[second_close.end..]);
    Some(out)
}

// ─────────────────────────────────────────────────────────────
// 组 A：零误报
// ─────────────────────────────────────────────────────────────

/// A1 自反：原文 vs 原文必须永远保真。
#[test]
fn reflexive_source_is_faithful() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        checked += 1;
        assert_faithful("自反", source, source);
    }
    assert_enough(checked, CORPUS_ENTRIES, "A1 自反");
}

/// A2 标记逐字节照抄、只把正文换成中文（最典型的模型输出）。
#[test]
fn markup_copied_verbatim_with_translated_body_is_faithful() {
    let sources = load_sources();
    let mut checked = 0usize;
    let mut changed = 0usize;
    for source in &sources {
        let translated = translate_body_keep_markup(source);
        if translated != *source {
            changed += 1;
        }
        checked += 1;
        assert_faithful("保留标记换正文", source, &translated);
    }
    assert_enough(checked, CORPUS_ENTRIES, "A2 保留标记换正文");
    assert_eq!(
        changed, CORPUS_ENTRIES,
        "每条语料都应该因为「正文被翻译」而真的发生变化，否则这个变体是空转的"
    );
}

/// A3 在第 2 条基础上把译文的尖括号重新转义（解析层本来会还原，不算结构变化）。
#[test]
fn escaped_angle_brackets_are_still_faithful() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let translated = translate_body_keep_markup(source);
        let escaped = escape_angle_brackets(&translated);
        checked += 1;
        assert_faithful("尖括号转义", source, &escaped);
    }
    assert_enough(checked, CORPUS_ENTRIES, "A3 尖括号转义");
}

/// A4 两个 `LSTag` 块整体换位（真实语料里最常见的语序调整）。
///
/// 标签顺序不参与结构比较，只有交叉嵌套才报 —— 这条把「换位必须保真」钉死。
#[test]
fn swapping_two_lstag_blocks_is_faithful() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(swapped) = swap_first_two_lstag_blocks(source) else {
            continue;
        };
        assert_ne!(swapped, *source, "换位变体必须真的改变了文本: {source:?}");
        checked += 1;
        assert_faithful("LSTag 块换位", source, &swapped);
    }
    assert_enough(checked, CORPUS_ENTRIES_GE2_LSTAG, "A4 LSTag 块换位");
}

/// A5 占位符周围空格增删（`[1]` → ` [1] `）。
#[test]
fn changing_placeholder_spacing_is_faithful() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(spaced) = add_spaces_around_placeholders(source) else {
            continue;
        };
        assert_ne!(spaced, *source, "空格变体必须真的改变了文本: {source:?}");
        checked += 1;
        assert_faithful("占位符空格", source, &spaced);
    }
    assert_enough(checked, CORPUS_PLACEHOLDER_ENTRIES, "A5 占位符空格");
}

// ─────────────────────────────────────────────────────────────
// 组 B：零漏报（变异测试）
// ─────────────────────────────────────────────────────────────

/// B1 丢掉一整对 `<LSTag ...>...</LSTag>`：必须报「开标签缺失」+「闭标签缺失」。
#[test]
fn dropping_an_lstag_block_reports_missing_tags() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(mutated) = drop_first_lstag_block(source) else {
            continue;
        };
        checked += 1;
        let issues = assert_not_faithful("丢掉 LSTag 块", source, &mutated);
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::MissingTag { tag, .. } if tag.starts_with("<LSTag")
            )),
            "应报开始标签缺失，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::MissingTag { tag, .. } if tag == "</LSTag>"
            )),
            "应报结束标签缺失，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
    }
    assert_enough(checked, CORPUS_ENTRIES_WITH_LSTAG, "B1 丢掉 LSTag 块");
}

/// B2 属性**名**被翻译 / 改坏（`Type=` → `类型=`）：必须报出标签签名对不上。
#[test]
fn renaming_a_tag_attribute_reports_missing_and_extra_tag() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(mutated) = rename_first_type_attribute(source) else {
            continue;
        };
        checked += 1;
        let issues = assert_not_faithful("属性名改坏", source, &mutated);
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::MissingTag { tag, .. } if tag.contains("Type")
            )),
            "应报原属性名的标签缺失，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::ExtraTag { tag, .. } if tag.contains("类型")
            )),
            "应报被改坏的属性名成为多余标签，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
    }
    assert_enough(checked, CORPUS_ENTRIES_WITH_TYPE, "B2 属性名改坏");
}

/// B3 丢一个占位符 / 改一个占位符编号 / 凭空加一个：三种都要报，且类型各不相同。
#[test]
fn placeholder_loss_change_or_invention_is_reported() {
    let sources = load_sources();
    let (mut dropped, mut renumbered, mut invented) = (0usize, 0usize, 0usize);
    for source in &sources {
        if let Some(mutated) = drop_first_placeholder(source) {
            dropped += 1;
            let issues = assert_not_faithful("丢掉占位符", source, &mutated);
            assert!(
                has_issue(&issues, |issue| matches!(
                    issue,
                    FidelityIssue::MissingPlaceholder { .. }
                )),
                "应报占位符缺失，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
            );
        }
        if let Some(mutated) = renumber_first_placeholder(source) {
            renumbered += 1;
            let issues = assert_not_faithful("改占位符编号", source, &mutated);
            assert!(
                has_issue(&issues, |issue| matches!(
                    issue,
                    FidelityIssue::MissingPlaceholder { .. }
                )),
                "应报原编号占位符缺失，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
            );
            assert!(
                has_issue(&issues, |issue| matches!(
                    issue,
                    FidelityIssue::ExtraPlaceholder { .. }
                )),
                "应报新编号占位符多出，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
            );
        }
        if let Some(mutated) = invent_placeholder(source) {
            invented += 1;
            let issues = assert_not_faithful("凭空加占位符", source, &mutated);
            assert!(
                has_issue(&issues, |issue| matches!(
                    issue,
                    FidelityIssue::ExtraPlaceholder { .. }
                )),
                "应报占位符多出，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
            );
        }
    }
    assert_enough(dropped, CORPUS_PLACEHOLDER_ENTRIES, "B3 丢掉占位符");
    assert_enough(renumbered, CORPUS_PLACEHOLDER_ENTRIES, "B3 改占位符编号");
    assert_enough(invented, CORPUS_PLACEHOLDER_ENTRIES, "B3 凭空加占位符");
}

/// B4 `[1]` 被写成 `{1}`：校验层必须报出来。
///
/// 修复（把 `{1}` 修回 `[1]`）在 retry 层，不在这里 —— 这里只要求「不许静默放行」，
/// 否则模型改写括号类型会被原样写进 PAK，游戏侧替换不了。
#[test]
fn swapped_bracket_style_is_reported() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(mutated) = brace_first_placeholder(source) else {
            continue;
        };
        checked += 1;
        let issues = assert_not_faithful("[N] 写成 {N}", source, &mutated);
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::MissingPlaceholder { token, .. } if token.starts_with('[')
            )),
            "应报方括号占位符缺失，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::ExtraPlaceholder { token, .. } if token.starts_with('{')
            )),
            "应报花括号占位符多出，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
    }
    assert_enough(checked, CORPUS_PLACEHOLDER_ENTRIES, "B4 [N] → {N}");
}

/// B5 两个相邻占位符粘在一起：多重集一致，只有粘连检查能报 —— 必须报对类型。
#[test]
fn glued_placeholders_are_reported() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(mutated) = glue_first_placeholder_pair(source) else {
            continue;
        };
        checked += 1;
        let issues = assert_not_faithful("占位符粘连", source, &mutated);
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::GluedPlaceholders { .. }
            )),
            "应报占位符粘连，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
    }
    assert_enough(checked, CORPUS_SEPARABLE_PLACEHOLDER_PAIRS, "B5 占位符粘连");
}

/// B6 丢掉 `<br>`：必须报标签缺失。
#[test]
fn dropping_a_br_reports_missing_tag() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(mutated) = drop_first_br(source) else {
            continue;
        };
        checked += 1;
        let issues = assert_not_faithful("丢掉 <br>", source, &mutated);
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::MissingTag { tag, .. } if tag.starts_with("<br")
            )),
            "应报 <br> 缺失，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
    }
    assert_enough(checked, CORPUS_ENTRIES_WITH_BR, "B6 丢掉 <br>");
}

/// B7 把 `</LSTag>` 挪到另一个 `<LSTag>` 之前，制造开闭不配对：必须报嵌套坏掉。
///
/// 语料里所有标签同名（`LSTag`），所以「可检测的不配对」只有一种形态：
/// 闭标签出现在任何开标签之前（栈下溢）。构造与理由见
/// [`unbalance_first_two_lstag_blocks`]。
#[test]
fn cross_nested_tags_are_reported() {
    let sources = load_sources();
    let mut checked = 0usize;
    for source in &sources {
        let Some(mutated) = unbalance_first_two_lstag_blocks(source) else {
            continue;
        };
        assert_ne!(
            mutated, *source,
            "交叉嵌套变体必须真的改变了文本: {source:?}"
        );
        checked += 1;
        let issues = assert_not_faithful("交叉嵌套", source, &mutated);
        assert!(
            has_issue(&issues, |issue| matches!(
                issue,
                FidelityIssue::TagNestingBroken
            )),
            "应报标签交叉嵌套，实际 {issues:?}\n原文: {source:?}\n变异: {mutated:?}"
        );
    }
    assert_enough(checked, CORPUS_ENTRIES_GE2_LSTAG, "B7 交叉嵌套");
}

// ─────────────────────────────────────────────────────────────
// 语料漂移哨兵
// ─────────────────────────────────────────────────────────────

/// 一边统计语料形态，一边喂给三条哨兵断言。
#[derive(Default)]
struct CorpusStats {
    entries: usize,
    empty: usize,
    total_chars: usize,
    max_chars: usize,
    no_tag_entries: usize,
    tag_names: BTreeMap<String, usize>,
    lstag_open: usize,
    lstag_close: usize,
    lstag_blocks: usize,
    br_plain: usize,
    br_self_closing: usize,
    attr_names: BTreeMap<String, usize>,
    unquoted_attrs: usize,
    tags_with_single_quote: usize,
    type_values: BTreeMap<String, usize>,
    tooltip_values: BTreeSet<String>,
    max_lstag_depth: usize,
    cross_nested_entries: usize,
    bare_angles: usize,
    ampersands: usize,
    control_chars: usize,
    entries_with_lstag: usize,
    entries_lstag_only: usize,
    entries_ge2_lstag: usize,
    entries_with_br: usize,
    entries_two_adjacent_br: usize,
    entries_with_type_attr: usize,
    placeholders: usize,
    placeholders_inside_tags: usize,
    brace_placeholders: usize,
    placeholder_entries: usize,
    ph_entries_with_tags: usize,
    ph_entries_without_tags: usize,
    separable_placeholder_pairs: usize,
    non_digit_placeholders: usize,
}

fn corpus_stats(sources: &[String]) -> CorpusStats {
    let mut stats = CorpusStats {
        entries: sources.len(),
        ..CorpusStats::default()
    };
    for source in sources {
        if source.trim().is_empty() {
            stats.empty += 1;
        }
        stats.total_chars += source.chars().count();
        stats.max_chars = stats.max_chars.max(source.chars().count());

        let tags = scan_tags(source);
        if tags.is_empty() {
            stats.no_tag_entries += 1;
        }
        for tag in &tags {
            let key = format!("{}{}", if tag.closing { "/" } else { "" }, tag.name);
            *stats.tag_names.entry(key).or_default() += 1;
            match (tag.name.as_str(), tag.closing) {
                ("LSTag", false) => stats.lstag_open += 1,
                ("LSTag", true) => stats.lstag_close += 1,
                ("br", false) if tag.self_closing => stats.br_self_closing += 1,
                ("br", false) => stats.br_plain += 1,
                _ => {}
            }
            for attr in &tag.attrs {
                *stats.attr_names.entry(attr.name.clone()).or_default() += 1;
                if !attr.quoted {
                    stats.unquoted_attrs += 1;
                }
                match attr.name.as_str() {
                    "Type" => *stats.type_values.entry(attr.value.clone()).or_default() += 1,
                    "Tooltip" => {
                        stats.tooltip_values.insert(attr.value.clone());
                    }
                    _ => {}
                }
            }
            if source[tag.start..tag.end].contains('\'') {
                stats.tags_with_single_quote += 1;
            }
        }

        let has_lstag = tags.iter().any(|tag| tag.name == "LSTag");
        let has_br = tags.iter().any(|tag| tag.name == "br");
        if has_lstag {
            stats.entries_with_lstag += 1;
            if !has_br {
                stats.entries_lstag_only += 1;
            }
        }
        if has_br {
            stats.entries_with_br += 1;
        }
        for pair in tags.windows(2) {
            if pair[0].name == "br" && pair[1].name == "br" && pair[0].end == pair[1].start {
                stats.entries_two_adjacent_br += 1;
                break;
            }
        }
        if tags
            .iter()
            .any(|tag| !tag.closing && tag.attrs.iter().any(|attr| attr.name == "Type"))
        {
            stats.entries_with_type_attr += 1;
        }

        let blocks = lstag_blocks(source);
        stats.lstag_blocks += blocks.len();
        if blocks.len() >= 2 {
            stats.entries_ge2_lstag += 1;
        }
        // LSTag 嵌套深度 + 交叉嵌套
        let mut depth = 0usize;
        let mut open_stack = 0usize;
        let mut crossed = false;
        for tag in tags.iter().filter(|tag| tag.name == "LSTag") {
            if tag.closing {
                if open_stack == 0 {
                    crossed = true;
                } else {
                    open_stack -= 1;
                }
                depth = depth.saturating_sub(1);
            } else {
                open_stack += 1;
                depth += 1;
                stats.max_lstag_depth = stats.max_lstag_depth.max(depth);
            }
        }
        if crossed || open_stack != 0 {
            stats.cross_nested_entries += 1;
        }

        // 裸尖括号 / 实体 / 控制字符：只在**标签之外**统计
        let covered = tag_coverage(source);
        for (index, byte) in source.bytes().enumerate() {
            if !covered[index] && matches!(byte, b'<' | b'>') {
                stats.bare_angles += 1;
            }
            if byte == b'&' {
                stats.ampersands += 1;
            }
            if byte < 0x20 && !matches!(byte, b'\n' | b'\t' | b'\r') {
                stats.control_chars += 1;
            }
        }

        let all_placeholders = scan_placeholders(source);
        let outside = placeholders_outside_tags(source);
        stats.placeholders += all_placeholders.len();
        stats.placeholders_inside_tags += all_placeholders.len() - outside.len();
        stats.brace_placeholders += all_placeholders
            .iter()
            .filter(|placeholder| placeholder.token.starts_with('{'))
            .count();
        stats.non_digit_placeholders += all_placeholders
            .iter()
            .filter(|placeholder| {
                let inner = &placeholder.token[1..placeholder.token.len() - 1];
                placeholder.token.starts_with('[')
                    && !inner.bytes().all(|byte| byte.is_ascii_digit())
            })
            .count();
        if !all_placeholders.is_empty() {
            stats.placeholder_entries += 1;
            if tags.is_empty() {
                stats.ph_entries_without_tags += 1;
            } else {
                stats.ph_entries_with_tags += 1;
            }
        }
        if outside.windows(2).any(|pair| {
            let gap = &source[pair[0].end..pair[1].start];
            !gap.is_empty() && !gap.contains('<')
        }) {
            stats.separable_placeholder_pairs += 1;
        }
    }
    stats
}

/// 哨兵 1：条数与文本长度分布。
#[test]
fn corpus_shape_sentinel_entries_and_text() {
    let sources = load_sources();
    let stats = corpus_stats(&sources);
    assert_eq!(
        stats.entries, CORPUS_ENTRIES,
        "语料条数变了：下面所有「全量回归」的结论都基于旧语料，先复核再更新快照"
    );
    assert_eq!(stats.empty, 0, "语料里不该有空文本");
    assert_eq!(
        stats.total_chars, CORPUS_TOTAL_CHARS,
        "语料总字符数变了（平均约 80 字符/条）"
    );
    assert_eq!(stats.max_chars, CORPUS_MAX_CHARS, "最长条目的字符数变了");
    assert_eq!(
        stats.no_tag_entries, CORPUS_NO_TAG_ENTRIES,
        "「纯文本（一条标签都没有）」的条目数变了"
    );
}

/// 哨兵 2：标签 / 属性形态。
#[test]
fn corpus_shape_sentinel_tags() {
    let sources = load_sources();
    let stats = corpus_stats(&sources);

    let expected_tags: BTreeMap<String, usize> = [
        ("LSTag".to_string(), CORPUS_LSTAG_OPEN),
        ("/LSTag".to_string(), CORPUS_LSTAG_CLOSE),
        ("br".to_string(), CORPUS_BR),
    ]
    .into_iter()
    .collect();
    assert_eq!(stats.tag_names, expected_tags, "语料里的标签种类/数量变了");
    assert_eq!(stats.lstag_open, CORPUS_LSTAG_OPEN, "LSTag 开标签数变了");
    assert_eq!(stats.lstag_close, CORPUS_LSTAG_CLOSE, "LSTag 闭标签数变了");
    assert_eq!(stats.lstag_blocks, CORPUS_LSTAG_OPEN, "LSTag 块数变了");
    assert_eq!(stats.br_plain, CORPUS_BR, "`<br>` 数量变了");
    assert_eq!(
        stats.br_self_closing, 0,
        "语料里从不写 `<br/>`；出现自闭合 br 说明语料换源了"
    );

    let expected_attrs: BTreeMap<String, usize> = [
        ("Tooltip".to_string(), CORPUS_TOOLTIP_ATTRS),
        ("Type".to_string(), CORPUS_TYPE_ATTRS),
    ]
    .into_iter()
    .collect();
    assert_eq!(stats.attr_names, expected_attrs, "语料里的属性名/数量变了");
    assert_eq!(stats.unquoted_attrs, 0, "语料里不该有无引号属性值");
    assert_eq!(stats.tags_with_single_quote, 0, "语料里不该有单引号属性");

    let expected_type_values: BTreeMap<String, usize> = [
        ("Spell".to_string(), 311),
        ("Status".to_string(), 259),
        ("Passive".to_string(), 55),
    ]
    .into_iter()
    .collect();
    assert_eq!(
        stats.type_values, expected_type_values,
        "`Type` 取值闭集变了（Spell 311 / Status 259 / Passive 55）"
    );
    assert_eq!(
        stats.tooltip_values.len(),
        CORPUS_TOOLTIP_DISTINCT,
        "`Tooltip` 的不同取值数变了"
    );
    assert!(
        stats
            .tooltip_values
            .iter()
            .all(|value| !value.contains(' ')),
        "`Tooltip` 取值里出现了空格，形如 `HitPoints` 的形态假设不再成立"
    );

    assert_eq!(
        stats.max_lstag_depth, 1,
        "LSTag 嵌套深度变了（原本从不嵌套）"
    );
    assert_eq!(
        stats.cross_nested_entries, 0,
        "原文里出现了交叉嵌套，语料被改坏了"
    );
    assert_eq!(stats.bare_angles, 0, "标签之外出现了裸 `<` / `>`");
    assert_eq!(stats.ampersands, 0, "原文里出现了 XML 实体");
    assert_eq!(stats.control_chars, 0, "原文里出现了控制字符");

    assert_eq!(
        stats.entries_with_lstag, CORPUS_ENTRIES_WITH_LSTAG,
        "含 LSTag 的条目数变了"
    );
    assert_eq!(
        stats.entries_lstag_only, CORPUS_ENTRIES_LSTAG_ONLY,
        "「只含 LSTag、不含 br」的条目数变了"
    );
    assert_eq!(
        stats.entries_ge2_lstag, CORPUS_ENTRIES_GE2_LSTAG,
        "含 ≥2 个 LSTag 块的条目数变了（换位/交叉嵌套变体的基数）"
    );
    assert_eq!(
        stats.entries_with_br, CORPUS_ENTRIES_WITH_BR,
        "含 br 的条目数变了"
    );
    assert_eq!(
        stats.entries_two_adjacent_br, CORPUS_ENTRIES_TWO_ADJACENT_BR,
        "含连续两个 `<br>` 的条目数变了"
    );
    assert_eq!(
        stats.entries_with_type_attr, CORPUS_ENTRIES_WITH_TYPE,
        "带 Type 属性的条目数变了（属性名改坏变体的基数）"
    );
}

/// 哨兵 3：占位符形态。
#[test]
fn corpus_shape_sentinel_placeholders() {
    let sources = load_sources();
    let stats = corpus_stats(&sources);
    assert_eq!(
        stats.placeholder_entries, CORPUS_PLACEHOLDER_ENTRIES,
        "含占位符的条目数变了"
    );
    assert_eq!(
        stats.placeholders, CORPUS_PLACEHOLDER_COUNT,
        "占位符总处数变了"
    );
    assert_eq!(
        stats.placeholders_inside_tags, 0,
        "出现了「标签属性值里的占位符」，正文/属性值的区分假设不再成立"
    );
    assert_eq!(
        stats.brace_placeholders, 0,
        "语料里原本没有花括号占位符 `{{...}}`"
    );
    assert_eq!(
        stats.non_digit_placeholders, 0,
        "方括号占位符原本全是纯数字"
    );
    assert_eq!(
        stats.ph_entries_with_tags, CORPUS_PH_WITH_TAGS,
        "「占位符 + 标签」的条目数变了"
    );
    assert_eq!(
        stats.ph_entries_without_tags, CORPUS_PH_WITHOUT_TAGS,
        "「占位符、无标签」的条目数变了"
    );
    assert_eq!(
        stats.separable_placeholder_pairs, CORPUS_SEPARABLE_PLACEHOLDER_PAIRS,
        "可制造粘连的相邻占位符对数变了（粘连变体的基数）"
    );
}
