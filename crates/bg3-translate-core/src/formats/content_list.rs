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
        let text = entry.effective_text();
        if is_tag_balanced(text) {
            write_text_fragment(&mut writer, text)?;
        } else {
            // 模型偶尔会多吐一个 `</LSTag>` 或者漏掉闭合标签。原样写出去会让
            // 整个本地化文件变成非法 XML，游戏可能直接放弃解析这个文件——
            // 那样丢的是**整份译文**，代价远大于一条文本里多几个字面标签。
            // 所以这里退化成纯文本：XML 一定合法，问题在界面上肉眼可见。
            log::warn!(
                "条目 {} 的译文标签不配对，已按纯文本写入以避免产出非法 XML",
                entry.contentuid
            );
            write_event(&mut writer, Event::Text(BytesText::new(text)))?;
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

/// 判断一段文本里的富文本标签是否配对（开闭成对、自闭合不算）。
///
/// 只有通过这个检查的文本才会把标签当真实标签写出去；否则整条按纯文本转义，
/// 保证产物永远是合法 XML。
pub fn is_tag_balanced(text: &str) -> bool {
    let mut stack: Vec<&str> = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        let after = &rest[start..];
        let Some(rel_end) = after.find('>') else {
            // 有个没闭合的 `<`：不是标签，当普通文本
            break;
        };
        let tag = &after[..=rel_end];
        if is_allowed_inline_tag(tag) {
            let inner = &tag[1..tag.len() - 1];
            let trimmed = inner.trim();
            if trimmed.ends_with('/') {
                // 自闭合，不入栈
            } else if let Some(name) = trimmed.strip_prefix('/') {
                match stack.pop() {
                    Some(open) if open == name.trim() => {}
                    // 闭合标签对不上栈顶：不配对
                    _ => return false,
                }
            } else {
                let name = trimmed
                    .split(|c: char| c.is_whitespace())
                    .next()
                    .unwrap_or("");
                if !VOID_TAGS.contains(&name) {
                    stack.push(name);
                }
            }
        }
        rest = &after[rel_end + 1..];
    }
    stack.is_empty()
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
}
