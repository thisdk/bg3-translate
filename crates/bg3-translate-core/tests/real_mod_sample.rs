//! 用仓库里**真实存在的 Nexus MOD 样本**跑一遍完整流程。
//!
//! 合成样本只能验证「我以为的格式」，真实 MOD 才能暴露「实际格式和我以为的不一样」。
//! 这个测试就是从真实样本里发现「`meta.lsx` 的 `Name` 其实是模块内部标识符
//! （`GustavDev`），类型同样是 `LSString`，翻译它会让 MOD 失效」之后补上的。
//!
//! 样本缺失时自动跳过（不影响本地开发），CI 上样本随仓库一起 checkout。

use std::fs;
use std::path::{Path, PathBuf};

use bg3_translate_core::types::{PakFileKind, TranslationEntry};
use bg3_translate_core::{formats, pak};

const SAMPLE_ZIP: &str = "samples/Appearance Edit Enhanced-899-3-1-3-1769898497.zip";

fn sample_zip() -> Option<PathBuf> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()?
        .parent()?
        .join(SAMPLE_ZIP);
    path.is_file().then_some(path)
}

fn read_all(work_dir: &Path, name: &str, kind: PakFileKind) -> Vec<TranslationEntry> {
    formats::read_entries(work_dir.to_str().unwrap(), name, kind)
        .unwrap_or_else(|err| panic!("读取 {name} 失败: {err}"))
}

#[test]
fn real_nexus_mod_unpacks_and_classifies_correctly() {
    let Some(zip) = sample_zip() else {
        eprintln!("跳过：未找到 {SAMPLE_ZIP}");
        return;
    };
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
    let Some(zip) = sample_zip() else {
        eprintln!("跳过：未找到 {SAMPLE_ZIP}");
        return;
    };
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

#[test]
fn real_meta_lsx_name_is_not_translatable() {
    let Some(zip) = sample_zip() else {
        eprintln!("跳过：未找到 {SAMPLE_ZIP}");
        return;
    };
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
    let Some(zip) = sample_zip() else {
        eprintln!("跳过：未找到 {SAMPLE_ZIP}");
        return;
    };
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
