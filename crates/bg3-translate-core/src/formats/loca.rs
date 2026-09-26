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
///
/// 磁盘上已有、但这次没提交的 key 会被**原样保留**：写回是按条目列表整体重建
/// 二进制索引表，只提交一部分就会把其余 key 永久删掉（游戏里对应文本变成句柄）。
/// 目标文件不存在、或不是合法 `.loca` 时按「整体重写」处理，与旧行为一致。
pub fn write(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let merged = merge_with_existing(path, entries);
    let resource = entries_to_resource(&merged);
    LocaUtils::save_with_format(&resource, path, LocaFormat::Loca)?;
    Ok(())
}

/// 把磁盘上已有、本次没提交的 key 补回写回列表。
fn merge_with_existing(path: &Path, entries: &[TranslationEntry]) -> Vec<TranslationEntry> {
    if !path.is_file() {
        return entries.to_vec();
    }
    let existing = match LocaUtils::load(path) {
        Ok(resource) => resource,
        Err(err) => {
            // 读不出来（空文件 / 不是 LOCA / 被截断）时按整体重写处理，
            // 但不能静默：日志里要留下痕迹。
            log::warn!(
                "目标 .loca 无法解析（{err}），将整体重写: {}",
                path.display()
            );
            return entries.to_vec();
        }
    };

    let known: std::collections::HashSet<&str> = entries
        .iter()
        .map(|entry| entry.contentuid.as_str())
        .collect();
    let mut merged = entries.to_vec();
    let before = merged.len();
    for text in &existing.entries {
        if !known.contains(text.key.as_str()) {
            merged.push(TranslationEntry::new(
                String::new(),
                text.key.clone(),
                text.version.to_string(),
                text.text.clone(),
            ));
        }
    }
    let kept = merged.len() - before;
    if kept > 0 {
        log::warn!("写回列表比磁盘上的 .loca 少 {kept} 个 key，已原样保留这些条目");
    }
    merged
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
    fn error_entries_are_written_as_source() {
        // F-01：status == error 的条目即使 target 非空也必须退回原文
        let mut entries = sample_entries();
        entries[0].mark_translated("坏译文 {1} 丢了");
        entries[0].mark_error("结构校验未通过：占位符 {1} 缺失（已重试 1 次）");

        let resource = entries_to_resource(&entries);

        assert_eq!(resource.entries[0].text, "Hello", "error 条目必须退回原文");
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

    /// 写回是按条目列表**整体重建** `.loca`：磁盘上已有、但本次没提交的 key
    /// 不能被删掉。复现（修复前）：先写 h1+h2，再只提交 h1 → h2 消失。
    #[test]
    fn write_keeps_keys_that_already_exist_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.loca");

        let existing = vec![
            TranslationEntry::new("English.loca", "h1", "1", "One"),
            TranslationEntry::new("English.loca", "h_only_chinese", "2", "只有中文才有的条目"),
        ];
        write(&path, &existing).unwrap();

        let mut translated = TranslationEntry::new("English.loca", "h1", "1", "One");
        translated.mark_translated("一");
        write(&path, &[translated]).unwrap();

        let back = read(&path, "Chinese.loca").unwrap();
        assert_eq!(back.len(), 2, "已有 key 不能被删掉: {back:?}");
        assert_eq!(back[0].contentuid, "h1");
        assert_eq!(back[0].source, "一");
        assert_eq!(back[1].contentuid, "h_only_chinese");
        assert_eq!(back[1].version, "2");
        assert_eq!(back[1].source, "只有中文才有的条目");
    }

    /// 空列表写回不会清空一个已有内容的 `.loca`。
    #[test]
    fn writing_an_empty_list_does_not_wipe_an_existing_loca() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.loca");
        write(&path, &sample_entries()).unwrap();

        write(&path, &[]).unwrap();

        assert_eq!(read(&path, "Chinese.loca").unwrap().len(), 2);
    }
}
