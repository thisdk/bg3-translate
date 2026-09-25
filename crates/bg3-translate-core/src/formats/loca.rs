//! `.loca` 二进制本地化的读写（基于 `bg3rustpaklib::loca`）。
//!
//! LOCA 是 BG3 的实际本地化存储格式：`contentuid -> 文本` 的定长索引表 + 文本区。
//! 这里只做「中间表示 ↔ LocaResource」的映射，二进制细节交给底层库。

use std::path::Path;

use bg3rustpaklib::loca::{LocaFormat, LocaResource, LocaUtils, LocalizedText};

use crate::error::Result;
use crate::types::TranslationEntry;

/// 读取 `.loca` 文件。
pub fn read(path: &Path, file_name: &str) -> Result<Vec<TranslationEntry>> {
    let resource = LocaUtils::load(path)?;
    Ok(resource_to_entries(&resource, file_name))
}

/// 写回 `.loca` 文件（保持二进制 LOCA 格式）。
pub fn write(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let resource = entries_to_resource(entries);
    LocaUtils::save_with_format(&resource, path, LocaFormat::Loca)?;
    Ok(())
}

/// `LocaResource` → 条目列表。
pub fn resource_to_entries(resource: &LocaResource, file_name: &str) -> Vec<TranslationEntry> {
    resource
        .entries
        .iter()
        .map(|text| {
            TranslationEntry::new(
                file_name,
                text.key.clone(),
                text.version.to_string(),
                text.text.clone(),
            )
        })
        .collect()
}

/// 条目列表 → `LocaResource`。
///
/// version 解析失败时退回 1（与游戏默认一致），不会因为脏数据整体失败。
pub fn entries_to_resource(entries: &[TranslationEntry]) -> LocaResource {
    let mapped = entries
        .iter()
        .map(|entry| {
            LocalizedText::new(
                entry.contentuid.clone(),
                entry.version.parse().unwrap_or(1),
                entry.effective_text().to_string(),
            )
        })
        .collect();
    LocaResource::with_entries(mapped)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_entries() -> Vec<TranslationEntry> {
        vec![
            TranslationEntry::new("Localization/English/a.loca", "h0001", "1", "Hello"),
            TranslationEntry::new("Localization/English/a.loca", "h0002", "3", "World"),
        ]
    }

    #[test]
    fn entries_roundtrip_through_resource() {
        let entries = sample_entries();
        let resource = entries_to_resource(&entries);
        assert_eq!(resource.len(), 2);
        assert_eq!(resource.entries[0].key, "h0001");
        assert_eq!(resource.entries[0].version, 1);
        assert_eq!(resource.entries[1].version, 3);

        let back = resource_to_entries(&resource, "Localization/English/a.loca");
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].source, "Hello");
        assert_eq!(back[0].contentuid, "h0001");
        assert_eq!(back[0].version, "1");
        assert_eq!(back[0].id, "Localization/English/a.loca#h0001");
    }

    #[test]
    fn resource_uses_target_when_present_and_source_otherwise() {
        let mut entries = sample_entries();
        entries[0].mark_translated("你好");
        let resource = entries_to_resource(&entries);
        assert_eq!(resource.entries[0].text, "你好");
        assert_eq!(resource.entries[1].text, "World");
    }

    #[test]
    fn invalid_version_falls_back_to_one() {
        let entry = TranslationEntry::new("a.loca", "h1", "not-a-number", "x");
        let resource = entries_to_resource(&[entry]);
        assert_eq!(resource.entries[0].version, 1);
    }

    #[test]
    fn binary_file_roundtrip_is_lossless() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.loca");

        let entries = sample_entries();
        write(&path, &entries).unwrap();
        assert!(path.is_file());

        let read_back = read(&path, "Localization/English/a.loca").unwrap();
        assert_eq!(read_back.len(), 2);
        assert_eq!(read_back[0].source, "Hello");
        assert_eq!(read_back[1].source, "World");
        assert_eq!(read_back[1].version, "3");
    }

    #[test]
    fn translated_text_survives_a_binary_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("b.loca");

        let mut entries = sample_entries();
        entries[0].mark_translated("你好，<LSTag Tag=\"Fire\">火球</LSTag> {1}");
        entries[1].mark_translated("世界");
        write(&path, &entries).unwrap();

        let read_back = read(&path, "b.loca").unwrap();
        assert_eq!(
            read_back[0].source,
            "你好，<LSTag Tag=\"Fire\">火球</LSTag> {1}"
        );
        assert_eq!(read_back[1].source, "世界");
        // 重打包必须保持 contentuid 与 version
        assert_eq!(read_back[0].contentuid, "h0001");
        assert_eq!(read_back[1].version, "3");
    }

    #[test]
    fn reading_a_missing_file_is_an_error() {
        let err = read(Path::new("/nope/missing.loca"), "missing.loca").unwrap_err();
        assert_eq!(err.code(), "loca");
    }
}
