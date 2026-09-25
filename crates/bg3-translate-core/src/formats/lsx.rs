//! `.lsx` 元数据（纯文本 XML）的读写。
//!
//! LSX 里只有一部分字段是玩家可见文本，其余是内部 ID。判定规则：
//! **字段名在白名单内 + 类型是 `LSString`/`LSWString` + 值非空**。
//!
//! 注意 `TranslatedString` 类型存放的是 contentuid 句柄，**不在**白名单里，
//! 绝不能翻译，否则游戏查表会失败。
//!
//! 实现上使用**同一套扫描器**同时服务读和写：读的时候记录每个可翻译
//! attribute 的值在文件中的字节区间，写的时候按区间原地替换。
//! 这样读写的"第 N 个同名属性"永远指向同一个位置——旧实现两边用了不同的
//! 计数规则，遇到空值/非文本类型的同名字段就会串行替换到错误的文本上。

use std::ops::Range;
use std::path::Path;

use crate::config::write_atomic;
use crate::error::{AppError, Result};
use crate::types::TranslationEntry;

/// LSX 中可翻译的 attribute id 白名单。
///
/// **`Name` 故意不在白名单里。** `Mods/<mod>/meta.lsx` 的
/// `<attribute id="Name" type="LSString" value="GustavDev" />` 是模块的内部
/// 标识符（游戏按它引用这个模块），而且类型同样是 `LSString`——只看类型
/// 分不出它和玩家可见文本。翻译它会让 MOD 直接失效。
///
/// 玩家可见的物品/技能名走的是 `TranslatedString`（contentuid 句柄），
/// 由 `Localization/*.xml` 与 `.loca` 负责，根本不该在 LSX 里改。
pub const TRANSLATABLE_FIELDS: &[&str] = &[
    "Description",
    "DisplayName",
    "Title",
    "Tooltip",
    "TooltipDescription",
];

/// 被当作玩家可见文本的 attribute 类型。
pub const TEXT_TYPES: &[&str] = &["LSString", "LSWString"];

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// 一个可翻译的 LSX attribute。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LsxField {
    /// 字段名，如 `Description`
    pub id: String,
    /// 类型，如 `LSString`
    pub type_name: String,
    /// 已反转义的字段值
    pub value: String,
    /// 值在文件中的字节区间（不含引号）
    pub value_span: Range<usize>,
    /// 同名字段的第几次出现（从 0 开始，只统计可翻译字段）
    pub occurrence: usize,
}

impl LsxField {
    /// 写回时使用的 contentuid 编码：`{字段名}#{出现序号}`。
    pub fn contentuid(&self) -> String {
        format!("{}#{}", self.id, self.occurrence)
    }
}

// ─────────────────────────────────────────────────────────────
// 对外的读写
// ─────────────────────────────────────────────────────────────

/// 从磁盘读取可翻译条目。
pub fn read(path: &Path, file_name: &str) -> Result<Vec<TranslationEntry>> {
    let bytes = std::fs::read(path)?;
    let xml = decode_utf8(&bytes)?;
    Ok(entries_from_fields(&scan_translatable(&xml), file_name))
}

/// 把带译文的条目原地写回磁盘，保留文件其余结构、缩进、换行与 BOM。
pub fn write(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let bytes = std::fs::read(path)?;
    let has_bom = bytes.starts_with(BOM);
    let xml = decode_utf8(&bytes)?;

    let fields = scan_translatable(&xml);
    let replacements = plan_replacements(&fields, entries);
    let updated = apply_replacements(&xml, &replacements);

    let mut out = Vec::with_capacity(updated.len() + 3);
    if has_bom {
        out.extend_from_slice(BOM);
    }
    out.extend_from_slice(updated.as_bytes());
    write_atomic(path, &out)?;
    Ok(())
}

/// 把 field 列表转成条目。
pub fn entries_from_fields(fields: &[LsxField], file_name: &str) -> Vec<TranslationEntry> {
    fields
        .iter()
        .map(|field| TranslationEntry::new(file_name, field.contentuid(), "1", field.value.clone()))
        .collect()
}

/// 计算需要替换的 `(字节区间, 新值)`，按顺序返回。
///
/// 只处理「能解析出 contentuid 且有译文」的条目；找不到对应字段的条目会被
/// 记 warning 并跳过（而不是静默写错位置）。
pub fn plan_replacements(
    fields: &[LsxField],
    entries: &[TranslationEntry],
) -> Vec<(Range<usize>, String)> {
    let mut plan = Vec::new();
    for entry in entries {
        if !entry.has_target() {
            continue;
        }
        let Some((id, occurrence)) = decode_contentuid(&entry.contentuid) else {
            log::warn!("LSX 条目 contentuid 无法解析，已跳过: {}", entry.contentuid);
            continue;
        };
        let Some(field) = fields
            .iter()
            .find(|f| f.id == id && f.occurrence == occurrence)
        else {
            log::warn!(
                "LSX 中找不到字段 {}#{}（文件可能已被修改），已跳过",
                id,
                occurrence
            );
            continue;
        };
        plan.push((field.value_span.clone(), entry.target.clone()));
    }
    // 从后往前替换，避免前面的替换影响后面的区间
    plan.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    plan
}

/// 按区间替换生成新字符串。
pub fn apply_replacements(xml: &str, replacements: &[(Range<usize>, String)]) -> String {
    let mut out = xml.to_string();
    for (span, value) in replacements {
        if span.start > span.end || span.end > out.len() {
            log::warn!("LSX 替换区间越界，已跳过: {span:?}");
            continue;
        }
        out.replace_range(span.clone(), &escape_attribute(value));
    }
    out
}

/// 解析 `{字段名}#{序号}`。
pub fn decode_contentuid(contentuid: &str) -> Option<(String, usize)> {
    let (id, occurrence) = contentuid.rsplit_once('#')?;
    Some((id.to_string(), occurrence.parse().ok()?))
}

// ─────────────────────────────────────────────────────────────
// 扫描器
// ─────────────────────────────────────────────────────────────

/// 扫描出所有可翻译的 `<attribute>` 字段。
pub fn scan_translatable(xml: &str) -> Vec<LsxField> {
    let mut counters: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut fields = Vec::new();

    for tag in find_tags(xml, "attribute") {
        let attrs = parse_attributes(xml, &tag);
        let Some(id) = attrs.get("id") else { continue };
        let Some(type_name) = attrs.get("type") else {
            continue;
        };
        let Some(value) = attrs.get("value") else {
            continue;
        };
        if !TRANSLATABLE_FIELDS.contains(&id.text.as_str())
            || !TEXT_TYPES.contains(&type_name.text.as_str())
        {
            continue;
        }
        let decoded = unescape_attribute(&value.text);
        if decoded.trim().is_empty() {
            continue;
        }
        let occurrence = counters.entry(id.text.clone()).or_insert(0);
        let index = *occurrence;
        *occurrence += 1;
        fields.push(LsxField {
            id: id.text.clone(),
            type_name: type_name.text.clone(),
            value: decoded,
            value_span: value.span.clone(),
            occurrence: index,
        });
    }

    fields
}

/// XML 标签在原文中的字节区间（`end` 指向 `>` 之后）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSpan {
    pub start: usize,
    pub end: usize,
}

/// attribute 的原始值及其字节区间。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawValue {
    pub text: String,
    /// 值在原文中的字节区间（不含两侧引号）
    pub span: Range<usize>,
}

/// 找出所有名为 `name` 的开始标签区间。
pub fn find_tags(xml: &str, name: &str) -> Vec<TagSpan> {
    let bytes = xml.as_bytes();
    let mut spans = Vec::new();
    let mut i = 0;

    while i < bytes.len() {
        let Some(offset) = xml[i..].find('<') else {
            break;
        };
        let start = i + offset;
        let rest = &xml[start..];

        // 跳过注释 / CDATA / 处理指令 / DOCTYPE
        if rest.starts_with("<!--") {
            i = xml[start..]
                .find("-->")
                .map(|p| start + p + 3)
                .unwrap_or(bytes.len());
            continue;
        }
        if rest.starts_with("<![CDATA[") {
            i = xml[start..]
                .find("]]>")
                .map(|p| start + p + 3)
                .unwrap_or(bytes.len());
            continue;
        }
        if rest.starts_with("<?") || rest.starts_with("<!") {
            i = xml[start..]
                .find('>')
                .map(|p| start + p + 1)
                .unwrap_or(bytes.len());
            continue;
        }

        // 解析标签名
        let name_start = start + 1;
        let name_end = xml[name_start..]
            .find(|c: char| c.is_whitespace() || c == '>' || c == '/')
            .map(|p| name_start + p)
            .unwrap_or(bytes.len());
        let tag_name = &xml[name_start..name_end];

        // 找标签结束的 '>'（跳过引号内的内容）
        let mut j = name_end;
        let mut quote: Option<u8> = None;
        let mut end = None;
        while j < bytes.len() {
            let b = bytes[j];
            match quote {
                Some(q) if b == q => quote = None,
                Some(_) => {}
                None if b == b'"' || b == b'\'' => quote = Some(b),
                None if b == b'>' => {
                    end = Some(j + 1);
                    break;
                }
                None => {}
            }
            j += 1;
        }
        let Some(end) = end else { break };

        if tag_name == name {
            spans.push(TagSpan { start, end });
        }
        i = end;
    }

    spans
}

/// 解析一个开始标签内的属性（保留原始转义文本与字节区间）。
pub fn parse_attributes(xml: &str, tag: &TagSpan) -> std::collections::HashMap<String, RawValue> {
    let bytes = xml.as_bytes();
    let mut attrs = std::collections::HashMap::new();
    let mut i = tag.start + 1;

    // 跳过标签名
    while i < tag.end && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>' && bytes[i] != b'/' {
        i += 1;
    }

    while i < tag.end {
        while i < tag.end && (bytes[i].is_ascii_whitespace() || bytes[i] == b'/') {
            i += 1;
        }
        if i >= tag.end || bytes[i] == b'>' {
            break;
        }

        let key_start = i;
        while i < tag.end && bytes[i] != b'=' && !bytes[i].is_ascii_whitespace() && bytes[i] != b'>'
        {
            i += 1;
        }
        let key = xml[key_start..i].to_string();

        while i < tag.end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag.end || bytes[i] != b'=' {
            continue;
        }
        i += 1;
        while i < tag.end && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= tag.end || (bytes[i] != b'"' && bytes[i] != b'\'') {
            continue;
        }
        let quote = bytes[i];
        i += 1;
        let value_start = i;
        while i < tag.end && bytes[i] != quote {
            i += 1;
        }
        let value_end = i;
        if i < tag.end {
            i += 1;
        }

        if !key.is_empty() {
            attrs.insert(
                key,
                RawValue {
                    text: xml[value_start..value_end].to_string(),
                    span: value_start..value_end,
                },
            );
        }
    }

    attrs
}

// ─────────────────────────────────────────────────────────────
// 转义
// ─────────────────────────────────────────────────────────────

/// 反转义 XML 属性值（含数字字符引用）。
pub fn unescape_attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(pos) = rest.find('&') {
        out.push_str(&rest[..pos]);
        let tail = &rest[pos..];
        let Some(semi) = tail.find(';') else {
            out.push_str(tail);
            return out;
        };
        let entity = &tail[1..semi];
        let decoded = decode_entity(entity);
        match decoded {
            Some(text) => out.push_str(&text),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

fn decode_entity(entity: &str) -> Option<String> {
    match entity {
        "amp" => return Some("&".into()),
        "lt" => return Some("<".into()),
        "gt" => return Some(">".into()),
        "quot" => return Some("\"".into()),
        "apos" => return Some("'".into()),
        _ => {}
    }
    let digits = entity
        .strip_prefix("#x")
        .or_else(|| entity.strip_prefix("#X"));
    if let Some(hex) = digits {
        let code = u32::from_str_radix(hex, 16).ok()?;
        return char::from_u32(code).map(String::from);
    }
    let dec = entity.strip_prefix('#')?;
    let code = dec.parse::<u32>().ok()?;
    char::from_u32(code).map(String::from)
}

/// 转义成可安全放进双引号属性值的形式。
pub fn escape_attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\r' => out.push_str("&#13;"),
            '\n' => out.push_str("&#10;"),
            '\t' => out.push_str("&#9;"),
            _ => out.push(ch),
        }
    }
    out
}

fn decode_utf8(bytes: &[u8]) -> Result<String> {
    let body = bytes.strip_prefix(BOM).unwrap_or(bytes);
    String::from_utf8(body.to_vec()).map_err(|e| AppError::xml(format!(".lsx 不是合法 UTF-8: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<save>
  <region id="Config">
    <node id="root">
      <attribute id="Name" type="FixedString" value="WPN_Sword_Internal" />
      <attribute id="Name" type="LSString" value="Iron Sword" />
      <attribute id="Description" type="LSString" value="A sturdy blade &amp; shield-breaker." />
      <attribute id="Description" type="TranslatedString" value="h1234abcd-5678" />
      <attribute id="Description" type="LSString" value="" />
      <attribute id="Description" type="LSString" value="Second description" />
      <attribute id="DisplayName" type="LSWString" value="Sword of &#39;Doom&#39;" />
      <attribute id="Unknown" type="LSString" value="should be ignored" />
    </node>
  </region>
</save>"#;

    fn ids(fields: &[LsxField]) -> Vec<String> {
        fields.iter().map(|f| f.contentuid()).collect()
    }

    #[test]
    fn scan_keeps_only_whitelisted_text_fields() {
        let fields = scan_translatable(SAMPLE);
        assert_eq!(
            ids(&fields),
            vec!["Description#0", "Description#1", "DisplayName#0"]
        );
        assert_eq!(fields[0].value, "A sturdy blade & shield-breaker.");
        assert_eq!(fields[1].value, "Second description");
        assert_eq!(fields[2].value, "Sword of 'Doom'");
    }

    #[test]
    fn module_name_is_never_treated_as_translatable() {
        // Mods/<mod>/meta.lsx 里的 Name 是模块内部标识符，类型同样是 LSString。
        // 翻掉它会让 MOD 直接失效，所以必须留在白名单之外。
        let meta = r#"<?xml version="1.0" encoding="utf-8"?>
<save>
  <region id="Config">
    <node id="ModuleInfo">
      <attribute id="Name" type="LSString" value="GustavDev" />
      <attribute id="Folder" type="LSString" value="AppearanceEditEnhanced" />
      <attribute id="Description" type="LSString" value="Enables Race/Body Type editing." />
    </node>
  </region>
</save>"#;
        let fields = scan_translatable(meta);
        assert_eq!(ids(&fields), vec!["Description#0"]);
        assert!(!fields.iter().any(|f| f.value == "GustavDev"));
        assert!(!fields.iter().any(|f| f.value == "AppearanceEditEnhanced"));
    }

    #[test]
    fn translated_string_handles_are_never_treated_as_text() {
        let fields = scan_translatable(SAMPLE);
        assert!(!fields.iter().any(|f| f.type_name == "TranslatedString"));
        assert!(!fields.iter().any(|f| f.value.contains("h1234abcd")));
    }

    #[test]
    fn read_produces_entries_matching_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meta.lsx");
        std::fs::write(&path, SAMPLE).unwrap();

        let entries = read(&path, "Mods/Meta.lsx").unwrap();
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].source, "A sturdy blade & shield-breaker.");
        assert_eq!(entries[0].contentuid, "Description#0");
        assert_eq!(entries[0].source_file, "Mods/Meta.lsx");
        assert_eq!(entries[1].source, "Second description");
        assert_eq!(entries[2].source, "Sword of 'Doom'");
    }

    #[test]
    fn empty_attributes_do_not_shift_occurrence_indexes() {
        // 旧实现在写回时按 `id="Description"` 出现次数定位，
        // 会跳过空值/非文本类型，导致序号错位；这里断言读写索引一致。
        let fields = scan_translatable(SAMPLE);
        let target = fields
            .iter()
            .find(|f| f.contentuid() == "Description#1")
            .unwrap();
        assert_eq!(&SAMPLE[target.value_span.clone()], "Second description");
    }

    #[test]
    fn write_only_touches_the_intended_attribute() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meta.lsx");
        std::fs::write(&path, SAMPLE).unwrap();

        let mut entry =
            TranslationEntry::new("Meta.lsx", "Description#1", "1", "Second description");
        entry.mark_translated("第二条描述");
        let mut display =
            TranslationEntry::new("Meta.lsx", "DisplayName#0", "1", "Sword of 'Doom'");
        display.mark_translated("末日之剑");

        write(&path, &[entry, display]).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();

        assert!(out.contains(r#"value="末日之剑""#));
        assert!(out.contains(r#"value="第二条描述""#));
        // 未翻译的字段保持原样，空值字段仍然是空的
        assert!(out.contains(r#"value="A sturdy blade &amp; shield-breaker.""#));
        assert!(out.contains(r#"value=""#));
        assert!(out.contains(r#"value="WPN_Sword_Internal""#));
        assert!(out.contains(r#"value="h1234abcd-5678""#));
    }

    #[test]
    fn entries_without_target_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meta.lsx");
        std::fs::write(&path, SAMPLE).unwrap();

        let entry = TranslationEntry::new("Meta.lsx", "Description#0", "1", "x");
        write(&path, &[entry]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), SAMPLE);
    }

    #[test]
    fn unknown_contentuid_is_skipped_not_misapplied() {
        let fields = scan_translatable(SAMPLE);
        let mut entry = TranslationEntry::new("Meta.lsx", "NoSuchField#0", "1", "x");
        entry.mark_translated("不该被写入");
        let plan = plan_replacements(&fields, &[entry]);
        assert!(plan.is_empty());
    }

    #[test]
    fn escapes_and_unescapes_roundtrip() {
        let raw = r#"a &amp; b &lt;c&gt; &quot;d&quot; &#39;e&#39; &#x4e2d;"#;
        let decoded = unescape_attribute(raw);
        assert_eq!(decoded, "a & b <c> \"d\" 'e' 中");
        let re_escaped = escape_attribute(&decoded);
        assert_eq!(unescape_attribute(&re_escaped), decoded);
    }

    #[test]
    fn unescape_keeps_unknown_entities_intact() {
        assert_eq!(unescape_attribute("a &nbsp; b"), "a &nbsp; b");
        assert_eq!(unescape_attribute("trailing &"), "trailing &");
        assert_eq!(unescape_attribute("&#xZZ;"), "&#xZZ;");
    }

    #[test]
    fn escapes_control_characters_that_are_illegal_in_attributes() {
        assert_eq!(escape_attribute("a\nb"), "a&#10;b");
        assert_eq!(escape_attribute("a\tb"), "a&#9;b");
    }

    #[test]
    fn write_preserves_bom() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meta.lsx");
        let mut raw = BOM.to_vec();
        raw.extend_from_slice(SAMPLE.as_bytes());
        std::fs::write(&path, &raw).unwrap();

        let mut entry = TranslationEntry::new("Meta.lsx", "Description#0", "1", "x");
        entry.mark_translated("第一条描述");
        write(&path, &[entry]).unwrap();

        let out = std::fs::read(&path).unwrap();
        assert!(out.starts_with(BOM));
    }

    #[test]
    fn tag_scanner_handles_comments_cdata_and_attribute_quotes() {
        let xml = r#"<root>
  <!-- <attribute id="Name" type="LSString" value="comment" /> -->
  <attribute id="Name" type="LSString" value="a &gt; b" />
  <attribute id="Description" type="LSString" value="quote &quot; inside" />
</root>"#;
        let fields = scan_translatable(xml);
        assert_eq!(ids(&fields), vec!["Description#0"]);
        assert_eq!(fields[0].value, "quote \" inside");
    }

    #[test]
    fn single_quoted_attributes_are_supported() {
        let xml = r#"<attribute id='Description' type='LSString' value='Iron Sword' />"#;
        let fields = scan_translatable(xml);
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].value, "Iron Sword");
        assert_eq!(&xml[fields[0].value_span.clone()], "Iron Sword");
    }

    #[test]
    fn decode_contentuid_parses_field_and_index() {
        assert_eq!(
            decode_contentuid("Description#12"),
            Some(("Description".to_string(), 12))
        );
        assert_eq!(decode_contentuid("NoIndex"), None);
        assert_eq!(decode_contentuid("Bad#x"), None);
    }

    #[test]
    fn files_without_translatable_fields_yield_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meta.lsx");
        std::fs::write(&path, "<save><region id=\"x\"/></save>").unwrap();
        assert!(read(&path, "Meta.lsx").unwrap().is_empty());
    }

    #[test]
    fn non_utf8_lsx_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.lsx");
        std::fs::write(&path, [0x00, 0xFF, 0xFE, 0x41]).unwrap();
        assert_eq!(read(&path, "bad.lsx").unwrap_err().code(), "xml");
    }

    #[test]
    fn multiline_values_are_escaped_on_write() {
        let xml = r#"<attribute id="Description" type="LSString" value="old" />"#;
        let fields = scan_translatable(xml);
        let mut entry = TranslationEntry::new("m.lsx", "Description#0", "1", "old");
        entry.mark_translated("第一行\n第二行");
        let plan = plan_replacements(&fields, &[entry]);
        let out = apply_replacements(xml, &plan);
        assert!(out.contains(r#"value="第一行&#10;第二行""#));
        // 再读回来必须一致
        assert_eq!(scan_translatable(&out)[0].value, "第一行\n第二行");
    }
}
