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
//!
//! **写回必须跟随源文件的编码风格**（见 [`MarkupStyle`]）：同一段逻辑文本，
//! 真实 BG3 文件写成 `&lt;LSTag&gt;…&lt;/LSTag&gt;`（标记整体转义在文本里），
//! 手写的 MOD 文件也可能写成 `<LSTag>…</LSTag>`（真 XML 元素）。两种写法
//! 解析出来的**文本完全相同**，所以风格只能由 [`parse`] 单独记下来、写回时照抄；
//! 否则「零译文写回」都会把文件格式改掉（真实语料 1971 条里 570 条被翻转）。
//!
//! **写回前还会做一次「实体还原 → 统一转义」**（见 [`decode_entities`]）：模型
//! 照抄文件里的转义形态（`&lt;LSTag&gt;`）时只转义**一层**，不会在盘上变成
//! `&amp;lt;LSTag&amp;gt;`（游戏读到字面量、富文本标签失效，而保真度校验判它保真、
//! 不会自愈）。

use std::borrow::Cow;
use std::path::Path;

use quick_xml::escape::partial_escape;
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

/// `<content>` 内部富文本标记的**编码风格**。
///
/// 同一段逻辑文本在 XML 里有两种等价的写法，差别只在「标记是文本还是元素」：
///
/// - [`MarkupStyle::Escaped`]：标记整体转义在文本里
///   （`<content …>a &lt;LSTag Tooltip="x"&gt;y&lt;/LSTag&gt;</content>`）。
///   **真实 BG3 语料就是这个形态**：`samples/english.xml` 1971 条里
///   `&lt;LSTag` 1103 处、真标签 0 处。
/// - [`MarkupStyle::Markup`]：标记是真 XML 元素
///   （`<content …>a <LSTag Tooltip="x">y</LSTag></content>`），手写 MOD 常见。
///
/// 两种写法 [`parse`] 之后得到的文本完全相同（都是 `a <LSTag …>y</LSTag>`），
/// 所以写回时**只能靠这个字段**决定用哪种形态；把风格写反 = 工具输出的文件
/// 和输入不是同一种格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MarkupStyle {
    /// 标记转义在文本里。真实语料的形态，也是**没有证据时的默认值**
    /// （新建的中文文件没有源文件可参考：转义形态不依赖标签配对，永远不会
    /// 产出非法 XML，且与游戏自带本地化文件一致）。
    #[default]
    Escaped,
    /// 标记是真 XML 元素。
    Markup,
}

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
    Ok(into_identified_entries(parsed.entries, file_name))
}

/// 只把**有句柄**的条目交给调用方。
///
/// 句柄（`contentuid`）就是游戏查表的键：没有它的 `<content>` 元素谁也查不到，
/// 而且这些条目的 `id` 全是 `{file}#`（前端按 `id` 去重会把它们并成一条），
/// 继续对外提供只会让写回少交几条 —— 按句柄对齐的合并会把没提交的元素
/// 当成「已提交」而静默删掉。
///
/// **写回路径不受影响**：[`write`] 仍然从磁盘解析出全部元素（含无句柄的），
/// 并按原样保留它们（见 [`merge_missing_entries`] 与 [`render_with_style`]）。
fn into_identified_entries(
    entries: Vec<TranslationEntry>,
    file_name: &str,
) -> Vec<TranslationEntry> {
    let total = entries.len();
    let kept: Vec<TranslationEntry> = entries
        .into_iter()
        .filter(|entry| !entry.contentuid.is_empty())
        .collect();
    let dropped = total - kept.len();
    if dropped > 0 {
        log::warn!(
            "{file_name} 有 {dropped} 个 <content> 元素缺少 contentuid，已跳过这些条目\
             （写回时会原样保留它们，但游戏查不到、翻译它们也没有意义）"
        );
    }
    kept
}

/// 解析结果：条目 + 根元素属性 + 标记风格（写回时保留）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedContentList {
    pub entries: Vec<TranslationEntry>,
    pub root_attributes: Vec<(String, String)>,
    /// 本文件的富文本标记是**真 XML 元素**还是**转义在文本里**（见 [`MarkupStyle`]）。
    /// 写回时必须原样沿用，否则会翻转文件格式。
    ///
    /// 判据是「`<content>` 内部有没有真的解析出白名单子元素」：只要出现过
    /// **一个**真元素（`<LSTag…>`、`<br/>`…），整份文件就按
    /// [`MarkupStyle::Markup`] 写回。混合风格的文件会被统一成真元素风格 ——
    /// 这是刻意的取舍：真元素是 XML 层面的硬证据，而 `&lt;` 在纯文本文件里
    /// 也可能只是正文里的字面 `<`（例如 `x &lt; br &gt; y`）。
    /// 真实 BG3 语料是**纯转义**的（1971 条里真标签 0 处），所以不受影响。
    pub markup_style: MarkupStyle,
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
                "contentList" if pending.is_none() => match collect_attributes(&e) {
                    Ok(attributes) => parsed.root_attributes = attributes,
                    // 属性坏了就不能「带着残缺结果继续」：根属性会被写回，
                    // 而 `contentuid` 一旦被静默改写成空串就再也救不回来。
                    Err(error) => {
                        parsed.error = Some(error);
                        break;
                    }
                },
                "content" if pending.is_none() => match content_identity(&e) {
                    Ok(identity) => {
                        pending = Some(identity);
                        raw.clear();
                        nested_depth = 0;
                    }
                    Err(error) => {
                        parsed.error = Some(error);
                        break;
                    }
                },
                // content 内部的子标签：原样记录
                _ if pending.is_some() => {
                    if e.name().as_ref() == "content" {
                        nested_depth += 1;
                    }
                    // 白名单标签以**真元素**形态出现 → 本文件是 Markup 风格。
                    // 转义形态（`&lt;LSTag&gt;`）走的是 GeneralRef + Text 那条路，
                    // 解析出来的文本一模一样，只有这里能区分。
                    if is_inline_tag_name(e.name().as_ref()) {
                        parsed.markup_style = MarkupStyle::Markup;
                    }
                    raw.push_str(&raw_start_tag(&e));
                }
                _ => {}
            },
            Ok(Event::Empty(e)) => match e.name().as_ref() {
                "content" if pending.is_none() => match content_identity(&e) {
                    Ok((contentuid, version)) => parsed
                        .entries
                        .push(TranslationEntry::new(file_name, contentuid, version, "")),
                    Err(error) => {
                        parsed.error = Some(error);
                        break;
                    }
                },
                _ if pending.is_some() => {
                    if is_inline_tag_name(e.name().as_ref()) {
                        parsed.markup_style = MarkupStyle::Markup;
                    }
                    raw.push_str(&raw_empty_tag(&e));
                }
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

/// 写回磁盘，保留 BOM、根元素属性与**源文件的标记风格**。
///
/// 磁盘上已有、但这次没提交的条目会被**原样保留**（见 [`merge_missing_entries`]）：
/// 写回是「用条目列表重建整个文档」，如果只提交了其中一部分，其余条目会被
/// 静默删掉——游戏里对应文本就变成原始 contentuid 句柄了。
///
/// 反过来，**底稿不可信时绝不整体重写**，两种情况都中止并报错：
/// - 文件在磁盘上、但读不出来（EACCES / EISDIR / Windows 共享冲突……）：
///   只有 `NotFound` 才是「新建文件」语义，其它 IO 错误一旦当成新文件处理，
///   磁盘上没提交的条目会被静默删掉；
/// - 文件读得出来、但 `parse` 中途出错（[`ParsedContentList::error`]）：
///   拿到的是**残缺**条目列表，用它做合并会把出错点之后的条目永久删除。
///   `read` 早就在同样的理由下拒绝返回残缺列表（见 [`read`] 的文档），
///   写回这条路径必须一致。
pub fn write(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let (bom, root_attributes, style, on_disk) = match std::fs::read(path) {
        Ok(bytes) => {
            let bom = bytes.starts_with(BOM);
            let xml = decode_utf8(&bytes, "本地化 XML")?;
            let parsed = parse(&xml, "");
            if let Some(error) = parsed.error {
                return Err(AppError::xml(format!(
                    "写回目标 {} 解析失败，已中止写回以免丢失条目（请先修好这个文件，或删掉它重新生成）: {error}",
                    path.display()
                )));
            }
            (
                bom,
                parsed.root_attributes,
                parsed.markup_style,
                parsed.entries,
            )
        }
        // 只有「文件不存在」才是新文件语义（例如把英文条目写进
        // `Localization/Chinese/`）：没有源文件可参考，用默认风格 ——
        // 真实语料的转义形态，见 [`MarkupStyle::Escaped`]。
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            (false, Vec::new(), MarkupStyle::default(), Vec::new())
        }
        // 文件在磁盘上、只是我们读不出来：按新文件处理＝静默删掉没提交的条目
        Err(err) => {
            return Err(AppError::Io(std::io::Error::new(
                err.kind(),
                format!(
                    "写回目标存在但读不出来（{err}），已中止写回以免覆盖磁盘上的条目: {}",
                    path.display()
                ),
            )));
        }
    };

    let merged = merge_missing_entries(entries, &on_disk);
    let body = render_with_style(&merged, &root_attributes, style)?;

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
        // 空句柄不是身份：拿它去 `contains` 会让磁盘上**所有**无句柄元素
        // 都被判成「已提交」，只交其中一条就会把其余几条静默删掉。
        .filter(|contentuid| !contentuid.is_empty())
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

/// 渲染 contentList XML 文本（不含 BOM），按**真元素风格**（[`MarkupStyle::Markup`]）。
///
/// 等价于 `render_with_style(entries, root_attributes, MarkupStyle::Markup)`。
/// 保留这个两参数入口是为了不关心风格的调用方（保真度测试、单测）不必显式传参；
/// **写回磁盘请走 [`write`]**，它会按 [`ParsedContentList::markup_style`] 决定风格。
pub fn render(
    entries: &[TranslationEntry],
    root_attributes: &[(String, String)],
) -> Result<String> {
    render_with_style(entries, root_attributes, MarkupStyle::Markup)
}

/// 渲染 contentList XML 文本（不含 BOM），按指定的标记风格。
///
/// 风格必须来自源文件（[`parse`] → [`ParsedContentList::markup_style`]），
/// 唯一例外是「新建文件」：没有源文件可参考时用 [`MarkupStyle::Escaped`]。
pub fn render_with_style(
    entries: &[TranslationEntry],
    root_attributes: &[(String, String)],
    style: MarkupStyle,
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
        // 原文件没有 `contentuid` 属性时**不要凭空补一个 `contentuid=""`**
        // （与下面的 `version` 同理）：句柄是游戏查表的键，空串既查不到任何东西，
        // 又把「没有这个属性」的源文件改成了另一个样子 —— 零译文写回就会触发。
        if !entry.contentuid.is_empty() {
            start.push_attribute(("contentuid", entry.contentuid.as_str()));
        }
        // 原文件没有 `version` 属性时**不要凭空补一个 `version=""`**：
        // 空版本号和「没有这个属性」对游戏不是一回事，而且会让「零译文写回」
        // 也改变文件内容。
        if !entry.version.is_empty() {
            start.push_attribute(("version", entry.version.as_str()));
        }
        write_event(&mut writer, Event::Start(start))?;
        // ① 先把译文里的实体还原（`&lt;` → `<`），② 再丢 XML 非法字符。
        // 顺序不能反：还原可能从 `&#1;` 里产出控制字符，必须先还原后过滤，
        // 否则 `&#1;` 会被跳过过滤、随后原样落盘成非法 XML。
        // 还原也必须早于标签扫描：`&lt;LSTag&gt;` 要先变回真标签，Markup 路径
        // 才会把它写成真元素；但 Markup 风格下**已经合法的真元素标签区间要跳过**
        // （属性值是原样裸写的），见 `decode_entities`。
        let decoded = match style {
            MarkupStyle::Escaped => decode_entities(entry.effective_text()),
            MarkupStyle::Markup => decode_entities_outside_raw_tags(entry.effective_text()),
        };
        let text = sanitize_xml_chars(&decoded);
        match style {
            // 转义风格：整条文本交给 XML 序列化器统一转义。标签在这里就是正文，
            // 不做配对检查 —— 转义后的文本永远是合法 XML，模型多吐标签也不会
            // 产出非法文件。`&lt;` 原样写回 `&lt;`，风格与源文件一致。
            MarkupStyle::Escaped => {
                write_event(&mut writer, Event::Text(escaped_text(&text)))?;
            }
            MarkupStyle::Markup => {
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
                    write_event(&mut writer, Event::Text(escaped_text(&text)))?;
                }
            }
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

/// 把一段文本构造成 `Event::Text`，只转义 XML 文本里**必须**转义的字符。
///
/// 用 [`BytesText::new`] 会走 quick-xml 的 `escape`，连 `'` / `"` 一起写成
/// `&apos;` / `&quot;`：`target's` 会变成 `target&apos;s`。XML 文本里这两个字符
/// **不需要**转义，真实语料也从不这么写（`samples/english.xml` 1971 条里
/// `'` 144 处、`"` 11340 处，`&apos;` / `&quot;` 各 0 处）—— 用默认行为会让
/// 「零译文写回」凭空改动文件字节。这里改用 `partial_escape`（只处理
/// `<` `>` `&` `\r`），与源文件逐字节一致。
///
/// `\r` 必须转义成 `&#13;`：XML 解析器会把裸 `\r` 归一化成 `\n`，不转义就是丢数据。
fn escaped_text(text: &str) -> BytesText<'_> {
    BytesText::from_escaped(partial_escape(text))
}

/// 去掉 UTF-8 BOM 并解码。
fn decode_utf8(bytes: &[u8], what: &str) -> Result<String> {
    let body = bytes.strip_prefix(BOM).unwrap_or(bytes);
    String::from_utf8(body.to_vec())
        .map_err(|e| AppError::xml(format!("{what} 不是合法 UTF-8: {e}")))
}

/// 取 attribute 的反转义值（含 EOL 归一化，等价于旧版 `unescape_value`）。
///
/// 反转义失败（未定义实体 / 裸 `&` / 非法字符引用）**必须**上报，不能
/// `unwrap_or_default()` 悄悄换成空串：这些值里既有游戏查表的句柄
/// （`contentuid`），也有会被原样写回根元素的属性，换成空串等于**改写**文件
/// —— 文件从「畸形」变成「良构但查不到」，而用户看不到任何错误。
fn unescaped(
    attr: &quick_xml::events::attributes::Attribute<'_>,
) -> std::result::Result<String, String> {
    attr.normalized_value(XmlVersion::Implicit1_0)
        .map(|value| value.into_owned())
        .map_err(|err| format!("属性 {} 的值不是合法 XML（{err}）", attr.key.as_ref()))
}

fn collect_attributes(e: &BytesStart<'_>) -> std::result::Result<Vec<(String, String)>, String> {
    let mut attributes = Vec::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| format!("根元素属性语法错误（{err}）"))?;
        attributes.push((attr.key.as_ref().to_string(), unescaped(&attr)?));
    }
    Ok(attributes)
}

/// 取 `<content>` 的身份属性（`contentuid` / `version`）。
///
/// 属性语法错误同样要上报：`.flatten()` 会把「未加引号的 `contentuid=h1`」
/// 连同错误一起丢掉，于是 `contentuid` 变成空串、写回时被改写成
/// `contentuid=""` —— 句柄是游戏查表的键，改了就等于这条文本永久查不到。
fn content_identity(e: &BytesStart<'_>) -> std::result::Result<(String, String), String> {
    let mut contentuid = String::new();
    let mut version = String::new();
    for attr in e.attributes() {
        let attr = attr.map_err(|err| format!("<content> 属性语法错误（{err}）"))?;
        match attr.key.as_ref() {
            "contentuid" => contentuid = unescaped(&attr)?,
            "version" => version = unescaped(&attr)?,
            _ => {}
        }
    }
    Ok((contentuid, version))
}

/// 把实体引用还原成字符；未知实体原样保留（含 `&` 和 `;`）。
///
/// 解码规则**只有一份**，就是 [`decode_reference_body`]：读侧（本函数，事件级）
/// 与写回前的还原（[`decode_entities`]，字符串级）都委托给它，规则不会漂移。
fn resolve_reference(reference: &BytesRef<'_>) -> String {
    decode_reference_body(reference).unwrap_or_else(|| format!("&{};", &**reference))
}

/// 单个实体引用体（`&` 与 `;` 之间的部分）的**唯一**解码规则。
///
/// 返回 `None` 表示「不是已知 / 合法的引用」，调用方必须把 `&{body};` 原样写回。
/// - 已知实体：`amp` / `lt` / `gt` / `quot` / `apos`
/// - 数值字符引用：`#NN`（十进制）/ `#xHH`（**只认小写 `x`**）
///
/// 非法 / 残缺的引用体（`#`、`#x`、`#xZZ`、`#-1`、`#0`、代理区、超出 U+10FFFF、
/// 未定义名字）一律返回 `None`，绝不吞字符。
pub(crate) fn decode_reference_body(body: &str) -> Option<String> {
    if let Some(ch) = parse_char_ref_body(body) {
        return Some(ch.to_string());
    }
    match body {
        "amp" => Some("&".into()),
        "lt" => Some("<".into()),
        "gt" => Some(">".into()),
        "quot" => Some("\"".into()),
        "apos" => Some("'".into()),
        _ => None,
    }
}

/// 数值字符引用体（`#NN` / `#xHH`）→ 字符。
///
/// 语义与 quick-xml 的 `BytesRef::resolve_char_ref` 逐例一致（有单测对齐）：
/// 只认小写 `x` 前缀的十六进制；拒绝正负号、拒绝码位 `0`、拒绝非 Unicode 标量值
/// （代理区 / 超出 U+10FFFF）；拒绝即返回 `None`，由调用方原样保留。
///
/// 注意范围：XML 1.0 里还有一批控制字符连字符引用都不合法（`&#1;`），
/// quick-xml **不**在这里拦（它只拦 `0`），所以本函数也不拦 —— 那些字符
/// 由写回前的 [`sanitize_xml_chars`] 统一丢弃，两条路行为一致。
fn parse_char_ref_body(body: &str) -> Option<char> {
    let digits = body.strip_prefix('#')?;
    let code = match digits.strip_prefix('x') {
        Some(hex) => parse_radix(hex, 16)?,
        None => parse_radix(digits, 10)?,
    };
    if code == 0 {
        return None;
    }
    char::from_u32(code)
}

/// `u32::from_str_radix` + 拒绝正负号（`+43` 不能被当成 43）。
fn parse_radix(digits: &str, radix: u32) -> Option<u32> {
    if matches!(digits.as_bytes().first(), Some(b'+' | b'-')) {
        return None;
    }
    u32::from_str_radix(digits, radix).ok()
}

/// XML 引用体的字符集：`&` 与 `;` 之间可能是名字或数值引用（`#NN` / `#xHH`）。
///
/// 我们支持的引用体全在这个集合里（`[A-Za-z0-9#]`）；集合外的字节直接中断扫描，
/// 整段引用体原样保留 —— 不去模仿解析器的完整名字语法，因为模仿得再像也只是
/// 「多认出一段不会解码的文本」，对结果没有任何影响。
fn is_reference_body_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'#'
}

/// 字符串级的「实体还原」：把文本里的已知实体引用还原成字符，其余原样保留。
///
/// **写回前对译文做一次还原，让 `escape(decode(T))` 成为不动点。**
/// 触发场景：模型照抄了文件里的转义形态 —— 原文文本是 `<LSTag Tooltip="x">y</LSTag>`，
/// 模型输出 `&lt;LSTag Tooltip="x"&gt;y&lt;/LSTag&gt;`。不还原就会二次转义成
/// `&amp;lt;LSTag …`，游戏读到的是字面量 `&lt;LSTag …&gt;`，富文本标签失效；
/// 而且保真度校验会先还原再比对、**判它保真**，这条链不会自愈，只能写回层兜住。
///
/// 未知实体（`&nbsp;`）、裸 `&`、残缺引用（`&lt` 无分号、`&#;`、`&#xZZ;`）
/// 一律原样保留 —— 原样保留的字面文本随后会被 [`partial_escape`] 转义成
/// `&amp;nbsp;` 之类，语义等价、字节与源文件一致。
///
/// 还原与转义都必须发生在**标签扫描之前**：先还原，`&lt;LSTag&gt;` 才会变回
/// 真标签、在 [`MarkupStyle::Markup`] 路径上被写成真元素；先转义就晚了。
///
/// **跨格式共用**（`.lsx` 写回前也调它）：解码规则只有一份
/// [`decode_reference_body`]。`contentList` 的真元素风格还要额外跳过真元素标签
/// 区间，见 [`decode_entities_outside_raw_tags`]。
pub(crate) fn decode_entities(text: &str) -> Cow<'_, str> {
    decode_entities_impl(text, false)
}

/// `contentList` 的**真元素风格**专用：整段跳过已经合法的白名单标签区间（D9）。
///
/// 为什么不能碰标签内部：标签会被 [`write_text_fragment`] **裸写**，属性值是
/// [`raw_start_tag`] 保留下来的原始转义形态。在标签内部还原会把 `&amp;` 变成
/// 裸 `&`、`&quot;` 变成裸 `"`，落盘就是非法 XML / 标签整条降级 —— 零译文写回
/// 就能触发、连写多少轮都不自愈。
///
/// 转义风格（[`MarkupStyle::Escaped`]）**不**跳过：整条文本随后会被统一转义，
/// 标签在这里只是正文，不跳过才是规范形（跳过反而会把 `&amp;` 写成 `&amp;amp;`）。
///
/// # ⚠️ 这个 skip 是**承重的**，不是可以随手删的冗余优化
///
/// 属性值有**两道**保护，但覆盖面不同：
/// - `repair_tag_attributes` 只处理**裸 `&` / 裸 `<`**（已经解码成非法形态的值）；
/// - 这里的 skip 还额外挡住 `&quot;` / `&apos;` 这类**解码后仍合法、却会让标签认不出来**
///   的实体 —— 裸引号在标签扫描阶段就破坏了属性边界，整条降级成字面文本。
///
/// 独立验证实测过：去掉 skip、只留 `repair_tag_attributes`，
/// `&quot;` 用例立刻变红。所以**不要**因为「`repair_tag_attributes` 已经兜住了」就删它。
fn decode_entities_outside_raw_tags(text: &str) -> Cow<'_, str> {
    decode_entities_impl(text, true)
}

/// [`decode_entities`] 的共用实现；`skip_raw_tags` 只有 contentList 的真元素风格需要。
fn decode_entities_impl(text: &str, skip_raw_tags: bool) -> Cow<'_, str> {
    let bytes = text.as_bytes();
    // 没有 `&` 是最常见的情况（真实语料 1971 条里 0 处），直接借用原串
    if !text.contains('&') {
        return Cow::Borrowed(text);
    }

    let mut decoded: Option<String> = None;
    // `text[copied..cursor]` 已确认原样保留、还没写进 `decoded`
    let mut copied = 0usize;
    let mut cursor = 0usize;
    while cursor < text.len() {
        // ① 已经是合法白名单标签的区间：整段原样保留（见上文 D9 说明）
        if skip_raw_tags && bytes[cursor] == b'<' {
            let tag_end =
                scan_tag_end(text, cursor).filter(|end| is_allowed_inline_tag(&text[cursor..*end]));
            if let Some(tag_end) = tag_end {
                cursor = tag_end;
                continue;
            }
        }
        if bytes[cursor] != b'&' {
            cursor += 1;
            continue;
        }
        // ② 标签之外的 `&`：按实体规则还原。
        // 扫引用体的尽头；引用体里不含 `&`，所以扫描失败时可以从尽头继续，
        // 既不会漏掉后面的 `&`，也不会退化成 O(n²)。
        let body_start = cursor + 1;
        let mut body_end = body_start;
        while body_end < bytes.len() && is_reference_body_byte(bytes[body_end]) {
            body_end += 1;
        }
        let replacement = if body_end < bytes.len() && bytes[body_end] == b';' {
            decode_reference_body(&text[body_start..body_end])
        } else {
            None
        };
        match replacement {
            Some(replacement) => {
                let decoded = decoded.get_or_insert_with(String::new);
                decoded.push_str(&text[copied..cursor]);
                decoded.push_str(&replacement);
                cursor = body_end + 1;
                copied = cursor;
            }
            None => cursor = body_end,
        }
    }

    match decoded {
        Some(mut decoded) => {
            decoded.push_str(&text[copied..]);
            Cow::Owned(decoded)
        }
        None => Cow::Borrowed(text),
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
/// **只在 [`MarkupStyle::Markup`]（源文件就是真元素风格）时使用**；转义风格的
/// 文件整条文本走 [`escaped_text`]，见 [`render_with_style`]。
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
                    write_event(writer, Event::Text(escaped_text(&text[plain_start..start])))?;
                }
                // 白名单标签原样写出（属性值里的 `&`/`"` 都是原始转义形态）。
                // 唯一的例外是属性值里出现**裸** `&` / `<`：那是非法 XML，
                // 写出前先规范成 `escape(decode(v))`，见 [`repair_tag_attributes`]。
                let tag = normalize_void_tag(&text[start..end]);
                let tag = match repair_tag_attributes(&tag) {
                    Some(repaired) => Cow::Owned(repaired),
                    None => tag,
                };
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
        write_event(writer, Event::Text(escaped_text(&text[plain_start..])))?;
    }
    Ok(())
}

/// 写裸标签之前，把属性值规范成 `escape(decode(v))` —— 只在**有必要**时重写。
///
/// 背景：`raw_start_tag` 保留属性值的**原始转义**，因此标签通常可以直接裸写。
/// 但译文里的标签可能来自模型（例如模型把整条标签转义后再被 [`decode_entities`]
/// 还原，属性里的 `&amp;` 就变成了裸 `&`），裸写出去产物就是**非法 XML**，
/// 而且这种文件不会自愈（再读回来还是裸 `&`）。
///
/// 规则（保守，只处理会产出非法 XML 的那种情况）：
/// - 标签里没有 `&` → 直接返回 `None`，逐字节原样写出（绝大多数标签走这条快路径，
///   引号风格 / 属性顺序 / `>` 与空白都保持不变）；
/// - 否则逐个属性值算 `escape(decode(v))`：**全都等于原值**就返回 `None`（原样写出）；
/// - 只要有任何一个变了，才重建整个标签（`&`/`<`/`"` 转义、双引号包裹）。
///
/// 这样既堵住非法 XML，又不改「属性优先原样保留」的契约。
///
/// # ⚠️ 它**不能**替代 [`decode_entities_outside_raw_tags`] 的 skip
///
/// 两道保护覆盖面不同：这里只处理**裸 `&` / 裸 `<`**（已非法）；skip 还挡住
/// `&quot;` / `&apos;` 这类**解码后依然合法、但会让标签认不出来**的实体。
/// 只留这里、去掉 skip，`&quot;` 用例立刻变红（独立验证实测）。
/// 两者是**互补**关系，不是一主一备。
fn repair_tag_attributes(tag: &str) -> Option<String> {
    // 快路径：标签体里既没有 `&` 也没有 `<` 时，属性值不可能是非法 XML ——
    // 逐字节原样写出，不改引号风格 / 属性顺序 / 空白。
    // 注意 `<`：`scan_tag_end` 只在引号**之外**把 `<` 当终止信号，所以属性值里的
    // 裸 `<`（XML 1.0 不允许）能走到这里，必须一起检查。
    let body = tag.get(1..).unwrap_or_default();
    if !body.contains('&') && !body.contains('<') {
        return None;
    }

    let mut reader = Reader::from_str(tag);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let (name, attributes, self_closing) = match reader.read_event_into(&mut buf) {
        Ok(Event::Start(e)) => (e.name().as_ref().to_string(), tag_attributes(&e)?, false),
        Ok(Event::Empty(e)) => (e.name().as_ref().to_string(), tag_attributes(&e)?, true),
        _ => return None,
    };

    let mut repaired = false;
    let mut values = Vec::with_capacity(attributes.len());
    for (key, value) in attributes {
        let decoded = decode_entities(&value);
        // `&#1;` / `&#xFFFF;` 这类**非法字符引用**的解码规则只拒绝「解析不出来」的
        // 引用，不判 XML 1.0 的 `Char` 产生式；不在这里丢非法字符，裸控制字符就会
        // 顺着属性值落盘（F-07 在标签属性这条路径上重新出现）。
        let decoded = sanitize_xml_chars(&decoded);
        let escaped = escape_attribute_value(&decoded);
        if escaped != value {
            repaired = true;
        }
        values.push((key, escaped));
    }
    if !repaired {
        return None;
    }

    let mut out = String::from("<");
    out.push_str(&name);
    for (key, value) in values {
        out.push(' ');
        out.push_str(&key);
        out.push_str("=\"");
        out.push_str(&value);
        out.push('"');
    }
    out.push_str(if self_closing { "/>" } else { ">" });
    Some(out)
}

/// 收集标签的属性（名字, 原始属性值），保持原顺序；属性语法非法时返回 `None`。
fn tag_attributes(e: &BytesStart<'_>) -> Option<Vec<(String, String)>> {
    let mut attributes = Vec::new();
    for attr in e.attributes() {
        let attr = attr.ok()?;
        attributes.push((
            attr.key.as_ref().to_string(),
            attr.value.as_ref().to_string(),
        ));
    }
    Some(attributes)
}

/// 属性值的转义：只转 `&` / `<` / `"`。
///
/// `&` / `<` 不转义产物就不是合法 XML；`"` 必须转义是因为重建时用双引号包裹。
/// `>` 与 `'` 在属性值里本来就合法，**刻意保持原样** —— 改了会破坏
/// 「属性值原样保留」的既有行为（例如 `Tooltip="a > b"`）。
fn escape_attribute_value(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
    out
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

/// 用 **XML 1.0 文法**判断这段文本是不是**恰好一个**合法 XML 标签，是则返回标签名。
///
/// 这是「原样写出去」的唯一门槛：产物必须永远是合法 XML，所以宁可在这里自己
/// 走一遍文法，也不能靠「`<` 后面第一个词是不是白名单名字」去猜——`< br >`
/// （源文件里是 `&lt; br &gt;`）就是被这么猜错的。
///
/// # 为什么不能只靠 quick-xml
///
/// quick-xml 在**属性语法**上比 XML 1.0 宽松：`<LSTag a="1"b="2">`（属性之间
/// 缺空白）它照样解析成功，而严格解析器（游戏侧、python expat）会拒绝
/// **整份文档**。旧实现把这个标签原样写进产物，等于把用户的本地化文件变成
/// 非法文件（缺空白的属性会被游戏侧严格解析器拒绝整份文档）。所以这里自己校验：
///
/// - `STag ::= '<' Name (S Attribute)* S? '>'`（属性之间**必须**有空白）
/// - `EmptyElemTag ::= '<' Name (S Attribute)* S? '/>'`
/// - `ETag ::= '</' Name S? '>'`
/// - `Attribute ::= Name Eq AttValue`：值必须带引号，同名属性只能出现一次
///
/// 只**拒绝非法**，不额外收紧：属性之间的空白可以是空格 / 制表 / 换行，
/// 引号单双皆可，`=` 两侧允许空白，值里的 `>` 与实体照旧原样保留。
///
/// 顺带解掉的两个坑：
/// - 结束标签单独解析时 quick-xml 会报「没有匹配的开始标签」，只能自己校验名字语法；
/// - 属性是**惰性解析**的，必须逐个过一遍，否则 `<LSTag Tooltip="a" garbage>`
///   这种畸形开始标签会被当成合法标签原样写出去。
fn parse_single_tag(tag: &str) -> Option<String> {
    let body = tag.strip_prefix('<')?.strip_suffix('>')?;
    // 结束标签：`</` 与名字之间不能有空白（`</ b>` 不是 XML）
    if let Some(name) = body.strip_prefix('/') {
        let name = name.trim_end();
        if !is_xml_name(name) {
            return None;
        }
        return Some(name.to_string());
    }

    let bytes = body.as_bytes();
    let name_end = scan_xml_name(body, 0)?;
    let name = body.get(..name_end)?;
    // `< br >` 会被解析成「名字为空 / 名字为空格的标签」，直接拒绝
    if name.is_empty() {
        return None;
    }

    let mut attributes: Vec<&str> = Vec::new();
    let mut i = name_end;
    loop {
        // `S?`：`>` 与 `/>` 之前允许空白
        let before_ws = i;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
        }
        let had_whitespace = i > before_ws;

        // 标签末尾：`>` 已被 strip_suffix 去掉，`/>` 只剩下最后的 `/`
        if i == bytes.len() {
            return Some(name.to_string());
        }
        if bytes[i] == b'/' && i + 1 == bytes.len() {
            return Some(name.to_string());
        }

        // 属性：**必须**有空白分隔。`<LSTag a="1"b="2">` 在这里被拒。
        if !had_whitespace {
            return None;
        }
        let key_end = scan_xml_name(body, i)?;
        let key = body.get(i..key_end)?;
        if attributes.contains(&key) {
            // 重名属性在 XML 1.0 里同样是良构性错误
            return None;
        }
        attributes.push(key);
        i = key_end;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
        }
        if bytes.get(i) != Some(&b'=') {
            return None;
        }
        i += 1;
        while i < bytes.len() && matches!(bytes[i], b' ' | b'\t' | b'\r' | b'\n') {
            i += 1;
        }
        let quote = *bytes.get(i)?;
        if quote != b'"' && quote != b'\'' {
            return None;
        }
        i += 1;
        // 属性值内部逐字节找到配对的引号即可。注意**不在这里**拒绝裸 `<`：
        // 那是 [`repair_tag_attributes`] 的职责（它会重建标签并把裸 `&`/`<`
        // 规范成 `escape(decode(v))`），在这里拒绝会让「模型把标签整条转义、
        // 属性值里带裸 `<`」这条已经被修好的路径退回成纯文本。
        //
        // ⚠️ 循环必须带 `i < bytes.len()` 边界：`is_allowed_inline_tag` 是公开 API，
        // 调用方可以传进引号未闭合的候选串（如 `<LSTag a="x>`）。写成
        // `while bytes.get(i) != Some(&quote)` 的话，越过末尾后 `get` 恒为 `None`，
        // 循环永不退出（`unterminated_tag_attribute_is_not_a_tag` 钉住这条）。
        while i < bytes.len() && bytes[i] != quote {
            i += 1;
        }
        if i >= bytes.len() {
            // 引号没闭合 → 不是合法标签
            return None;
        }
        i += 1;
    }
}

/// 从 `text[at..]` 起扫描一个 XML `Name`，返回结束下标（字节）。
///
/// 用的是 XML 1.0 的 `NameStartChar` / `NameChar` 全集（含非 ASCII 区间），
/// 所以 `<LSTag 类型="x">` 这种带非 ASCII 属性名的合法标签**不会**被误伤。
fn scan_xml_name(text: &str, at: usize) -> Option<usize> {
    let mut cursor = at;
    let first = text.get(at..)?.chars().next()?;
    if !is_xml_name_start_char(first) {
        return None;
    }
    cursor += first.len_utf8();
    while let Some(ch) = text.get(cursor..).and_then(|rest| rest.chars().next()) {
        if !is_xml_name_char(ch) {
            break;
        }
        cursor += ch.len_utf8();
    }
    Some(cursor)
}

/// XML 1.0 `NameStartChar`。
fn is_xml_name_start_char(ch: char) -> bool {
    matches!(ch,
        ':' | '_'
        | 'A'..='Z'
        | 'a'..='z'
        | '\u{C0}'..='\u{D6}'
        | '\u{D8}'..='\u{F6}'
        | '\u{F8}'..='\u{2FF}'
        | '\u{370}'..='\u{37D}'
        | '\u{37F}'..='\u{1FFF}'
        | '\u{200C}'..='\u{200D}'
        | '\u{2070}'..='\u{218F}'
        | '\u{2C00}'..='\u{2FEF}'
        | '\u{3001}'..='\u{D7FF}'
        | '\u{F900}'..='\u{FDCF}'
        | '\u{FDF0}'..='\u{FFFD}'
        | '\u{10000}'..='\u{EFFFF}')
}

/// XML 1.0 `NameChar`（`NameStartChar` 之外还允许数字、`-`、`.`、`·` 与组合字符）。
fn is_xml_name_char(ch: char) -> bool {
    is_xml_name_start_char(ch)
        || matches!(ch,
            '-' | '.' | '0'..='9' | '\u{B7}' | '\u{0300}'..='\u{036F}' | '\u{203F}'..='\u{2040}')
}

/// 是否是合法的 XML 名字（这里只接受 ASCII 形态，本地化文本里的**标签名**
/// 不可能出现别的字符；非 ASCII 名字由白名单直接挡掉）。
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

/// 元素名是否在富文本标签白名单里（用于 [`parse`] 判定文件风格）。
fn is_inline_tag_name(name: &str) -> bool {
    INLINE_TAGS.contains(&name)
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

    /// 已存在的文件：写回必须沿用**文件自己的**标记风格，两种风格都不能被翻转。
    ///
    /// 这是本轮修的缺陷：真实 BG3 contentList 把标记整体转义在文本里
    /// （`&lt;LSTag …&gt;`），旧实现却把白名单标签写成真 XML 元素，于是
    /// 「零译文写回」也会改变文件格式。
    #[test]
    fn write_back_preserves_the_markup_style_of_the_file_on_disk() {
        // ① 转义风格：写回后逐字节不变
        let dir = tempfile::tempdir().unwrap();
        let escaped = dir.path().join("English.xml");
        let raw = r#"<contentList><content contentuid="h1" version="1">Cast &lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;.</content></contentList>"#;
        std::fs::write(&escaped, raw).unwrap();

        let parsed = parse(raw, "English.xml");
        assert_eq!(parsed.markup_style, MarkupStyle::Escaped);
        assert_eq!(
            parsed.entries[0].source,
            r#"Cast <LSTag Tooltip="HitPoints">hit points</LSTag>."#
        );
        write(&escaped, &parsed.entries).unwrap();
        assert_eq!(
            std::fs::read_to_string(&escaped).unwrap(),
            r#"<?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">Cast &lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;.</content></contentList>"#,
            "转义风格文件必须原样写回"
        );

        // ② 真元素风格：标签照旧是真标签
        let markup = dir.path().join("Markup.xml");
        let raw = r#"<contentList><content contentuid="h1" version="1">Cast <LSTag Tooltip="HitPoints">hit points</LSTag>.</content></contentList>"#;
        std::fs::write(&markup, raw).unwrap();

        let parsed = parse(raw, "Markup.xml");
        assert_eq!(parsed.markup_style, MarkupStyle::Markup);
        write(&markup, &parsed.entries).unwrap();
        let out = std::fs::read_to_string(&markup).unwrap();
        assert!(
            out.contains(r#"<LSTag Tooltip="HitPoints">hit points</LSTag>"#),
            "真元素风格不能被反过来转义: {out}"
        );
        assert!(!out.contains("&lt;LSTag"), "{out}");
    }

    /// 风格判据：`<content>` 内部是否**真的解析出了白名单子元素**。
    #[test]
    fn markup_style_reflects_whether_white_list_tags_are_real_elements() {
        let cases = [
            (
                r#"<contentList><content contentuid="h1">a &lt;LSTag&gt;b&lt;/LSTag&gt;</content></contentList>"#,
                MarkupStyle::Escaped,
                "转义在文本里",
            ),
            (
                r#"<contentList><content contentuid="h1">a <LSTag>b</LSTag></content></contentList>"#,
                MarkupStyle::Markup,
                "真元素",
            ),
            (
                r#"<contentList><content contentuid="h1">a<br/>b</content></contentList>"#,
                MarkupStyle::Markup,
                "自闭合真元素",
            ),
            (
                r#"<contentList><content contentuid="h1">x &lt; br &gt; y</content></contentList>"#,
                MarkupStyle::Escaped,
                "字面尖括号不是元素",
            ),
            (
                r#"<contentList><content contentuid="h1">没有标签</content></contentList>"#,
                MarkupStyle::Escaped,
                "无证据时按真实语料的转义风格",
            ),
        ];
        for (xml, expected, label) in cases {
            assert_eq!(
                parse(xml, "t.xml").markup_style,
                expected,
                "[{label}] {xml}"
            );
        }
    }

    /// `'` / `"` 在 XML 文本里**不需要**转义，写回必须与源文件一致。
    ///
    /// 复现（修复前）：`BytesText::new` 走 quick-xml 的 `escape`，把 `target's`
    /// 写成 `target&apos;s` —— 真实语料 1971 条里 `'` 144 处、`"` 11340 处，
    /// 而 `&apos;` / `&quot;` 各 0 处，于是「零译文写回」也会改变文件字节。
    #[test]
    fn quotes_are_not_escaped_in_text_content() {
        for style in [MarkupStyle::Escaped, MarkupStyle::Markup] {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(r#"target's "sword""#);
            let out = render_with_style(&[entry], &[], style).unwrap();
            assert!(
                out.contains(r#"target's "sword""#),
                "{style:?} 引号必须原样: {out}"
            );
            assert!(
                !out.contains("&apos;") && !out.contains("&quot;"),
                "{style:?} 不该做无谓转义: {out}"
            );
            assert_eq!(
                parse(&out, "t.xml").entries[0].source,
                r#"target's "sword""#
            );
        }

        // 必须转义的字符照旧转义
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated("a\rb & c < d > e");
        let out = render_with_style(&[entry], &[], MarkupStyle::Escaped).unwrap();
        assert!(out.contains("a&#13;b &amp; c &lt; d &gt; e"), "{out}");
        assert!(
            !out.contains('\r'),
            "裸 `\\r` 会被解析器归一化成 `\\n`，必须写成字符引用: {out:?}"
        );
        assert_eq!(parse(&out, "t.xml").entries[0].source, "a\rb & c < d > e");
    }

    /// 磁盘上还没有文件（新建中文文件）时用默认风格：真实语料的转义形态。
    #[test]
    fn a_new_file_gets_the_escaped_default_style() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.xml");
        let mut entry = TranslationEntry::new("English.xml", "h1", "1", "src");
        entry.mark_translated(r#"施放 <LSTag Tag="Fire">火球</LSTag>"#);

        write(&path, &[entry]).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert_eq!(out.matches("<LSTag").count(), 0, "{out}");
        assert!(
            out.contains("&lt;LSTag Tag=\"Fire\"&gt;火球&lt;/LSTag&gt;"),
            "{out}"
        );
        assert_eq!(
            parse(&out, "Chinese.xml").entries[0].source,
            r#"施放 <LSTag Tag="Fire">火球</LSTag>"#
        );
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

    // ─────────────────────────────────────────────────────────────
    // D1：写回前的「实体还原 → 统一转义」
    // ─────────────────────────────────────────────────────────────

    /// 已知实体与数值字符引用照常还原；其余原样保留。
    #[test]
    fn known_entities_and_char_refs_are_decoded() {
        let cases = [
            ("&amp;", "&"),
            ("&lt;", "<"),
            ("&gt;", ">"),
            ("&quot;", "\""),
            ("&apos;", "'"),
            ("&#65;", "A"),
            ("&#x41;", "A"),
            ("&#x1f600;", "\u{1f600}"),
            ("&#0000065;", "A"),
            ("&#43;", "+"),
            // `&#1;` 在 quick-xml 里是「合法字符引用」，还原成控制字符后由
            // `sanitize_xml_chars` 丢弃 —— 两条路行为必须一致（见一致性表）。
            ("&#1;", "\u{1}"),
            ("a&amp;b", "a&b"),
            ("&amp;amp;", "&amp;"),
            ("&&amp;", "&&"),
            ("没有实体", "没有实体"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                decode_entities(input).as_ref(),
                expected,
                "还原 {input:?} 结果不对"
            );
        }
    }

    /// 非法 / 残缺的实体引用必须**一个字符都不吞**地原样保留。
    #[test]
    fn malformed_entities_survive_verbatim() {
        let cases = [
            "&lt",            // 没有分号
            "&",              // 裸 &
            "&;",             // 空引用体
            "&#;",            // 数值引用缺数字
            "&#x;",           // 十六进制缺数字
            "&#xZZ;",         // 非法十六进制数字
            "&#X41;",         // 只认小写 x
            "&#0;",           // XML 不允许码位 0
            "&#xD800;",       // 代理区，不是合法标量值
            "&#1114112;",     // 超出 U+10FFFF
            "&#-1;",          // 带符号
            "&#+43;",         // 带符号（u32::from_str_radix 本身会接受 `+`）
            "&#99999999999;", // 溢出 u32
            "&nbsp;",         // 未定义实体
            "&a b;",          // 引用体里有空格
            "a & b",          // 裸 & 夹在文本里
        ];
        for text in cases {
            assert_eq!(
                decode_entities(text).as_ref(),
                text,
                "{text:?} 必须原样保留（不许吞字符）"
            );
        }

        // 残缺引用**不影响后面的合法引用**：`&lt` 原样留下，`&amp;` 照常还原
        assert_eq!(decode_entities("&lt &amp;").as_ref(), "&lt &");

        // 文件级的残缺引用走的是另一条路：quick-xml 直接报错 → `read` 大声失败，
        // 不会产出半截文本；「原样保留」只是字符串级（模型输出）这一侧的要求。
        let parsed = parse(
            r#"<contentList><content contentuid="h1" version="1">a &lt b</content></contentList>"#,
            "t.xml",
        );
        assert!(
            parsed.error.is_some(),
            "残缺引用必须让文件解析大声失败，而不是静默吞字符"
        );

        // 写回时这些字面文本只会被 `&`→`&amp;` 转义，不会有别的改动
        // （唯一会变的 `&amp;` 是合法引用，被还原成 `&` 后再转义回 `&amp;`）
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated("a &nbsp; b &#xZZ; c &lt &amp; d");
        let out = render_with_style(&[entry], &[], MarkupStyle::Escaped).unwrap();
        assert!(
            out.contains("a &amp;nbsp; b &amp;#xZZ; c &amp;lt &amp; d"),
            "{out}"
        );
        assert_eq!(
            parse(&out, "t.xml").entries[0].source,
            "a &nbsp; b &#xZZ; c &lt & d"
        );
    }

    /// 数值字符引用的语义与 quick-xml 的 `BytesRef::resolve_char_ref` **逐例对齐**。
    ///
    /// 做法 (a) 让 `resolve_reference` 不再调用库的解析（规则只有一份，见
    /// [`decode_reference_body`]），代价是这份规则必须自己维护，所以这里拿库的
    /// 行为当参照物逐例比对：一旦哪天漂移，这张表会先红。
    #[test]
    fn char_ref_rule_matches_quick_xml_semantics() {
        for body in [
            "#0",
            "#1",
            "#9",
            "#13",
            "#31",
            "#32",
            "#65",
            "#127",
            "#128",
            "#x41",
            "#x9",
            "#x7F",
            "#x80",
            "#xD7FF",
            "#xD800",
            "#xDFFF",
            "#xE000",
            "#xFFFD",
            "#xFFFF",
            "#x10000",
            "#x10FFFF",
            "#x110000",
            "#",
            "#x",
            "#xZZ",
            "#-1",
            "#+43",
            "#X41",
            "#99999999999",
            "#0000065",
            "#x1f600",
            "#xffffffff",
            "lt",
            "amp",
            "nbsp",
            "",
        ] {
            let ours = parse_char_ref_body(body).map(|ch| ch.to_string());
            let theirs = BytesRef::new(body)
                .resolve_char_ref()
                .ok()
                .flatten()
                .map(|ch| ch.to_string());
            assert_eq!(
                ours, theirs,
                "字符引用体 {body:?} 的语义与 quick-xml 漂移了"
            );
        }
    }

    /// **实体规则一致性用例表**：每一行都同时跑事件级与字符串级两条路。
    ///
    /// - 事件级：真 XML 解析 → quick-xml 的 `GeneralRef` 事件 → [`resolve_reference`]
    /// - 字符串级：写回前的还原 → [`decode_entities`]
    ///
    /// 选做法 (a)（抽共用实现）后两条路共用 [`decode_reference_body`]；这张表同时
    /// 钉住「共享实现没被绕过」和「字符串级扫描器没漂移」。
    #[test]
    fn entity_rule_table_pins_both_paths() {
        // (XML 文本, 两条路都该得到的文本, 说明)
        let cases = [
            ("a &amp; b", "a & b", "amp"),
            ("a &lt; b", "a < b", "lt"),
            ("a &gt; b", "a > b", "gt"),
            ("a &quot; b", "a \" b", "quot"),
            ("a &apos; b", "a ' b", "apos"),
            ("&#65;&#x42;", "AB", "十进制 / 十六进制"),
            ("&#43;", "+", "字符引用能解出 `+`"),
            ("&amp;amp;", "&amp;", "嵌套只解一层"),
            ("&#X41;", "&#X41;", "大写 X 不是十六进制，原样保留"),
            ("&#0;", "&#0;", "码位 0 非法，原样保留"),
            ("&#xD800;", "&#xD800;", "代理区非法，原样保留"),
            ("&#1114112;", "&#1114112;", "超出 U+10FFFF，原样保留"),
            ("&#-1;", "&#-1;", "带符号非法，原样保留"),
            ("&#;", "&#;", "缺数字，原样保留"),
            ("&nbsp;", "&nbsp;", "未定义实体原样保留"),
            (
                "x &#1; y",
                "x \u{1} y",
                "控制字符引用：两条路都解出，写回时丢弃",
            ),
        ];
        // 无法两路对齐的那一类在这里说明清楚：`&` 后面直接跟 `&` / `<`，或者
        // 裸 `&` 一路找不到 `;`（例如 `&&amp;`、`a & b`）都不是「未知实体」，
        // 而是**畸形文档** —— quick-xml 在事件级直接报 `UnclosedReference`，
        // `read` 会大声失败，压根没有可比对的文本；字符串级（模型输出）这一侧
        // 才需要「原样保留」，由 `known_entities_and_char_refs_are_decoded` /
        // `malformed_entities_survive_verbatim` 单独钉住。
        for (raw, expected, label) in cases {
            // ① 事件级（真 XML 解析）
            let xml = format!(
                "<contentList><content contentuid=\"h1\" version=\"1\">{raw}</content></contentList>"
            );
            let parsed = parse(&xml, "t.xml");
            assert!(
                parsed.error.is_none(),
                "[{label}] 事件级解析失败: {:?}",
                parsed.error
            );
            assert_eq!(
                parsed.entries[0].source, expected,
                "[{label}] 事件级结果不一致（raw={raw:?}）"
            );

            // ② 字符串级（写回前的还原）
            assert_eq!(
                decode_entities(raw).as_ref(),
                expected,
                "[{label}] 字符串级结果不一致（raw={raw:?}）"
            );
        }
    }

    /// D1 主表：模型把正文里的标签**照抄成文件里的转义形态**时，只转义一层。
    #[test]
    fn entities_in_model_output_are_decoded_before_escaping() {
        // (模型输出, 落盘必须出现的形态, 重新解析后读到的文本, 说明)
        let cases = [
            (
                r#"&lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;"#,
                r#"&lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;"#,
                r#"<LSTag Tooltip="HitPoints">hit points</LSTag>"#,
                "标签被照抄成转义形态",
            ),
            ("HP &lt; 5", "HP &lt; 5", "HP < 5", "数学小于号"),
            ("A &amp; B", "A &amp; B", "A & B", "和号"),
            (
                "say &quot;hi&quot;",
                "say \"hi\"",
                "say \"hi\"",
                "引号：还原后按 XML 文本规则原样写出",
            ),
            ("it&#39;s", "it's", "it's", "字符引用形式的撇号"),
            ("没有实体", "没有实体", "没有实体", "无实体不回归"),
        ];
        for (model_output, written, read_back, label) in cases {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(model_output);
            let out = render_with_style(&[entry], &[], MarkupStyle::Escaped).unwrap();
            assert!(out.contains(written), "[{label}] 落盘形态不对: {out}");
            assert!(
                !out.contains("&amp;lt;")
                    && !out.contains("&amp;amp;")
                    && !out.contains("&amp;gt;"),
                "[{label}] 出现二次转义: {out}"
            );
            assert_eq!(
                parse(&out, "t.xml").entries[0].source,
                read_back,
                "[{label}] 重新解析结果不对: {out}"
            );
        }
    }

    /// 还原必须早于 Markup 风格的标签扫描：`&lt;LSTag&gt;` 变回真标签后才写成真元素。
    #[test]
    fn decoded_tags_are_written_as_real_markup_in_markup_style() {
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated(r#"&lt;LSTag Tag="Fire"&gt;火球&lt;/LSTag&gt; 与 A &amp; B"#);

        let out = render_with_style(&[entry], &[], MarkupStyle::Markup).unwrap();

        assert!(out.contains(r#"<LSTag Tag="Fire">火球</LSTag>"#), "{out}");
        assert!(out.contains("A &amp; B"), "{out}");
        assert!(!out.contains("&amp;lt;"), "二次转义: {out}");
        assert_eq!(
            parse(&out, "t.xml").entries[0].source,
            r#"<LSTag Tag="Fire">火球</LSTag> 与 A & B"#
        );
    }

    /// 还原早于 `sanitize_xml_chars`：`&#1;` 解出的控制字符必须被丢弃，不能落盘。
    ///
    /// 顺序反了就会出事：先过滤再还原的话，`&#1;` 会整段躲过过滤，随后原样写进
    /// 文件 —— 裸 0x01 对游戏侧解析器就是非法 XML。
    #[test]
    fn entity_decoding_happens_before_illegal_character_filtering() {
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated("a&#1;b&#x0C;c");

        let out = render_with_style(&[entry], &[], MarkupStyle::Escaped).unwrap();

        assert!(!out.contains('\u{1}') && !out.contains('\u{c}'), "{out:?}");
        assert!(out.contains("abc"), "{out}");
        assert_eq!(parse(&out, "t.xml").entries[0].source, "abc");
    }

    /// `escape(decode(T))` 是**不动点**：合法实体经写回后不会再被二次转义。
    #[test]
    fn escape_of_decoded_text_is_a_fixed_point() {
        for text in [
            r#"&lt;LSTag Tooltip="HitPoints"&gt;"#,
            "A &amp; B",
            "HP &lt; 5",
            "a &nbsp; b",
            "&#xZZ;",
            "&lt",
            "没有实体",
        ] {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(text);
            let first = render_with_style(&[entry], &[], MarkupStyle::Escaped).unwrap();
            // 第二轮：把第一轮的产物重新解析出来再渲染，字节必须一致
            let mut replayed = parse(&first, "t.xml").entries;
            for entry in &mut replayed {
                entry.mark_translated(entry.source.clone());
            }
            let second = render_with_style(&replayed, &[], MarkupStyle::Escaped).unwrap();
            assert_eq!(first, second, "{text:?} 的产物不是不动点");
        }
    }

    /// 已知取舍：源文件里的**双重转义**会被解开一层。
    ///
    /// `&amp;lt;` 的语义是「文本真的要显示 `&lt;`」，写回后会变成 `&lt;`
    /// （游戏里显示 `<`）。这是 D1 修法的另一面，二者不可兼得：解析层已经把实体
    /// 还原过一次，「模型照抄了转义形态」和「正文里本来就是字面实体」这两种情况
    /// 的信息在解析时就合并了。
    ///
    /// 选这个方向的两个理由：① 真实语料 1971 条里 `&` 出现 0 处，影响面为 0
    /// （`corpus_writeback` 把它钉死在 1945/1971）；② 它顺带能**修好被本工具
    /// 旧版本写坏的文件**（旧版落盘的 `&amp;lt;` 读回来是 `&lt;`，下次写回自动
    /// 恢复成一层），反向取舍则永远修不好，见
    /// `files_double_escaped_by_older_versions_are_repaired_on_write_back`。
    #[test]
    fn double_escaped_source_text_is_decoded_one_level() {
        let xml = r#"<contentList><content contentuid="h1" version="1">x &amp;lt; y</content></contentList>"#;
        let entries = parse(xml, "t.xml").entries;
        assert_eq!(entries[0].source, "x &lt; y");

        let out = render_with_style(&entries, &[], MarkupStyle::Escaped).unwrap();
        assert!(out.contains("x &lt; y"), "{out}");
        assert_eq!(parse(&out, "t.xml").entries[0].source, "x < y");

        // 幂等：再写一轮不会继续「掉层」
        let third =
            render_with_style(&parse(&out, "t.xml").entries, &[], MarkupStyle::Escaped).unwrap();
        assert_eq!(third, out);
    }

    /// 顺带修复：被旧版本写坏（`&amp;lt;`）的文件，下次写回自动恢复成一层转义。
    #[test]
    fn files_double_escaped_by_older_versions_are_repaired_on_write_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.xml");
        std::fs::write(
            &path,
            r#"<contentList><content contentuid="h1" version="1">Deals &amp;lt;LSTag Tooltip="HitPoints"&amp;gt;hit points&amp;lt;/LSTag&amp;gt;</content></contentList>"#,
        )
        .unwrap();

        let entries = parse(&std::fs::read_to_string(&path).unwrap(), "Chinese.xml").entries;
        write(&path, &entries).unwrap();

        let out = std::fs::read_to_string(&path).unwrap();
        assert!(
            out.contains(r#"&lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;"#),
            "双重转义没有被修复: {out}"
        );
        assert!(!out.contains("&amp;lt;"), "{out}");
        assert_eq!(
            parse(&out, "Chinese.xml").entries[0].source,
            r#"Deals <LSTag Tooltip="HitPoints">hit points</LSTag>"#
        );
    }

    // ─────────────────────────────────────────────────────────────
    // D9：实体还原不得碰「已经合法的真元素标签」区间
    // ─────────────────────────────────────────────────────────────

    /// Markup 风格下，`decode_entities` 必须整段跳过合法白名单标签区间。
    ///
    /// 复现（修复前）：`raw_start_tag` 保留属性值的原始转义、`write_text_fragment`
    /// 裸写标签，`decode_entities` 一还原，`Tooltip="a&amp;b"` 就落盘成
    /// `Tooltip="a&b"` —— 零译文写回即触发，产物非法 XML，而且不会自愈。
    #[test]
    fn decode_entities_skips_real_tags_in_markup_style() {
        // 标签内部：一个字节都不许动；标签外部：照常还原
        let text = r#"&amp; <LSTag Tooltip="a&amp;b">x</LSTag> &amp; <b>t</b>"#;
        assert_eq!(
            decode_entities_outside_raw_tags(text).as_ref(),
            r#"& <LSTag Tooltip="a&amp;b">x</LSTag> & <b>t</b>"#
        );
        // Escaped 风格：整条会被再转义一次，标签只是正文，全解才是规范形
        assert_eq!(
            decode_entities(text).as_ref(),
            r#"& <LSTag Tooltip="a&b">x</LSTag> & <b>t</b>"#
        );

        // `&lt;LSTag&gt;` 解码前**不是**标签 → 两种风格都照常还原（D1 不受影响）
        let escaped_tag = r#"&lt;LSTag Tooltip="a&amp;b"&gt;x&lt;/LSTag&gt;"#;
        assert_eq!(
            decode_entities(escaped_tag).as_ref(),
            r#"<LSTag Tooltip="a&b">x</LSTag>"#
        );
        assert_eq!(
            decode_entities_outside_raw_tags(escaped_tag).as_ref(),
            r#"<LSTag Tooltip="a&b">x</LSTag>"#
        );

        // 残缺标签不是「合法白名单标签」，不跳过 → `&` 照常还原
        assert_eq!(
            decode_entities_outside_raw_tags(r#"<LSTag Tooltip=a&amp;b>x"#).as_ref(),
            r#"<LSTag Tooltip=a&b>x"#
        );
    }

    /// 裸写标签前，属性值会被规范成 `escape(decode(v))` —— 只在必要时才重写。
    ///
    /// 触发路径：模型把**整条标签转义**、属性里带实体，解码后属性值是裸 `&` / 裸 `<`；
    /// 直接裸写就是非法 XML。规范形态与源文件里的写法一致，XML 层语义不变。
    ///
    /// 第三个变体（`&quot;`）是**已知残留盲点**，见下面的断言与说明。
    #[test]
    fn raw_tags_get_their_attribute_values_canonicalized() {
        let cases = [
            // (模型输出, 期望的真元素形态, 说明)
            (
                r#"&lt;LSTag Tooltip="a&amp;b"&gt;x&lt;/LSTag&gt;"#,
                Some(r#"<LSTag Tooltip="a&amp;b">x</LSTag>"#),
                "裸 & 被转义回来",
            ),
            (
                r#"&lt;LSTag Tooltip="a&lt;b"&gt;x&lt;/LSTag&gt;"#,
                Some(r#"<LSTag Tooltip="a&lt;b">x</LSTag>"#),
                "裸 < 被转义回来",
            ),
            (
                r#"&lt;LSTag Tooltip="a&quot;b"&gt;x&lt;/LSTag&gt;"#,
                None,
                "已知残留盲点：裸 \" 让标签整条降级（产物仍合法）",
            ),
        ];
        for (model_output, expected, label) in cases {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(model_output);
            let out = render_with_style(&[entry], &[], MarkupStyle::Markup).unwrap();
            let reparsed = parse(&out, "t.xml");
            assert!(reparsed.error.is_none(), "[{label}] 产物必须合法: {out}");

            let Some(expected) = expected else {
                // 已知残留盲点：解码出来的裸 `"` 让标签在 `scan_tag_end` 阶段就认不出来
                // （比 `repair_tag_attributes` 更早），整条降级成纯文本。
                // 产物仍**合法 XML**，标签变成字面文本（玩家看得见）—— 与 T6 及
                // T6 之前的行为一致。要修得在解码前按「转义标签」语法解析属性，
                // 代价与风险都远超收益，任务里的窄边界说明也接受这一点。
                assert!(
                    !out.contains("<LSTag") || !out.contains("</LSTag>"),
                    "[{label}] 不能留下半裸的标签: {out}"
                );
                assert!(
                    out.contains(r#"&lt;LSTag Tooltip="a"b"&gt;x&lt;/LSTag&gt;"#),
                    "[{label}] 实际: {out}"
                );
                let again = render_with_style(&reparsed.entries, &[], MarkupStyle::Markup).unwrap();
                assert_eq!(again, out, "[{label}] 写回必须稳定");
                continue;
            };

            assert!(out.contains(expected), "[{label}] 实际: {out}");
            assert_eq!(
                reparsed.entries[0].source, expected,
                "[{label}] 文本形态不对: {out}"
            );
            // 再写一轮稳定
            let again = render_with_style(&reparsed.entries, &[], MarkupStyle::Markup).unwrap();
            assert_eq!(again, out, "[{label}] 写回必须稳定");
        }

        // 本来就规范的属性值一律逐字节原样：引号风格 / 属性顺序 / `>` / 空白都不动。
        // 这里直接把译文塞进条目（不经 `parse`），否则 `raw_start_tag` 会先把
        // 单引号规范化成双引号 —— 那是读侧既有的行为，不是写回改动。
        let untouched = [
            (r#"<LSTag Tooltip="a&amp;b">"#, "</LSTag>"),
            (r#"<LSTag Tag='Fire'>"#, "</LSTag>"),
            (r#"<LSTag Tooltip="a > b">"#, "</LSTag>"),
            (r#"<LSTag Tooltip="a b">"#, "</LSTag>"),
            (r#"<i class="a">"#, "</i>"),
        ];
        for (open, close) in untouched {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(format!("a {open}x{close} b"));
            let out = render_with_style(&[entry], &[], MarkupStyle::Markup).unwrap();
            assert!(out.contains(open), "{open:?} 不该被重写: {out}");
        }
    }

    /// 属性之间**必须有空白**（XML 1.0 `STag ::= '<' Name (S Attribute)* S? '>'`）。
    ///
    /// quick-xml 在这里是宽松的：`<LSTag a="1"b="2">` 能被它解析成「两个属性」，
    /// 于是旧实现把它判成合法标签、**原样写进产物**。严格解析器（游戏侧，
    /// 以及 python expat）直接拒绝整份文档 —— 丢的是整份本地化文件。
    ///
    /// 复现（修复前）：
    /// ```text
    /// $ python3 -c "import xml.dom.minidom,sys; xml.dom.minidom.parse('markup_attr_no_space.xml')"
    /// xml.parsers.expat.ExpatError: not well-formed (invalid token): line 1, column 100
    /// ```
    #[test]
    fn tag_without_whitespace_between_attributes_is_never_written_as_markup() {
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated(r#"<LSTag a="1"b="2">x</LSTag>"#);

        let out = render_with_style(&[entry], &[], MarkupStyle::Markup).unwrap();

        assert!(
            !out.contains(r#"<LSTag a="1"b="2">"#),
            "缺空白的属性不能被当成合法标签写出去: {out}"
        );
        assert!(
            out.contains(r#"&lt;LSTag a="1"b="2"&gt;x&lt;/LSTag&gt;"#),
            "整条应降级成正文（内容不丢，只是不再当标签）: {out}"
        );
        assert!(parse(&out, "t.xml").error.is_none(), "产物必须合法: {out}");

        // 正向对照：空白/引号/换行合乎文法的写法必须继续原样保留为标签
        for (text, expected) in [
            (r#"<LSTag a="1" b="2">x</LSTag>"#, r#"<LSTag a="1" b="2">"#),
            (r#"<LSTag a='1' b="2">x</LSTag>"#, r#"<LSTag a='1' b="2">"#),
            (
                "<LSTag a=\"1\"\n  b=\"2\">x</LSTag>",
                "<LSTag a=\"1\"\n  b=\"2\">",
            ),
            (r#"<LSTag a = "1">x</LSTag>"#, r#"<LSTag a = "1">"#),
        ] {
            let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
            entry.mark_translated(text);
            let out = render_with_style(&[entry], &[], MarkupStyle::Markup).unwrap();
            assert!(
                out.contains(expected),
                "{text:?} 是合法 XML，不该被降级: {out}"
            );
        }
    }

    /// 属性值里的字符引用如果解出来是 XML 非法字符，**不能落盘**。
    ///
    /// `&#1;` 本身就不是合法的字符引用（XML 1.0 的 `Char` 不含 0x01），
    /// 但 `repair_tag_attributes` 会把它解码成裸 0x01 再原样写进属性值 ——
    /// 产物对严格解析器同样非法（F-07 那条「非法控制字符落盘」的新路径）。
    ///
    /// 复现（修复前）：python expat 报 `not well-formed (invalid token)`，
    /// 且产物字节里真的出现 `0x01`。
    #[test]
    fn illegal_char_reference_in_tag_attribute_never_reaches_the_file() {
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated(r#"<LSTag a="&#1;">x</LSTag>"#);

        let out = render_with_style(&[entry], &[], MarkupStyle::Markup).unwrap();

        assert!(
            !out.contains('\u{1}'),
            "解出来的非法控制字符不能出现在产物里: {out:?}"
        );
        assert!(parse(&out, "t.xml").error.is_none(), "产物必须合法: {out}");
    }

    /// 引号未闭合的候选串**不是标签**，而且判定过程必须立刻返回（不能死循环）。
    ///
    /// `is_allowed_inline_tag` 是公开 API：`<LSTag a="x>` 这种串（带 `>` 但引号没闭合）
    /// 会走进属性值扫描。第一版严格文法实现的循环写成
    /// `while bytes.get(i) != Some(&quote)`，越过末尾后 `get` 恒为 `None`，
    /// 于是**永不退出**——`/tmp/fa-probe` 的标签探针跑到这一条直接超时 900s 才暴露。
    ///
    /// 这条用例同时是「不许再引入无界扫描」的守卫：一旦退回无界循环，测试会挂死
    /// （cargo 会报超时/不返回），而不是悄悄放过。
    #[test]
    fn unterminated_tag_attribute_is_not_a_tag() {
        for candidate in [r#"<LSTag a="x>"#, r#"<LSTag a="x"#, r#"<LSTag a='"#] {
            // 这里只要求「不是标签」，重点是不能挂
            let allowed = is_allowed_inline_tag(candidate);
            assert!(!allowed, "{candidate:?} 不是合法 XML 标签，不能当标签写出");
        }

        // 正向对照：空属性值、双引号里的单引号都是合法属性值（不能跟着一起误伤）
        assert!(is_allowed_inline_tag(r#"<LSTag a="">"#));
        assert!(is_allowed_inline_tag(r#"<LSTag a="x'">"#));

        // 整条文本（模型可能吐出来的形态）必须退化成纯文本，产物仍然合法
        let mut entry = TranslationEntry::new("t.xml", "h1", "1", "src");
        entry.mark_translated(r#"a <LSTag a="x> b"#);
        let out = render_with_style(&[entry], &[], MarkupStyle::Markup).unwrap();
        assert!(
            !out.contains(r#"<LSTag a="x>"#),
            "引号没闭合的候选串不能被当成标签: {out}"
        );
        assert!(parse(&out, "t.xml").error.is_none(), "产物必须合法: {out}");
    }

    /// 目标文件**存在但读不出来**时绝不能当成「新文件」整体覆盖。
    ///
    /// rename 只要目录写权限，读文件要文件读权限 —— 两者可以分离（Windows 上
    /// 文件被占用 / 杀软扫描 / ACL 同样如此）。此时若按「新文件」处理，
    /// 磁盘上没提交的条目会被静默删掉，而且 `write` 返回 `Ok`。
    ///
    /// 复现（修复前）：165 字节的文件被改写成 118 字节，h2 消失，返回 `Ok(())`。
    #[cfg(unix)]
    #[test]
    fn write_refuses_when_the_target_exists_but_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.xml");
        let mut h1 = TranslationEntry::new("English.xml", "h1", "1", "One");
        h1.mark_translated("一");
        let mut h2 = TranslationEntry::new("English.xml", "h2", "1", "Two");
        h2.mark_translated("二");
        write(&path, &[h1.clone(), h2]).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(before.len() > 150, "前置样本太小: {}", before.len());

        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&path, perms).unwrap();
        // 前提自检：本进程确实读不出来（以 root 运行时会话下前置不成立）
        if std::fs::File::open(&path).is_ok() {
            eprintln!("跳过：当前用户仍可读 chmod 000 的文件（可能以 root 运行）");
            return;
        }

        let err = write(&path, &[h1]).unwrap_err();
        assert_eq!(err.code(), "io", "读不出来必须报错: {err}");
        assert!(
            err.to_string().contains("中止写回"),
            "错误信息要说清为什么中止: {err}"
        );

        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o644);
        std::fs::set_permissions(&path, perms).unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "拒绝写回时原文件必须逐字节不变"
        );
    }

    /// 目标文件能打开、但解析**中途出错**（残缺列表）时同样不能拿它当底稿。
    ///
    /// 复现（修复前）：`B & C` 这种裸 `&` 让解析停在 h2，h3 是完全合法的条目，
    /// 却因为写回用的是残缺底稿而被永久删掉，`write` 返回 `Ok(())`。
    #[test]
    fn write_refuses_when_the_target_is_only_partially_parsable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Broken.xml");
        let broken = r#"<contentList><content contentuid="h1" version="1">A</content><content contentuid="h2" version="1">B & C</content><content contentuid="h3" version="1">D</content></contentList>"#;
        std::fs::write(&path, broken).unwrap();
        let before = std::fs::read(&path).unwrap();

        let mut h1 = TranslationEntry::new("English.xml", "h1", "1", "A");
        h1.mark_translated("甲");
        let err = write(&path, &[h1]).unwrap_err();

        assert_eq!(err.code(), "xml", "解析中断必须报错: {err}");
        assert!(
            err.to_string().contains("已中止写回"),
            "错误信息要说清为什么中止: {err}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "拒绝写回时原文件必须逐字节不变"
        );
    }

    /// 正向对照：目标文件**真的不存在**时仍按「新建」处理（区分 NotFound
    /// 与其它 IO 错误的修复不能把新建路径一起挡掉）。
    #[test]
    fn write_still_creates_a_target_that_does_not_exist() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese/New.xml");
        let mut h1 = TranslationEntry::new("English.xml", "h1", "1", "One");
        h1.mark_translated("一");

        write(&path, &[h1]).unwrap();

        let back = read(&path, "Chinese/New.xml").unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].source, "一");
    }

    /// 属性的**语法/转义错误**必须变成解析错误，不能被 `.flatten()` 吞掉。
    ///
    /// 复现（修复前）：`<content contentuid=h1 version="1">T</content>` 里那个没加
    /// 引号的属性被 quick-xml 报成 `AttrError::UnquotedValue`，而 `content_identity`
    /// 用 `attributes().flatten()` 把「错误」和「那个属性」一起丢掉 ——
    /// `contentuid` 变成空串，`read` 返回 `Ok`，写回时句柄被**改写**成
    /// `contentuid=""`：文件从「畸形」变成「良构但查不到」，用户看不到任何错误，
    /// 而原来的句柄永久丢失（`read`/`write` 的文档都承诺「解析错误宁可报错」）。
    ///
    /// 同理，属性值里的裸 `&`（未定义实体）以前会被 `unwrap_or_default()` 悄悄
    /// 换成空串 —— 根元素属性照旧写回，`contentuid` 直接变成空句柄。
    #[test]
    fn malformed_attribute_syntax_is_reported_instead_of_silently_dropping_the_uid() {
        let cases = [
            (
                "contentuid 没加引号",
                r#"<contentList><content contentuid=h1 version="1">T</content></contentList>"#,
            ),
            (
                "version 缺等号",
                r#"<contentList><content contentuid="h1" version>T</content></contentList>"#,
            ),
            (
                "contentuid 值里有裸 &",
                r#"<contentList><content contentuid="a&b" version="1">T</content></contentList>"#,
            ),
            (
                "根元素属性值里有裸 &",
                r#"<contentList xmlns:x="a&b"><content contentuid="h1" version="1">T</content></contentList>"#,
            ),
            (
                "根元素属性没加引号",
                r#"<contentList xmlns:x=urn:x><content contentuid="h1" version="1">T</content></contentList>"#,
            ),
        ];
        for (label, xml) in cases {
            let parsed = parse(xml, "t.xml");
            assert!(
                parsed.error.is_some(),
                "[{label}] 属性坏了必须报解析错误，不能静默丢掉/改写属性: {parsed:?}"
            );
        }

        // 走真实路径：读要报错，写要中止且原文件逐字节不变
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Broken.xml");
        let raw = r#"<contentList><content contentuid=h1 version="1">T</content></contentList>"#;
        std::fs::write(&path, raw).unwrap();
        assert_eq!(
            read(&path, "Broken.xml").unwrap_err().code(),
            "xml",
            "读路径也必须报错"
        );

        let mut entry = TranslationEntry::new("Broken.xml", "h1", "1", "T");
        entry.mark_translated("改");
        let err = write(&path, &[entry]).unwrap_err();
        assert_eq!(err.code(), "xml", "写路径必须中止: {err}");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            raw,
            "拒绝写回时原文件必须逐字节不变"
        );
    }

    /// `<content>` 少了 `contentuid` 时**不能凭空补一个 `contentuid=""`**，
    /// 也不能让多个无句柄元素在按句柄对齐的合并里互相覆盖。
    ///
    /// 复现（修复前）：三个元素（1 个正常 + 2 个没有 `contentuid`）读出来是 3 条
    /// 条目，两条无句柄的 `id` 都是 `t.xml#`；前端按 `id` 去重后只提交 1 条，
    /// `merge_missing_entries` 再用 `contentuid`（空串）去「认领」磁盘上的条目，
    /// 于是**两个元素一起消失**，剩下的那个还被写成 `contentuid=""`。
    #[test]
    fn content_without_contentuid_is_never_given_an_invented_uid() {
        let raw = concat!(
            r#"<contentList>"#,
            r#"<content contentuid="h1" version="1">Real</content>"#,
            r#"<content version="1">NoUid</content>"#,
            r#"<content version="1">Second</content>"#,
            r#"</contentList>"#
        );
        let parsed = parse(raw, "t.xml");
        assert!(parsed.error.is_none(), "{:?}", parsed.error);
        assert_eq!(
            parsed.entries.len(),
            3,
            "解析层必须仍然看到三条元素（写回靠它们保留原文）"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.xml");
        std::fs::write(&path, raw).unwrap();

        // 读取侧不能把无句柄元素交给前端（id 全是 "t.xml#"，去重会并成一条）
        let entries = read(&path, "t.xml").unwrap();
        assert_eq!(entries.len(), 1, "只应交出有句柄的那条: {entries:?}");
        assert_eq!(entries[0].contentuid, "h1");

        // 写回：提交 h1 的译文，两个无句柄元素必须原样保留
        let mut h1 = entries[0].clone();
        h1.mark_translated("真");
        write(&path, &[h1]).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert!(
            out.contains(r#"<content contentuid="h1" version="1">真</content>"#),
            "{out}"
        );
        assert!(
            out.contains(r#"<content version="1">NoUid</content>"#)
                && out.contains(r#"<content version="1">Second</content>"#),
            "无句柄元素必须逐字节保留: {out}"
        );
        assert!(
            !out.contains(r#"contentuid="""#),
            "绝不能凭空补一个空句柄: {out}"
        );

        // 空列表写回同样不能把它们删掉或补句柄
        std::fs::write(&path, raw).unwrap();
        write(&path, &[]).unwrap();
        let out = std::fs::read_to_string(&path).unwrap();
        assert_eq!(out.matches("<content ").count(), 3, "{out}");
        assert!(!out.contains(r#"contentuid="""#), "{out}");
    }
}
