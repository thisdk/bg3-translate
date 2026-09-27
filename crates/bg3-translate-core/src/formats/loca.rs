//! `.loca` 二进制本地化的读写（基于 `bg3rustpaklib::loca`）。
//!
//! LOCA 是 BG3 的实际本地化存储格式：`contentuid -> 文本` 的定长索引表 + 文本区。
//! 这里只做「中间表示 ↔ LocaResource」的映射，二进制细节交给底层库。

use std::path::Path;

use bg3rustpaklib::loca::{
    ENTRY_SIZE, HEADER_SIZE, LOCA_SIGNATURE, LocaError, LocaFormat, LocaResource, LocaUtils,
    LocalizedText,
};

use crate::config::write_atomic;
use crate::error::{AppError, Result};
use crate::types::TranslationEntry;

/// 索引表里 `length` 字段相对表项开头的偏移（key 64 + version 2）。
const LENGTH_FIELD_OFFSET: usize = 66;

/// 读取 `.loca` 文件。
pub fn read(path: &Path, file_name: &str) -> Result<Vec<TranslationEntry>> {
    // 读文件失败保持 `loca` 错误类别（既有行为：调用方按 code 区分）
    let bytes = std::fs::read(path).map_err(|err| AppError::Loca(LocaError::Io(err)))?;
    // 上游读取器**完全信任文件头**（`Vec::with_capacity(num_entries)` +
    // `vec![0u8; length]`），而 `.loca` 来自用户的 MOD、属于不可信输入：
    // 12 字节的伪造文件（num_entries = 0xFFFFFFFF）会让它申请 137 GB，
    // Rust 的分配失败是 **abort**（不是可捕获的错误），整个应用会直接死掉。
    check_layout(&bytes, path)?;
    let resource = LocaUtils::load(path)?;
    Ok(resource_to_entries(&resource, file_name))
}

/// 写回 `.loca` 文件（保持二进制 LOCA 格式）。
///
/// 磁盘上已有、但这次没提交的 key 会被**原样保留**：写回是按条目列表整体重建
/// 二进制索引表，只提交一部分就会把其余 key 永久删掉（游戏里对应文本变成句柄）。
/// 目标文件不存在、或不是合法 `.loca` 时按「整体重写」处理，与旧行为一致。
///
/// **先序列化到内存、再原子落盘**：上游的 `save_with_format` 是
/// `File::create`（截断）+ 边写边校验，key 超长或磁盘写满都会把用户已有的
/// `.loca` 变成 0 字节 / 半截文件（整份译文没了）。这里与 `content_list` /
/// `lsx` 一致，走 [`write_atomic`]。
pub fn write(path: &Path, entries: &[TranslationEntry]) -> Result<()> {
    let merged = merge_with_existing(path, entries)?;
    let resource = entries_to_resource(&merged);
    let mut bytes = Vec::new();
    LocaUtils::save_to_writer(&resource, &mut bytes, LocaFormat::Loca)?;
    write_atomic(path, &bytes)?;
    Ok(())
}

/// 把磁盘上已有、本次没提交的 key 补回写回列表。
///
/// 底稿不可信时**中止并报可执行错误**，决不整体重写：
/// - 文件在磁盘上、但读不出来（EACCES / Windows 共享冲突……）：按新文件处理会
///   把磁盘上没提交的 key 静默删掉；
/// - 布局非法（见 [`layout_problem`]）：此时上游**必然**读不出任何条目，
///   「整体重写」在物理上不会丢掉可读内容，但那要靠上面的论证成立；
///   一旦自检本身有误判，重写就是不可逆的数据损失，所以宁可不写。
///   唯一的例外是 0 字节文件：它**没有内容**（不需要推断），按新建处理。
fn merge_with_existing(path: &Path, entries: &[TranslationEntry]) -> Result<Vec<TranslationEntry>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        // 只有「文件不存在」才是真正的新文件语义
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(entries.to_vec()),
        Err(err) => {
            return Err(AppError::Loca(LocaError::invalid_format(format!(
                "写回目标存在但读不出来（{err}），已中止写回以免覆盖磁盘上的译文: {}",
                path.display()
            ))));
        }
    };
    // 0 字节文件没有任何可读内容（这一点不需要推断），按新建处理，避免把用户卡住
    if bytes.is_empty() {
        log::warn!("目标 .loca 是 0 字节空文件，按新建处理: {}", path.display());
        return Ok(entries.to_vec());
    }
    if let Err(reason) = layout_problem(&bytes) {
        return Err(AppError::Loca(LocaError::invalid_format(format!(
            "目标 .loca 布局不可信（{reason}），已中止写回以免丢掉磁盘上的译文；\
             确认这个文件损坏、想用当前译文重建它时请先删除它: {}",
            path.display()
        ))));
    }
    let existing = match LocaUtils::load(path) {
        Ok(resource) => resource,
        Err(err) => {
            return Err(AppError::Loca(LocaError::invalid_format(format!(
                "目标 .loca 解析失败（{err}），已中止写回以免丢掉磁盘上的译文；\
                 确认这个文件损坏、想用当前译文重建它时请先删除它: {}",
                path.display()
            ))));
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
    Ok(merged)
}

/// 头部 + 索引 + 文本区的**布局自检**（见 [`read`] 的说明）。
fn check_layout(bytes: &[u8], path: &Path) -> Result<()> {
    layout_problem(bytes).map_err(|reason| {
        AppError::Loca(LocaError::invalid_format(format!(
            ".loca 布局非法（{reason}），拒绝解析: {}",
            path.display()
        )))
    })
}

/// 返回布局问题的人话描述；`None` 表示布局自洽。
///
/// # 判定边界（只拒绝「物理上不可能」，逐条对照上游 `LocaReader::read`）
///
/// 上游行为（`bg3rustpaklib 0.1.5/src/loca/reader.rs`，本机 cargo registry 源码）：
/// 头 12 字节 → 逐条 `read_exact` 固定 70 字节（key 64 + version u16 + length u32，
/// **与 version 字段取值无关**）→ 若 `texts_offset > 12 + 70n` 则 skip 到该偏移
/// （否则原地不动，**不回退**）→ 逐条 `read_exact(length)`。
///
/// 因此下面每一条都只是「上游那次 `read_exact` 根本不可能成功」的等价条件，
/// 顺带把上游的几处无界分配（`Vec::with_capacity(num_entries)`、
/// `vec![0u8; texts_offset - bytes_read]`、`vec![0u8; length]`）压到文件大小以内：
///
/// | # | 判定 | 依据 | 只拒绝物理不可能？ |
/// |---|---|---|---|
/// | ① | 文件 ≥ 12 字节 | 上游 `read_exact(12)` | 是 |
/// | ② | 签名 == `LOCA_SIGNATURE` | 上游第一个检查 | 是 |
/// | ③ | `12 + 70·n ≤ 文件长度` | 上游逐条 `read_exact(70)` | 是 |
/// | ④ | `texts_offset ≤ 文件长度` | 该偏移只用于 skip，越界时 skip 的 `read_exact` 必失败 | 是 |
/// | ⑤ | `max(texts_offset, 12+70n) + Σlength ≤ 文件长度` | 文本区逐条 `read_exact(length)` | 是 |
///
/// **刻意不做的事**：不要求 `texts_offset ≥ 12 + 70n` —— 上游对更小的值原地不动、
/// 照读不误（`texts_offset_below_the_table_is_still_readable` 钉住了这条）。
/// 全部算术走 `u64` + `checked_*`，不依赖 `usize` 宽度。
fn layout_problem(bytes: &[u8]) -> std::result::Result<(), String> {
    let file_len = bytes.len() as u64;
    let signature = u32_at(bytes, 0).ok_or_else(|| {
        format!(
            "文件只有 {} 字节，连 {HEADER_SIZE} 字节的头部都不完整",
            bytes.len()
        )
    })?;
    if signature != LOCA_SIGNATURE {
        return Err(format!("签名不是 LOCA（0x{signature:08X}）"));
    }
    let num_entries = u64::from(u32_at(bytes, 4).ok_or_else(|| "头部不完整".to_string())?);
    let texts_offset = u64::from(u32_at(bytes, 8).ok_or_else(|| "头部不完整".to_string())?);

    let table_end = num_entries
        .checked_mul(ENTRY_SIZE as u64)
        .and_then(|size| size.checked_add(HEADER_SIZE as u64))
        .ok_or_else(|| format!("条目数 {num_entries} 过大，索引表长度溢出"))?;
    if table_end > file_len {
        return Err(format!(
            "条目数 {num_entries} 需要 {table_end} 字节的索引表，但文件只有 {} 字节",
            bytes.len()
        ));
    }
    if texts_offset > file_len {
        return Err(format!(
            "文本区偏移 {texts_offset} 越界（文件只有 {} 字节）",
            bytes.len()
        ));
    }

    let text_start = texts_offset.max(table_end);
    let mut text_total: u64 = 0;
    for index in 0..num_entries {
        let at = index
            .checked_mul(ENTRY_SIZE as u64)
            .and_then(|offset| offset.checked_add(HEADER_SIZE as u64))
            .and_then(|offset| offset.checked_add(LENGTH_FIELD_OFFSET as u64))
            .ok_or_else(|| format!("第 {index} 条表项下标溢出"))?;
        let at = usize::try_from(at).map_err(|_| format!("第 {index} 条表项下标超出平台范围"))?;
        let length =
            u64::from(u32_at(bytes, at).ok_or_else(|| format!("第 {index} 条表项被截断"))?);
        text_total = text_total
            .checked_add(length)
            .ok_or_else(|| format!("第 {index} 条文本长度累加溢出"))?;
    }
    let text_end = text_start
        .checked_add(text_total)
        .ok_or_else(|| "文本区长度溢出".to_string())?;
    if text_end > file_len {
        return Err(format!(
            "文本区声称 {text_total} 字节（起点 {text_start}），但文件只有 {} 字节",
            bytes.len()
        ));
    }
    Ok(())
}

/// 读一个小端 `u32`；越界返回 `None`（不 panic）。
fn u32_at(bytes: &[u8], at: usize) -> Option<u32> {
    let end = at.checked_add(4)?;
    let slice = bytes.get(at..end)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
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

    /// 写回失败（key 超长 / 磁盘写满 / IO 错误）**不能摧毁磁盘上已有的 `.loca`**。
    ///
    /// 旧实现走 `LocaUtils::save_with_format`：它先 `File::create`（截断），
    /// 再由 `LocaWriter` 校验 key 长度 —— 校验失败时文件已经是 0 字节，
    /// 用户整份已有译文都没了。
    ///
    /// 复现（修复前）：`before_len=160 after_len=0`，原文件被清空。
    #[test]
    fn failed_write_does_not_destroy_the_existing_loca() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.loca");
        write(&path, &sample_entries()).unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(!before.is_empty());

        // 65 字节的 key 必然让 LocaWriter 报 KeyTooLong（写盘发生在校验之后）
        let long_key = "k".repeat(65);
        let err = write(
            &path,
            &[TranslationEntry::new("English.loca", &long_key, "1", "坏")],
        )
        .unwrap_err();
        assert_eq!(err.code(), "loca", "超长 key 必须报 loca 错误: {err}");

        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "写回失败时原文件必须逐字节不变"
        );
        // 原译文仍可读回
        let back = read(&path, "Chinese.loca").unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].source, "Hello");
    }

    /// 目标文件**存在但读不出来**时不能当成「新文件」整体重建。
    ///
    /// 复现（修复前）：`merge_with_existing` 把读取失败一律降级成 `log::warn!`
    /// 后整体重写 —— 磁盘上没提交的 key 会被静默删掉。
    #[cfg(unix)]
    #[test]
    fn write_refuses_when_the_target_exists_but_cannot_be_read() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Chinese.loca");
        write(&path, &sample_entries()).unwrap();
        let before = std::fs::read(&path).unwrap();

        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o000);
        std::fs::set_permissions(&path, perms).unwrap();
        if std::fs::File::open(&path).is_ok() {
            eprintln!("跳过：当前用户仍可读 chmod 000 的文件（可能以 root 运行）");
            return;
        }

        let err = write(
            &path,
            &[TranslationEntry::new("English.loca", "h0001", "1", "改")],
        )
        .unwrap_err();
        assert_eq!(err.code(), "loca", "读不出来必须报 loca 错误: {err}");
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

    /// 文件头撒谎（`num_entries = 0xFFFFFFFF`）不能被直接送进上游读取器。
    ///
    /// `LocaReader::read_entries` 会 `Vec::with_capacity(num_entries)`：12 字节的
    /// 伪造文件让进程申请 137 GB，Rust 的分配失败是 **abort**（不是可捕获的错误），
    /// 整个应用连同用户未保存的进度一起死。`.loca` 来自用户的 MOD，是不可信输入。
    ///
    /// 复现（修复前）：`memory allocation of 137438953440 bytes failed` → SIGABRT
    /// （整条测试进程被杀，当时的探针输出见发布说明）。
    #[test]
    fn lying_entry_count_is_rejected_without_a_giant_allocation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bomb.loca");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&0x4143_4f4cu32.to_le_bytes()); // "LOCC"
        bytes.extend_from_slice(&u32::MAX.to_le_bytes()); // 声称 42 亿条
        bytes.extend_from_slice(&12u32.to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();

        let err = read(&path, "bomb.loca").unwrap_err();
        assert_eq!(err.code(), "loca");
        assert!(
            err.to_string().contains("条目数"),
            "错误信息要指出条目数不合理: {err}"
        );

        // 写回路径同样不能把这种文件当底稿送去解析：布局不可信 ⇒ 中止（不重写）
        let before = std::fs::read(&path).unwrap();
        let err = write(
            &path,
            &[TranslationEntry::new("English.loca", "h1", "1", "x")],
        )
        .unwrap_err();
        assert_eq!(err.code(), "loca", "{err}");
        assert!(
            err.to_string().contains("中止写回"),
            "错误信息要说清为什么中止: {err}"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "布局不可信的目标必须逐字节保持原样"
        );
    }

    /// 正向用例：上游对 `texts_offset < 索引表结尾` 的文件**原地不动、照读不误**，
    /// 所以自检绝不能要求 `texts_offset >= 12 + 70n`（那会把合法文件判成非法，
    /// 用户既读不出来、写回又被中止）。
    #[test]
    fn texts_offset_below_the_table_is_still_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("offset0.loca");
        write(&path, &sample_entries()).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        bytes[8..12].copy_from_slice(&0u32.to_le_bytes()); // texts_offset = 0
        std::fs::write(&path, &bytes).unwrap();

        let back = read(&path, "offset0.loca").unwrap();
        assert_eq!(back.len(), 2, "texts_offset=0 是上游允许的形态");
        assert_eq!(back[0].source, "Hello");
        assert_eq!(back[1].source, "World");

        // 写回也必须能读底稿做合并（不能因为 texts_offset=0 就中止）
        let mut first = TranslationEntry::new("English.loca", "h0001", "1", "Hello");
        first.mark_translated("你好");
        write(&path, &[first]).unwrap();
        let back = read(&path, "offset0.loca").unwrap();
        assert_eq!(back.len(), 2, "未提交的 key 必须被保留: {back:?}");
        assert_eq!(back[0].source, "你好");
    }

    /// 正向用例：`texts_offset > 索引表结尾`（索引表与文本区之间有空隙）的文件，
    /// 上游会 skip 过去，必须照常读出来。
    #[test]
    fn texts_offset_beyond_the_table_is_skipped_and_still_readable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gap.loca");
        write(&path, &sample_entries()).unwrap();

        let mut bytes = std::fs::read(&path).unwrap();
        let table_end = HEADER_SIZE + 2 * ENTRY_SIZE;
        assert_eq!(bytes[8..12], (table_end as u32).to_le_bytes());
        // 在索引表与文本区之间插入 2 字节空隙，并把偏移指过去
        bytes.splice(table_end..table_end, [0u8, 0u8]);
        bytes[8..12].copy_from_slice(&((table_end + 2) as u32).to_le_bytes());
        std::fs::write(&path, &bytes).unwrap();

        let back = read(&path, "gap.loca").unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].source, "Hello");
        assert_eq!(back[1].source, "World");
    }

    /// 正向用例：条目 `length = 0`（上游 `read_text(0)` 直接返回空串）与
    /// 空表（只有 12 字节头部）都必须照常读出来。
    #[test]
    fn empty_text_and_empty_table_are_readable() {
        let dir = tempfile::tempdir().unwrap();

        // ① length = 0
        let zero = dir.path().join("zero.loca");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&LOCA_SIGNATURE.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&((HEADER_SIZE + ENTRY_SIZE) as u32).to_le_bytes());
        bytes.extend_from_slice(b"h1");
        bytes.resize(HEADER_SIZE + 64, 0); // key 补满 64 字节
        bytes.extend_from_slice(&1u16.to_le_bytes()); // version
        bytes.extend_from_slice(&0u32.to_le_bytes()); // length = 0
        assert_eq!(bytes.len(), HEADER_SIZE + ENTRY_SIZE);
        std::fs::write(&zero, &bytes).unwrap();
        let back = read(&zero, "zero.loca").unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].contentuid, "h1");
        assert_eq!(back[0].source, "");

        // ② n = 0（只有头部）
        let empty = dir.path().join("empty.loca");
        write(&empty, &[]).unwrap();
        assert_eq!(std::fs::metadata(&empty).unwrap().len(), HEADER_SIZE as u64);
        assert!(read(&empty, "empty.loca").unwrap().is_empty());
    }

    /// 正向用例：`version` 字段取任意 u16 都不影响条目尺寸（上游固定 70 字节），
    /// 自检因此与 version 无关。我们 writer 产出的文件必须逐条通过。
    #[test]
    fn versions_do_not_affect_the_layout_check() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("versions.loca");
        let entries = vec![
            TranslationEntry::new("English.loca", "h1", "1", "One"),
            TranslationEntry::new("English.loca", "h2", "2", "Two"),
            TranslationEntry::new("English.loca", "h3", "65535", "Max"),
        ];
        write(&path, &entries).unwrap();
        let back = read(&path, "versions.loca").unwrap();
        assert_eq!(back.len(), 3);
        assert_eq!(back[2].version, "65535");
        // 表长恒为 12 + 70×3，与 version 取值无关
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            (HEADER_SIZE + 3 * ENTRY_SIZE) as u64 + 4 + 4 + 4
        );
    }

    /// 写回路径的安全语义：**非空**但不可信的底稿一律中止，不整体重写；
    /// 0 字节文件没有任何内容可丢，按新建处理（避免把用户卡死）。
    #[test]
    fn suspicious_target_aborts_but_zero_byte_target_is_rebuilt() {
        let dir = tempfile::tempdir().unwrap();
        let entry = TranslationEntry::new("English.loca", "h1", "1", "One");

        // ① 非空的垃圾文件：中止 + 原样保留
        let trash = dir.path().join("trash.loca");
        std::fs::write(&trash, b"this is not a loca file at all").unwrap();
        let before = std::fs::read(&trash).unwrap();
        let err = write(&trash, std::slice::from_ref(&entry)).unwrap_err();
        assert_eq!(err.code(), "loca", "{err}");
        assert!(err.to_string().contains("中止写回"), "{err}");
        assert_eq!(std::fs::read(&trash).unwrap(), before);

        // ② 0 字节文件：没有内容可丢 → 按新建处理
        let blank = dir.path().join("blank.loca");
        std::fs::write(&blank, b"").unwrap();
        write(&blank, std::slice::from_ref(&entry)).unwrap();
        let back = read(&blank, "blank.loca").unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0].source, "One");

        // ③ 布局自洽、但上游仍然解析失败（key 不是合法 UTF-8）：同样中止，不重写
        let bad_utf8 = dir.path().join("badkey.loca");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&LOCA_SIGNATURE.to_le_bytes());
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&((HEADER_SIZE + ENTRY_SIZE) as u32).to_le_bytes());
        bytes.extend_from_slice(&[0xFF, 0xFE, 0x00]); // 非法 UTF-8 的 key
        bytes.resize(HEADER_SIZE + 64, 0);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&2u32.to_le_bytes()); // "x\0"
        bytes.extend_from_slice(b"x\0");
        std::fs::write(&bad_utf8, &bytes).unwrap();
        let before = std::fs::read(&bad_utf8).unwrap();
        let err = write(&bad_utf8, std::slice::from_ref(&entry)).unwrap_err();
        assert_eq!(err.code(), "loca", "{err}");
        assert!(err.to_string().contains("中止写回"), "{err}");
        assert_eq!(std::fs::read(&bad_utf8).unwrap(), before);
    }

    /// 索引表 / 文本区越界的文件同样要在我们这层被拒（不能只在读时报 IO 错）。
    #[test]
    fn lying_offsets_and_lengths_are_rejected() {
        let dir = tempfile::tempdir().unwrap();

        // ① texts_offset 声称指向 4 GiB 处（上游会 vec![0u8; 4 GiB] 再 read_exact）
        let p1 = dir.path().join("offset.loca");
        let mut b1 = Vec::new();
        b1.extend_from_slice(&0x4143_4f4cu32.to_le_bytes());
        b1.extend_from_slice(&0u32.to_le_bytes());
        b1.extend_from_slice(&u32::MAX.to_le_bytes());
        std::fs::write(&p1, &b1).unwrap();
        let err = read(&p1, "offset.loca").unwrap_err();
        assert_eq!(err.code(), "loca", "{err}");
        assert!(
            err.to_string().contains("布局"),
            "必须由布局自检拦下（上游只会报 IO 错）: {err}"
        );

        // ② 索引表声称有 1 条，但文件里连一条表项都没有
        let p2 = dir.path().join("short_table.loca");
        let mut b2 = Vec::new();
        b2.extend_from_slice(&0x4143_4f4cu32.to_le_bytes());
        b2.extend_from_slice(&1u32.to_le_bytes());
        b2.extend_from_slice(&82u32.to_le_bytes());
        b2.resize(12, 0);
        std::fs::write(&p2, &b2).unwrap();
        let err = read(&p2, "short_table.loca").unwrap_err();
        assert_eq!(err.code(), "loca", "{err}");
        assert!(
            err.to_string().contains("布局"),
            "必须由布局自检拦下: {err}"
        );

        // ③ 单条文本 length 声称 4 GiB，文件里只有 1 字节
        let p3 = dir.path().join("length.loca");
        let mut b3 = Vec::new();
        b3.extend_from_slice(&0x4143_4f4cu32.to_le_bytes());
        b3.extend_from_slice(&1u32.to_le_bytes());
        b3.extend_from_slice(&82u32.to_le_bytes());
        b3.push(b'h');
        b3.resize(12 + 64, 0);
        b3.extend_from_slice(&1u16.to_le_bytes());
        b3.extend_from_slice(&u32::MAX.to_le_bytes());
        b3.push(b'x');
        b3.push(0);
        std::fs::write(&p3, &b3).unwrap();
        let err = read(&p3, "length.loca").unwrap_err();
        assert_eq!(err.code(), "loca", "{err}");
        assert!(
            err.to_string().contains("布局"),
            "必须由布局自检拦下: {err}"
        );
    }

    /// 正向对照：我们自己写出来的 `.loca` 必须通过布局自检（不能误伤）。
    #[test]
    fn layout_check_accepts_files_we_wrote() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ok.loca");
        let mut entries = sample_entries();
        entries[0].mark_translated("你好，世界");
        write(&path, &entries).unwrap();

        let back = read(&path, "ok.loca").unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].source, "你好，世界");
    }
}
