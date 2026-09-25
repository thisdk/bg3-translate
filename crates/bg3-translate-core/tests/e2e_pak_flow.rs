//! 端到端闭环测试：**真造一个 PAK**，跑完整流程。
//!
//! 解包 → 识别类型 → 读条目 → 翻译 → 写回 → 重打包 → 再解包 → 校验内容。
//!
//! 这个测试是「重构没有破坏功能」的最强证据，而且不依赖任何 GUI 系统库，
//! 本地 `cargo test -p bg3-translate-core` 就能跑。

use std::fs;
use std::path::{Path, PathBuf};

use bg3_translate_core::formats;
use bg3_translate_core::pak;
use bg3_translate_core::types::{PakFileKind, TranslationEntry};
use bg3rustpaklib::PackageBuilder;

const CONTENT_LIST_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<contentList>
  <content contentuid="h11111111g2222u3333i4444" version="1">Hello, adventurer.</content>
  <content contentuid="h55555555g6666u7777i8888" version="2">Cast <LSTag Tag="Fire">Fireball</LSTag> for {1} damage</content>
</contentList>"#;

const META_LSX: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<save>
  <region id="Config">
    <node id="root">
      <attribute id="Name" type="FixedString" value="Internal_Id_Dont_Touch" />
      <attribute id="Name" type="LSString" value="Mod Display Name" />
      <attribute id="Description" type="LSString" value="Adds a shiny sword &amp; more." />
      <attribute id="Description" type="TranslatedString" value="h99999999g0000u1111i2222" />
    </node>
  </region>
</save>"#;

/// 造一个最小可用的 MOD 目录树。
fn build_mod_tree(root: &Path) -> PathBuf {
    let localization = root.join("Localization/English");
    fs::create_dir_all(&localization).unwrap();
    fs::write(localization.join("test.xml"), CONTENT_LIST_XML).unwrap();

    let script_names = root.join("Scripts");
    fs::create_dir_all(&script_names).unwrap();
    fs::write(script_names.join("boot.lua"), b"-- nothing to translate").unwrap();

    let stats = root.join("Stats");
    fs::create_dir_all(&stats).unwrap();
    fs::write(stats.join("Weapons.txt"), b"new entry \"X\"").unwrap();

    let mods = root.join("Mods");
    fs::create_dir_all(&mods).unwrap();
    fs::write(mods.join("Meta.lsx"), META_LSX).unwrap();

    root.to_path_buf()
}

fn pack(source_dir: &Path, pak_path: &Path, priority: u8) {
    PackageBuilder::new()
        .priority(priority)
        .add_directory(source_dir)
        .expect("add_directory")
        .build(pak_path)
        .expect("build pak");
}

fn write_targets(work_dir: &Path, file_name: &str, translator: impl Fn(&str) -> String) -> usize {
    let kind = pak::classify_file(file_name);
    let entries =
        formats::read_entries(work_dir.to_str().unwrap(), file_name, kind).expect("read entries");

    let translated: Vec<TranslationEntry> = entries
        .into_iter()
        .map(|mut entry| {
            let target = translator(&entry.source);
            entry.mark_translated(target);
            entry
        })
        .collect();

    formats::write_entries(work_dir.to_str().unwrap(), file_name, kind, &translated)
        .expect("write entries");
    translated.len()
}

#[test]
fn full_roundtrip_pak_unpack_translate_repack() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("src");
    let work_root = tmp.path().join("work");
    fs::create_dir_all(&work_root).unwrap();

    build_mod_tree(&source);
    let input_pak = tmp.path().join("TestMod.pak");
    pack(&source, &input_pak, 7);

    // ── 1. 解包 ──
    let (work_dir, files) =
        pak::open_and_extract_in(input_pak.to_str().unwrap(), &work_root).unwrap();
    assert!(work_dir.join("unpacked").is_dir());

    let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
    assert!(
        names.contains(&"Localization/English/test.xml"),
        "{names:?}"
    );
    assert!(names.contains(&"Mods/Meta.lsx"), "{names:?}");
    assert!(names.contains(&"Scripts/boot.lua"), "{names:?}");
    assert!(names.contains(&"Stats/Weapons.txt"), "{names:?}");

    let xml_file = files.iter().find(|f| f.name.ends_with("test.xml")).unwrap();
    assert_eq!(xml_file.kind, PakFileKind::LocalizationXml);
    assert_eq!(xml_file.language.as_deref(), Some("English"));
    assert!(xml_file.size > 0);

    let lsx_file = files.iter().find(|f| f.name == "Mods/Meta.lsx").unwrap();
    assert_eq!(lsx_file.kind, PakFileKind::MetadataLsx);

    let lua_file = files.iter().find(|f| f.name.ends_with("boot.lua")).unwrap();
    assert_eq!(lua_file.kind, PakFileKind::ScriptLua);
    assert!(!lua_file.kind.is_translatable());

    // priority 元信息落盘
    assert_eq!(pak::read_priority(&work_dir), 7);

    // ── 2. 读条目 ──
    let xml_entries = formats::read_entries(
        work_dir.to_str().unwrap(),
        "Localization/English/test.xml",
        PakFileKind::LocalizationXml,
    )
    .unwrap();
    assert_eq!(xml_entries.len(), 2);
    assert_eq!(xml_entries[0].source, "Hello, adventurer.");
    assert_eq!(xml_entries[0].version, "1");
    assert!(xml_entries[1].source.contains("<LSTag Tag=\"Fire\">"));
    assert!(xml_entries[1].source.contains("{1}"));
    assert!(
        xml_entries
            .iter()
            .all(TranslationEntry::is_pending_translation)
    );

    let lsx_entries = formats::read_entries(
        work_dir.to_str().unwrap(),
        "Mods/Meta.lsx",
        PakFileKind::MetadataLsx,
    )
    .unwrap();
    assert_eq!(lsx_entries.len(), 2);
    assert_eq!(lsx_entries[0].source, "Mod Display Name");
    assert_eq!(lsx_entries[1].source, "Adds a shiny sword & more.");
    // TranslatedString（句柄）不能出现在条目里
    assert!(!lsx_entries.iter().any(|e| e.source.contains("h99999999")));

    // ── 3. 翻译 + 写回 ──
    let count = write_targets(&work_dir, "Localization/English/test.xml", |source| {
        format!("【译】{source}")
    });
    assert_eq!(count, 2);

    let count = write_targets(&work_dir, "Mods/Meta.lsx", |source| {
        format!("【译】{source}")
    });
    assert_eq!(count, 2);

    // 写回后的磁盘内容
    let xml_out =
        fs::read_to_string(work_dir.join("unpacked/Localization/English/test.xml")).unwrap();
    assert!(xml_out.contains("【译】Hello, adventurer."));
    assert!(
        xml_out.contains("<LSTag Tag=\"Fire\">"),
        "富文本标签必须保留"
    );

    let lsx_out = fs::read_to_string(work_dir.join("unpacked/Mods/Meta.lsx")).unwrap();
    assert!(lsx_out.contains("【译】Mod Display Name"));
    // 内部 ID 与句柄字段必须原样保留
    assert!(lsx_out.contains("Internal_Id_Dont_Touch"));
    assert!(lsx_out.contains("h99999999g0000u1111i2222"));
    // 属性顺序与其余结构不应被重排
    assert!(lsx_out.contains(r#"<attribute id="Name" type="FixedString""#));

    // ── 4. 重打包 ──
    let output_pak = tmp.path().join("TestMod_zh.pak");
    pak::repack(work_dir.to_str().unwrap(), output_pak.to_str().unwrap()).unwrap();
    assert!(output_pak.is_file());
    assert!(fs::metadata(&output_pak).unwrap().len() > 0);

    // ── 5. 再解包校验 ──
    let verify_root = tmp.path().join("verify");
    fs::create_dir_all(&verify_root).unwrap();
    let (verify_dir, verify_files) =
        pak::open_and_extract_in(output_pak.to_str().unwrap(), &verify_root).unwrap();
    assert_eq!(verify_files.len(), files.len(), "重打包后文件数量应一致");
    assert_eq!(pak::read_priority(&verify_dir), 7, "priority 必须保留");

    let xml_final =
        fs::read_to_string(verify_dir.join("unpacked/Localization/English/test.xml")).unwrap();
    assert!(xml_final.contains("【译】Hello, adventurer."));
    assert!(xml_final.contains("{1} damage") || xml_final.contains("【译】Cast"));

    let lsx_final = fs::read_to_string(verify_dir.join("unpacked/Mods/Meta.lsx")).unwrap();
    assert!(lsx_final.contains("【译】Mod Display Name"));
    assert!(lsx_final.contains("Internal_Id_Dont_Touch"));

    // 未翻译的文件内容保持不变
    let lua_final = fs::read_to_string(verify_dir.join("unpacked/Scripts/boot.lua")).unwrap();
    assert_eq!(lua_final, "-- nothing to translate");
}

#[test]
fn empty_target_keeps_source_text_on_write_back() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("src");
    fs::create_dir_all(source.join("Localization/English")).unwrap();
    fs::write(source.join("Localization/English/a.xml"), CONTENT_LIST_XML).unwrap();

    let input_pak = tmp.path().join("M.pak");
    pack(&source, &input_pak, 0);

    let work_root = tmp.path().join("work");
    fs::create_dir_all(&work_root).unwrap();
    let (work_dir, _) = pak::open_and_extract_in(input_pak.to_str().unwrap(), &work_root).unwrap();

    // 一个都没翻译：写回后原文必须一字不差
    let entries = formats::read_entries(
        work_dir.to_str().unwrap(),
        "Localization/English/a.xml",
        PakFileKind::LocalizationXml,
    )
    .unwrap();
    formats::write_entries(
        work_dir.to_str().unwrap(),
        "Localization/English/a.xml",
        PakFileKind::LocalizationXml,
        &entries,
    )
    .unwrap();

    let out = fs::read_to_string(work_dir.join("unpacked/Localization/English/a.xml")).unwrap();
    assert!(out.contains("Hello, adventurer."));
    assert!(out.contains("Fireball"));
    assert!(out.contains("{1} damage"));
}

#[test]
fn zip_wrapped_mod_is_unwrapped_transparently() {
    use std::io::Write as _;

    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("src");
    build_mod_tree(&source);
    let inner_pak = tmp.path().join("Inner.pak");
    pack(&source, &inner_pak, 3);

    // Nexus 风格：zip 里放 pak + README
    let zip_path = tmp.path().join("Nexus.zip");
    {
        let file = fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options: zip::write::FileOptions<'_, ()> =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);

        zip.start_file("README.txt", options).unwrap();
        zip.write_all(b"install me").unwrap();
        // 故意让 README 之外的噪音 pak 更小，验证「取最大 pak」策略
        zip.start_file("Mods/tiny.pak", options).unwrap();
        zip.write_all(b"not a real pak").unwrap();
        zip.start_file("Mods/Inner.pak", options).unwrap();
        zip.write_all(&fs::read(&inner_pak).unwrap()).unwrap();
        zip.finish().unwrap();
    }

    let work_root = tmp.path().join("work");
    fs::create_dir_all(&work_root).unwrap();
    let (work_dir, files) =
        pak::open_and_extract_in(zip_path.to_str().unwrap(), &work_root).unwrap();

    assert!(!files.is_empty());
    assert!(files.iter().any(|f| f.name == "Mods/Meta.lsx"));
    // 挑中的是真正的 pak，而不是体积更小的 tiny.pak
    let meta = fs::read_to_string(work_dir.join("unpacked/Mods/Meta.lsx")).unwrap();
    assert!(meta.contains("Mod Display Name"));
}

#[test]
fn extract_to_directory_writes_files_without_work_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("src");
    build_mod_tree(&source);
    let input_pak = tmp.path().join("M.pak");
    pack(&source, &input_pak, 0);

    let output = tmp.path().join("extracted");
    let files =
        pak::extract_to_directory(input_pak.to_str().unwrap(), output.to_str().unwrap()).unwrap();

    assert_eq!(files.len(), 4);
    assert!(output.join("Localization/English/test.xml").is_file());
    assert!(output.join("Mods/Meta.lsx").is_file());
    // 只解压，不应该产生 unpacked/ 这层
    assert!(!output.join("unpacked").exists());
}

#[test]
fn path_helpers_target_the_chinese_localization_directory() {
    assert_eq!(
        bg3_translate_core::to_target_localization_path("Localization/English/test.xml"),
        "Localization/Chinese/test.xml"
    );
    assert!(bg3_translate_core::is_target_language("ChineseSimplified"));
}
