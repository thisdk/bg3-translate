//! 真实语料（`samples/english.xml`，1971 条）的**写回格式**回归。
//!
//! 盯的是一个编码风格缺陷：真实 BG3 contentList 把富文本标记**整体转义**在文本里
//! （`&lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;`），而 `content_list::render`
//! 会把白名单标签写成**真 XML 元素** —— 工具输出的文件和输入不是同一种格式。
//!
//! 判据不是「解析出来的文本是否相同」（两种写法解析后本来就相同，所以那种断言
//! 抓不到这个缺陷），而是**写回前后每个 `<content>` 元素的原始字节是否一致**。
//!
//! 语料是只读输入：这些测试只读 `samples/english.xml`，绝不改写它。

use std::fs;
use std::path::{Path, PathBuf};

use bg3_translate_core::formats::content_list::{MarkupStyle, parse, render, write};
use bg3_translate_core::types::TranslationEntry;

/// 真实 BG3 contentList 语料（用户从真实 MOD 提取）。
fn corpus_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/english.xml")
}

fn corpus() -> String {
    let path = corpus_path();
    fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "真实语料读取失败：{}（{err}）。该文件随仓库提交（非 Git LFS），\
             缺失说明 checkout 不完整 —— 重新 clone，或取回该文件后重跑。\
             注意 `git checkout -- samples/english.xml` 不管用。",
            path.display()
        )
    })
}

/// 写回前后每个 `<content>` 元素的**原始字节**对比结果。
#[derive(Debug)]
struct RoundTrip {
    identical: usize,
    total: usize,
    mismatched: Vec<usize>,
}

impl RoundTrip {
    fn rate(&self) -> f64 {
        self.identical as f64 * 100.0 / self.total as f64
    }
}

/// 取出每个 `<content ...>...</content>` 元素的原始切片。
///
/// 刻意从 `<content` 开始、到 `</content>` 结束，**不含**元素之间的缩进与换行：
/// `render` 输出的是单行文档，缩进差异不属于「风格」，混进来只会稀释判据。
fn content_regions(xml: &str) -> Vec<&str> {
    let mut regions = Vec::new();
    let mut cursor = 0usize;
    while let Some(rel) = xml[cursor..].find("<content") {
        let start = cursor + rel;
        cursor = start + "<content".len();
        let rest = &xml[cursor..];
        // 只认元素开始（`<content>` / `<content ...>`）；文本里的 `<content` 不算
        if !(rest.starts_with('>') || rest.starts_with(char::is_whitespace)) {
            continue;
        }
        let open_end = cursor + rest.find('>').expect("`<content` 开始标签未闭合") + 1;
        if xml[start..open_end].trim_end().ends_with("/>") {
            regions.push(&xml[start..open_end]);
            continue;
        }
        let close = xml[open_end..]
            .find("</content>")
            .expect("`<content` 元素未闭合");
        let end = open_end + close + "</content>".len();
        regions.push(&xml[start..end]);
        cursor = end;
    }
    regions
}

/// `<content ...>` 的开始标签（含属性）。
fn start_tag(region: &str) -> &str {
    &region[..region.find('>').expect("开始标签未闭合") + 1]
}

/// `<content>` 元素的内部文本。
fn inner_text(region: &str) -> &str {
    let open = region.find('>').expect("开始标签未闭合") + 1;
    &region[open..region.rfind("</content>").expect("结束标签缺失")]
}

fn round_trip(before: &str, after: &str) -> RoundTrip {
    let before = content_regions(before);
    let after = content_regions(after);
    assert_eq!(
        before.len(),
        after.len(),
        "写回前后 `<content>` 条目数必须一致"
    );
    let mismatched: Vec<usize> = (0..before.len())
        .filter(|&i| before[i] != after[i])
        .collect();
    RoundTrip {
        identical: before.len() - mismatched.len(),
        total: before.len(),
        mismatched,
    }
}

/// 走真实写回路径（复制到临时文件 → `write` → 读回）。
fn write_back(raw: &str, entries: &[TranslationEntry]) -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("English.xml");
    fs::write(&path, raw).unwrap();
    write(&path, entries).unwrap();
    fs::read_to_string(&path).unwrap()
}

/// 真实语料「零译文写回」：内容、句柄、编码风格三样都不能变。
#[test]
fn real_corpus_write_back_keeps_the_escaped_markup_style() {
    let raw = corpus();
    let parsed = parse(&raw, "English.xml");
    assert!(parsed.error.is_none(), "{:?}", parsed.error);
    assert_eq!(parsed.entries.len(), 1971, "真实语料条目数");

    let out = write_back(&raw, &parsed.entries);
    let before_regions = content_regions(&raw);
    let after_regions = content_regions(&out);

    // 语料构成（后续断言都建立在这三个数字上）
    assert_eq!(before_regions.len(), 1971);
    assert_eq!(
        before_regions
            .iter()
            .filter(|r| !r.contains("&lt;"))
            .count(),
        1401,
        "无标记的纯文本条目数"
    );
    assert_eq!(
        before_regions.iter().filter(|r| r.contains("&lt;")).count(),
        570,
        "带转义标记的条目数"
    );

    // ── ① 风格必须和源文件一致：转义标记照旧转义，真标签一个都不能出现 ──
    assert_eq!(raw.matches("&lt;LSTag").count(), 1103, "语料基线");
    assert_eq!(raw.matches("&lt;/LSTag").count(), 1103, "语料基线");
    assert_eq!(raw.matches("&lt;br&gt;").count(), 410, "语料基线");
    assert_eq!(raw.matches("<LSTag").count(), 0, "语料基线：真标签 0 处");

    assert_eq!(
        out.matches("&lt;LSTag").count(),
        1103,
        "转义形态的 `<LSTag` 数量被改成了 {}（风格被翻转）",
        out.matches("&lt;LSTag").count()
    );
    assert_eq!(out.matches("&lt;/LSTag").count(), 1103);
    assert_eq!(out.matches("&lt;br&gt;").count(), 410);
    assert_eq!(
        out.matches("<LSTag").count(),
        0,
        "写回后出现了真 XML 元素 `<LSTag`：输出和输入不是同一种格式"
    );
    assert_eq!(out.matches("<br").count(), 0, "写回后出现了真 `<br` 元素");

    // ── ② 不做无谓转义：`'` / `"` 与源文件一致（语料里 `&apos;` / `&quot;` 各 0 处）──
    assert_eq!(raw.matches("&apos;").count(), 0, "语料基线");
    assert_eq!(raw.matches("&quot;").count(), 0, "语料基线");
    assert_eq!(
        out.matches("&apos;").count(),
        0,
        "`'` 在 XML 文本里不需要转义"
    );
    assert_eq!(
        out.matches("&quot;").count(),
        0,
        "`\"` 在 XML 文本里不需要转义"
    );
    let apostrophes = |xml: &str| {
        content_regions(xml)
            .iter()
            .map(|r| r.matches('\'').count())
            .sum::<usize>()
    };
    assert_eq!(apostrophes(&raw), 144, "语料基线：条目内共 144 个 `'`");
    assert_eq!(apostrophes(&out), 144, "`'` 必须原样保留");

    // ── ③ 逐条重新解析：contentuid / version / 文本逐字节一致 ──
    let replayed = parse(&out, "English.xml");
    assert!(replayed.error.is_none(), "{:?}", replayed.error);
    assert_eq!(replayed.entries.len(), parsed.entries.len());
    for (before, after) in parsed.entries.iter().zip(&replayed.entries) {
        assert_eq!(before.contentuid, after.contentuid);
        assert_eq!(before.version, after.version);
        assert_eq!(
            before.source, after.source,
            "条目 {} 文本往返不一致",
            before.contentuid
        );
    }

    // ── ④ 原始字节一致率：每个 `<content>` 元素逐字节对比 ──
    let rt = round_trip(&raw, &out);
    println!(
        "真实语料写回：<content> 原始字节一致 {}/{}（{:.2}%）",
        rt.identical,
        rt.total,
        rt.rate()
    );

    // 根元素属性与 BOM 状态也要原样保留（语料无 BOM，根上有两个 xmlns 声明）
    assert!(!raw.starts_with('\u{feff}') && !out.starts_with('\u{feff}'));
    assert!(
        out.contains(r#"xmlns:xsd="http://www.w3.org/2001/XMLSchema""#)
            && out.contains(r#"xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance""#),
        "根元素属性必须保留: {out}"
    );

    // 刻意不声称「整文件逐字节相等」：源文件是 CRLF + 2 空格缩进的漂亮打印，
    // `render` 输出单行文档（元素之间的空白会被丢掉）。这是**预先存在**的格式差异，
    // 与标记风格无关、也不影响游戏解析；要保的是每个 `<content>` 元素内部的字节。
    println!(
        "整文件字节（不参与断言）：源 {} / 写回 {}；换行数 {} -> {}",
        raw.len(),
        out.len(),
        raw.matches('\n').count(),
        out.matches('\n').count()
    );

    // 唯一允许的差异：源文件里 26 条（9 条纯文本 + 17 条带标记）文本**末尾带空格**，
    // `parse` 的 `raw.trim()` 会归一化掉它。这是**预先存在**的归一化，与风格无关，
    // 本轮不动它；这里把差异集合钉死，保证「不一致」只能来自这一类。
    assert_eq!(rt.mismatched.len(), 26, "差异条目: {:?}", rt.mismatched);
    for &i in &rt.mismatched {
        let (before, after) = (before_regions[i], after_regions[i]);
        assert_eq!(
            start_tag(before),
            start_tag(after),
            "第 {i} 条的开始标签（contentuid/version）必须一字不差"
        );
        assert_eq!(
            inner_text(before).trim_end(),
            inner_text(after),
            "第 {i} 条只允许「尾部空白被 trim」"
        );
        assert!(
            inner_text(before).ends_with(' ') && !inner_text(after).ends_with(' '),
            "第 {i} 条的差异必须**恰好**是丢失一个尾空格"
        );
    }

    // 纯文本条目（1401 条）里只有那 9 条尾空格差异，其余全部逐字节不变
    let plain_changed = before_regions
        .iter()
        .zip(after_regions.iter())
        .filter(|(before, _)| !before.contains("&lt;"))
        .filter(|(before, after)| before != after)
        .count();
    assert_eq!(plain_changed, 9, "纯文本条目里只有 9 条尾空格差异");
}

/// 修复前的行为（真元素风格渲染）必须**继续可复现**，把被修的缺陷钉在测试里。
///
/// 断言两件事：(1) 真元素风格渲染确实会把 570 条带标记的条目写成真标签 ——
/// 这就是缺陷本身；(2) 它的字节一致率显著低于「跟随源文件风格」的写回。
#[test]
fn legacy_markup_render_still_flips_the_style_which_is_the_regression() {
    let raw = corpus();
    let entries = parse(&raw, "English.xml").entries;

    let legacy = render(&entries, &[]).unwrap();
    let legacy_rt = round_trip(&raw, &legacy);
    println!(
        "修复前（真元素风格 render）：<content> 原始字节一致 {}/{}（{:.2}%）",
        legacy_rt.identical,
        legacy_rt.total,
        legacy_rt.rate()
    );
    assert!(
        legacy.matches("&lt;LSTag").count() == 0 && legacy.matches("<LSTag").count() == 1103,
        "legacy render 应该写出 1103 个真标签（缺陷复现）"
    );

    // 跟随源文件风格的写回：570 条带标记条目的风格不再被翻转
    let out = write_back(&raw, &entries);
    let rt = round_trip(&raw, &out);
    assert!(
        rt.identical > legacy_rt.identical,
        "跟随源文件风格后一致率必须上升：{} -> {}",
        legacy_rt.identical,
        rt.identical
    );
}

/// 真元素风格的文件照旧按真元素写回（风格不能被反向翻转）。
#[test]
fn real_markup_file_is_written_back_as_real_markup() {
    const MARKUP: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<contentList>
  <content contentuid="h1" version="1">Cast <LSTag Tag="Fire">Fireball</LSTag> for {1} damage</content>
  <content contentuid="h2" version="2">line1<br/>line2</content>
  <content contentuid="h3" version="3">target's sword &amp; shield</content>
</contentList>"#;

    let entries = parse(MARKUP, "M.xml").entries;
    assert!(entries[0].source.contains(r#"<LSTag Tag="Fire">"#));

    let out = write_back(MARKUP, &entries);

    assert!(
        out.contains(r#"<LSTag Tag="Fire">Fireball</LSTag>"#),
        "真元素风格必须保持: {out}"
    );
    assert!(out.contains("line1<br/>line2"), "{out}");
    assert_eq!(out.matches("&lt;LSTag").count(), 0, "不能反过来转义: {out}");
    assert_eq!(out.matches("&apos;").count(), 0, "`'` 不需要转义: {out}");
    assert!(out.contains("target's sword"), "{out}");

    // 重新解析后文本逐字节一致
    let replayed = parse(&out, "M.xml").entries;
    assert_eq!(replayed.len(), entries.len());
    for (before, after) in entries.iter().zip(&replayed) {
        assert_eq!(before.contentuid, after.contentuid);
        assert_eq!(before.version, after.version);
        assert_eq!(before.source, after.source);
    }
}

/// 无标记的纯文本文件：写回不变，也不会凭空长出标签。
#[test]
fn plain_text_entries_round_trip_unchanged() {
    const PLAIN: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<contentList>
  <content contentuid="h1" version="1">Touch of Fire</content>
  <content contentuid="h2" version="2">target's sword &amp; shield &lt;3</content>
  <content contentuid="h3" version="3">%%% Empty</content>
</contentList>"#;

    let entries = parse(PLAIN, "P.xml").entries;
    let out = write_back(PLAIN, &entries);

    assert!(!out.contains("<LSTag"), "{out}");
    assert!(!out.contains("&apos;"), "`'` 必须原样写出: {out}");
    let replayed = parse(&out, "P.xml").entries;
    assert_eq!(replayed.len(), 3);
    assert_eq!(replayed[0].source, "Touch of Fire");
    assert_eq!(replayed[1].source, "target's sword & shield <3");
    assert_eq!(replayed[2].source, "%%% Empty");
    assert_eq!(replayed[1].contentuid, "h2");
}

/// 风格字段的判据：`<content>` 内部是否**真的解析出了白名单子元素**。
#[test]
fn markup_style_is_detected_from_the_source_file() {
    // 真实语料：转义形态
    let escaped = corpus();
    assert_eq!(
        parse(&escaped, "E.xml").markup_style,
        MarkupStyle::Escaped,
        "1971 条真实语料全是转义形态"
    );

    // 真元素形态
    let markup = r#"<contentList><content contentuid="h1" version="1">a <LSTag Tag="Fire">b</LSTag></content></contentList>"#;
    assert_eq!(parse(markup, "M.xml").markup_style, MarkupStyle::Markup);

    // 自闭合的真元素`<br/>`也算真元素形态
    let self_closing =
        r#"<contentList><content contentuid="h1" version="1">a<br/>b</content></contentList>"#;
    assert_eq!(
        parse(self_closing, "M.xml").markup_style,
        MarkupStyle::Markup
    );

    // 白名单之外的子元素不改变判据：它们本来就不会被写成真标签
    let other = r#"<contentList><content contentuid="h1" version="1">a <b>b</b> <script>c</script></content></contentList>"#;
    assert_eq!(parse(other, "M.xml").markup_style, MarkupStyle::Markup);

    // 纯文本（一个标签都没有）：没有证据 → 默认转义（真实语料的形态）
    let plain =
        r#"<contentList><content contentuid="h1" version="1">hello</content></contentList>"#;
    assert_eq!(parse(plain, "P.xml").markup_style, MarkupStyle::Escaped);

    // 字面 `&lt;` 不是元素，不能把文件判成真元素风格
    let literal = r#"<contentList><content contentuid="h1" version="1">x &lt; br &gt; y</content></contentList>"#;
    assert_eq!(parse(literal, "L.xml").markup_style, MarkupStyle::Escaped);
}

/// 新建文件（磁盘上没有源文件）按默认风格写：真实语料的转义形态。
#[test]
fn a_brand_new_file_defaults_to_the_escaped_style() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Chinese.xml");
    assert!(!path.exists());

    let entries = parse(
        r#"<contentList><content contentuid="h1" version="1">Cast <LSTag Tag="Fire">Fireball</LSTag> for {1} damage</content></contentList>"#,
        "English.xml",
    )
    .entries;
    assert_eq!(
        entries[0].source,
        r#"Cast <LSTag Tag="Fire">Fireball</LSTag> for {1} damage"#
    );

    write(&path, &entries).unwrap();
    let out = fs::read_to_string(&path).unwrap();

    assert_eq!(
        out.matches("<LSTag").count(),
        0,
        "新建文件不能凭空长出真标签: {out}"
    );
    assert_eq!(out.matches("&lt;LSTag").count(), 1, "{out}");
    assert_eq!(out.matches("&lt;/LSTag&gt;").count(), 1, "{out}");
    // 文本内容不变
    let replayed = parse(&out, "Chinese.xml").entries;
    assert_eq!(replayed[0].source, entries[0].source);

    // 再写一次：风格已经记在文件里，必须稳定（不能来回翻）
    write(&path, &replayed).unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), out, "写回必须幂等");
}

/// 模型在多/少标签时的输出，在转义风格里也只是正文，产物必须仍是合法 XML。
#[test]
fn escaped_style_never_produces_invalid_xml_for_broken_tags() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("English.xml");
    fs::write(
        &path,
        r#"<contentList><content contentuid="h1" version="1">plain &lt;b&gt;text&lt;/b&gt;</content></contentList>"#,
    )
    .unwrap();

    let mut entry = parse(&fs::read_to_string(&path).unwrap(), "English.xml")
        .entries
        .remove(0);
    assert_eq!(entry.source, "plain <b>text</b>");
    // 模型多吐一个闭合标签
    entry.mark_translated("多余 </LSTag> 与 <b>未闭合 的译文");
    write(&path, &[entry]).unwrap();

    let out = fs::read_to_string(&path).unwrap();
    let replayed = parse(&out, "English.xml");
    assert!(replayed.error.is_none(), "产物必须是合法 XML: {out}");
    assert_eq!(
        replayed.entries[0].source,
        "多余 </LSTag> 与 <b>未闭合 的译文"
    );
    assert_eq!(out.matches("<LSTag").count(), 0, "{out}");
    assert_eq!(
        out.matches("<b>").count(),
        0,
        "不配对的白名单标签也不能写成元素: {out}"
    );
}

/// D1 端到端：模型把标签照抄成**文件里的转义形态**时，写回只转义一层。
///
/// 缺陷链（修复前）：模型输出 `&lt;LSTag&gt;` → 保真度校验先还原再比对、判**保真**
/// → 写回再转义一次 → 盘上 `&amp;lt;LSTag…` → 游戏读到字面量 `&lt;LSTag&gt;`，
/// tooltip 失效；再读回来校验**仍判保真**（不会自愈）。这条链只能靠写回层兜住，
/// 所以这里同时验「落盘只转义一层」和「两轮写回字节稳定」。
#[test]
fn model_escaped_output_is_written_back_with_a_single_escape() {
    const SOURCE: &str = r#"<?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h11111111g2222u3333i4444" version="1">Deals &lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt; damage &amp; more</content></contentList>"#;
    const SOURCE_TEXT: &str =
        r#"Deals <LSTag Tooltip="HitPoints">hit points</LSTag> damage & more"#;

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("English.xml");
    fs::write(&path, SOURCE).unwrap();

    // ⓪ 零译文写回：源文件里的 `&lt;` / `&amp;` 逐字节不变（不是「少转义一层」）
    let entries = parse(&fs::read_to_string(&path).unwrap(), "English.xml").entries;
    assert_eq!(entries[0].source, SOURCE_TEXT);
    write(&path, &entries).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        SOURCE,
        "零译文写回必须逐字节不变"
    );

    // ① 触发条件：模型把文件里的转义形态照抄进译文
    let mut translated = entries.clone();
    translated[0].mark_translated(
        r#"造成 &lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt; 伤害 &amp; 更多"#,
    );
    write(&path, &translated).unwrap();

    let out = fs::read_to_string(&path).unwrap();
    assert!(
        out.contains(r#"&lt;LSTag Tooltip="HitPoints"&gt;hit points&lt;/LSTag&gt;"#),
        "产物里必须是一层转义的真标签形态: {out}"
    );
    assert!(out.contains("&amp; 更多"), "{out}");
    assert!(!out.contains("&amp;lt;"), "出现二次转义: {out}");
    assert!(!out.contains("&amp;amp;"), "出现二次转义: {out}");
    assert!(!out.contains("&amp;gt;"), "出现二次转义: {out}");

    // ② 重新解析：读到的是真标签形态（游戏侧看到的文本）
    let replayed = parse(&out, "English.xml").entries;
    assert_eq!(
        replayed[0].source,
        r#"造成 <LSTag Tooltip="HitPoints">hit points</LSTag> 伤害 & 更多"#
    );

    // ③ 再写一轮：字节稳定 —— 「不会自愈」的那条链被不动点切断
    write(&path, &replayed).unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        out,
        "两轮写回必须字节稳定"
    );
}

// ─────────────────────────────────────────────────────────────
// 严格 XML 校验（D9 判据）
// ─────────────────────────────────────────────────────────────

/// XML 1.0 `Char` 产生式不允许的字符（与 `content_list` 内部实现同规则）。
fn is_illegal_xml_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{0}'..='\u{8}' | '\u{B}' | '\u{C}' | '\u{E}'..='\u{1F}' | '\u{FFFE}' | '\u{FFFF}'
    )
}

/// **严格** XML 校验：产物必须是合法 XML，而不是「本仓库读得进去」。
///
/// 本仓库的 `parse` 故意宽松（`raw_start_tag` 保留属性值原始转义、不做反转义），
/// 所以 `Tooltip="a&b"` 这种非法产物它照样接受 —— D9 就是这么漏过去的。
/// 判据因此比 `parse` 严：
/// - quick-xml 事件循环，`check_end_names` + `check_comments` 打开；
/// - 每个属性值都必须能**反转义**（裸 `&` / 未定义实体在这里报错）；
/// - 属性值里不许有裸 `<`（XML 1.0：`AttValue ::= '"' ([^<&"] | Reference)* '"'`）；
/// - 文本里不许出现裸 `&` 或 XML 1.0 非法控制字符。
///
/// 失败直接 panic（测试里当断言用）。
fn strict_xml_check(xml: &str) {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(xml);
    {
        let config = reader.config_mut();
        config.check_end_names = true;
        config.check_comments = true;
        config.trim_text(false);
    }

    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Err(err) => panic!("严格校验失败（quick-xml）: {err}\n产物: {xml}"),
            Ok(Event::Eof) => break,
            Ok(Event::Start(e) | Event::Empty(e)) => {
                for attr in e.attributes() {
                    let attr =
                        attr.unwrap_or_else(|err| panic!("属性语法非法: {err}\n产物: {xml}"));
                    let raw = attr.value.as_ref();
                    assert!(
                        !raw.contains('<'),
                        "属性值里有裸 `<`（XML 1.0 不允许）: {raw:?}\n产物: {xml}"
                    );
                    attr.normalized_value(quick_xml::XmlVersion::Implicit1_0)
                        .unwrap_or_else(|err| {
                            panic!(
                                "属性值不是合法 XML（裸 `&` / 未定义实体）: {err:?}；值={raw:?}\n产物: {xml}"
                            )
                        });
                }
            }
            Ok(Event::Text(t)) => {
                let text = t.xml10_content();
                assert!(
                    !text.contains('&'),
                    "文本里出现裸 `&`: {text:?}\n产物: {xml}"
                );
                for ch in text.chars() {
                    assert!(
                        !is_illegal_xml_char(ch),
                        "文本里出现 XML 非法字符 {ch:?}\n产物: {xml}"
                    );
                }
            }
            Ok(_) => {}
        }
        buf.clear();
    }
}

/// D9（Markup 风格）：属性值里的实体在**零译文写回**时必须逐字节保留。
///
/// 修复前（T6 引入的回归）：`decode_entities` 作用在整条文本（含标签内部），
/// 而 `raw_start_tag` 故意保留属性值的原始转义、`write_text_fragment` 故意裸写
/// 标签 —— `Tooltip="a&amp;b"` 被写成 `Tooltip="a&b"`（非法 XML）；
/// `Tooltip="a&quot;b"` 更狠：裸引号让标签都认不出来，整条降级成纯文本、标签全丢。
/// 零译文就触发，连续 3 轮字节相同（**不自愈**），而且宽松的 `parse` 照样读得进去
/// —— 所以判据必须是上面的严格校验。
#[test]
fn markup_style_attribute_entities_survive_zero_translation_write_back() {
    let cases = [
        (
            r#"a&amp;b"#,
            "属性值里是 `&amp;`（修复前被还原成裸 `&`：expat 报 not well-formed）",
        ),
        (
            r#"a&lt;b"#,
            "属性值里是 `&lt;`（修复前被还原成裸 `<`：同样非法）",
        ),
        (
            r#"a&quot;b"#,
            "属性值里是 `&quot;`（修复前被还原成裸 `\"`：标签认不出来、整条降级）",
        ),
    ];

    for (attr, label) in cases {
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">a <LSTag Tooltip="{attr}">x</LSTag> b <b>keep</b></content></contentList>"#
        );
        // 夹具本身必须是合法 XML，且被判定为 Markup 风格
        strict_xml_check(&xml);
        let parsed = parse(&xml, "M.xml");
        assert!(parsed.error.is_none(), "[{label}] 夹具解析失败");
        assert_eq!(
            parsed.markup_style,
            MarkupStyle::Markup,
            "[{label}] 夹具必须是真元素风格"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("M.xml");
        fs::write(&path, &xml).unwrap();

        // 零译文写回（target 为空 → effective_text() = source）
        write(&path, &parsed.entries).unwrap();
        let out = fs::read_to_string(&path).unwrap();

        strict_xml_check(&out);
        println!("[D9-Markup] {label}\n  输入: {xml}\n  产物: {out}");
        assert_eq!(out, xml, "[{label}] 零译文写回必须逐字节不变");

        // 连续 3 轮 write→read→write：字节必须稳定（坏文件不会自愈，好文件也不该漂移）
        let mut current = out;
        for round in 1..=3 {
            let entries = parse(&current, "M.xml").entries;
            write(&path, &entries).unwrap();
            let next = fs::read_to_string(&path).unwrap();
            strict_xml_check(&next);
            assert_eq!(next, current, "[{label}] 第 {round} 轮字节漂移");
            current = next;
        }
    }
}

/// D9（Escaped 风格）：同一组属性实体保持**一层**转义，产物严格合法、语义不变。
///
/// 逐字节相等只对 `&amp;` / `&lt;` 成立：`&quot;` 会被规范成裸 `"`（XML 文本里
/// `"` 不需要转义，T5 的规则），两种写法对 XML 解析器完全等价。
#[test]
fn escaped_style_attribute_entities_stay_single_escaped() {
    let cases = [
        (r#"a&amp;b"#, true, "&amp; 一层转义"),
        (r#"a&lt;b"#, true, "&lt; 一层转义"),
        (
            r#"a&quot;b"#,
            false,
            "&quot; 规范成裸引号（等价、不是二次转义）",
        ),
    ];

    for (attr, byte_identical, label) in cases {
        let xml = format!(
            r#"<?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="h1" version="1">a &lt;LSTag Tooltip="{attr}"&gt;x&lt;/LSTag&gt; b</content></contentList>"#
        );
        strict_xml_check(&xml);
        let parsed = parse(&xml, "M.xml");
        assert!(parsed.error.is_none(), "[{label}] 夹具解析失败");
        assert_eq!(
            parsed.markup_style,
            MarkupStyle::Escaped,
            "[{label}] 夹具必须是转义风格"
        );

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("E.xml");
        fs::write(&path, &xml).unwrap();

        write(&path, &parsed.entries).unwrap();
        let out = fs::read_to_string(&path).unwrap();

        strict_xml_check(&out);
        println!("[D9-Escaped] {label}\n  输入: {xml}\n  产物: {out}");
        assert!(!out.contains("&amp;amp;"), "[{label}] 出现二次转义: {out}");
        assert!(!out.contains("&amp;quot;"), "[{label}] 出现二次转义: {out}");
        assert!(
            !out.contains("&amp;lt;LSTag"),
            "[{label}] 标签被二次转义: {out}"
        );
        // 语义（重新解析后的文本）必须不变
        assert_eq!(
            parse(&out, "M.xml").entries[0].source,
            parsed.entries[0].source,
            "[{label}] 文本语义变了: {out}"
        );
        if byte_identical {
            assert_eq!(out, xml, "[{label}] 零译文写回必须逐字节不变");
        } else {
            assert_eq!(
                out,
                xml.replace("a&quot;b", "a\"b"),
                "[{label}] `&quot;` 应规范成裸引号: {out}"
            );
        }

        // 3 轮稳定
        let mut current = out;
        for round in 1..=3 {
            let entries = parse(&current, "M.xml").entries;
            write(&path, &entries).unwrap();
            let next = fs::read_to_string(&path).unwrap();
            strict_xml_check(&next);
            assert_eq!(next, current, "[{label}] 第 {round} 轮字节漂移");
            current = next;
        }
    }
}

/// 窄边界（T9 说明里点名的第三条）：Markup 风格 + 模型把**整条标签转义**
/// + 属性里带实体。
///
/// 解码前它不是「合法白名单标签区间」，所以属性里的实体会被还原成裸 `&`；
/// 写回层在裸写标签前把属性值规范成 `escape(decode(v))`，产物仍是**合法 XML**
/// 且 XML 层语义不变 —— 这个盲点已经被关闭（不是「记下来就算了」）。
///
/// 注意重新解析后的**文本形态**：真元素是裸写的，属性按 `raw_start_tag` 契约
/// 保留原始转义（`Tooltip="a&amp;b"`），XML 层的属性值才是 `a&b` —— 与源文件里
/// `<LSTag Tooltip="a&amp;b">` 的读法完全一致。
#[test]
fn fully_escaped_tag_with_entity_in_attribute_is_repaired_into_valid_xml() {
    // 文件里另有一个真元素 `<b>` → 整份文件按 Markup 风格写回
    const SOURCE: &str = r#"<?xml version="1.0" encoding="utf-8"?><contentList><content contentuid="hkeep" version="1">keep <b>bold</b></content><content contentuid="h1" version="1">src</content></contentList>"#;

    let cases = [
        (
            r#"&lt;LSTag Tooltip="a&amp;b"&gt;x&lt;/LSTag&gt;"#,
            r#"<LSTag Tooltip="a&amp;b">x</LSTag>"#,
        ),
        (
            r#"&lt;LSTag Tooltip="a&lt;b"&gt;x&lt;/LSTag&gt;"#,
            r#"<LSTag Tooltip="a&lt;b">x</LSTag>"#,
        ),
    ];

    for (model_output, expected_tag) in cases {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("M.xml");
        fs::write(&path, SOURCE).unwrap();
        let mut entries = parse(SOURCE, "M.xml").entries;
        assert_eq!(parse(SOURCE, "M.xml").markup_style, MarkupStyle::Markup);
        entries[1].mark_translated(model_output);

        write(&path, &entries).unwrap();
        let out = fs::read_to_string(&path).unwrap();

        strict_xml_check(&out);
        assert!(out.contains("keep <b>bold</b>"), "锚点标签必须保留: {out}");
        assert!(
            !out.contains(r#"Tooltip="a&b""#) && !out.contains(r#"Tooltip="a<b""#),
            "属性里的实体必须被转义，不能裸写: {out}"
        );
        assert!(
            out.contains(expected_tag),
            "产物里必须是转义好的真元素 {expected_tag:?}: {out}"
        );
        // 重新解析：真元素按 `raw_start_tag` 契约保留属性的原始转义形态
        assert_eq!(
            parse(&out, "M.xml").entries[1].source,
            expected_tag,
            "重新解析的文本形态不对: {out}"
        );

        // 3 轮稳定
        let mut current = out;
        for round in 1..=3 {
            let entries = parse(&current, "M.xml").entries;
            write(&path, &entries).unwrap();
            let next = fs::read_to_string(&path).unwrap();
            strict_xml_check(&next);
            assert_eq!(next, current, "第 {round} 轮字节漂移: {next}");
            current = next;
        }
    }
}
