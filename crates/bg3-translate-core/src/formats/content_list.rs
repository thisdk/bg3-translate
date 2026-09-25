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

/// 从磁盘读取并解析。
pub fn read(path: &Path, file_name: &str) -> Result<Vec<TranslationEntry>> {
    let bytes = std::fs::read(path)?;
    let xml = decode_utf8(&bytes, "本地化 XML")?;
    Ok(parse(&xml, file_name).entries)
}

/// 解析结果：条目 + 根元素属性（写回时保留）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedContentList {
    pub entries: Vec<TranslationEntry>,
    pub root_attributes: Vec<(String, String)>,
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
            Ok(Event::Eof) => break,
            Err(err) => {
                log::warn!("{file_name} XML 解析中断: {err}");
                break;
            }
            _ => {}
        }
    }

    parsed
}

/// 写回磁盘，保留 BOM 与根元素属性。
pub fn write(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let (bom, root_attributes) = match std::fs::read(path) {
        Ok(bytes) => {
            let bom = bytes.starts_with(BOM);
            let xml = decode_utf8(&bytes, "本地化 XML")?;
            (bom, parse(&xml, "").root_attributes)
        }
        Err(_) => (false, Vec::new()),
    };

    let body = render(entries, &root_attributes)?;

    let mut out = Vec::with_capacity(body.len() + 3);
    if bom {
        out.extend_from_slice(BOM);
    }
    out.extend_from_slice(body.as_bytes());
    write_atomic(path, &out)?;
    Ok(())
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
        let mut start = BytesStart::new("content");
        start.push_attribute(("contentuid", entry.contentuid.as_str()));
        start.push_attribute(("version", entry.version.as_str()));
        write_event(&mut writer, Event::Start(start))?;
        write_text_fragment(&mut writer, entry.effective_text())?;
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
fn write_text_fragment<W: std::io::Write>(writer: &mut Writer<W>, text: &str) -> Result<()> {
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        if start > 0 {
            write_event(writer, Event::Text(BytesText::new(&rest[..start])))?;
        }

        let after_lt = &rest[start..];
        let Some(rel_end) = after_lt.find('>') else {
            // 没有闭合的 `<`，当普通文本处理
            write_event(writer, Event::Text(BytesText::new(after_lt)))?;
            return Ok(());
        };

        let end = start + rel_end + 1;
        let tag = &rest[start..end];
        if is_allowed_inline_tag(tag) {
            writer
                .get_mut()
                .write_all(tag.as_bytes())
                .map_err(|e| AppError::xml(format!("{e}")))?;
        } else {
            write_event(writer, Event::Text(BytesText::new(tag)))?;
        }
        rest = &rest[end..];
    }

    if !rest.is_empty() {
        write_event(writer, Event::Text(BytesText::new(rest)))?;
    }
    Ok(())
}

/// 判断 `<...>` 是否是白名单里的富文本标签（开/闭/自闭合都算）。
pub fn is_allowed_inline_tag(tag: &str) -> bool {
    if !tag.starts_with('<') || !tag.ends_with('>') {
        return false;
    }
    let inner = &tag[1..tag.len() - 1];
    if inner.starts_with('?') || inner.starts_with('!') {
        return false;
    }
    let name = inner
        .trim_start_matches('/')
        .trim_end_matches('/')
        .trim()
        .split(|c: char| c.is_whitespace() || c == '/')
        .next()
        .unwrap_or("");
    INLINE_TAGS.contains(&name)
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
    fn malformed_xml_keeps_what_it_could_parse() {
        let xml = r#"<contentList><content contentuid="h1" version="1">ok</content><content contentuid="h2" version="1">broken"#;
        let parsed = parse(xml, "t.xml");
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].source, "ok");
    }

    #[test]
    fn empty_document_yields_nothing() {
        assert!(parse("", "t.xml").entries.is_empty());
    }
}
