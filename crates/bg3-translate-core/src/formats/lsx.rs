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

use super::content_list::{decode_entities, decode_reference_body};
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
/// 只处理「能解析出 contentuid 且有**可写回**译文」的条目（`error` 状态的条目
/// 即使 target 非空也要退回原文，见 [`TranslationEntry::has_writable_target`]）；
/// 找不到对应字段的条目会被记 warning 并跳过（而不是静默写错位置）。
///
/// 同一个区间只会被替换一次：`entries` 里出现重复 contentuid（前端重读/合并出
/// 错时会发生）会产生两个完全相同的区间，逐个从后往前替换会让第二次用到**已经
/// 失效**的偏移，把刚落盘的译文又切一刀。
pub fn plan_replacements(
    fields: &[LsxField],
    entries: &[TranslationEntry],
) -> Vec<(Range<usize>, String)> {
    let mut plan: Vec<(Range<usize>, String)> = Vec::new();
    for entry in entries {
        if !entry.has_writable_target() {
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
        if plan.iter().any(|(span, _)| *span == field.value_span) {
            log::warn!("LSX 字段 {}#{} 被重复提交，只应用第一条", id, occurrence);
            continue;
        }
        plan.push((field.value_span.clone(), entry.target.clone()));
    }
    // 从后往前替换，避免前面的替换影响后面的区间
    plan.sort_by_key(|(span, _)| std::cmp::Reverse(span.start));
    plan
}

/// 按区间替换生成新字符串。
///
/// 区间必须互不重叠且从后往前（[`plan_replacements`] 的排序保证）；一旦发现
/// 重叠就跳过——用失效偏移替换会把文件写坏，宁可少写一条也不能产出坏文件。
///
/// 替换值在转义**之前**先做一次实体还原（[`decode_entities`]）：模型照抄文件里的
/// 转义形态（`&amp;` / `&lt;LSTag&gt;`）时只转义一层，不会落盘成 `&amp;amp;`。
/// 顺序与 `content_list` 一致：**还原 → 丢非法字符（在 `escape_attribute` 里）
/// → 转义**；反过来 `&#1;` 会整段躲过过滤、以裸控制字符落盘。
pub fn apply_replacements(xml: &str, replacements: &[(Range<usize>, String)]) -> String {
    let mut out = xml.to_string();
    // 已应用的替换里最靠左的起点：后面的替换必须整体落在它左边
    let mut applied_start = out.len();
    for (span, value) in replacements {
        if span.start > span.end || span.end > out.len() {
            log::warn!("LSX 替换区间越界，已跳过: {span:?}");
            continue;
        }
        if span.end > applied_start {
            log::warn!("LSX 替换区间重叠，已跳过: {span:?}");
            continue;
        }
        // 还原不能塞进 `escape_attribute`：它必须是纯转义器
        // （`escapes_and_unescapes_roundtrip` 钉住了 `unescape(escape(x)) == x`）。
        let decoded = decode_entities(value);
        out.replace_range(span.clone(), &escape_attribute(&decoded));
        applied_start = span.start;
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
///
/// 解码规则**只有一份**：[`decode_reference_body`]（与 `content_list` 共用）。
/// 这里只负责「扫描出 `&…;` 引用体」这件事 —— 之前 lsx 自己那份 `decode_entity`
/// 更宽松（还接受大写 `#X`、带符号的数字、码位 0），与写回侧用的规则不一致，
/// T8 把它删掉改为委托；差异只出现在**非法 XML 引用**上：
/// `&#X41;` / `&#+65;` / `&#0;` 以前会被解出字符，现在按 XML 文法原样保留。
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
        match decode_reference_body(entity) {
            Some(text) => out.push_str(&text),
            None => out.push_str(&tail[..=semi]),
        }
        rest = &tail[semi + 1..];
    }
    out.push_str(rest);
    out
}

/// 转义成可安全放进双引号属性值的形式。
///
/// XML 1.0 的 `Char` 产生式不接受控制字符（除 `\t`/`\n`/`\r`），而且**连字符
/// 引用都不允许**（`&#1;` 同样非法），所以只能丢弃：留着它们会让整个 `.lsx`
/// 变成非法文件，游戏侧解析直接失败，丢的是整份元数据。
pub fn escape_attribute(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 8);
    let mut dropped = 0usize;
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
            _ if is_illegal_xml_char(ch) => dropped += 1,
            _ => out.push(ch),
        }
    }
    if dropped > 0 {
        log::warn!("属性值含 {dropped} 个 XML 非法控制字符，已丢弃以免产出非法 XML");
    }
    out
}

/// XML 1.0 的 `Char` 产生式不允许的字符。
fn is_illegal_xml_char(c: char) -> bool {
    matches!(
        c,
        '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}'
    )
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

    /// F-01：`status == error` 的条目即使 target 非空也不得改写字段。
    #[test]
    fn error_entries_are_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meta.lsx");
        std::fs::write(&path, SAMPLE).unwrap();

        let mut entry = TranslationEntry::new(
            "Meta.lsx",
            "Description#0",
            "1",
            "A sturdy blade & shield-breaker.",
        );
        entry.mark_translated("坏译文：丢了 & 与标签");
        entry.mark_error("结构校验未通过：占位符 {1} 缺失（已重试 1 次）");

        assert!(entry.has_target(), "target 仍然保留给用户看");
        let fields = scan_translatable(SAMPLE);
        assert!(
            plan_replacements(&fields, std::slice::from_ref(&entry)).is_empty(),
            "error 条目不参与替换"
        );

        write(&path, &[entry]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            SAMPLE,
            "error 条目必须退回原文（字段保持原样）"
        );
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

    /// 同一 contentuid 出现两次时，两个替换区间完全相同：从后往前替换会导致
    /// 第二次用的是**已经失效**的区间，把刚落盘的译文又切一刀。
    ///
    /// 复现（修复前）：`value="old"` 写两个译文 → `value="新二一"`（串了）。
    #[test]
    fn duplicate_entries_are_applied_once_instead_of_corrupting_the_value() {
        let xml = r#"<save><region id="x"><node id="n"><attribute id="Description" type="LSString" value="old" /></node></region></save>"#;
        let fields = scan_translatable(xml);

        let mut first = TranslationEntry::new("m.lsx", "Description#0", "1", "old");
        first.mark_translated("新一");
        let mut second = first.clone();
        second.mark_translated("新二");

        let plan = plan_replacements(&fields, std::slice::from_ref(&first));
        assert_eq!(plan.len(), 1, "单条必须只有一个替换区间");

        let out = apply_replacements(xml, &plan);
        assert_eq!(scan_translatable(&out)[0].value, "新一");
        assert!(!out.contains("新二一"));

        // ② 同一条目被提交两次 → 仍然只能出现一个区间（否则第二次会用失效偏移）
        let dup_plan = plan_replacements(&fields, &[first.clone(), second.clone()]);
        assert_eq!(
            dup_plan.len(),
            1,
            "重复 contentuid 必须只产生一个替换区间: {dup_plan:?}"
        );
        let out_dup = apply_replacements(xml, &dup_plan);
        assert_eq!(first_value(&out_dup), "新一", "实际: {out_dup}");

        // ③ 直接给一个含重复区间的计划，也必须只生效一次（纵深防御）
        let doubled: Vec<_> = plan.iter().cloned().chain(plan.iter().cloned()).collect();
        let out2 = apply_replacements(xml, &doubled);
        assert_eq!(first_value(&out2), "新一", "实际: {out2}");

        // ④ 重叠但不相等的区间同样要拒绝（防止用失效偏移把文件切坏）
        let span = fields[0].value_span.clone();
        let overlapping = vec![
            (span.clone(), "甲".to_string()),
            ((span.start + 1)..span.end, "乙".to_string()),
        ];
        let out3 = apply_replacements(xml, &overlapping);
        assert_eq!(first_value(&out3), "甲", "实际: {out3}");
    }

    /// 取第一个可翻译字段的值（测试内部用）。
    fn first_value(xml: &str) -> String {
        scan_translatable(xml)
            .first()
            .map(|f| f.value.clone())
            .unwrap_or_default()
    }

    /// XML 1.0 不允许控制字符：落进属性值就是整个 `.lsx` 变非法文件。
    ///
    /// 复现（修复前）：`escape_attribute("你好\u{1}世界")` 原样保留 0x01。
    #[test]
    fn illegal_control_characters_never_reach_the_attribute_value() {
        let mut entry = TranslationEntry::new("m.lsx", "Description#0", "1", "old");
        entry.mark_translated("你好\u{1}世界\u{7}");

        let xml = r#"<save><node><attribute id="Description" type="LSString" value="old" /></node></save>"#;
        let plan = plan_replacements(&scan_translatable(xml), &[entry]);
        let out = apply_replacements(xml, &plan);

        assert!(
            !out.contains('\u{1}') && !out.contains('\u{7}'),
            "非法控制字符必须被丢弃: {out:?}"
        );
        assert_eq!(scan_translatable(&out)[0].value, "你好世界");
        // 合法的换行/制表符仍按字符引用写出（不能变成裸控制字符）
        assert_eq!(escape_attribute("a\nb"), "a&#10;b");
        assert_eq!(escape_attribute("a\tb"), "a&#9;b");
    }

    // ─────────────────────────────────────────────────────────────
    // T8：写回前先还原实体（与 content_list 共用同一份规则）
    // ─────────────────────────────────────────────────────────────

    const ONE_FIELD: &str =
        r#"<save><node><attribute id="Description" type="LSString" value="原文" /></node></save>"#;

    /// 走真实 `write` 路径把一条译文写进单字段 `.lsx`，返回落盘内容。
    fn write_description(target: &str) -> String {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Meta.lsx");
        std::fs::write(&path, ONE_FIELD).unwrap();
        let mut entry = TranslationEntry::new("Meta.lsx", "Description#0", "1", "原文");
        entry.mark_translated(target);
        write(&path, &[entry]).unwrap();
        std::fs::read_to_string(&path).unwrap()
    }

    /// 读侧（`unescape_attribute`）与写回侧共用同一份实体规则。
    ///
    /// 这张表同时跑两条路，钉住「跨格式只有一份规则」：任意一行的两边必须给出
    /// 相同结果（包括非法 / 未知引用一律原样保留）。
    #[test]
    fn entity_rule_agrees_with_content_list() {
        for (raw, expected) in [
            ("&amp;", "&"),
            ("&lt;", "<"),
            ("&gt;", ">"),
            ("&quot;", "\""),
            ("&apos;", "'"),
            ("&#65;", "A"),
            ("&#x41;", "A"),
            ("&#x4e2d;", "中"),
            ("&#x1f600;", "\u{1f600}"),
            ("&amp;amp;", "&amp;"),
            ("a & b", "a & b"),
            ("&nbsp;", "&nbsp;"),
            ("&lt", "&lt"),
            ("&#;", "&#;"),
            ("&#xZZ;", "&#xZZ;"),
            ("&#0;", "&#0;"),
            ("&#X41;", "&#X41;"),
            ("&#-1;", "&#-1;"),
        ] {
            assert_eq!(unescape_attribute(raw), expected, "LSX 读侧 {raw:?}");
            assert_eq!(decode_entities(raw).as_ref(), expected, "共享规则 {raw:?}");
        }
    }

    /// 写回前先还原实体：模型照抄文件里的转义形态时只转义**一层**。
    ///
    /// 复现（修复前）：`escape_attribute(value)` 直接转义 → 模型输出 `&amp;`
    /// 落盘成 `&amp;amp;`（游戏读到 `&amp;`）、`&lt;LSTag&gt;` 落盘成
    /// `&amp;lt;LSTag&amp;gt;`（富文本标签失效）。与 T6 修掉的 D1 同类。
    #[test]
    fn entity_in_target_is_decoded_before_escaping() {
        // (译文, 落盘必须出现, 读回来必须等于, 说明)
        let cases = [
            (
                "a &amp; b",
                r#"value="a &amp; b""#,
                "a & b",
                "`&amp;` 只转义一层",
            ),
            (
                r#"&lt;LSTag&gt;x&lt;/LSTag&gt;"#,
                r#"value="&lt;LSTag&gt;x&lt;/LSTag&gt;""#,
                "<LSTag>x</LSTag>",
                "标签只转义一层",
            ),
            ("a & b", r#"value="a &amp; b""#, "a & b", "裸 `&` 照常转义"),
            (
                "没有实体",
                r#"value="没有实体""#,
                "没有实体",
                "无实体不回归",
            ),
        ];
        for (target, written, read_back, label) in cases {
            let out = write_description(target);
            assert!(out.contains(written), "[{label}] 落盘不对: {out}");
            assert!(!out.contains("&amp;amp;"), "[{label}] 二次转义: {out}");
            assert!(!out.contains("&amp;lt;"), "[{label}] 标签二次转义: {out}");
            assert_eq!(
                scan_translatable(&out)[0].value,
                read_back,
                "[{label}] 读回来不对: {out}"
            );
        }
    }

    /// `escape(decode(T))` 是**不动点**：把落盘值再当译文写一遍，字节不变。
    #[test]
    fn rewriting_the_written_value_is_byte_stable() {
        for target in [
            r#"&lt;LSTag&gt;x&lt;/LSTag&gt;"#,
            "a &amp; b",
            "a & b",
            "&nbsp;",
            "&#xZZ;",
            "没有实体",
        ] {
            let first = write_description(target);
            let written_back = scan_translatable(&first)[0].value.clone();
            let second = write_description(&written_back);
            assert_eq!(first, second, "{target:?} 两轮写回不稳定");
        }
    }

    /// 非法 / 残缺 / 未知引用一律原样保留（一个字符都不吞）。
    #[test]
    fn malformed_entities_in_target_are_kept_verbatim() {
        for raw in [
            "&lt",       // 没有分号
            "&#;",       // 数值引用缺数字
            "&#xZZ;",    // 非法十六进制
            "&unknown;", // 未定义实体
            "&",         // 裸 &
            "&#X41;",    // 大写 X 不是 XML 的字符引用文法
            "&#0;",      // 码位 0 非法
            "a & b",     // 正文里的裸 &
            "&lt &amp;", // 残缺引用后面跟一个合法引用
        ] {
            let out = write_description(raw);
            let field = scan_translatable(&out)[0].value.clone();
            let expected = if raw == "&lt &amp;" { "&lt &" } else { raw };
            assert_eq!(field, expected, "{raw:?} 被改写了: {out}");
        }
    }

    /// 顺序必须是**还原 → 丢非法字符 → 转义**：`&#1;` 解出的控制字符要被丢掉，
    /// 不能整段躲过过滤、以裸控制字符落盘（那会让整个 `.lsx` 变成非法文件）。
    #[test]
    fn char_ref_to_illegal_control_char_is_dropped_after_decoding() {
        let out = write_description("a&#1;b&#x0C;c");

        assert!(!out.contains('\u{1}') && !out.contains('\u{c}'), "{out:?}");
        assert!(out.contains(r#"value="abc""#), "{out}");
        assert_eq!(scan_translatable(&out)[0].value, "abc");
    }

    /// 回归基线：真实 MOD 的 `.lsx` 零译文写回必须**逐字节不变**。
    ///
    /// `write` 只替换「有可写回译文」的区间，所以未翻译条目提交上来时文件应当
    /// 一字不动 —— T8 的改动不许让这条回退（`meta.lsx` / `Rulebook.lsx` 都覆盖）。
    #[test]
    fn real_mod_lsx_files_round_trip_byte_identical() {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let zip = repo_root.join("samples/Appearance Edit Enhanced-899-3-1-3-1769898497.zip");
        assert!(zip.is_file(), "真实样本缺失: {}", zip.display());

        let tmp = tempfile::tempdir().unwrap();
        let (work_dir, files) =
            crate::pak::open_and_extract_in(zip.to_str().unwrap(), tmp.path()).unwrap();
        let lsx_files: Vec<_> = files
            .iter()
            .filter(|file| file.kind == crate::types::PakFileKind::MetadataLsx)
            .collect();
        assert!(!lsx_files.is_empty(), "真实样本里应该有 .lsx 文件");

        for file in &lsx_files {
            let path = work_dir.join("unpacked").join(&file.name);
            let before = std::fs::read(&path).unwrap();
            let entries = read(&path, &file.name).unwrap();
            write(&path, &entries).unwrap();
            let after = std::fs::read(&path).unwrap();
            println!(
                "{}: {} 字节 / {} 个条目 / `&` 出现 {} 次 → {}",
                file.name,
                before.len(),
                entries.len(),
                before.iter().filter(|byte| **byte == b'&').count(),
                if after == before {
                    "往返逐字节相同".to_string()
                } else {
                    format!("**不同**（写回后 {} 字节）", after.len())
                }
            );
            assert_eq!(after, before, "{} 零译文写回必须逐字节不变", file.name);
        }
    }
}
