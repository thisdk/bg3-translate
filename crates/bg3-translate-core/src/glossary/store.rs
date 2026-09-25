//! 术语表：加载 / 保存 / 重置 / 导入。
//!
//! 两条铁律：
//! 1. 路径一律走 [`crate::config::glossary_path_in`]，模块内**不再自己拼目录**，
//!    否则便携模式/系统模式切换时会出现两个 glossary.json；
//! 2. 解析与落盘分离 —— [`Glossary::from_json`] 只解析+清洗，**绝不写盘**。
//!    旧实现的 `import_json` 顺手写盘，导致单测跑一次就污染真实用户配置。

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config::{data_dir, glossary_path_in, write_atomic};
use crate::error::{AppError, Result};

use super::entry::{GlossaryEntry, clean_entry};
use super::matcher::GlossaryMatcher;
use super::seed::SEED_ENTRIES;

/// 整个术语表（结构兼容导入文件：`{ "terms": [...] }`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Glossary {
    #[serde(default)]
    pub terms: Vec<GlossaryEntry>,
}

impl Default for Glossary {
    /// 默认表 = 官方种子：旧代码里 `Glossary::default()` 就是这个语义，
    /// 调用方（含 `serde` 的 `unwrap_or_default` 回退路径）依赖它。
    fn default() -> Self {
        Self::seeded()
    }
}

impl Glossary {
    /// 内置官方种子（不读盘、不写盘）。
    pub fn seeded() -> Self {
        Self {
            terms: SEED_ENTRIES.iter().map(|seed| seed.to_entry()).collect(),
        }
    }

    /// 从当前数据目录加载；文件不存在则写入种子后返回。
    pub fn load() -> Result<Self> {
        Self::load_from(&data_dir()?.path)
    }

    /// 从指定目录加载（测试用）。
    ///
    /// 文件损坏时回退到种子并告警：术语表坏掉不该让整个应用起不来。
    pub fn load_from(dir: &Path) -> Result<Self> {
        let path = glossary_path_in(dir);
        if !path.exists() {
            let glossary = Self::seeded();
            glossary.save_to(dir)?;
            return Ok(glossary);
        }
        let content = std::fs::read_to_string(&path)?;
        match serde_json::from_str::<Glossary>(&content) {
            Ok(glossary) => Ok(glossary),
            Err(err) => {
                log::warn!("术语表损坏（{}），已回退官方种子: {err}", path.display());
                Ok(Self::seeded())
            }
        }
    }

    /// 保存到当前数据目录。
    pub fn save(&self) -> Result<()> {
        self.save_to(&data_dir()?.path)
    }

    /// 保存到指定目录（原子写，避免断电留下半个 JSON）。
    pub fn save_to(&self, dir: &Path) -> Result<()> {
        let path = glossary_path_in(dir);
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| AppError::Config(format!("序列化失败: {e}")))?;
        write_atomic(&path, content.as_bytes())
    }

    /// 重置为官方种子并落盘。
    pub fn reset() -> Result<Self> {
        let glossary = Self::seeded();
        glossary.save()?;
        Ok(glossary)
    }

    /// 从 JSON 字符串解析并清洗：**纯函数，不落盘**。
    ///
    /// 这是导入路径的核心，写成纯函数的原因是可测且无副作用：
    /// 单测可以放心地喂 2 万条真实术语，而不会覆盖用户自己的 glossary.json。
    pub fn from_json(json_str: &str) -> Result<Self> {
        let imported: Glossary = serde_json::from_str(json_str)
            .map_err(|e| AppError::Config(format!("术语表 JSON 解析失败: {e}")))?;

        let mut cleaned = Glossary { terms: Vec::new() };
        let mut removed = 0usize;
        for mut entry in imported.terms {
            match clean_entry(&mut entry) {
                Some(entry) => cleaned.terms.push(entry),
                None => removed += 1,
            }
        }
        log::info!(
            "术语表导入清洗：{} 条保留，{} 条噪音过滤",
            cleaned.terms.len(),
            removed
        );
        Ok(cleaned)
    }

    /// 从 JSON 字符串导入：解析 + 清洗 + 落盘。
    pub fn import_json(json_str: &str) -> Result<Self> {
        let glossary = Self::from_json(json_str)?;
        glossary.save()?;
        Ok(glossary)
    }

    /// 新增或按 `source` 覆盖同名条目。
    pub fn add(&mut self, entry: GlossaryEntry) -> Result<()> {
        if entry.source.trim().is_empty() || entry.target.trim().is_empty() {
            return Err(AppError::Config("术语的中英文均不能为空".into()));
        }
        if let Some(existing) = self.terms.iter_mut().find(|e| e.source == entry.source) {
            *existing = entry;
        } else {
            self.terms.push(entry);
        }
        Ok(())
    }

    /// 把 `old_source` 指向的条目替换为 `entry`。
    pub fn update(&mut self, old_source: &str, entry: GlossaryEntry) -> Result<()> {
        if entry.source.trim().is_empty() || entry.target.trim().is_empty() {
            return Err(AppError::Config("术语的中英文均不能为空".into()));
        }
        let index = self
            .terms
            .iter()
            .position(|e| e.source == old_source)
            .ok_or_else(|| AppError::Config("找不到要更新的术语".into()))?;
        self.terms[index] = entry;
        Ok(())
    }

    /// 删除指定 `source` 的条目；官方术语只能禁用/覆盖，不能删。
    pub fn delete(&mut self, source: &str) -> Result<()> {
        let entry = self
            .terms
            .iter()
            .find(|e| e.source == source)
            .ok_or_else(|| AppError::Config("找不到要删除的术语".into()))?;
        if entry.source_kind == "official" {
            return Err(AppError::Config(
                "官方术语不可删除（可禁用或编辑覆盖）".into(),
            ));
        }
        self.terms.retain(|e| e.source != source);
        Ok(())
    }

    /// 构造匹配器（一次性预处理，可反复使用）。
    pub fn matcher(&self) -> GlossaryMatcher {
        GlossaryMatcher::new(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn user_entry(source: &str, target: &str) -> GlossaryEntry {
        GlossaryEntry {
            source: source.to_string(),
            target: target.to_string(),
            ..GlossaryEntry::default()
        }
    }

    #[test]
    fn seeded_contains_official_terms() {
        let glossary = Glossary::seeded();
        assert!(
            glossary.terms.len() >= 70,
            "官方种子过少: {}",
            glossary.terms.len()
        );
        assert!(glossary.terms.iter().all(|e| e.source_kind == "official"));
        assert!(glossary.terms.iter().any(|e| e.source == "Paladin"));
        assert_eq!(glossary, Glossary::default());
        assert_eq!(Glossary::default(), Glossary::seeded());
    }

    #[test]
    fn glossary_deserializes_terms_object() {
        let json = r#"{"terms":[{"source":"Fireball","target":"火球术","category":"spell","source_kind":"official","enabled":true,"ambiguous":false,"whole_word":true,"case_sensitive":false,"count":5}]}"#;
        let glossary: Glossary = serde_json::from_str(json).unwrap();
        assert_eq!(glossary.terms.len(), 1);
        assert_eq!(glossary.terms[0].target, "火球术");
        assert_eq!(glossary.terms[0].count, 5);

        // 缺省 terms 也要能读（空表）
        let empty: Glossary = serde_json::from_str("{}").unwrap();
        assert!(empty.terms.is_empty());
    }

    #[test]
    fn glossary_serializes_without_category() {
        let glossary = Glossary {
            terms: vec![user_entry("Fireball", "火球术")],
        };
        let json = serde_json::to_string(&glossary).unwrap();
        assert!(!json.contains("category"), "category 不应落盘: {json}");
        assert!(json.contains("\"sourceKind\""));
    }

    #[test]
    fn roundtrips_through_explicit_dir() {
        let dir = tempfile::tempdir().unwrap();
        // 目录里没有文件：load_from 会写入种子
        let loaded = Glossary::load_from(dir.path()).unwrap();
        assert_eq!(loaded, Glossary::seeded());
        assert!(glossary_path_in(dir.path()).exists());

        let mut glossary = loaded;
        glossary.add(user_entry("My Term", "我的术语")).unwrap();
        glossary.save_to(dir.path()).unwrap();

        let reloaded = Glossary::load_from(dir.path()).unwrap();
        // category 是 skip_serializing 的兼容字段，落盘再读会回到默认值；
        // 真正进游戏的是 source/target，只比较它们
        let pairs = |g: &Glossary| -> Vec<(String, String)> {
            g.terms
                .iter()
                .map(|e| (e.source.clone(), e.target.clone()))
                .collect()
        };
        assert_eq!(pairs(&reloaded), pairs(&glossary));
        assert_eq!(reloaded.terms.last().unwrap().target, "我的术语");
        assert_eq!(reloaded.terms.last().unwrap().source_kind, "user");
        assert_eq!(
            reloaded.terms[0].category, "name_or_title",
            "category 不落盘"
        );
    }

    #[test]
    fn corrupted_file_falls_back_to_seed() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(glossary_path_in(dir.path()), b"{ not json").unwrap();
        let loaded = Glossary::load_from(dir.path()).unwrap();
        assert_eq!(loaded, Glossary::seeded());
    }

    #[test]
    fn from_json_is_pure_and_cleans() {
        let dir = tempfile::tempdir().unwrap();
        // 先放一份「用户已有」的术语表，随后证明 from_json 完全没碰它
        Glossary {
            terms: vec![user_entry("Existing", "已有")],
        }
        .save_to(dir.path())
        .unwrap();
        let before = fs::read_to_string(glossary_path_in(dir.path())).unwrap();

        let json = r#"{"terms":[
            {"source":"[1] from [2]","target":"占位符"},
            {"source":"Paladin","target":"圣武士","count":5}
        ]}"#;
        let cleaned = Glossary::from_json(json).unwrap();

        assert_eq!(cleaned.terms.len(), 1);
        assert_eq!(cleaned.terms[0].source, "Paladin");
        // 关键回归：from_json 不允许落盘（旧 import_json 会污染真实数据目录）
        assert_eq!(
            fs::read_to_string(glossary_path_in(dir.path())).unwrap(),
            before,
            "from_json 不应改写已有文件"
        );
        assert_eq!(
            fs::read_dir(dir.path()).unwrap().count(),
            1,
            "from_json 不应产生任何新文件"
        );
    }

    #[test]
    fn from_json_reports_parse_errors() {
        let err = Glossary::from_json("{ oops").unwrap_err();
        assert_eq!(err.code(), "config");
        assert!(err.to_string().contains("术语表 JSON 解析失败"));
    }

    #[test]
    fn from_json_accepts_empty_glossary() {
        assert!(
            Glossary::from_json(r#"{"terms":[]}"#)
                .unwrap()
                .terms
                .is_empty()
        );
        assert!(Glossary::from_json("{}").unwrap().terms.is_empty());
    }

    #[test]
    fn add_replaces_same_source_and_rejects_blanks() {
        let mut glossary = Glossary { terms: Vec::new() };
        glossary.add(user_entry("Hello", "你好")).unwrap();
        glossary.add(user_entry("Hello", "您好")).unwrap();
        assert_eq!(glossary.terms.len(), 1);
        assert_eq!(glossary.terms[0].target, "您好");

        assert!(glossary.add(user_entry("  ", "空")).is_err());
        assert!(glossary.add(user_entry("Empty", "  ")).is_err());
    }

    #[test]
    fn update_replaces_by_old_source() {
        let mut glossary = Glossary {
            terms: vec![user_entry("Hello", "你好")],
        };
        glossary.update("Hello", user_entry("Hi", "嗨")).unwrap();
        assert_eq!(glossary.terms[0].source, "Hi");
        assert_eq!(glossary.terms[0].target, "嗨");

        assert!(glossary.update("Nope", user_entry("X", "Y")).is_err());
        assert!(glossary.update("Hi", user_entry("", "Y")).is_err());
    }

    #[test]
    fn delete_protects_official_entries() {
        let mut glossary = Glossary::seeded();
        let official = glossary.terms[0].source.clone();
        let err = glossary.delete(&official).unwrap_err();
        assert_eq!(err.code(), "config");
        assert!(err.to_string().contains("官方术语不可删除"));

        glossary.add(user_entry("My Term", "我的术语")).unwrap();
        glossary.delete("My Term").unwrap();
        assert!(glossary.terms.iter().all(|e| e.source != "My Term"));

        assert!(glossary.delete("Not There").is_err());
    }

    /// 真实 20K 官方术语表（`samples/`）内容。
    ///
    /// 样本随仓库提交且非 Git LFS，缺失 = checkout 不完整：这里**直接失败**
    /// 而不是跳过 —— 跳过会让这条真实数据用例静默变空，而测试依然全绿。
    fn real_glossary_json() -> String {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .and_then(std::path::Path::parent)
            .expect("crate 应位于 <repo>/crates/bg3-translate-core")
            .join("samples/bg3-official-glossary.json");
        std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "真实术语表样本缺失或不可读：{}（{err}）。该文件随仓库提交（非 Git LFS），\
                 缺失说明 checkout 不完整；请执行 `git checkout -- samples/` 或重新 clone。",
                path.display()
            )
        })
    }

    #[test]
    fn real_glossary_import_filters_noise_but_keeps_terms() {
        let json = real_glossary_json();
        let raw: Glossary = serde_json::from_str(&json).unwrap();
        let cleaned = Glossary::from_json(&json).unwrap();

        assert!(
            cleaned.terms.len() < raw.terms.len(),
            "应过滤掉部分噪音：原始 {} 条",
            raw.terms.len()
        );
        assert!(
            cleaned.terms.len() > 15_000,
            "应保留绝大多数有效术语，实际 {}",
            cleaned.terms.len()
        );
        assert!(!cleaned.terms.iter().any(|t| t.source.contains("[1]")));
        assert!(!cleaned.terms.iter().any(|t| t.source.contains("[IE_")));
        assert!(
            !cleaned.terms.iter().any(|t| t.source == "'Barnabus'"),
            "整条外层引号应被剥离"
        );
        assert!(
            cleaned
                .terms
                .iter()
                .any(|t| t.source.starts_with('\'') && !t.source.ends_with('\'')),
            "局部引号应保留"
        );
        assert!(
            cleaned
                .terms
                .iter()
                .any(|t| t.source == "Mind Flayer" && t.target == "夺心魔"),
            "真实条目的译名不能被动过"
        );

        // 清洗是幂等的：把清洗结果再导一次，数量不再变化（否则重复导入会持续丢数据）
        let roundtrip = serde_json::to_string(&cleaned).unwrap();
        let again = Glossary::from_json(&roundtrip).unwrap();
        assert_eq!(again.terms.len(), cleaned.terms.len());
    }

    #[test]
    fn matcher_reflects_current_entries() {
        let glossary = Glossary {
            terms: vec![user_entry("Fireball", "火球术")],
        };
        let matcher = glossary.matcher();
        assert_eq!(matcher.len(), 1);
        assert_eq!(matcher.find_matches("Cast Fireball")[0].target, "火球术");
    }
}
