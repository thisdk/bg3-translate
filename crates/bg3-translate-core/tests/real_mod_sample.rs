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
/// （`<LSTag Type=... Tooltip=...>`、`<i>`、`<b>`、`<br/>`、`{1}`、比较用的 `<`）。
/// 对每条语料做三种**合法翻译**，三种都必须判为保真：
/// 1. 只翻正文，标记逐字照抄；
/// 2. 把译文里的 `<` 重新转义成 `&lt;`（解析层本来也还原过一次，不算结构变化）；
/// 3. 连标签属性值一起翻译（系统 prompt 不允许，但属性值本来就不参与结构比较）。
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
                ("改写属性值", rewrite_attribute_values(&translated)),
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
fn markup_variants(source: &str) -> Vec<String> {
    vec![
        source.to_string(),
        format!(r#"<LSTag Type="Spell" Tooltip="Deals {{1}} damage">{source}</LSTag>"#),
        format!("<i>{source}</i> 造成 {{1}} 点伤害"),
        format!("<b>{source}</b><br/>{{2}}"),
        format!("{source} (HP < 5%)"),
        format!("{source} (HP &lt; 5%)"),
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
            '<' | '{' => {
                let closer = if chars[index] == '<' { '>' } else { '}' };
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

/// 改写每个标签的属性值 —— 属性值不参与结构比较，不该因此判失败。
///
/// 注意占位符必须原样保留：`Tooltip="Deals {1} damage"` 里的 `{1}` 是给游戏填参数的，
/// 属性值可以重写，但把它丢了就是真的结构损坏（`fidelity` 会、也应该报出来）。
fn rewrite_attribute_values(text: &str) -> String {
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
                out.push(' ');
                out.push_str(&keep_placeholders(attrs));
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

/// 把除 `{...}` 占位符之外的内容整体压成一个 `字`。
fn keep_placeholders(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::new();
    let mut index = 0usize;
    while index < chars.len() {
        if chars[index] == '{' {
            if let Some(offset) = chars[index..].iter().position(|c| *c == '}') {
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
