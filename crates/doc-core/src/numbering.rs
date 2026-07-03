//! numbering.xml 列表模型 + 计数引擎(PDF-EXPORT C-6)。
//!
//! WordprocessingML 的列表分两层间接(ECMA-376 §17.9):段落 `w:numPr` 携带
//! `numId + ilvl`;`word/numbering.xml` 里 `w:num`(numId → abstractNumId + 层级
//! 覆盖)指向 `w:abstractNum`(每层 `w:lvl`:起值 / 编号格式 / `%1.%2` 标签模板 /
//! 层级缩进 pPr)。这里放两样东西:
//!
//! - **纯数据表** [`NumberingTable`](doc-parse 机械填充,挂在 `Document` 上);
//! - **计数引擎** [`ListCounters`]:按文档顺序逐段推进的 per-numId per-level 状态机,
//!   产出**最终标签串**(`%N` 模板展开 + numFmt 格式化 + Symbol/Wingdings PUA 圆点
//!   归一)。消费者(doc-render)把标签喂给排版引擎的 `ListLabel`,引擎不懂 numbering。
//!
//! 层级 pPr(缩进)的级联位置:样式层之下、直接格式化之上,由
//! [`crate::style::resolve_para`] 合并(本模块只存片段)。restart 语义:计数按
//! numId 隔离 —— 两个不同 numId(即便共享同一 abstractNum)各自从头计数;推进第
//! `L` 层会重置更深层级的计数(Word 的多级列表语义);`w:lvlOverride >
//! w:startOverride` 改写该 numId 在该层的起值。`styleLink`/`numStyleLink` 间接
//! v1 不解(PRD 声明降级,渲染侧告警)。

use std::collections::BTreeMap;

use crate::style::{Justification, ParaProps};

/// 编号格式(`w:numFmt@w:val` 的常用子集;未知值按 decimal 容错)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NumFmt {
    /// 阿拉伯数字(缺省)。
    #[default]
    Decimal,
    /// 小写字母(a, b, …, z, aa, bb, …——Word 超过 26 重复同字母)。
    LowerLetter,
    /// 大写字母。
    UpperLetter,
    /// 小写罗马数字。
    LowerRoman,
    /// 大写罗马数字。
    UpperRoman,
    /// 项目符号(标签取 lvlText 字面,不计数展示)。
    Bullet,
    /// 无编号(`val="none"`:该层不显示编号)。
    None,
}

impl NumFmt {
    /// 解析 `w:numFmt@w:val`。未知取值容错为 [`NumFmt::Decimal`](编号照出,格式近似)。
    pub fn from_attr(s: &str) -> Self {
        match s {
            "decimal" => NumFmt::Decimal,
            "lowerLetter" => NumFmt::LowerLetter,
            "upperLetter" => NumFmt::UpperLetter,
            "lowerRoman" => NumFmt::LowerRoman,
            "upperRoman" => NumFmt::UpperRoman,
            "bullet" => NumFmt::Bullet,
            "none" => NumFmt::None,
            _ => NumFmt::Decimal,
        }
    }
}

/// 一个列表层级定义(`w:lvl`)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NumLevel {
    /// 起值(`w:start@w:val`;缺省 1)。
    pub start: Option<i64>,
    /// 编号格式(`w:numFmt@w:val`)。
    pub fmt: NumFmt,
    /// 标签模板(`w:lvlText@w:val`,如 `"%1."` / `"%1.%2"` / 圆点字面)。
    pub lvl_text: Option<String>,
    /// 编号对齐(`w:lvlJc@w:val`;v1 仅刻画,渲染侧标签一律右对齐到正文起点)。
    pub jc: Option<Justification>,
    /// 层级段落属性(`w:lvl > w:pPr`,主要是 `w:ind` 缩进)。级联位置:样式层之下、
    /// 直接格式化之上(由 [`crate::style::resolve_para`] 合并)。
    pub ppr: ParaProps,
}

/// 一条抽象编号定义(`w:abstractNum`):层级表 + styleLink 间接(v1 不解)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AbstractNum {
    /// 层级定义(键 = `w:lvl@w:ilvl`)。
    pub levels: BTreeMap<u32, NumLevel>,
    /// `w:styleLink@w:val`(本定义作为编号样式暴露;仅刻画)。
    pub style_link: Option<String>,
    /// `w:numStyleLink@w:val`(层级转从编号样式取;v1 不解 → 渲染侧告警)。
    pub num_style_link: Option<String>,
}

/// 一层的覆盖(`w:num > w:lvlOverride`)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LevelOverride {
    /// 起值覆盖(`w:startOverride@w:val`,restart 语义的载体)。
    pub start_override: Option<i64>,
    /// 整层替换(`w:lvlOverride > w:lvl`)。
    pub level: Option<NumLevel>,
}

/// 一条具体编号实例(`w:num`):指向 abstractNum + 层级覆盖。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Num {
    /// `w:abstractNumId@w:val`。
    pub abstract_id: i64,
    /// 层级覆盖(键 = `w:lvlOverride@w:ilvl`)。
    pub overrides: BTreeMap<u32, LevelOverride>,
}

/// 编号表(`word/numbering.xml`)。部件缺失时为空表(所有查询 → `None`,段落按
/// 普通段渲染)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NumberingTable {
    /// 抽象定义(键 = `w:abstractNum@w:abstractNumId`)。
    pub abstracts: BTreeMap<i64, AbstractNum>,
    /// 具体实例(键 = `w:num@w:numId`)。
    pub nums: BTreeMap<u32, Num>,
}

impl NumberingTable {
    /// 是否空表(部件缺失/无定义)。
    pub fn is_empty(&self) -> bool {
        self.abstracts.is_empty() && self.nums.is_empty()
    }

    /// 查一层的有效定义:`lvlOverride > w:lvl` 整层替换优先于 abstractNum 的该层。
    pub fn level(&self, num_id: u32, ilvl: u32) -> Option<&NumLevel> {
        let num = self.nums.get(&num_id)?;
        if let Some(o) = num.overrides.get(&ilvl) {
            if let Some(level) = &o.level {
                return Some(level);
            }
        }
        self.abstracts.get(&num.abstract_id)?.levels.get(&ilvl)
    }

    /// 一层的有效起值:`startOverride` > `w:start` > 1。
    pub fn start(&self, num_id: u32, ilvl: u32) -> i64 {
        if let Some(num) = self.nums.get(&num_id) {
            if let Some(o) = num.overrides.get(&ilvl) {
                if let Some(s) = o.start_override {
                    return s;
                }
            }
        }
        self.level(num_id, ilvl).and_then(|l| l.start).unwrap_or(1)
    }

    /// 该 numId 是否落在**未解的** `numStyleLink` 间接上(abstractNum 只有间接、
    /// 无自有层级)。渲染侧据此浮 v1 降级告警。
    pub fn uses_num_style_link(&self, num_id: u32) -> bool {
        self.nums
            .get(&num_id)
            .and_then(|n| self.abstracts.get(&n.abstract_id))
            .is_some_and(|a| a.num_style_link.is_some() && a.levels.is_empty())
    }
}

/// per-numId per-level 计数状态机。按文档顺序对每个带 `numPr` 的段落调用
/// [`ListCounters::advance`],产出该段的最终标签串。
#[derive(Debug, Clone, Default)]
pub struct ListCounters {
    /// numId → (层级 → 当前计数)。层级缺席 = 尚未开始(下次从起值起)。
    counts: BTreeMap<u32, BTreeMap<u32, i64>>,
}

impl ListCounters {
    /// 新的空计数器(一份文档一实例)。
    pub fn new() -> Self {
        ListCounters::default()
    }

    /// 推进 `(num_id, ilvl)` 的计数并产出最终标签串。
    ///
    /// - `num_id == 0`(Word 语义:显式去除编号)或层级无定义 → `None`(普通段);
    /// - 推进第 `ilvl` 层会重置该 numId 更深层级的计数(多级列表语义);
    /// - `%N` 模板按各层当前计数展开(未开始的层用起值),各层按自身 numFmt 格式化;
    /// - `numFmt="none"` 或空标签 → `None`(缩进仍经层级 pPr 级联生效)。
    pub fn advance(&mut self, table: &NumberingTable, num_id: u32, ilvl: u32) -> Option<String> {
        if num_id == 0 {
            return None;
        }
        let level = table.level(num_id, ilvl)?;
        let per = self.counts.entry(num_id).or_default();
        let n = match per.get(&ilvl) {
            Some(v) => v + 1,
            None => table.start(num_id, ilvl),
        };
        per.insert(ilvl, n);
        // 更深层级重置:下次出现从各自起值重新计。
        per.retain(|l, _| *l <= ilvl);

        let label = match &level.lvl_text {
            Some(template) => expand_lvl_text(template, table, num_id, per),
            None => match level.fmt {
                NumFmt::Bullet => DEFAULT_BULLET.to_string(),
                NumFmt::None => String::new(),
                fmt => format!("{}.", format_number(fmt, n)),
            },
        };
        if label.is_empty() {
            None
        } else {
            Some(label)
        }
    }
}

/// 缺省项目符号(lvlText 缺失的 bullet 层)。
const DEFAULT_BULLET: &str = "\u{2022}";

/// 展开 `w:lvlText` 模板:`%1`..`%9` → 对应层级(1 基)的当前计数按该层 numFmt
/// 格式化;`%%` → `%`;其余字符经 Symbol/Wingdings PUA 归一后原样保留。
fn expand_lvl_text(
    template: &str,
    table: &NumberingTable,
    num_id: u32,
    counts: &BTreeMap<u32, i64>,
) -> String {
    let mut out = String::new();
    let mut chars = template.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '%' {
            out.push(map_symbol_char(ch));
            continue;
        }
        match chars.peek() {
            Some(d @ '1'..='9') => {
                let lvl = d.to_digit(10).unwrap_or(1) - 1;
                chars.next();
                let n = counts
                    .get(&lvl)
                    .copied()
                    .unwrap_or_else(|| table.start(num_id, lvl));
                let fmt = table.level(num_id, lvl).map(|l| l.fmt).unwrap_or_default();
                out.push_str(&format_number(fmt, n));
            }
            Some('%') => {
                chars.next();
                out.push('%');
            }
            _ => out.push('%'),
        }
    }
    out
}

/// 把 Symbol/Wingdings 私用区(U+F000..U+F0FF)的常见圆点字符归一成 Unicode
/// 等价物(Word 的 bullet lvlText 常存 PUA 码点,直接渲染必然缺字形)。
/// 其余 PUA 字符兜底成缺省圆点;非 PUA 字符原样返回。
fn map_symbol_char(ch: char) -> char {
    let cp = ch as u32;
    if !(0xF000..=0xF0FF).contains(&cp) {
        return ch;
    }
    match cp & 0xFF {
        0xB7 => '\u{2022}', // Symbol/Wingdings 实心圆点 → •
        0xA7 => '\u{25AA}', // Wingdings 实心小方块 → ▪
        0x6F => 'o',        // Courier New 空心圆点(Word 二级 bullet 惯用)
        0xD8 => '\u{27A2}', // Wingdings 立体箭头 → ➢
        0xFC => '\u{2713}', // Wingdings 对勾 → ✓
        0x76 => '\u{2756}', // Wingdings 菱形花 → ❖
        _ => '\u{2022}',
    }
}

/// 按编号格式把计数格式化成字面(`Bullet`/`None` 不产数字:分别给缺省圆点与空串)。
pub fn format_number(fmt: NumFmt, n: i64) -> String {
    match fmt {
        NumFmt::Decimal => n.to_string(),
        NumFmt::LowerLetter => letter(n, false),
        NumFmt::UpperLetter => letter(n, true),
        NumFmt::LowerRoman => roman(n, false),
        NumFmt::UpperRoman => roman(n, true),
        NumFmt::Bullet => DEFAULT_BULLET.to_string(),
        NumFmt::None => String::new(),
    }
}

/// 字母编号(Word 语义:1..26 = a..z;超过 26 **重复同字母**,27 = aa、28 = bb)。
/// 非正数无字母形,按十进制兜底。
fn letter(n: i64, upper: bool) -> String {
    if n < 1 {
        return n.to_string();
    }
    let idx = ((n - 1) % 26) as u8;
    let reps = ((n - 1) / 26 + 1) as usize;
    let ch = (if upper { b'A' } else { b'a' } + idx) as char;
    ch.to_string().repeat(reps)
}

/// 罗马数字(标准减法记法)。非正数无罗马形,按十进制兜底。
fn roman(n: i64, upper: bool) -> String {
    if n < 1 {
        return n.to_string();
    }
    const PAIRS: [(i64, &str); 13] = [
        (1000, "m"),
        (900, "cm"),
        (500, "d"),
        (400, "cd"),
        (100, "c"),
        (90, "xc"),
        (50, "l"),
        (40, "xl"),
        (10, "x"),
        (9, "ix"),
        (5, "v"),
        (4, "iv"),
        (1, "i"),
    ];
    let mut n = n;
    let mut out = String::new();
    for (value, sym) in PAIRS {
        while n >= value {
            out.push_str(sym);
            n -= value;
        }
    }
    if upper {
        out.to_uppercase()
    } else {
        out
    }
}

// ============================================================ 单测:计数与格式化语义

#[cfg(test)]
mod tests {
    use super::*;

    /// 便利:一张单 abstractNum 的编号表(numId 1..=ids 都指向它)。
    fn table_with_levels(levels: Vec<(u32, NumLevel)>, num_ids: &[u32]) -> NumberingTable {
        let mut t = NumberingTable::default();
        t.abstracts.insert(
            0,
            AbstractNum {
                levels: levels.into_iter().collect(),
                ..AbstractNum::default()
            },
        );
        for &id in num_ids {
            t.nums.insert(
                id,
                Num {
                    abstract_id: 0,
                    ..Num::default()
                },
            );
        }
        t
    }

    fn lvl(fmt: NumFmt, text: &str) -> NumLevel {
        NumLevel {
            fmt,
            lvl_text: Some(text.to_string()),
            ..NumLevel::default()
        }
    }

    /// 两级嵌套:1. → a. b. → 2.(推进上层重置下层)。
    #[test]
    fn two_level_counting_and_reset() {
        let t = table_with_levels(
            vec![
                (0, lvl(NumFmt::Decimal, "%1.")),
                (1, lvl(NumFmt::LowerLetter, "%2.")),
            ],
            &[1],
        );
        let mut c = ListCounters::new();
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("1."));
        assert_eq!(c.advance(&t, 1, 1).as_deref(), Some("a."));
        assert_eq!(c.advance(&t, 1, 1).as_deref(), Some("b."));
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("2."));
        // 上层推进后,下层重新从 a 起。
        assert_eq!(c.advance(&t, 1, 1).as_deref(), Some("a."));
    }

    /// 两个独立 numId(共享同一 abstractNum)各自计数。
    #[test]
    fn independent_num_ids_count_separately() {
        let t = table_with_levels(vec![(0, lvl(NumFmt::Decimal, "%1."))], &[1, 2]);
        let mut c = ListCounters::new();
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("1."));
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("2."));
        assert_eq!(
            c.advance(&t, 2, 0).as_deref(),
            Some("1."),
            "新 numId 重新计数"
        );
    }

    /// `%1.%2` 模板:上层计数嵌入下层标签;上层未推进也能用起值。
    #[test]
    fn multi_level_template_expansion() {
        let t = table_with_levels(
            vec![
                (0, lvl(NumFmt::Decimal, "%1.")),
                (1, lvl(NumFmt::Decimal, "%1.%2")),
            ],
            &[1],
        );
        let mut c = ListCounters::new();
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("1."));
        assert_eq!(c.advance(&t, 1, 1).as_deref(), Some("1.1"));
        assert_eq!(c.advance(&t, 1, 1).as_deref(), Some("1.2"));
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("2."));
        assert_eq!(
            c.advance(&t, 1, 1).as_deref(),
            Some("2.1"),
            "上层推进后下层重置"
        );
    }

    /// startOverride:该 numId 在该层的起值被改写(restart 载体)。
    #[test]
    fn start_override_rewrites_first_value() {
        let mut t = table_with_levels(vec![(0, lvl(NumFmt::Decimal, "%1."))], &[1]);
        t.nums.get_mut(&1).expect("num 1").overrides.insert(
            0,
            LevelOverride {
                start_override: Some(5),
                level: None,
            },
        );
        let mut c = ListCounters::new();
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("5."));
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("6."));
    }

    /// lvlOverride 整层替换优先于 abstractNum 的该层。
    #[test]
    fn lvl_override_replaces_level_definition() {
        let mut t = table_with_levels(vec![(0, lvl(NumFmt::Decimal, "%1."))], &[1]);
        t.nums.get_mut(&1).expect("num 1").overrides.insert(
            0,
            LevelOverride {
                start_override: None,
                level: Some(lvl(NumFmt::UpperRoman, "[%1]")),
            },
        );
        let mut c = ListCounters::new();
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("[I]"));
    }

    /// bullet 层:lvlText 的 Symbol PUA 圆点归一成 Unicode;计数照常推进不外显。
    #[test]
    fn bullet_pua_normalized() {
        let t = table_with_levels(vec![(0, lvl(NumFmt::Bullet, "\u{F0B7}"))], &[1]);
        let mut c = ListCounters::new();
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("\u{2022}"));
        assert_eq!(c.advance(&t, 1, 0).as_deref(), Some("\u{2022}"));
    }

    /// numFmt="none" / numId=0 / 未定义层级:都不产标签。
    #[test]
    fn no_label_cases() {
        let t = table_with_levels(vec![(0, lvl(NumFmt::None, "%1."))], &[1]);
        let mut c = ListCounters::new();
        // "none" 层:%1 展开为空,标签 "." 非空 —— 但纯 none 无模板时给 None。
        let t2 = table_with_levels(
            vec![(
                0,
                NumLevel {
                    fmt: NumFmt::None,
                    ..NumLevel::default()
                },
            )],
            &[1],
        );
        assert_eq!(c.advance(&t2, 1, 0), None);
        assert_eq!(c.advance(&t, 1, 99), None, "未定义层级");
        assert_eq!(c.advance(&t, 0, 0), None, "numId=0 显式去编号");
        assert_eq!(c.advance(&t, 7, 0), None, "未登记的 numId");
    }

    /// 编号格式:字母超过 26 重复同字母;罗马减法记法;非正数十进制兜底。
    #[test]
    fn number_formatting() {
        assert_eq!(format_number(NumFmt::Decimal, 12), "12");
        assert_eq!(format_number(NumFmt::LowerLetter, 1), "a");
        assert_eq!(format_number(NumFmt::LowerLetter, 26), "z");
        assert_eq!(format_number(NumFmt::LowerLetter, 27), "aa");
        assert_eq!(format_number(NumFmt::UpperLetter, 28), "BB");
        assert_eq!(format_number(NumFmt::LowerRoman, 4), "iv");
        assert_eq!(format_number(NumFmt::LowerRoman, 1994), "mcmxciv");
        assert_eq!(format_number(NumFmt::UpperRoman, 9), "IX");
        assert_eq!(format_number(NumFmt::LowerRoman, 0), "0");
        assert_eq!(format_number(NumFmt::UpperLetter, -3), "-3");
    }

    /// `%%` 转义与越界 `%` 容错;未开始的祖先层用起值。
    #[test]
    fn template_edge_cases() {
        let mut lv0 = lvl(NumFmt::Decimal, "%1.");
        lv0.start = Some(3);
        let t = table_with_levels(
            vec![(0, lv0), (1, lvl(NumFmt::Decimal, "100%% -> %1-%2%"))],
            &[1],
        );
        let mut c = ListCounters::new();
        // 直接从第 1 层开始:第 0 层未推进,%1 用其起值 3。
        assert_eq!(c.advance(&t, 1, 1).as_deref(), Some("100% -> 3-1%"));
    }

    /// numStyleLink 间接(无自有层级)可被探知(渲染侧降级告警的依据)。
    #[test]
    fn num_style_link_detection() {
        let mut t = NumberingTable::default();
        t.abstracts.insert(
            0,
            AbstractNum {
                num_style_link: Some("ListStyle".into()),
                ..AbstractNum::default()
            },
        );
        t.nums.insert(
            1,
            Num {
                abstract_id: 0,
                ..Num::default()
            },
        );
        assert!(t.uses_num_style_link(1));
        assert_eq!(ListCounters::new().advance(&t, 1, 0), None);
    }
}
