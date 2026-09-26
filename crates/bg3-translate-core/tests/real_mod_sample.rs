//! 用仓库里**真实存在的 Nexus MOD 样本**跑一遍完整流程。
//!
//! 合成样本只能验证「我以为的格式」，真实 MOD 才能暴露「实际格式和我以为的不一样」。
//! 这个测试就是从真实样本里发现「`meta.lsx` 的 `Name` 其实是模块内部标识符
//! （`GustavDev`），类型同样是 `LSString`，翻译它会让 MOD 失效」之后补上的。
//!
//! 样本缺失时**直接失败**，不再 `eprintln!` 后跳过：样本
//! （`samples/Appearance Edit Enhanced-899-3-1-3-1769898497.zip`）随仓库提交且
//! 非 Git LFS，缺失只可能是 checkout 不完整 —— 跳过会让这 4 个真实数据用例
//! 静默变空，而 `cargo test` 依然全绿。

use std::fs;
use std::path::{Path, PathBuf};

use bg3_translate_core::translation::{check_fidelity, is_faithful};
use bg3_translate_core::types::{PakFileKind, TranslationEntry};
use bg3_translate_core::{formats, pak};

const SAMPLE_ZIP: &str = "samples/Appearance Edit Enhanced-899-3-1-3-1769898497.zip";

/// 真实样本路径；缺失即测试失败（附可操作提示）。
fn sample_zip() -> PathBuf {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crate 应位于 <repo>/crates/bg3-translate-core");
    sample_zip_at(repo_root)
}

/// 在指定仓库根目录下找样本（抽出来是为了能测「缺失时必须炸」这条防线本身）。
fn sample_zip_at(repo_root: &Path) -> PathBuf {
    let path = repo_root.join(SAMPLE_ZIP);
    assert!(
        path.is_file(),
        "真实样本缺失：{}。该文件随仓库提交（非 Git LFS），缺失说明 checkout 不完整；\
         请执行 `git checkout -- samples/` 或重新 clone 后重跑 \
         `cargo test -p bg3-translate-core --all-targets`。",
        path.display()
    );
    path
}

/// 这条测试是「测试防线」的防线：样本缺失必须炸，而不是静默跳过。
///
/// 这个用例会往 stderr 打一行 panic 信息：那是被 `catch_unwind` 捕获的预期输出。
#[test]
fn missing_sample_fails_loudly_instead_of_skipping() {
    let panic = std::panic::catch_unwind(|| sample_zip_at(Path::new("/definitely/not/a/repo")))
        .expect_err("样本缺失必须 panic");
    let message = panic
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| panic.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_default();
    assert!(message.contains("真实样本缺失"), "实际: {message}");
    assert!(
        message.contains("git checkout -- samples/"),
        "提示必须可操作: {message}"
    );
}

fn read_all(work_dir: &Path, name: &str, kind: PakFileKind) -> Vec<TranslationEntry> {
    formats::read_entries(work_dir.to_str().unwrap(), name, kind)
        .unwrap_or_else(|err| panic!("读取 {name} 失败: {err}"))
}

#[test]
fn real_nexus_mod_unpacks_and_classifies_correctly() {
    let zip = sample_zip();
    let tmp = tempfile::tempdir().unwrap();
    let (_work_dir, files) = pak::open_and_extract_in(zip.to_str().unwrap(), tmp.path()).unwrap();

    // zip 里的 .pak 被解开，info.json 之类的杂物不参与
    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"Localization/English/AppearanceEditEnhanced.xml"),
        "应解出英文本地化 XML，实际: {names:?}"
    );

    let english_xml = files
        .iter()
        .find(|f| f.name == "Localization/English/AppearanceEditEnhanced.xml")
        .unwrap();
    assert_eq!(english_xml.kind, PakFileKind::LocalizationXml);
    assert_eq!(english_xml.language.as_deref(), Some("English"));

    // 多语言：英文 + 波兰语，各自都要能被识别出语言
    let languages: std::collections::BTreeSet<&str> =
        files.iter().filter_map(|f| f.language.as_deref()).collect();
    assert!(languages.contains("English"), "语言集合: {languages:?}");
    assert!(languages.contains("Polish"), "语言集合: {languages:?}");

    // 脚本与 stats 不该被当成可翻译文件
    for file in &files {
        if file.name.ends_with(".lua") || file.name.ends_with(".txt") {
            assert!(!file.kind.is_translatable(), "{} 不该可翻译", file.name);
        }
    }
    assert!(
        files
            .iter()
            .any(|f| f.kind == PakFileKind::MetadataLsx && f.name.ends_with("meta.lsx"))
    );
    assert!(
        files
            .iter()
            .any(|f| f.name == "Mods/AppearanceEditEnhanced/ScriptExtender/Config.json")
    );
}

#[test]
fn real_localization_files_read_without_errors() {
    let zip = sample_zip();
    let tmp = tempfile::tempdir().unwrap();
    let (work_dir, files) = pak::open_and_extract_in(zip.to_str().unwrap(), tmp.path()).unwrap();

    let mut localization_files = 0usize;
    for file in files.iter().filter(|f| f.kind.is_translatable()) {
        let entries = read_all(&work_dir, &file.name, file.kind);
        for entry in &entries {
            assert!(!entry.contentuid.is_empty(), "contentuid 不能为空");
            assert_eq!(
                entry.status,
                bg3_translate_core::types::TranslationStatus::Pending
            );
            assert!(entry.id.starts_with(&file.name));
        }

        // 本地化文件（XML/LOCA）一定有条目；LSX 要看有没有白名单字段，
        // 比如这个 MOD 的 Rulebook.lsx 就一个可翻译字段都没有。
        if file.kind != PakFileKind::MetadataLsx {
            assert!(
                !entries.is_empty(),
                "{} 应该能读出条目（真实 MOD 里本地化文件不是空的）",
                file.name
            );
            localization_files += 1;
        }
    }
    assert!(
        localization_files >= 4,
        "至少应检查 4 个本地化文件，实际 {localization_files}"
    );
}

/// 结构保真校验在**真实文本 + 真实富文本形态**上不许误报。
///
/// 语料 = 真实样本里 41 条原文，各自套上几种真实 MOD 里常见的标记形态
/// （`<LSTag Type=... Tooltip=...>`、`<i>`、`<b>`、`<br/>`、`{1}`、`[1]`、
/// `[IE_PanelSelect]`、相邻的 `[1] [2]`、比较用的 `<`）。
/// 对每条语料做三种**合法翻译**，三种都必须判为保真：
/// 1. 只翻正文，标记逐字照抄；
/// 2. 把译文里的 `<` 重新转义成 `&lt;`（解析层本来也还原过一次，不算结构变化）；
/// 3. 把标签里**自然语言形态**的属性值一起翻译（`Tooltip="Deals {1} damage"` 这类
///    含空格的值；系统 prompt 不允许，但这类值不参与结构比较）。**标识符形态**的
///    key 型属性值（`Type="Spell"`、`Tooltip="HitPoints"`）不在此列 —— 它们是查表
///    key，翻了必须报，见反向用例 [`key_attribute_values_are_not_faithful_when_translated`]。
///
/// 这是"接进主流程前先证明不会误报"的证据；断言失败会直接打印原文与问题。
#[test]
fn realistic_translations_are_never_flagged_by_structure_check() {
    let sources = real_sample_sources();
    assert!(sources.len() >= 40, "真实样本条目太少: {}", sources.len());

    let mut variants_checked = 0usize;
    let mut translations_checked = 0usize;
    for source in &sources {
        for variant in markup_variants(source) {
            variants_checked += 1;
            let translated = keep_markup_translate_text(&variant);
            let candidates = [
                ("保留标记", translated.clone()),
                ("转义尖括号", escape_angle_brackets(&translated)),
                (
                    "改写自然语言属性值",
                    rewrite_natural_language_attribute_values(&translated),
                ),
            ];
            for (label, candidate) in candidates {
                translations_checked += 1;
                if !is_faithful(&variant, &candidate) {
                    let issues = check_fidelity(&variant, &candidate);
                    panic!(
                        "[{label}] 真实文本被误报: {variant:?} → {candidate:?}，问题: {issues:?}"
                    );
                }
            }
        }
    }

    assert!(variants_checked >= 240, "语料太少: {variants_checked}");
    assert!(
        translations_checked >= 700,
        "检查过的译文太少: {translations_checked}"
    );
}

/// 反向用例：**key 型属性值**被翻译，结构校验必须报出来。
///
/// 这里订正一条旧结论：这个文件以前断言「只改属性值 → 保真」。但 `Tooltip` / `Type`
/// 的值不是显示文本，而是**查表 key** —— 语料里 `Tooltip="VENOMOUS_BARBS_CONDITION"`
/// 包着的正文是 "Deadly Toxin"、`Type` 只有 Spell / Status / Passive 三个取值。
/// 翻了 KEY，游戏按 key 查表就查不到，而产物仍是合法 XML、写盘一路静默通过。
///
/// 收口范围刻意窄：只有**属性名是 `Tooltip` / `Type`、且原文的值是标识符形态**
/// （非空、只含 ASCII 字母数字下划线）才要求逐字一致。自然语言形态的值仍然自由，
/// 由 [`realistic_translations_are_never_flagged_by_structure_check`] 那条盯着。
#[test]
fn key_attribute_values_are_not_faithful_when_translated() {
    for (source, translated) in [
        // `Type` 的闭集取值（游戏据此决定链接样式）
        (
            r#"<LSTag Type="Spell">Fireball</LSTag>"#,
            r#"<LSTag Type="法术">火球术</LSTag>"#,
        ),
        // `Tooltip` 的驼峰 key：`HitPoints` 是 key，包着的 "hit points" 才是显示文本
        (
            r#"<LSTag Tooltip="HitPoints">hit points</LSTag>"#,
            r#"<LSTag Tooltip="生命值">生命值</LSTag>"#,
        ),
        // 内部 ID 形态 + 两个属性一起被翻
        (
            r#"<LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">Deadly Toxin</LSTag>"#,
            r#"<LSTag Type="Status" Tooltip="致命毒素">致命毒素</LSTag>"#,
        ),
    ] {
        assert!(
            !is_faithful(source, translated),
            "key 型属性值被翻译必须判为不保真: {source:?} → {translated:?}（问题: {:?}）",
            check_fidelity(source, translated)
        );
    }

    // 只翻 KEY、正文照抄也一样报（正文是不是中文与这条判据无关）
    assert_eq!(
        check_fidelity(
            r#"<LSTag Tooltip="Projectile_MagicStoneThrow">Throw Magic Stone</LSTag>"#,
            r#"<LSTag Tooltip="投掷魔法石">Throw Magic Stone</LSTag>"#
        )
        .len(),
        1
    );

    // 正确的做法：KEY 逐字保留、只翻正文与语序 → 保真
    assert!(
        is_faithful(
            r#"Inflicts <LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">Deadly Toxin</LSTag> for 2 turns."#,
            r#"使其陷入<LSTag Type="Status" Tooltip="VENOMOUS_BARBS_CONDITION">致命毒素</LSTag>状态，持续 2 回合。"#
        ),
        "KEY 逐字保留的正确译文不该被误报"
    );
}

/// 反向用例：**属性名**被改坏、或属性被整个删掉，结构校验必须报出来。
///
/// 为什么必须有这条：构造「属性值改写」的辅助函数以前把属性名也压成了 `字`，
/// 于是 `<LSTag Type="Spell">` → `<LSTag 字="字字">` 也能过校验。但真实链路里
/// 模型输出会被**原样写进 PAK**：`Type="Spell"` 变成 `类型="法术"`、或者整个属性
/// 消失，游戏就读不到这个标签的属性了 —— 属于「会把 MOD 弄坏」的漏报。
#[test]
fn mangled_tag_attribute_names_are_not_faithful() {
    let source = r#"<LSTag Type="Spell" Tooltip="Deals {1} damage">Fireball</LSTag>"#;

    // 对照组：只改写**自然语言形态**的属性值（`Tooltip="Deals {1} damage"`），
    // 属性名与标识符形态的 key 值（`Type="Spell"`）逐字保留 → 必须仍然判为保真。
    // 如果这条红了，说明校验被改成了「连自然语言属性值都要一样」，那是过度收紧
    // （见 `fidelity.rs` 里 `natural_language_attribute_values_are_still_free`）。
    let values_rewritten = rewrite_natural_language_attribute_values(source);
    assert_ne!(values_rewritten, source, "属性值应确实被改写");
    assert!(
        values_rewritten.contains(r#"Type="Spell""#),
        "标识符形态的 key 值必须原样保留: {values_rewritten:?}"
    );
    assert!(
        is_faithful(source, &values_rewritten),
        "只改自然语言属性值不该被判失败: {values_rewritten:?}"
    );

    // 对照组反面：同一个辅助函数**不能**把标识符形态的 key 值洗白 ——
    // `Type="Spell"` → `Type="字"` 必须报（旧结论在这里是放行的）
    let key_rewritten = source.replace(r#"Type="Spell""#, r#"Type="字""#);
    assert!(
        !is_faithful(source, &key_rewritten),
        "key 型属性值被改写必须判为不保真: {key_rewritten:?}（问题: {:?}）",
        check_fidelity(source, &key_rewritten)
    );

    // 属性名被改：`Type` → `类型`（模型把标记也翻译了）
    let renamed = source.replace("Type=", "类型=");
    assert!(
        !is_faithful(source, &renamed),
        "属性名被改坏必须判为不保真: {renamed:?}（问题: {:?}）",
        check_fidelity(source, &renamed)
    );

    // 整个属性被删掉：`Type="Spell" ` 消失
    let dropped = source.replace(r#"Type="Spell" "#, "");
    assert!(
        !is_faithful(source, &dropped),
        "属性被删掉必须判为不保真: {dropped:?}（问题: {:?}）",
        check_fidelity(source, &dropped)
    );

    // 属性名大小写被改（`Type` → `type`）同样是结构变化
    let lowercased = source.replace("Type=", "type=");
    assert!(
        !is_faithful(source, &lowercased),
        "属性名大小写被改必须判为不保真: {lowercased:?}（问题: {:?}）",
        check_fidelity(source, &lowercased)
    );
}

/// 真实样本里所有可翻译条目的原文。
fn real_sample_sources() -> Vec<String> {
    let zip = sample_zip();
    let tmp = tempfile::tempdir().unwrap();
    let (work_dir, files) = pak::open_and_extract_in(zip.to_str().unwrap(), tmp.path()).unwrap();

    let mut sources = Vec::new();
    for file in files.iter().filter(|f| f.kind.is_translatable()) {
        for entry in read_all(&work_dir, &file.name, file.kind) {
            if !entry.source.trim().is_empty() {
                sources.push(entry.source);
            }
        }
    }
    sources
}

/// 真实 MOD 里常见的富文本 / 占位符形态（拿真实原文当正文）。
///
/// `[N]` 这一类是后补的：官方术语表里 163 条含 `[数字]` 的条目，**官方简中
/// 163/163 全部原样保留**，但仓库里这个真实样本恰好一条方括号都没有（41 条
/// 全是纯 UI 文案），所以只能在这里合成形态覆盖 —— 不补的话，`[N]` 这条回归
/// 在任何真实/端到端测试里都不会被触发。
fn markup_variants(source: &str) -> Vec<String> {
    vec![
        source.to_string(),
        format!(r#"<LSTag Type="Spell" Tooltip="Deals {{1}} damage">{source}</LSTag>"#),
        format!("<i>{source}</i> 造成 {{1}} 点伤害"),
        format!("<b>{source}</b><br/>{{2}}"),
        format!("{source} (HP < 5%)"),
        format!("{source} (HP &lt; 5%)"),
        // ── 方括号占位符 `[N]` ──
        format!("[1] {source}"),
        format!("{source} (deal [2] otherwise deal [1])"),
        format!(r#"<LSTag Tooltip="Deals [2] damage">{source}</LSTag>"#),
        format!("[IE_PanelSelect] {source}"),
        // 两个占位符相邻：中间的分隔是**载重**的（丢了会被渲染成一个数），
        // 所以翻译时必须原样保留 —— 这条也顺带盯住「粘连校验不误报」
        format!("{source} [1] [2]"),
    ]
}

/// 朴素版「保留标记、翻译正文」：标签与占位符原样拷贝，其余字符换成 `译`。
///
/// 刻意不复用 `fidelity` 的内部扫描器 —— 两边独立，才能互相验证。
fn keep_markup_translate_text(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        match chars[index] {
            // `{}` 与 `[]` 是两套独立的占位符写法，都要原样保留
            '<' | '{' | '[' => {
                let closer = match chars[index] {
                    '<' => '>',
                    '{' => '}',
                    _ => ']',
                };
                match chars[index..].iter().position(|c| *c == closer) {
                    Some(offset) => {
                        out.extend(&chars[index..=index + offset]);
                        index += offset + 1;
                    }
                    None => {
                        out.push(chars[index]);
                        index += 1;
                    }
                }
            }
            _ => {
                // 一整个中文词块只留一个字符，避免输出过长
                if !out.ends_with('译') {
                    out.push('译');
                }
                index += 1;
            }
        }
    }
    out
}

/// 把译文里的尖括号重新转义成实体（模型偶尔会这么干）。
fn escape_angle_brackets(text: &str) -> String {
    text.replace('<', "&lt;").replace('>', "&gt;")
}

/// 改写标签里**自然语言形态**的属性值 —— 模拟「模型顺手把非 key 的属性值也翻了」。
///
/// **标识符形态的值一律原样保留**：`Type="Spell"`、`Tooltip="HitPoints"` 这类值是
/// 查表 key，不是显示文本，改了会把 MOD 弄坏，`fidelity` 也会（应该）报出来 ——
/// 见反向用例 [`key_attribute_values_are_not_faithful_when_translated`]。
/// 这个辅助函数只负责构造「合法的那一半」：含空格的自然语言值被翻译。
///
/// **属性名必须原样保留**：这个用例的意图是「属性值被翻译」而不是「属性可以随便改」。
/// 如果连属性名一起压成 `字`，就会掩盖真实漏报——`Type=` 被改坏成 `类型=`、或者整个
/// 属性被删掉，都在结构上真的弄坏了标签，写回时模型输出会被原样写进 PAK。
/// 见反向用例 [`mangled_tag_attribute_names_are_not_faithful`]。
///
/// 注意占位符必须原样保留：`Tooltip="Deals {1} damage"` 里的 `{1}` 是给游戏填参数的，
/// 属性值可以重写，但把它丢了就是真的结构损坏（`fidelity` 会、也应该报出来）。
fn rewrite_natural_language_attribute_values(text: &str) -> String {
    rewrite_attribute_values_matching(text, |value| !is_identifier_shaped(value))
}

/// 属性值是否是「标识符形态」：非空、只含 ASCII 字母数字下划线。
///
/// 刻意**不复用** `fidelity` 内部的判断（它在模块私有）：两边独立，才能互相验证。
fn is_identifier_shaped(value: &str) -> bool {
    !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 把每个标签里**满足 `rewrite` 条件**的属性值改写掉，其余属性值逐字保留。
fn rewrite_attribute_values_matching(text: &str, rewrite: impl Fn(&str) -> bool) -> String {
    let mut out = String::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find('>') else {
            out.push_str(after);
            return out;
        };
        let trimmed = after[1..end].trim();
        out.push('<');
        if let Some(name) = trimmed.strip_prefix('/') {
            // 结束标签没有属性
            out.push('/');
            out.push_str(name.trim());
        } else {
            let self_closing = trimmed.ends_with('/');
            let body = trimmed.trim_end_matches('/').trim_end();
            let (name, attrs) = match body.find(char::is_whitespace) {
                Some(idx) => (&body[..idx], &body[idx..]),
                None => (body, ""),
            };
            out.push_str(name);
            if !attrs.trim().is_empty() {
                // attrs 自带前导空白，不能额外补空格，否则属性间空白会翻倍
                out.push_str(&rewrite_quoted_values(attrs, &rewrite));
            }
            if self_closing {
                out.push('/');
            }
        }
        out.push('>');
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

/// 只重写引号里**满足 `rewrite` 条件**的内容，引号外的一切（属性名、`=`、空白、`/`）
/// 逐字保留。
fn rewrite_quoted_values(attrs: &str, rewrite: &impl Fn(&str) -> bool) -> String {
    let mut out = String::new();
    let mut i = 0usize;
    while i < attrs.len() {
        let Some(c) = attrs[i..].chars().next() else {
            break;
        };
        if c == '"' || c == '\'' {
            let value_start = i + c.len_utf8();
            let value_end = attrs[value_start..]
                .find(c)
                .map(|offset| value_start + offset)
                .unwrap_or(attrs.len());
            out.push(c); // 开引号
            let value = &attrs[value_start..value_end];
            if rewrite(value) {
                out.push_str(&keep_placeholders(value));
            } else {
                out.push_str(value);
            }
            out.push(c); // 闭引号
            i = if value_end < attrs.len() {
                value_end + c.len_utf8()
            } else {
                value_end
            };
        } else {
            out.push(c);
            i += c.len_utf8();
        }
    }
    out
}

/// 把除 `{...}` / `[...]` 占位符之外的内容整体压成一个 `字`。
fn keep_placeholders(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        let closer = match chars[index] {
            '{' => Some('}'),
            '[' => Some(']'),
            _ => None,
        };
        if let Some(closer) = closer {
            if let Some(offset) = chars[index..].iter().position(|c| *c == closer) {
                out.extend(&chars[index..=index + offset]);
                index += offset + 1;
                continue;
            }
        }
        if !out.ends_with('字') {
            out.push('字');
        }
        index += 1;
    }
    out
}

#[test]
fn real_meta_lsx_name_is_not_translatable() {
    let zip = sample_zip();
    let tmp = tempfile::tempdir().unwrap();
    let (work_dir, _) = pak::open_and_extract_in(zip.to_str().unwrap(), tmp.path()).unwrap();

    let meta = "Mods/AppearanceEditEnhanced/meta.lsx";
    let entries = read_all(&work_dir, meta, PakFileKind::MetadataLsx);

    // meta.lsx 里 Name=GustavDev（模块内部标识符）、Folder=模块目录名、
    // Description=玩家可见描述。只有 Description 能翻。
    assert!(
        !entries.iter().any(|e| e.source.contains("GustavDev")),
        "模块内部标识符 Name 绝不能被当成可翻译文本: {entries:#?}"
    );
    assert!(
        !entries
            .iter()
            .any(|e| e.source.contains("AppearanceEditEnhanced")),
        "模块目录名 Folder 绝不能被当成可翻译文本: {entries:#?}"
    );
    assert!(
        entries
            .iter()
            .any(|e| e.contentuid == "Description#0" && e.source.contains("Race")),
        "玩家可见的 Description 应该被采集到: {entries:#?}"
    );
}

#[test]
fn real_mod_survives_translate_write_repack_roundtrip() {
    let zip = sample_zip();
    let tmp = tempfile::tempdir().unwrap();
    let work_root = tmp.path().join("work");
    fs::create_dir_all(&work_root).unwrap();

    let (work_dir, files) = pak::open_and_extract_in(zip.to_str().unwrap(), &work_root).unwrap();

    // 只翻英文本地化，模拟真实使用（前端会把译文写到 Chinese 目录）
    let english_name = "Localization/English/AppearanceEditEnhanced.xml";
    let entries = read_all(&work_dir, english_name, PakFileKind::LocalizationXml);
    let translated: Vec<TranslationEntry> = entries
        .iter()
        .cloned()
        .map(|mut entry| {
            let target = format!("[译]{}", entry.source);
            entry.mark_translated(target);
            entry
        })
        .collect();

    formats::write_entries(
        work_dir.to_str().unwrap(),
        "Localization/Chinese/AppearanceEditEnhanced.xml",
        PakFileKind::LocalizationXml,
        &translated,
    )
    .unwrap();

    let output_pak = tmp.path().join("AppearanceEditEnhanced_zh.pak");
    pak::repack(work_dir.to_str().unwrap(), output_pak.to_str().unwrap()).unwrap();

    // 重新解开，确认：文件数量一致（没丢文件）、译文在、没动过的文件字节不变
    let verify_root = tmp.path().join("verify");
    fs::create_dir_all(&verify_root).unwrap();
    let (verify_dir, verify_files) =
        pak::open_and_extract_in(output_pak.to_str().unwrap(), &verify_root).unwrap();

    assert_eq!(
        verify_files.len(),
        files.len() + 1,
        "重打包后应比原来多一个新建的中文文件"
    );

    let chinese = read_all(
        &verify_dir,
        "Localization/Chinese/AppearanceEditEnhanced.xml",
        PakFileKind::LocalizationXml,
    );
    assert_eq!(chinese.len(), entries.len());
    assert!(chinese[0].source.starts_with("[译]"));
    // contentuid 必须原样保留，否则游戏查不到表
    assert_eq!(chinese[0].contentuid, entries[0].contentuid);
    assert_eq!(chinese[0].version, entries[0].version);

    // 脚本这类不该动的文件必须逐字节一致
    let lua = "Mods/AppearanceEditEnhanced/ScriptExtender/Lua/BootstrapServer.lua";
    assert_eq!(
        fs::read(work_dir.join("unpacked").join(lua)).unwrap(),
        fs::read(verify_dir.join("unpacked").join(lua)).unwrap(),
        "未翻译的脚本文件必须逐字节保持一致"
    );

    // 原来的英文文件也还在（我们只是新增中文，不覆盖原文）
    assert!(
        verify_files.iter().any(|f| f.name == english_name),
        "英文原文文件应保留"
    );
}
