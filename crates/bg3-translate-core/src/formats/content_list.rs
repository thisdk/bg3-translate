//! `<contentList>` 本地化 XML 的读写。
//!
//! 格式：
//! ```xml
//! <?xml version="1.0" encoding="utf-8"?>
//! <contentList>
//!   <content contentuid="h1234" version="1">Cast &lt;LSTag&gt;Fireball&lt;/LSTag&gt; for {1} damage</content>
//! </contentList>
//! ```
//!
//! 关键点：`<content>` 内部可能嵌套富文本标签，必须原样保留。
//! 因此进入 `<content>` 后，我们把内部所有事件重新拼成"带标签的逻辑文本"，
//! 而不是做结构化反序列化。

use std::borrow::Cow;
use std::path::Path;

use quick_xml::events::{BytesDecl, BytesEnd, BytesRef, BytesStart, BytesText, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::config::write_atomic;
use crate::error::{AppError, Result};
use crate::types::TranslationEntry;

/// UTF-8 BOM。
const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// 允许以原始 XML 形式保留在译文里的 BG3 富文本标签白名单。
///
/// 白名单是必要的：游戏文本里真实的 `<` 会以 `&lt;` 形式存在，
/// 解析后与标签无法区分；只放行已知标签可以避免把文本内容误写成标签。
const INLINE_TAGS: &[&str] = &["LSTag", "font", "i", "b", "u", "br", "span", "em", "strong"];

/// 空元素：写成 `<br>` 或 `<br/>` 都不需要闭合标签，配对检查时要跳过。
const VOID_TAGS: &[&str] = &["br"];

/// 从磁盘读取并解析。
///
/// XML 格式错误时**返回错误而不是返回残缺的条目列表**：写回是「用条目重建整个
/// 文档」，如果带着残缺列表去写，畸形标签之后的所有原文都会被永久删掉。
/// 宁可让用户看到一条明确的错误，也不能悄悄产出内容缺失的 PAK。
pub fn read(path: &Path, file_name: &str) -> Result<Vec<TranslationEntry>> {
    let bytes = std::fs::read(path)?;
    let xml = decode_utf8(&bytes, "本地化 XML")?;
    let parsed = parse(&xml, file_name);
    if let Some(error) = parsed.error {
        return Err(AppError::xml(format!(
            "{file_name} 解析失败，已中止读取以避免写回时丢失条目: {error}"
        )));
    }
    Ok(parsed.entries)
}

/// 解析结果：条目 + 根元素属性（写回时保留）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedContentList {
    pub entries: Vec<TranslationEntry>,
    pub root_attributes: Vec<(String, String)>,
    /// 解析中断的原因。`Some` 表示文档没有完整读下来，
    /// 调用方**不应该**拿这份残缺结果去覆盖原文件。
    pub error: Option<String>,
}

/// 解析 contentList XML，提取所有 `<content>` 条目。
pub fn parse(xml: &str, file_name: &str) -> ParsedContentList {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);

    let mut parsed = ParsedContentList::default();
    let mut buf = Vec::new();
    // 当前 `<content>` 的属性：(contentuid, version)
    let mut pending: Option<(String, String)> = None;
    let mut raw = String::new();
    // `<content>` 内部若又出现 `<content>`（极少见，但真实文件里存在），
    // 需要按深度配对，不能把内层的 `</content>` 当成外层的结束标签。
    let mut nested_depth = 0usize;

    loop {
        buf.clear();
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match e.name().as_ref() {
                "contentList" if pending.is_none() => {
                    parsed.root_attributes = collect_attributes(&e);
                }
                "content" if pending.is_none() => {
                    pending = Some(content_identity(&e));
                    raw.clear();
                    nested_depth = 0;
                }
                // content 内部的子标签：原样记录
                _ if pending.is_some() => {
                    if e.name().as_ref() == "content" {
                        nested_depth += 1;
                    }
                    raw.push_str(&raw_start_tag(&e));
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "content" if pending.is_none() => {
                    let (contentuid, version) = content_identity(&e);
                    parsed
                        .entries
                        .push(TranslationEntry::new(file_name, contentuid, version, ""));
                }
                _ if pending.is_some() => raw.push_str(&raw_empty_tag(&e)),
                _ => {}
            },
            Ok(Event::End(e)) => {
                if pending.is_some() && e.name().as_ref() == "content" {
                    if nested_depth > 0 {
                        nested_depth -= 1;
                        raw.push_str(&raw_end_tag(&e));
                    } else {
                        if let Some((contentuid, version)) = pending.take() {
                            parsed.entries.push(TranslationEntry::new(
                                file_name,
                                contentuid,
                                version,
                                raw.trim(),
                            ));
                        }
                        raw.clear();
                    }
                } else if pending.is_some() {
                    raw.push_str(&raw_end_tag(&e));
                }
            }
            Ok(Event::Text(e)) if pending.is_some() => {
                raw.push_str(&e.xml10_content());
            }
            // 实体引用（&amp; / &#39; / …）在 quick-xml 0.38+ 是独立事件
            Ok(Event::GeneralRef(e)) if pending.is_some() => {
                raw.push_str(&resolve_reference(&e));
            }
            Ok(Event::CData(e)) if pending.is_some() => {
                raw.push_str(e.as_ref());
            }
            Ok(Event::Eof) => {
                // quick-xml 对「文档还没闭合就到头了」不报错，只在 Eof 处停下。
                // 这种截断必须自己发现，否则后面那些条目会静默消失。
                if pending.is_some() {
                    parsed.error =
                        Some("文档在 <content> 元素内部意外结束（文件可能被截断）".into());
                }
                break;
            }
            Err(err) => {
                log::warn!("{file_name} XML 解析中断: {err}");
                parsed.error = Some(err.to_string());
                break;
            }
            _ => {}
        }
    }

    parsed
}

/// 写回磁盘，保留 BOM 与根元素属性。
///
/// 磁盘上已有、但这次没提交的条目会被**原样保留**（见 [`merge_missing_entries`]）：
/// 写回是「用条目列表重建整个文档」，如果只提交了其中一部分，其余条目会被
/// 静默删掉——游戏里对应文本就变成原始 contentuid 句柄了。
pub fn write(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let (bom, root_attributes, on_disk) = match std::fs::read(path) {
        Ok(bytes) => {
            let bom = bytes.starts_with(BOM);
            let xml = decode_utf8(&bytes, "本地化 XML")?;
            let parsed = parse(&xml, "");
            (bom, parsed.root_attributes, parsed.entries)
        }
        Err(_) => (false, Vec::new(), Vec::new()),
    };

    let merged = merge_missing_entries(entries, &on_disk);
    let body = render(&merged, &root_attributes)?;

    let mut out = Vec::with_capacity(body.len() + 3);
    if bom {
        out.extend_from_slice(BOM);
    }
    out.extend_from_slice(body.as_bytes());
    write_atomic(path, &out)?;
    Ok(())
}

/// 把「磁盘上已有、本次没提交」的条目补回写回列表。
///
/// 真实触发路径：MOD 自带 `Localization/Chinese/x.xml`（里面可能有英文原文
/// 没有的 contentuid），前端按「英文 > 中文」优先级只提交英文那份的条目。
/// 直接按提交列表重建文档，那几个只在中文文件里存在的条目就被永久删掉了。
/// 保留它们没有任何代价：contentuid 不变、文本原样。
fn merge_missing_entries(
    submitted: &[TranslationEntry],
    on_disk: &[TranslationEntry],
) -> Vec<TranslationEntry> {
    if on_disk.is_empty() {
        return submitted.to_vec();
    }
    let known: std::collections::HashSet<&str> = submitted
        .iter()
        .map(|entry| entry.contentuid.as_str())
        .collect();
    let mut merged = submitted.to_vec();
    let before = merged.len();
    for entry in on_disk {
        if !known.contains(entry.contentuid.as_str()) {
            merged.push(entry.clone());
        }
    }
    let kept = merged.len() - before;
    if kept > 0 {
        log::warn!(
            "写回列表比磁盘上的条目少 {kept} 条（文件可能被多个语言版本共用），已原样保留这些条目"
        );
    }
    merged
}

/// 渲染 contentList XML 文本（不含 BOM）。
pub fn render(
    entries: &[TranslationEntry],
    root_attributes: &[(String, String)],
) -> Result<String> {
    let mut writer = Writer::new(Vec::new());

    write_event(
        &mut writer,
        Event::Decl(BytesDecl::new("1.0", Some("utf-8"), None)),
    )?;

    let mut root = BytesStart::new("contentList");
    for (key, value) in root_attributes {
        root.push_attribute((key.as_str(), value.as_str()));
    }
    write_event(&mut writer, Event::Start(root))?;

    for entry in entries {
        reject_illegal_identity(&entry.contentuid, "contentuid")?;
        reject_illegal_identity(&entry.version, "version")?;
        let mut start = BytesStart::new("content");
        start.push_attribute(("contentuid", entry.contentuid.as_str()));
        // 原文件没有 `version` 属性时**不要凭空补一个 `version=""`**：
        // 空版本号和「没有这个属性」对游戏不是一回事，而且会让「零译文写回」
        // 也改变文件内容。
        if !entry.version.is_empty() {
            start.push_attribute(("version", entry.version.as_str()));
        }
        write_event(&mut writer, Event::Start(start))?;
        let text = sanitize_xml_chars(entry.effective_text());
        if is_tag_balanced(&text) {
            write_text_fragment(&mut writer, &text)?;
        } else {
            // 模型偶尔会多吐一个 `</LSTag>` 或者漏掉闭合标签。原样写出去会让
            // 整个本地化文件变成非法 XML，游戏可能直接放弃解析这个文件——
            // 那样丢的是**整份译文**，代价远大于一条文本里多几个字面标签。
            // 所以这里退化成纯文本：XML 一定合法，问题在界面上肉眼可见。
            log::warn!(
                "条目 {} 的译文标签不配对，已按纯文本写入以避免产出非法 XML",
                entry.contentuid
            );
            write_event(&mut writer, Event::Text(BytesText::new(&text)))?;
        }
        write_event(&mut writer, Event::End(BytesEnd::new("content")))?;
    }

    write_event(&mut writer, Event::End(BytesEnd::new("contentList")))?;

    String::from_utf8(writer.into_inner())
        .map_err(|e| AppError::xml(format!("生成 XML 时出现非法 UTF-8: {e}")))
}

// ─────────────────────────────────────────────────────────────
// 内部工具
// ─────────────────────────────────────────────────────────────

fn write_event<W: std::io::Write>(writer: &mut Writer<W>, event: Event<'_>) -> Result<()> {
    writer
        .write_event(event)
        .map_err(|e| AppError::xml(format!("{e}")))
}

/// 去掉 UTF-8 BOM 并解码。
fn decode_utf8(bytes: &[u8], what: &str) -> Result<String> {
    let body = bytes.strip_prefix(BOM).unwrap_or(bytes);
    String::from_utf8(body.to_vec())
        .map_err(|e| AppError::xml(format!("{what} 不是合法 UTF-8: {e}")))
}

/// 取 attribute 的反转义值（含 EOL 归一化，等价于旧版 `unescape_value`）。
fn unescaped(attr: &quick_xml::events::attributes::Attribute<'_>) -> String {
    attr.normalized_value(XmlVersion::Implicit1_0)
        .unwrap_or_default()
        .into_owned()
}

fn collect_attributes(e: &BytesStart<'_>) -> Vec<(String, String)> {
    e.attributes()
        .flatten()
        .map(|attr| (attr.key.as_ref().to_string(), unescaped(&attr)))
        .collect()
}

fn content_identity(e: &BytesStart<'_>) -> (String, String) {
    let mut contentuid = String::new();
    let mut version = String::new();
    for attr in e.attributes().flatten() {
        match attr.key.as_ref() {
            "contentuid" => contentuid = unescaped(&attr),
            "version" => version = unescaped(&attr),
            _ => {}
        }
    }
    (contentuid, version)
}

/// 把实体引用还原成字符；未知实体原样保留（含 `&` 和 `;`）。
fn resolve_reference(reference: &BytesRef<'_>) -> String {
    if let Ok(Some(ch)) = reference.resolve_char_ref() {
        return ch.to_string();
    }
    match &**reference {
        "amp" => "&".into(),
        "lt" => "<".into(),
        "gt" => ">".into(),
        "quot" => "\"".into(),
        "apos" => "'".into(),
        name => format!("&{name};"),
    }
}

/// `<Tag attr="raw">`：属性值保留原始转义，避免二次转义/反转义损坏内容。
fn raw_start_tag(e: &BytesStart<'_>) -> String {
    let mut out = String::from("<");
    out.push_str(e.name().as_ref());
    for attr in e.attributes().flatten() {
        out.push(' ');
        out.push_str(attr.key.as_ref());
        out.push_str("=\"");
        out.push_str(attr.value.as_ref());
        out.push('"');
    }
    out.push('>');
    out
}

fn raw_end_tag(e: &BytesEnd<'_>) -> String {
    let mut out = String::from("</");
    out.push_str(e.name().as_ref());
    out.push('>');
    out
}

fn raw_empty_tag(e: &BytesStart<'_>) -> String {
    let mut out = raw_start_tag(e);
    out.pop();
    out.push_str("/>");
    out
}

/// 写入 `<content>` 内部：普通文本转义，白名单内的 BG3 富文本标签保留为真实标签。
///
/// 「是不是标签」由 [`scan_tag_end`] + [`is_allowed_inline_tag`] 共同判定，
/// 与 [`is_tag_balanced`] 用的是同一套判定，因此「配对检查通过」就等于
/// 「写出去的东西是合法 XML」。
fn write_text_fragment<W: std::io::Write>(writer: &mut Writer<W>, text: &str) -> Result<()> {
    // 未写出的普通文本从这里开始（含那些不是标签的 `<`）
    let mut plain_start = 0usize;
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find('<') {
        let start = cursor + offset;
        match scan_tag_end(text, start).filter(|end| is_allowed_inline_tag(&text[start..*end])) {
            Some(end) => {
                if plain_start < start {
                    write_event(
                        writer,
                        Event::Text(BytesText::new(&text[plain_start..start])),
                    )?;
                }
                // 白名单标签原样写出（属性值里的 `&`/`"` 都是原始转义形态）
                let tag = normalize_void_tag(&text[start..end]);
                writer
                    .get_mut()
                    .write_all(tag.as_bytes())
                    .map_err(|e| AppError::xml(format!("{e}")))?;
                cursor = end;
                plain_start = end;
            }
            // 不是标签：这个 `<` 是正文字符，继续往后找（后面的真标签仍要保留）
            None => cursor = start + 1,
        }
    }

    if plain_start < text.len() {
        write_event(writer, Event::Text(BytesText::new(&text[plain_start..])))?;
    }
    Ok(())
}

/// 判断一段文本里的富文本标签是否配对（开闭成对、自闭合不算）。
///
/// 只有通过这个检查的文本才会把标签当真实标签写出去；否则整条按纯文本转义，
/// 保证产物永远是合法 XML。
pub fn is_tag_balanced(text: &str) -> bool {
    let mut stack: Vec<String> = Vec::new();
    let mut cursor = 0usize;
    while let Some(offset) = text[cursor..].find('<') {
        let start = cursor + offset;
        match scan_tag_end(text, start) {
            Some(end) if is_allowed_inline_tag(&text[start..end]) => {
                let inner = text[start + 1..end - 1].trim();
                if inner.ends_with('/') {
                    // 自闭合，不入栈
                } else if let Some(name) = inner.strip_prefix('/') {
                    match stack.pop() {
                        Some(open) if open == name.trim() => {}
                        // 闭合标签对不上栈顶：不配对
                        _ => return false,
                    }
                } else {
                    let name = inner.split_whitespace().next().unwrap_or("");
                    if !VOID_TAGS.contains(&name) {
                        stack.push(name.to_string());
                    }
                }
                cursor = end;
            }
            // 不是标签：这个 `<` 是正文字符，从下一个字符继续找
            _ => cursor = start + 1,
        }
    }
    stack.is_empty()
}

/// 从 `text[start]`（必须是 `<`）开始扫描一个标签候选，返回 `>` **之后**的下标。
///
/// 属性值里的 `>` 不算标签结束：`<LSTag Tooltip="a > b">` 是合法 XML，
/// 朴素地取「第一个 `>`」会把它劈成 `<LSTag Tooltip="a >` + 文本 ` b">x`，
/// 写回后属性没闭合，整个文件变成非法 XML。扫描中途再遇到 `<` 说明这一段
/// 根本不是标签，直接放弃（返回 `None`）。
pub fn scan_tag_end(text: &str, start: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    if bytes.get(start) != Some(&b'<') {
        return None;
    }
    let mut quote: Option<u8> = None;
    let mut i = start + 1;
    while i < bytes.len() {
        let byte = bytes[i];
        match quote {
            Some(q) => {
                if byte == q {
                    quote = None;
                }
            }
            None => match byte {
                b'"' | b'\'' => quote = Some(byte),
                b'>' => return Some(i + 1),
                b'<' => return None,
                _ => {}
            },
        }
        i += 1;
    }
    None
}

/// 用 quick-xml 判断这段文本是不是**恰好一个**合法 XML 标签，是则返回标签名。
///
/// 这是「原样写出去」的唯一门槛：产物必须永远是合法 XML，所以宁可在这里多解析
/// 一次，也不能靠「`<` 后面第一个词是不是白名单名字」去猜——`< br >`
/// （源文件里是 `&lt; br &gt;`）就是被这么猜错的。
///
/// 两个坑：
/// - 结束标签单独解析时 quick-xml 会报「没有匹配的开始标签」，只能自己校验名字语法；
/// - 属性是**惰性解析**的，必须逐个过一遍 `attributes()`，否则
///   `<LSTag Tooltip="a" garbage>` 这种畸形开始标签会被当成合法标签原样写出去。
fn parse_single_tag(tag: &str) -> Option<String> {
    let body = tag.strip_prefix('<')?.strip_suffix('>')?;
    if let Some(name) = body.strip_prefix('/') {
        let name = name.trim_end();
        if !is_xml_name(name) {
            return None;
        }
        return Some(name.to_string());
    }

    let mut reader = Reader::from_str(tag);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let name = match reader.read_event_into(&mut buf) {
        Ok(Event::Start(e)) | Ok(Event::Empty(e)) => {
            for attr in e.attributes() {
                if attr.is_err() {
                    return None;
                }
            }
            e.name().as_ref().to_string()
        }
        _ => return None,
    };
    if name.is_empty() {
        // `< br >` 会被 quick-xml 解析成「名字为空」的开始标签
        return None;
    }
    Some(name)
}

/// 是否是合法的 XML 名字（`NameStartChar` + `NameChar`，这里只接受 ASCII 形态，
/// 本地化文本里的富文本标签名不可能出现别的字符）。
fn is_xml_name(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first.is_ascii_alphabetic() || first == '_' || first == ':') {
        return false;
    }
    chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '-'))
}

/// 判断 `<...>` 是否是白名单里的富文本标签，**且本身就是一个合法 XML 标签**。
pub fn is_allowed_inline_tag(tag: &str) -> bool {
    if !tag.starts_with('<') || !tag.ends_with('>') {
        return false;
    }
    match parse_single_tag(tag) {
        Some(name) => INLINE_TAGS.contains(&name.as_str()),
        None => false,
    }
}

/// 空元素（`<br>`）在译文里可能被写成 XHTML 风格的不闭合形态。
///
/// 纯 XML 没有「空元素」这回事：`<br>` 必须闭合，原样写出去会让**整个本地化
/// 文件**变成非法 XML。这里统一规范成 `<br/>`（语义不变）。
fn normalize_void_tag(tag: &str) -> Cow<'_, str> {
    let is_void = parse_single_tag(tag).is_some_and(|name| VOID_TAGS.contains(&name.as_str()));
    if is_void && !tag.trim_end().ends_with("/>") {
        let mut owned = tag.to_string();
        owned.insert(tag.len() - 1, '/');
        return Cow::Owned(owned);
    }
    Cow::Borrowed(tag)
}

/// XML 1.0 的 `Char` 产生式不允许的字符。
///
/// 注意这些字符**连字符引用都不能用**（`&#1;` 在 XML 1.0 里同样非法），
/// 所以只能丢弃——不丢弃就会让整个本地化文件变成非法文件，丢的是整份译文。
fn is_illegal_xml_char(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}'
    )
}

/// 去掉 XML 非法字符；没有需要处理的字符时借用原串，不做多余分配。
fn sanitize_xml_chars(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_illegal_xml_char) {
        return Cow::Borrowed(text);
    }
    let cleaned: String = text.chars().filter(|c| !is_illegal_xml_char(*c)).collect();
    log::warn!(
        "文本含 {} 个 XML 非法控制字符，已丢弃（XML 1.0 连字符引用都不允许它们）",
        text.chars().count().saturating_sub(cleaned.chars().count())
    );
    Cow::Owned(cleaned)
}

/// 句柄字段（contentuid / version）含 XML 非法字符时直接报错。
///
/// 这些值是游戏查表的键，**宁可让用户看到错误也不能悄悄改写**：
/// 去掉一个字符就等于换了一个 ID，游戏会查不到这条文本。
fn reject_illegal_identity(value: &str, what: &str) -> Result<()> {
    if value.chars().any(is_illegal_xml_char) {
        return Err(AppError::xml(format!(
            "{what} 含 XML 非法控制字符，拒绝写回（句柄不允许被改写）：{value:?}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<contentList>
  <content contentuid="habc123" version="1">Hello</content>
  <content contentuid="hdef456" version="2">World &amp; friends</content>
  <content contentuid="hghi789" version="1"/>
</contentList>"#;

    #[test]
    fn parses_all_content_entries_in_order() {
        let parsed = parse(SAMPLE, "test.xml");
        assert_eq!(parsed.entries.len(), 3);
        assert_eq!(parsed.entries[0].contentuid, "habc123");
        assert_eq!(parsed.entries[0].version, "1");
        assert_eq!(parsed.entries[0].source, "Hello");
        assert_eq!(parsed.entries[1].version, "2");
        assert_eq!(parsed.entries[2].source, "");
        assert_eq!(parsed.entries[2].source_file, "test.xml");
        assert_eq!(parsed.entries[0].id, "test.xml#habc123");
    }

    #[test]
    fn resolves_entities_to_readable_text() {
        let parsed = parse(SAMPLE, "test.xml");
        assert_eq!(parsed.entries[1].source, "World & friends");

        let xml = r#"<contentList><content contentuid="h1" version="1">a &lt; b &#39;c&#39; &quot;d&quot;</content></contentList>"#;
        let parsed = parse(xml, "t.xml");
        assert_eq!(parsed.entries[0].source, "a < b 'c' \"d\"");
    }

    #[test]
    fn preserves_rich_text_tags_and_placeholders() {
        let xml = r#"<contentList>
  <content contentuid="h1" version="1">Cast <LSTag Tag="Fire" Tooltip="x">Fireball</LSTag> for {1} <i>damage</i></content>
</contentList>"#;
        let parsed = parse(xml, "t.xml");
        assert_eq!(parsed.entries.len(), 1);
        let source = &parsed.entries[0].source;
        assert_eq!(
            source,
            r#"Cast <LSTag Tag="Fire" Tooltip="x">Fireball</LSTag> for {1} <i>damage</i>"#
        );
    }

    #[test]
    fn nested_content_like_tags_do_not_break_parsing() {
        let xml = r#"<contentList><content contentuid="h1" version="1">outer <content>inner</content> tail</content></contentList>"#;
        let parsed = parse(xml, "t.xml");
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(
            parsed.entries[0].source,
            "outer <content>inner</content> tail"
        );
    }

    #[test]
    fn captures_root_element_attributes() {
        let xml = r#"<contentList xmlns:x="urn:x"><content contentuid="h1" version="1">A</content></contentList>"#;
        let parsed = parse(xml, "t.xml");
        assert_eq!(parsed.root_attributes.len(), 1);
        assert_eq!(parsed.root_attributes[0].0, "xmlns:x");
        assert_eq!(parsed.root_attributes[0].1, "urn:x");
    }

    #[test]
    fn renders_and_reparses_without_loss() {
        let entries = vec![
            TranslationEntry::new("t.xml", "h1", "1", "Hello & <world>"),
            TranslationEntry::new(
                "t.xml",
                "h2",
                "3",
                "带 <LSTag Tag=\"Fire\">火球</LSTag> 的 {1} 文本",
            ),
        ];
        let xml = render(&entries, &[]).unwrap();
        assert!(xml.contains(r#"<LSTag Tag="Fire">"#));
        assert!(xml.contains("Hello &amp; &lt;world&gt;"));

        let replayed = parse(&xml, "t.xml");
        assert_eq!(replayed.entries.len(), 2);
        assert_eq!(replayed.entries[0].source, "Hello & <world>");
        assert_eq!(replayed.entries[1].source, entries[1].source);
    }

    #[test]
    fn write_prefers_target_but_keeps_source_when_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Localization.xml");
        std::fs::write(&path, SAMPLE).unwrap();

        let mut first = TranslationEntry::new("test.xml", "habc123", "1", "Hello");
        first.mark_translated("你好");
        let second = TranslationEntry::new("test.xml", "hdef456", "2", "World & friends");

        write(&path, &[first, second]).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains(">你好<"));
        assert!(out.contains("World &amp; friends"));
        assert!(!out.contains(">Hello<"));

        let reparsed = parse(&out, "test.xml");
        assert_eq!(reparsed.entries[0].target, "");
        assert_eq!(reparsed.entries[0].source, "你好");
    }

    /// F-01 最小复现：`status == error` + 非空 target 的条目**不得**写译文。
    ///
    /// 结构校验失败后前端把条目置为 `error`，但 target 里留着被拒译文（给用户看）。
    /// 用户在界面上直接打包时，写回必须退回原文，否则坏译文照样进 PAK。
    #[test]
    fn write_reverts_to_source_for_error_entries() {
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "Deals {1} damage");
        entry.mark_translated("造成伤害"); // 漏了占位符的坏译文
        entry.mark_error("结构校验未通过：占位符 {1} 缺失（已重试 1 次）");

        let xml = render(&[entry], &[]).unwrap();

        assert!(
            xml.contains("Deals {1} damage"),
            "error 条目必须退回原文，实际: {xml}"
        );
        assert!(!xml.contains("造成伤害"), "被拒译文绝不能落盘，实际: {xml}");
    }

    #[test]
    fn write_preserves_bom_and_root_attributes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bom.xml");
        let mut raw = BOM.to_vec();
        raw.extend_from_slice(
            br#"<contentList xmlns:x="urn:x"><content contentuid="h1" version="1">A</content></contentList>"#,
        );
        std::fs::write(&path, &raw).unwrap();

        let mut entry = TranslationEntry::new("bom.xml", "h1", "1", "A");
        entry.mark_translated("甲");
        write(&path, &[entry]).unwrap();

        let out = std::fs::read(&path).unwrap();
        assert!(out.starts_with(BOM), "BOM 必须保留");
        let text = String::from_utf8(out[3..].to_vec()).unwrap();
        assert!(text.contains(r#"xmlns:x="urn:x""#));
        assert!(text.contains(">甲<"));
    }

    #[test]
    fn write_creates_file_when_it_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/new.xml");
        let entry = TranslationEntry::new("new.xml", "h1", "1", "A");
        // 父目录不存在时应自动创建
        assert!(write(&path, &[entry]).is_err() || path.exists());
    }

    #[test]
    fn rejects_non_utf8_input() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.xml");
        std::fs::write(&path, [0xFF, 0xFE, 0x00, 0x41]).unwrap();
        let err = read(&path, "bad.xml").unwrap_err();
        assert_eq!(err.code(), "xml");
    }

    #[test]
    fn tag_balance_detection() {
        assert!(is_tag_balanced("plain text"));
        assert!(is_tag_balanced("a <LSTag>inner</LSTag> b"));
        assert!(is_tag_balanced(r#"<LSTag Tag="Fire">x</LSTag><i>y</i>"#));
        assert!(is_tag_balanced("<br/>"));
        assert!(is_tag_balanced("<br>"));
        assert!(is_tag_balanced("a < b")); // 不是标签

        // 多一个闭合标签（模型最常见的幻觉）
        assert!(!is_tag_balanced("text </LSTag>"));
        assert!(!is_tag_balanced("text </LSTag></LSTag>"));
        // 只有开标签、没有闭合
        assert!(!is_tag_balanced("text <LSTag>"));
        // 闭合标签名字对不上
        assert!(!is_tag_balanced("<LSTag>x</font>"));
        // 白名单外的标签不参与配对，也不会被当成标签写出去
        assert!(is_tag_balanced("<script>x</script>"));
    }

    #[test]
    fn unbalanced_model_output_never_produces_invalid_xml() {
        let entries = vec![
            {
                let mut e = TranslationEntry::new("t.xml", "h1", "1", "src");
                e.mark_translated("正常 <LSTag Tag=\"Fire\">火球</LSTag> 文本");
                e
            },
            {
                let mut e = TranslationEntry::new("t.xml", "h2", "1", "src");
                // 模型多吐了一个闭合标签
                e.mark_translated("多余闭合 </LSTag> 文本");
                e
            },
        ];

        let xml = render(&entries, &[]).unwrap();

        // 产物必须能被自己重新解析，且不报错
        let reparsed = parse(&xml, "t.xml");
        assert!(reparsed.error.is_none(), "产出的 XML 必须合法: {xml}");
        assert_eq!(reparsed.entries.len(), 2);

        // 正常的标签照旧保留
        assert!(xml.contains(r#"<LSTag Tag="Fire">火球</LSTag>"#));
        // 不配对的标签退化成字面文本，而不是变成非法标签
        assert!(
            xml.contains("&lt;/LSTag&gt;"),
            "不配对的标签应被转义: {xml}"
        );
        assert!(!xml.contains("多余闭合 </LSTag>"));
    }

    #[test]
    fn inline_tag_whitelist_is_strict() {
        assert!(is_allowed_inline_tag(r#"<LSTag Tag="Fire">"#));
        assert!(is_allowed_inline_tag("</LSTag>"));
        assert!(is_allowed_inline_tag("<br/>"));
        assert!(is_allowed_inline_tag("<font color=\"red\">"));
        assert!(!is_allowed_inline_tag("<script>"));
        assert!(!is_allowed_inline_tag("<?xml?>"));
        assert!(!is_allowed_inline_tag("<!DOCTYPE html>"));
        assert!(!is_allowed_inline_tag("not a tag"));
    }

    #[test]
    fn malformed_xml_is_reported_instead_of_silently_truncating() {
        // 畸形标签之后的内容都读不到了。此时若返回残缺列表，写回就会把
        // 后面所有条目删掉——所以必须把错误暴露出来。
        let xml = r#"<contentList><content contentuid="h1" version="1">ok</content><content contentuid="h2" version="1">broken"#;
        let parsed = parse(xml, "t.xml");
        assert_eq!(parsed.entries.len(), 1, "解析器仍应保留能读到的部分");
        assert!(parsed.error.is_some(), "解析中断必须被记录");

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Broken.xml");
        std::fs::write(&path, xml).unwrap();
        let err = read(&path, "Broken.xml").unwrap_err();
        assert_eq!(err.code(), "xml");
        assert!(err.to_string().contains("Broken.xml"));
    }

    #[test]
    fn complete_documents_report_no_error() {
        let parsed = parse(SAMPLE, "t.xml");
        assert!(parsed.error.is_none());
    }

    #[test]
    fn empty_document_yields_nothing() {
        assert!(parse("", "t.xml").entries.is_empty());
    }

    /// 写回后的文件必须永远是合法 XML —— 用「自己能不能重新读」当判据。
    fn assert_reparses_cleanly(xml: &str) -> Vec<TranslationEntry> {
        let parsed = parse(xml, "t.xml");
        assert!(
            parsed.error.is_none(),
            "产出的 XML 必须合法可用: {:?}\n{xml}",
            parsed.error
        );
        parsed.entries
    }

    /// 原文里的字面 `< ... >`（源文件写成 `&lt; ... &gt;`）绝不能被当成富文本标签。
    ///
    /// 复现（修复前）：`x &lt; br &gt; y` → 解析出 `x < br > y` → 写回时被
    /// `is_allowed_inline_tag` 当成 `<br>` 原样输出 → 整个文件变成非法 XML
    /// （`<` 后面紧跟空格不是合法开始标签），游戏会放弃解析**整份本地化文件**。
    /// 注意这条路径**不需要任何翻译**：未翻译条目写回原文时就会触发。
    #[test]
    fn literal_angle_bracket_text_is_never_written_as_markup() {
        let xml = r#"<contentList>
  <content contentuid="h1" version="1">x &lt; br &gt; y</content>
  <content contentuid="h2" version="1">a &lt; i &gt; b &lt;/i&gt;</content>
</contentList>"#;
        let entries = parse(xml, "t.xml").entries;
        assert_eq!(entries[0].source, "x < br > y");
        assert_eq!(entries[1].source, "a < i > b </i>");

        let out = render(&entries, &[]).unwrap();
        let reparsed = assert_reparses_cleanly(&out);
        assert_eq!(reparsed.len(), 2);
        assert_eq!(reparsed[0].source, "x < br > y");
        assert_eq!(reparsed[1].source, "a < i > b </i>");
    }

    /// 真标签的属性值里带字面 `>`（XML 允许）时，标签不能被从中间劈开。
    ///
    /// 复现（修复前）：朴素地取「第一个 `>`」会把 `<LSTag Tooltip="a > b">`
    /// 切成 `<LSTag Tooltip="a >` + 文本 ` b">fire`，写回后属性值没闭合，
    /// 重新解析直接报 `attribute value not closed`。
    #[test]
    fn tag_attribute_value_containing_gt_survives_write_back() {
        let xml = r#"<contentList><content contentuid="h1" version="1">Deals <LSTag Tooltip="a > b">fire</LSTag> damage</content></contentList>"#;
        let entries = parse(xml, "t.xml").entries;
        assert_eq!(
            entries[0].source,
            r#"Deals <LSTag Tooltip="a > b">fire</LSTag> damage"#
        );

        let out = render(&entries, &[]).unwrap();
        let reparsed = assert_reparses_cleanly(&out);
        assert_eq!(
            reparsed[0].source, entries[0].source,
            "属性值里的 `>` 不能被吞掉或复制: {out}"
        );
    }

    /// XML 1.0 不允许的控制字符（除了 `\t\n\r`）绝不能原样落盘。
    ///
    /// 复现（修复前）：译文里带 `\u{1}` 时 `render` 直接把 0x01 写进文件，
    /// 而 XML 1.0 的 Char 产生式根本不含它（连 `&#1;` 都是非法的）——
    /// 产物对游戏侧解析器就是非法文件，丢的是整份译文。
    #[test]
    fn illegal_control_characters_are_never_written_into_the_xml() {
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated("你好\u{1}世界\u{7}");

        let out = render(&[entry], &[]).unwrap();

        assert!(
            !out.contains('\u{1}') && !out.contains('\u{7}'),
            "非法控制字符必须被丢弃: {out:?}"
        );
        let reparsed = assert_reparses_cleanly(&out);
        assert_eq!(reparsed[0].source, "你好世界");
        // 合法的空白仍然保留
        let mut with_ws = TranslationEntry::new("t.xml", "h2", "1", "src");
        with_ws.mark_translated("a\tb\nc");
        let out2 = render(&[with_ws], &[]).unwrap();
        assert_eq!(assert_reparses_cleanly(&out2)[0].source, "a\tb\nc");
    }

    /// contentuid / version 是游戏查表句柄，含非法字符宁可报错也不能悄悄改写。
    #[test]
    fn identity_attributes_with_control_characters_are_rejected() {
        let mut entry = TranslationEntry::new("t.xml", "h\u{1}1", "1", "src");
        entry.mark_translated("译文");
        let err = render(&[entry], &[]).unwrap_err();
        assert_eq!(err.code(), "xml");

        let mut entry = TranslationEntry::new("t.xml", "h1", "1\u{2}", "src");
        entry.mark_translated("译文");
        assert_eq!(render(&[entry], &[]).unwrap_err().code(), "xml");
    }

    /// 写回「重建整个文档」时，磁盘上已有、但本次没提交的条目必须原样保留。
    ///
    /// 复现（修复前）：MOD 自带 `Localization/Chinese/x.xml`（含英文原文里没有的
    /// contentuid），前端按「英文优先」只提交英文那份的条目 —— 直接重建会把
    /// 只在中文文件里存在的条目永久删掉，游戏里对应文本变成原始句柄。
    #[test]
    fn write_keeps_entries_that_already_exist_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.xml");
        std::fs::write(
            &path,
            r#"<contentList><content contentuid="h1" version="1">旧译文一</content><content contentuid="h_only_chinese" version="2">只有中文文件才有的条目</content></contentList>"#,
        )
        .unwrap();

        let mut entry = TranslationEntry::new("English.xml", "h1", "1", "One");
        entry.mark_translated("新译文一");
        write(&path, &[entry]).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("新译文一"), "提交的译文必须写回: {out}");
        assert!(
            out.contains("h_only_chinese") && out.contains("只有中文文件才有的条目"),
            "磁盘上已有、本次未提交的条目被删掉了: {out}"
        );
        let reparsed = assert_reparses_cleanly(&out);
        assert_eq!(reparsed.len(), 2);
    }

    /// 空列表写回**不会**清空一个已有内容的文件（同一缺陷的另一面）。
    #[test]
    fn writing_an_empty_list_does_not_wipe_an_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.xml");
        std::fs::write(
            &path,
            r#"<contentList><content contentuid="h1" version="1">甲</content></contentList>"#,
        )
        .unwrap();

        write(&path, &[]).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(out.contains("甲"), "已有条目不能被清空: {out}");
    }

    /// 原文件没有 `version` 属性时，写回不能凭空补一个 `version=""`。
    ///
    /// 复现（修复前）：`<content contentuid="h1">A</content>` 读出来 version 是
    /// 空串，`render` 无条件 push_attribute，于是产物里多出 `version=""`——
    /// 「没有这个属性」和「版本号为空」在游戏侧不是一回事。
    #[test]
    fn missing_version_attribute_is_not_invented_on_write_back() {
        let xml = r#"<contentList><content contentuid="h1">A</content><content contentuid="h2" version="3">B</content></contentList>"#;
        let entries = parse(xml, "t.xml").entries;
        assert_eq!(entries[0].version, "");
        assert_eq!(entries[1].version, "3");

        let out = render(&entries, &[]).unwrap();

        assert!(
            !out.contains("version=\"\""),
            "不能凭空补出空 version 属性: {out}"
        );
        assert!(out.contains(r#"<content contentuid="h1">"#), "{out}");
        assert!(
            out.contains(r#"<content contentuid="h2" version="3">"#),
            "{out}"
        );
        // 属性缺失也要能原样读回来
        let reparsed = parse(&out, "t.xml");
        assert!(reparsed.error.is_none());
        assert_eq!(reparsed.entries[0].version, "");
    }

    /// 宽容解析不能被收紧：LLM / 真人常见但**合法**的等价写法照旧要写成真标签。
    ///
    /// 断言两件事：(1) 第一次写回就原样保留成标签（没有被转义成正文）；
    /// (2) 再解析一次之后写回是**幂等**的（`<br />` 会被 quick-xml 规范成
    /// `<br/>`，这是本来就有的行为，语义相同）。
    #[test]
    fn equivalent_tag_spellings_are_still_written_as_markup() {
        // (译文, 第一次写回后应该出现的标签形态, 说明)
        let cases = [
            ("<br/>", "<br/>", "标准自闭合"),
            ("<br />", "<br />", "斜杠前有空格"),
            ("<br>", "<br/>", "XHTML 风格：统一规范成自闭合"),
            (
                "<LSTag Tag='Fire'>x</LSTag>",
                "<LSTag Tag='Fire'>",
                "单引号属性",
            ),
            (
                r#"<LSTag Tooltip="t" Tag="Fire">x</LSTag>"#,
                r#"<LSTag Tooltip="t" Tag="Fire">"#,
                "属性换序",
            ),
            (
                r#"<i class="a">x</i>"#,
                r#"<i class="a">"#,
                "额外 class 属性",
            ),
            ("<b>粗</b>", "<b>粗</b>", "最短标签"),
            ("a <LSTag>内</LSTag> b", "a <LSTag>内</LSTag> b", "无属性"),
        ];
        for (text, expected, label) in cases {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(text);
            let out = render(&[entry], &[]).unwrap();
            assert!(
                out.contains(expected),
                "[{label}] {text:?} 应该原样保留为标签 {expected:?}: {out}"
            );
            assert!(
                !out.contains("&lt;LSTag") && !out.contains("&lt;br") && !out.contains("&lt;b"),
                "[{label}] 标签不该被转义成正文: {out}"
            );
            let reparsed = parse(&out, "t.xml");
            assert!(reparsed.error.is_none(), "[{label}] 产物必须合法: {out}");
            let second = render(&reparsed.entries, &[]).unwrap();
            let third = render(&parse(&second, "t.xml").entries, &[]).unwrap();
            assert_eq!(second, third, "[{label}] 写回必须稳定: {text:?}");
        }
    }

    /// `<br>`（不闭合的空元素）必须被规范成 `<br/>`，否则产物是非法 XML。
    #[test]
    fn unclosed_void_tag_is_normalized_to_self_closing() {
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated("line1<br>line2<br >line3");

        let out = render(&[entry], &[]).unwrap();

        assert!(
            !out.contains("<br>") && !out.contains("<br >"),
            "实际: {out}"
        );
        assert!(out.contains("line1<br/>line2<br />line3"), "实际: {out}");
        let reparsed = parse(&out, "t.xml");
        assert!(reparsed.error.is_none(), "产物必须合法: {out}");
        assert_eq!(reparsed.entries[0].source, "line1<br/>line2<br/>line3");
    }

    /// 斜杠后带空格的「伪结束标签」是正文，不是标签。
    ///
    /// 复现（修复前）：`is_allowed_inline_tag` 先 `trim_start_matches('/')` 再
    /// `trim()`，把 `</ b>` 当成合法 `</b>` 原样写出，产物变成非法 XML
    /// （`expected </b>, but </ b> was found`）。
    #[test]
    fn pseudo_end_tag_with_space_is_treated_as_text() {
        // 关键是 `<b>` 与 `</ b>` 一开一「闭」：若 `</ b>` 被当成合法结束标签，
        // 配对检查会通过，产物里就会出现非法的 `</ b>`。
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated("<b>粗体</ b> 尾巴");

        let out = render(&[entry], &[]).unwrap();
        assert!(
            out.contains("&lt;/ b&gt;"),
            "`</ b>` 必须被转义成正文: {out}"
        );
        let reparsed = parse(&out, "t.xml");
        assert!(reparsed.error.is_none(), "产物必须合法: {out}");
        assert_eq!(reparsed.entries[0].source, "<b>粗体</ b> 尾巴");
    }

    /// 白名单外的标签与畸形开始标签也必须是正文（不能原样写出去）。
    #[test]
    fn unknown_or_malformed_tags_are_escaped() {
        for text in [
            "<script>alert(1)</script>",
            r#"<LSTag Tooltip="a" garbage>"#,
            "<LSTag Tooltip=a>",
            "< b>",
            "<?xml version=\"1.0\"?>",
            // 这里的关键是「畸形开始标签 + 配对结束标签」：如果畸形标签被当成
            // 真标签放行，配对检查会通过，产物里就会出现非法的 `<b class=>`
            "<b class=>x</b>",
            "<br =x>y<br/>",
        ] {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(text);
            let out = render(&[entry], &[]).unwrap();
            assert!(
                !out.contains("<script")
                    && !out.contains("<LSTag Tooltip=")
                    && !out.contains("< b>"),
                "{text:?} 不该被当成标签写出去: {out}"
            );
            let reparsed = parse(&out, "t.xml");
            assert!(reparsed.error.is_none(), "产物必须合法: {out}");
            assert_eq!(reparsed.entries[0].source, text, "{text:?} 往返必须一致");
        }
    }

    /// 已知取舍（R-07）：XML 未定义实体（如 `&nbsp;`）会保持字面文本，
    /// 但写回时会被转义成 `&amp;nbsp;` —— 语义等价、字节不同。
    ///
    /// 不修的理由：把 `&name;` 原样写回需要「只放行源文件里出现过的实体」，
    /// 一旦模型/用户新写一个未定义实体就会产出**非法 XML**（正是本轮在修的
    /// 那类缺陷），风险大于收益。这条测试把行为钉住，避免以后悄悄变化。
    #[test]
    fn unknown_entities_are_pinned_as_literal_text() {
        let xml = r#"<contentList><content contentuid="h1" version="1">a &nbsp; b</content></contentList>"#;
        let entries = parse(xml, "t.xml").entries;
        assert_eq!(entries[0].source, "a &nbsp; b");

        let out = render(&entries, &[]).unwrap();
        assert!(out.contains("a &amp;nbsp; b"), "实际: {out}");
        assert!(parse(&out, "t.xml").error.is_none());
        assert_eq!(parse(&out, "t.xml").entries[0].source, "a &nbsp; b");
    }
}
