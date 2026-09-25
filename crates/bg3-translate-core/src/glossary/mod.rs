//! 术语表：数据、清洗、命中匹配。
//!
//! 公开入口只有三个：
//! - [`Glossary`]：整表 + 加载/保存/导入/CRUD
//! - [`GlossaryEntry`]：单条术语（serde 表示即前端/导入 JSON 的字段名）
//! - [`GlossaryMatcher`]：一次性预处理后的命中匹配器（20K 条术语的关键优化）
//!
//! 子模块分工：
//! - [`entry`]：serde 表示 + 导入噪音清洗
//! - [`seed`]：102 条官方种子数据（纯数据表）
//! - [`store`]：加载/保存/重置/导入
//! - [`matcher`]：命中匹配

pub mod entry;
pub mod matcher;
pub mod seed;
pub mod store;

pub use entry::{Category, GlossaryEntry};
pub use matcher::{GlossaryMatcher, MatchedTerm};
pub use seed::{SEED_ENTRIES, SeedEntry};
pub use store::Glossary;
