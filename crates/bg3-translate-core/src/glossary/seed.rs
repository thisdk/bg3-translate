//! 内置官方术语种子（102 条，开箱即用）。
//!
//! 数据与逻辑分离：这里只有**纯数据表**，如何变成 [`GlossaryEntry`]、
//! 如何持久化都在 [`super::store`] 里。这么拆的原因是种子数据是被反复
//! review 的语料，而加载/清洗逻辑是被反复改的代码，混在一起两边都难改。
//!
//! 数据源：BG3 官方简体中文版 + D&D 5e 三宝书译名体系。
//! `count` 一律为 0：种子条目没有语料统计，命中排序靠 `source` 长度。

use super::entry::GlossaryEntry;

/// 一条静态种子：三要素都是 `'static`，可以安全地放进 `const` 表。
///
/// 用独立的 `SeedEntry` 类型（而不是裸元组）是为了让上百条数据仍然可读：
/// 字段名直接说明了每个字符串的含义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeedEntry {
    /// 英文术语
    pub source: &'static str,
    /// 中文译名
    pub target: &'static str,
    /// 分类，仅用于兼容旧导入数据
    pub category: &'static str,
}

impl SeedEntry {
    /// const 构造器，让数据表可以写成紧凑的 `SeedEntry::new(...)`。
    pub const fn new(source: &'static str, target: &'static str, category: &'static str) -> Self {
        Self {
            source,
            target,
            category,
        }
    }

    /// 转成运行时的术语条目：种子一律「官方 / 启用 / 非歧义 / 整词 / 不区分大小写」。
    pub fn to_entry(&self) -> GlossaryEntry {
        GlossaryEntry {
            source: self.source.to_string(),
            target: self.target.to_string(),
            category: self.category.to_string(),
            source_kind: "official".to_string(),
            enabled: true,
            ambiguous: false,
            whole_word: true,
            case_sensitive: false,
            count: 0,
        }
    }
}

/// 官方种子条目表；顺序即默认展示顺序。
pub const SEED_ENTRIES: &[SeedEntry] = &[
    // ── 职业 Class ──
    SeedEntry::new("Barbarian", "野蛮人", "class"),
    SeedEntry::new("Bard", "吟游诗人", "class"),
    SeedEntry::new("Cleric", "牧师", "class"),
    SeedEntry::new("Druid", "德鲁伊", "class"),
    SeedEntry::new("Fighter", "战士", "class"),
    SeedEntry::new("Monk", "武僧", "class"),
    SeedEntry::new("Paladin", "圣武士", "class"),
    SeedEntry::new("Ranger", "游侠", "class"),
    SeedEntry::new("Rogue", "游荡者", "class"),
    SeedEntry::new("Sorcerer", "术士", "class"),
    SeedEntry::new("Warlock", "邪术师", "class"),
    SeedEntry::new("Wizard", "法师", "class"),
    SeedEntry::new("Eldritch Knight", "奥法骑士", "class"),
    SeedEntry::new("Oathbreaker", "破誓者", "class"),
    SeedEntry::new("Oath of Devotion", "奉献之誓", "class"),
    SeedEntry::new("Oath of the Ancients", "远古之誓", "class"),
    SeedEntry::new("Oath of Vengeance", "复仇之誓", "class"),
    // ── 种族 Race ──
    SeedEntry::new("Half-Elf", "半精灵", "race"),
    SeedEntry::new("High Elf", "高等精灵", "race"),
    SeedEntry::new("Wood Elf", "木精灵", "race"),
    SeedEntry::new("Drow", "卓尔", "race"),
    SeedEntry::new("Duergar", "灰矮人", "race"),
    SeedEntry::new("Halfling", "半身人", "race"),
    SeedEntry::new("Githyanki", "吉斯洋基人", "race"),
    SeedEntry::new("Tiefling", "提夫林", "race"),
    SeedEntry::new("Dragonborn", "龙裔", "race"),
    SeedEntry::new("Half-Orc", "半兽人", "race"),
    // ── 地点 Location ──
    SeedEntry::new("Baldur's Gate", "博德之门", "location"),
    SeedEntry::new("Faerûn", "费伦", "location"),
    SeedEntry::new("Forgotten Realms", "被遗忘的国度", "location"),
    SeedEntry::new("Toril", "托瑞尔", "location"),
    SeedEntry::new("the Underdark", "幽暗地域", "location"),
    SeedEntry::new("Avernus", "阿佛纳斯", "location"),
    SeedEntry::new("Sword Coast", "剑湾", "location"),
    SeedEntry::new("Candlekeep", "烛堡", "location"),
    SeedEntry::new("Menzoberranzan", "魔索布莱城", "location"),
    SeedEntry::new("Nine Hells", "九层地狱", "location"),
    SeedEntry::new("Nautiloid", "地狱螺壳舰", "location"),
    SeedEntry::new("Emerald Grove", "翡翠林苑", "location"),
    // ── 角色 Character ──
    SeedEntry::new("Astarion", "阿斯代伦", "character"),
    SeedEntry::new("Shadowheart", "影心", "character"),
    SeedEntry::new("Gale", "盖尔", "character"),
    SeedEntry::new("Lae'zel", "莱埃泽尔", "character"),
    SeedEntry::new("Karlach", "卡尔拉赫", "character"),
    SeedEntry::new("Halsin", "哈尔辛", "character"),
    SeedEntry::new("Minthara", "明萨拉", "character"),
    SeedEntry::new("Withers", "威瑟斯", "character"),
    SeedEntry::new("The Emperor", "皇帝", "character"),
    SeedEntry::new("Vlaakith", "弗拉基丝", "character"),
    SeedEntry::new("Jaheira", "洁希拉", "character"),
    SeedEntry::new("Minsc", "敏斯克", "character"),
    SeedEntry::new("Raphael", "拉斐尔", "character"),
    SeedEntry::new("Mizora", "米佐拉", "character"),
    SeedEntry::new("Orin", "奥林", "character"),
    SeedEntry::new("Ketheric", "凯瑟里克", "character"),
    SeedEntry::new("Gortash", "戈塔什", "character"),
    SeedEntry::new("Mystra", "密斯特拉", "character"),
    SeedEntry::new("Dream Visitor", "梦境访客", "character"),
    SeedEntry::new("Voss", "维斯", "character"),
    SeedEntry::new("Novice of the Absolute", "至上真神学徒", "character"),
    // ── 生物 Creature ──
    SeedEntry::new("Mind Flayer", "夺心魔", "creature"),
    SeedEntry::new("Illithid", "夺心魔", "creature"),
    SeedEntry::new("Tadpole", "蝌蚪", "creature"),
    SeedEntry::new("Beholder", "眼魔", "creature"),
    SeedEntry::new("Lich", "巫妖", "creature"),
    SeedEntry::new("Vampire Spawn", "吸血鬼衍体", "creature"),
    SeedEntry::new("Lycanthrope", "兽化人", "creature"),
    SeedEntry::new("Hobgoblin", "大地精", "creature"),
    SeedEntry::new("Bugbear", "熊地精", "creature"),
    SeedEntry::new("Owlbear", "枭熊", "creature"),
    SeedEntry::new("Cambion", "坎比翁", "creature"),
    SeedEntry::new("Intellect Devourer", "噬脑怪", "creature"),
    SeedEntry::new("Elder Brain", "主脑", "creature"),
    // ── 机制 Mechanic ──
    SeedEntry::new("Cantrip", "戏法", "mechanic"),
    SeedEntry::new("Spell Slot", "法术位", "mechanic"),
    SeedEntry::new("Proficiency Bonus", "熟练度加值", "mechanic"),
    SeedEntry::new("Advantage", "优势", "mechanic"),
    SeedEntry::new("Disadvantage", "劣势", "mechanic"),
    SeedEntry::new("Inspiration", "激励", "mechanic"),
    SeedEntry::new("Bonus Action", "附赠动作", "mechanic"),
    SeedEntry::new("Saving Throw", "豁免检定", "mechanic"),
    SeedEntry::new("Ability Check", "属性检定", "mechanic"),
    SeedEntry::new("Armor Class", "护甲等级", "mechanic"),
    SeedEntry::new("Hit Points", "生命值", "mechanic"),
    SeedEntry::new("Long Rest", "长休", "mechanic"),
    SeedEntry::new("Short Rest", "短休", "mechanic"),
    SeedEntry::new("Concentration", "专注", "mechanic"),
    SeedEntry::new("Difficulty Class", "难度等级", "mechanic"),
    SeedEntry::new("Initiative", "先攻", "mechanic"),
    SeedEntry::new("Critical Hit", "重击", "mechanic"),
    SeedEntry::new("Sneak Attack", "偷袭", "mechanic"),
    SeedEntry::new("Divine Smite", "至圣斩", "mechanic"),
    SeedEntry::new("Wild Shape", "荒野形态", "mechanic"),
    SeedEntry::new("Necrotic", "黯蚀", "mechanic"),
    SeedEntry::new("Radiant", "光耀", "mechanic"),
    // ── 法术 Spell ──
    SeedEntry::new("Fireball", "火球术", "spell"),
    SeedEntry::new("Magic Missile", "魔法飞弹", "spell"),
    SeedEntry::new("Eldritch Blast", "魔能爆", "spell"),
    SeedEntry::new("Healing Word", "治愈真言", "spell"),
    SeedEntry::new("Misty Step", "迷踪步", "spell"),
    SeedEntry::new("Counterspell", "反制法术", "spell"),
    SeedEntry::new("Speak with Dead", "与亡者交谈", "spell"),
];

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn seed_table_is_well_formed() {
        assert!(
            SEED_ENTRIES.len() >= 70,
            "官方种子过少: {}",
            SEED_ENTRIES.len()
        );
        let mut seen = HashSet::new();
        for seed in SEED_ENTRIES {
            assert!(!seed.source.trim().is_empty());
            assert!(!seed.target.trim().is_empty());
            assert!(!seed.category.trim().is_empty());
            assert!(seen.insert(seed.source), "种子条目重复: {}", seed.source);
        }
    }

    #[test]
    fn seed_entries_convert_to_enabled_official_terms() {
        for seed in SEED_ENTRIES {
            let entry = seed.to_entry();
            assert_eq!(entry.source, seed.source);
            assert_eq!(entry.target, seed.target);
            assert_eq!(entry.category, seed.category);
            assert_eq!(entry.source_kind, "official");
            assert!(entry.enabled && !entry.ambiguous && entry.whole_word && !entry.case_sensitive);
            assert_eq!(entry.count, 0);
        }
    }
}
