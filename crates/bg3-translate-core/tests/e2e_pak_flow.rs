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
      <attribute id="Name" type="LSString" value="Module Internal Name" />
      <attribute id="DisplayName" type="LSString" value="Mod Display Name" />
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
    // Name（模块内部标识符，LSString 类型）也不能被采集：翻掉它 MOD 就废了
    assert!(
        !lsx_entries
            .iter()
            .any(|e| e.source.contains("Module Internal Name")),
        "meta.lsx 的 Name 是模块内部标识符，不能当可翻译文本"
    );

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
    // 模块内部标识符与句柄字段必须一字未动
    assert!(lsx_final.contains(r#"value="Module Internal Name""#));
    assert!(lsx_final.contains("h99999999g0000u1111i2222"));

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
fn writing_to_a_not_yet_existing_target_file_creates_it() {
    // 前端会把 `Localization/English/foo.xml` 的译文写到 `Localization/Chinese/foo.xml`，
    // 而这个文件在 MOD 里往往并不存在——后端必须能凭空创建出来。
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("src");
    build_mod_tree(&source);
    let input_pak = tmp.path().join("M.pak");
    pack(&source, &input_pak, 0);

    let work_root = tmp.path().join("work");
    fs::create_dir_all(&work_root).unwrap();
    let (work_dir, _) = pak::open_and_extract_in(input_pak.to_str().unwrap(), &work_root).unwrap();

    // 先按英文原文读出条目
    let entries = formats::read_entries(
        work_dir.to_str().unwrap(),
        "Localization/English/test.xml",
        PakFileKind::LocalizationXml,
    )
    .unwrap();
    let translated: Vec<TranslationEntry> = entries
        .into_iter()
        .map(|mut entry| {
            let target = format!("中文：{}", entry.source);
            entry.mark_translated(target);
            entry
        })
        .collect();

    // 再写到「还不存在」的中文目录
    formats::write_entries(
        work_dir.to_str().unwrap(),
        "Localization/Chinese/test.xml",
        PakFileKind::LocalizationXml,
        &translated,
    )
    .unwrap();

    let new_file = work_dir.join("unpacked/Localization/Chinese/test.xml");
    assert!(new_file.is_file(), "后端应能创建目标语言目录与文件");
    let content = fs::read_to_string(&new_file).unwrap();
    assert!(content.contains("中文：Hello, adventurer."));
    assert!(content.contains("<LSTag Tag=\"Fire\">"));

    // 重新打包后应该同时包含英文原文与新建的中文文件
    let output_pak = tmp.path().join("M_zh.pak");
    pak::repack(work_dir.to_str().unwrap(), output_pak.to_str().unwrap()).unwrap();

    let verify_root = tmp.path().join("verify");
    fs::create_dir_all(&verify_root).unwrap();
    let (verify_dir, verify_files) =
        pak::open_and_extract_in(output_pak.to_str().unwrap(), &verify_root).unwrap();
    assert!(
        verify_files
            .iter()
            .any(|f| f.name == "Localization/Chinese/test.xml"),
        "打包后应包含新建的中文文件"
    );
    assert!(
        verify_files
            .iter()
            .any(|f| f.name == "Localization/English/test.xml")
    );
    let created =
        fs::read_to_string(verify_dir.join("unpacked/Localization/Chinese/test.xml")).unwrap();
    assert!(created.contains("中文：Hello, adventurer."));
}

/// F-01 端到端：`status == error` 的条目**绝不能**把 target 带进 PAK。
///
/// 复现路径：结构校验失败 → 前端把条目置为 `error` 但保留被拒译文（给用户看），
/// 用户不点「重试失败」直接打包。修复前坏译文照样写进中文文件与 PAK；
/// 修复后写回退回原文，且「人工编辑成 edited」的条目仍然照常写回。
#[test]
fn error_entries_never_reach_the_packed_pak() {
    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("src");
    let work_root = tmp.path().join("work");
    fs::create_dir_all(&work_root).unwrap();

    const XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<contentList>
  <content contentuid="h11111111g2222u3333i4444" version="1">Hello, adventurer.</content>
  <content contentuid="h55555555g6666u7777i8888" version="2">Cast <LSTag Tag="Fire">Fireball</LSTag> for {1} damage</content>
  <content contentuid="h99999999g0000u1111i2222" version="3">Cast <LSTag Tag="Ice">Ice Storm</LSTag> for {2} damage</content>
  <content contentuid="haaaaaaaagbbbbuccccidddd" version="4">Sword of Doom</content>
</contentList>"#;
    let localization = source.join("Localization/English");
    fs::create_dir_all(&localization).unwrap();
    fs::write(localization.join("test.xml"), XML).unwrap();

    let input_pak = tmp.path().join("TestMod.pak");
    pack(&source, &input_pak, 7);
    let (work_dir, _) = pak::open_and_extract_in(input_pak.to_str().unwrap(), &work_root).unwrap();

    let source_entries = formats::read_entries(
        work_dir.to_str().unwrap(),
        "Localization/English/test.xml",
        PakFileKind::LocalizationXml,
    )
    .unwrap();
    assert_eq!(source_entries.len(), 4);

    // ① 网络中断：target 里是半截文本，状态 error
    let mut interrupted = source_entries[0].clone();
    interrupted.mark_translated("[坏]你好，冒险");
    interrupted.mark_error("大模型调用错误: 请求失败: timeout");

    // ② 结构校验失败：漏了 `{1}` 与 `<LSTag>`，状态 error
    let mut rejected = source_entries[1].clone();
    rejected.mark_translated("造成火焰伤害");
    rejected.mark_error("结构校验未通过：占位符 {1} 缺失（已重试 1 次）");

    // ③ 对照组：合法译文，必须照常写回
    let mut good = source_entries[2].clone();
    good.mark_translated(r#"施放 <LSTag Tag="Ice">冰风暴</LSTag>，造成 {2} 点伤害"#);

    // ④ 人工抢救：先失败，用户改成 edited，编辑内容必须写回
    let mut rescued = source_entries[3].clone();
    rescued.mark_translated("坏译文");
    rescued.mark_error("结构校验未通过：占位符 {9} 缺失（已重试 1 次）");
    rescued.target = "末日之剑".into();
    rescued.status = bg3_translate_core::types::TranslationStatus::Edited;

    let entries = vec![interrupted, rejected, good, rescued];
    formats::write_entries(
        work_dir.to_str().unwrap(),
        "Localization/Chinese/test.xml",
        PakFileKind::LocalizationXml,
        &entries,
    )
    .unwrap();

    // ── 落盘内容：error 条目是原文，edited / translated 是译文 ──
    let written =
        fs::read_to_string(work_dir.join("unpacked/Localization/Chinese/test.xml")).unwrap();
    assert!(
        written.contains("Hello, adventurer."),
        "① error 条目必须退回原文: {written}"
    );
    assert!(
        !written.contains("[坏]你好，冒险"),
        "① 半截流式文本不许落盘"
    );
    assert!(
        written.contains(r#"Cast <LSTag Tag="Fire">Fireball</LSTag> for {1} damage"#),
        "② error 条目必须原样退回原文（标签与占位符都在）: {written}"
    );
    assert!(!written.contains("造成火焰伤害"), "② 被拒译文不许落盘");
    assert!(
        written.contains(r#"施放 <LSTag Tag="Ice">冰风暴</LSTag>，造成 {2} 点伤害"#),
        "③ 合法译文必须照常写回: {written}"
    );
    assert!(written.contains("末日之剑"), "④ 人工抢救的译文必须写回");

    // contentuid / version 原样保留
    for entry in &source_entries {
        let attr = format!(
            r#"contentuid="{}" version="{}""#,
            entry.contentuid, entry.version
        );
        assert!(written.contains(&attr), "缺少 {attr}: {written}");
    }

    // ── 打包 → 重新解包：坏译文依然不在 PAK 里 ──
    let output_pak = tmp.path().join("TestMod_zh.pak");
    pak::repack(work_dir.to_str().unwrap(), output_pak.to_str().unwrap()).unwrap();

    let verify_root = tmp.path().join("verify");
    fs::create_dir_all(&verify_root).unwrap();
    let (verify_dir, verify_files) =
        pak::open_and_extract_in(output_pak.to_str().unwrap(), &verify_root).unwrap();
    assert!(
        verify_files
            .iter()
            .any(|f| f.name == "Localization/Chinese/test.xml"),
        "打包后应包含中文文件"
    );

    let chinese_path = verify_dir.join("unpacked/Localization/Chinese/test.xml");
    let packed = fs::read_to_string(&chinese_path).unwrap();
    assert!(!packed.contains("[坏]你好，冒险"));
    assert!(!packed.contains("造成火焰伤害"));

    let reparsed = formats::read_entries(
        verify_dir.to_str().unwrap(),
        "Localization/Chinese/test.xml",
        PakFileKind::LocalizationXml,
    )
    .unwrap();
    assert_eq!(reparsed.len(), 4);
    // 解出来的「原文」就是写进 PAK 的文本：前两条是原文，后两条是译文
    assert_eq!(reparsed[0].source, "Hello, adventurer.");
    assert_eq!(reparsed[1].source, source_entries[1].source);
    assert_eq!(reparsed[0].contentuid, source_entries[0].contentuid);
    assert_eq!(reparsed[1].contentuid, source_entries[1].contentuid);
    assert_eq!(reparsed[1].version, source_entries[1].version);
    assert!(reparsed[1].source.contains(r#"<LSTag Tag="Fire">"#));
    assert!(reparsed[1].source.contains("{1}"));
    assert!(reparsed[2].source.contains("冰风暴"));
    assert_eq!(reparsed[3].source, "末日之剑");
}

/// 端到端：MOD 自带的中文文件里「英文原文没有」的条目，绝不能因为写回而消失。
///
/// 复现（修复前）：`Localization/Chinese/extra.xml` 已存在于 PAK 中，含一个英文
/// 文件里没有的 contentuid；前端按「英文优先」只提交英文那份条目 → 写回按条目
/// 列表重建整个 XML，多出来的那条被永久删掉，游戏里对应文本变成原始句柄。
#[test]
fn existing_target_entries_survive_a_subset_write_back_and_repack() {
    const EN_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<contentList>
  <content contentuid="h11111111g2222u3333i4444" version="1">Hello, adventurer.</content>
</contentList>"#;
    const ZH_XML: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<contentList>
  <content contentuid="h11111111g2222u3333i4444" version="1">你好，冒险者。</content>
  <content contentuid="h99999999g0000u1111i2222" version="5">模组作者自己加的中文条目</content>
</contentList>"#;

    let tmp = tempfile::tempdir().unwrap();
    let source = tmp.path().join("src");
    fs::create_dir_all(source.join("Localization/English")).unwrap();
    fs::create_dir_all(source.join("Localization/Chinese")).unwrap();
    fs::write(source.join("Localization/English/extra.xml"), EN_XML).unwrap();
    fs::write(source.join("Localization/Chinese/extra.xml"), ZH_XML).unwrap();

    let input_pak = tmp.path().join("M.pak");
    pack(&source, &input_pak, 0);

    let work_root = tmp.path().join("work");
    fs::create_dir_all(&work_root).unwrap();
    let (work_dir, _) = pak::open_and_extract_in(input_pak.to_str().unwrap(), &work_root).unwrap();

    // 只读英文那份，翻译后写回中文路径（与前端「英文优先」的策略一致）
    let entries = formats::read_entries(
        work_dir.to_str().unwrap(),
        "Localization/English/extra.xml",
        PakFileKind::LocalizationXml,
    )
    .unwrap();
    assert_eq!(entries.len(), 1, "英文文件只有 1 条");
    let translated: Vec<TranslationEntry> = entries
        .into_iter()
        .map(|mut entry| {
            entry.mark_translated("【译】Hello, adventurer.");
            entry
        })
        .collect();

    formats::write_entries(
        work_dir.to_str().unwrap(),
        "Localization/Chinese/extra.xml",
        PakFileKind::LocalizationXml,
        &translated,
    )
    .unwrap();

    let written =
        fs::read_to_string(work_dir.join("unpacked/Localization/Chinese/extra.xml")).unwrap();
    assert!(written.contains("【译】Hello, adventurer."), "{written}");
    assert!(
        written.contains("h99999999g0000u1111i2222"),
        "只属于中文文件的条目被删掉了: {written}"
    );

    // 打包 → 重新解包，条目仍然在，且 XML 依然合法
    let output_pak = tmp.path().join("M_zh.pak");
    pak::repack(work_dir.to_str().unwrap(), output_pak.to_str().unwrap()).unwrap();
    let verify_root = tmp.path().join("verify");
    fs::create_dir_all(&verify_root).unwrap();
    let (verify_dir, _) =
        pak::open_and_extract_in(output_pak.to_str().unwrap(), &verify_root).unwrap();

    let reparsed = formats::read_entries(
        verify_dir.to_str().unwrap(),
        "Localization/Chinese/extra.xml",
        PakFileKind::LocalizationXml,
    )
    .unwrap();
    assert_eq!(reparsed.len(), 2, "重打包后条目数必须还是 2: {reparsed:#?}");
    assert_eq!(reparsed[0].source, "【译】Hello, adventurer.");
    assert_eq!(reparsed[0].contentuid, "h11111111g2222u3333i4444");
    assert_eq!(reparsed[1].contentuid, "h99999999g0000u1111i2222");
    assert_eq!(reparsed[1].version, "5");
    assert_eq!(reparsed[1].source, "模组作者自己加的中文条目");
}
