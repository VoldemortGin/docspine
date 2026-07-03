//! 解析 `word/numbering.xml`(编号部件)-> [`NumberingTable`](PDF-EXPORT C-6)。
//!
//! 结构(ECMA-376 §17.9):
//!
//! ```xml
//! <w:numbering>
//!   <w:abstractNum w:abstractNumId="0">
//!     <w:styleLink w:val="…"/> <w:numStyleLink w:val="…"/>
//!     <w:lvl w:ilvl="0">
//!       <w:start w:val="1"/> <w:numFmt w:val="decimal"/> <w:lvlText w:val="%1."/>
//!       <w:lvlJc w:val="left"/> <w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr>
//!     </w:lvl> …
//!   </w:abstractNum>
//!   <w:num w:numId="1">
//!     <w:abstractNumId w:val="0"/>
//!     <w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/><w:lvl>…</w:lvl></w:lvlOverride>
//!   </w:num>
//! </w:numbering>
//! ```
//!
//! 解析是**机械搬运**:层级定义 / 实例映射 / 覆盖装进纯数据表,计数与标签展开在
//! doc-core 的 [`ListCounters`](doc_core::numbering::ListCounters)(渲染侧推进)。
//! `w:lvl > w:pPr` 经共享的 [`props::parse_ppr`] 解析(主要是 `w:ind` 缩进;
//! 级联位置——样式层之下、直格之上——由 doc-core 的 `resolve_para` 决定)。
//! 容错:未知元素跳过、缺失属性 → 缺省、绝不 panic。

use doc_core::numbering::{AbstractNum, LevelOverride, Num, NumFmt, NumLevel, NumberingTable};
use doc_core::style::Justification;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{attr_of, local_name, props, skip_element};

/// 解析 `word/numbering.xml` 文本。部件整体畸形时返回已得部分(最坏空表)。
pub fn parse(xml: &str) -> NumberingTable {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    // 先定位到 w:numbering 根,再解析其直接子元素。
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"numbering" {
                    return parse_numbering_children(&mut reader);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    NumberingTable::default()
}

/// 解析 `w:numbering` 的直接子元素。假定 reader 已消费 `<w:numbering>` 起始标签。
fn parse_numbering_children<R: std::io::BufRead>(reader: &mut Reader<R>) -> NumberingTable {
    let mut table = NumberingTable::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"abstractNum" => {
                    let id = attr_of(&e, b"abstractNumId").and_then(|s| s.parse().ok());
                    let abs = parse_abstract_num(reader);
                    if let Some(id) = id {
                        table.abstracts.insert(id, abs);
                    }
                }
                b"num" => {
                    let id = attr_of(&e, b"numId").and_then(|s| s.parse().ok());
                    let num = parse_num(reader);
                    if let Some(id) = id {
                        table.nums.insert(id, num);
                    }
                }
                // 其余(w:numPicBullet 等)整体跳过。
                _ => skip_element(reader),
            },
            Ok(Event::End(_)) => break, // numbering 根结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    table
}

/// 解析一条 `w:abstractNum`(层级表 + styleLink/numStyleLink)。已消费其起始标签。
fn parse_abstract_num<R: std::io::BufRead>(reader: &mut Reader<R>) -> AbstractNum {
    let mut abs = AbstractNum::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_abstract_child(&e, &mut abs),
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"lvl" => {
                    let ilvl = attr_of(&e, b"ilvl").and_then(|s| s.parse().ok());
                    let level = parse_lvl(reader);
                    if let Some(ilvl) = ilvl {
                        abs.levels.insert(ilvl, level);
                    }
                }
                _ => {
                    apply_abstract_child(&e, &mut abs);
                    skip_element(reader);
                }
            },
            Ok(Event::End(_)) => break, // abstractNum 自身结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    abs
}

/// `w:abstractNum` 的元数据子元素:`w:styleLink` / `w:numStyleLink`(其余忽略)。
fn apply_abstract_child(e: &BytesStart, abs: &mut AbstractNum) {
    match local_name(e.name().as_ref()) {
        b"styleLink" => abs.style_link = attr_of(e, b"val").or(abs.style_link.take()),
        b"numStyleLink" => abs.num_style_link = attr_of(e, b"val").or(abs.num_style_link.take()),
        _ => {}
    }
}

/// 解析一个 `w:lvl`(层级定义):start / numFmt / lvlText / lvlJc + 层级 pPr。
/// 已消费其起始标签。`w:rPr`(编号符 run 属性,v1 不渲染)整体跳过。
fn parse_lvl<R: std::io::BufRead>(reader: &mut Reader<R>) -> NumLevel {
    let mut level = NumLevel::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_lvl_prop(&e, &mut level),
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"pPr" => level.ppr = props::parse_ppr(reader),
                b"rPr" => skip_element(reader),
                _ => {
                    apply_lvl_prop(&e, &mut level);
                    skip_element(reader);
                }
            },
            Ok(Event::End(_)) => break, // lvl 自身结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    level
}

/// 把一个 `w:lvl` 子元素的属性写进层级定义。
fn apply_lvl_prop(e: &BytesStart, level: &mut NumLevel) {
    match local_name(e.name().as_ref()) {
        b"start" => level.start = attr_of(e, b"val").and_then(|s| s.parse().ok()),
        b"numFmt" => {
            if let Some(v) = attr_of(e, b"val") {
                level.fmt = NumFmt::from_attr(&v);
            }
        }
        b"lvlText" => level.lvl_text = attr_of(e, b"val").or(level.lvl_text.take()),
        b"lvlJc" => {
            if let Some(j) = attr_of(e, b"val").and_then(|s| Justification::from_attr(&s)) {
                level.jc = Some(j);
            }
        }
        _ => {}
    }
}

/// 解析一条 `w:num`(实例):abstractNumId 指向 + 层级覆盖。已消费其起始标签。
fn parse_num<R: std::io::BufRead>(reader: &mut Reader<R>) -> Num {
    let mut num = Num::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"abstractNumId" {
                    if let Some(v) = attr_of(&e, b"val").and_then(|s| s.parse().ok()) {
                        num.abstract_id = v;
                    }
                }
            }
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"abstractNumId" => {
                    if let Some(v) = attr_of(&e, b"val").and_then(|s| s.parse().ok()) {
                        num.abstract_id = v;
                    }
                    skip_element(reader);
                }
                b"lvlOverride" => {
                    let ilvl = attr_of(&e, b"ilvl").and_then(|s| s.parse().ok());
                    let over = parse_lvl_override(reader);
                    if let Some(ilvl) = ilvl {
                        num.overrides.insert(ilvl, over);
                    }
                }
                _ => skip_element(reader),
            },
            Ok(Event::End(_)) => break, // num 自身结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    num
}

/// 解析一个 `w:lvlOverride`:`w:startOverride`(restart 载体)+ 可选的整层替换 `w:lvl`。
/// 已消费其起始标签。
fn parse_lvl_override<R: std::io::BufRead>(reader: &mut Reader<R>) -> LevelOverride {
    let mut over = LevelOverride::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"startOverride" {
                    over.start_override = attr_of(&e, b"val").and_then(|s| s.parse().ok());
                }
            }
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"lvl" => over.level = Some(parse_lvl(reader)),
                b"startOverride" => {
                    over.start_override = attr_of(&e, b"val").and_then(|s| s.parse().ok());
                    skip_element(reader);
                }
                _ => skip_element(reader),
            },
            Ok(Event::End(_)) => break, // lvlOverride 自身结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    over
}

// ============================================================ 单测:walker 的机械搬运语义

#[cfg(test)]
mod tests {
    use super::*;

    const NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0">
    <w:lvl w:ilvl="0">
      <w:start w:val="1"/>
      <w:numFmt w:val="decimal"/>
      <w:lvlText w:val="%1."/>
      <w:lvlJc w:val="left"/>
      <w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr>
      <w:rPr><w:rFonts w:ascii="Symbol"/></w:rPr>
    </w:lvl>
    <w:lvl w:ilvl="1">
      <w:start w:val="1"/>
      <w:numFmt w:val="lowerLetter"/>
      <w:lvlText w:val="%2."/>
      <w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr>
    </w:lvl>
  </w:abstractNum>
  <w:abstractNum w:abstractNumId="1">
    <w:numStyleLink w:val="ListStyle"/>
  </w:abstractNum>
  <w:num w:numId="1">
    <w:abstractNumId w:val="0"/>
  </w:num>
  <w:num w:numId="2">
    <w:abstractNumId w:val="0"/>
    <w:lvlOverride w:ilvl="0">
      <w:startOverride w:val="5"/>
    </w:lvlOverride>
  </w:num>
  <w:num w:numId="3">
    <w:abstractNumId w:val="1"/>
  </w:num>
</w:numbering>"#;

    /// abstractNum 层级(start/numFmt/lvlText/lvlJc/pPr-ind)与 num 映射照搬进表。
    #[test]
    fn parses_levels_and_num_mapping() {
        let t = parse(NUMBERING);
        let l0 = t.level(1, 0).expect("numId 1 / ilvl 0");
        assert_eq!(l0.start, Some(1));
        assert_eq!(l0.fmt, NumFmt::Decimal);
        assert_eq!(l0.lvl_text.as_deref(), Some("%1."));
        assert_eq!(l0.jc, Some(Justification::Left));
        assert_eq!(l0.ppr.ind_left, Some(720));
        assert_eq!(l0.ppr.ind_hanging, Some(360));
        let l1 = t.level(1, 1).expect("numId 1 / ilvl 1");
        assert_eq!(l1.fmt, NumFmt::LowerLetter);
        assert_eq!(l1.ppr.ind_left, Some(1440));
        assert_eq!(t.level(1, 9), None, "未定义层级");
        assert_eq!(t.level(9, 0), None, "未登记 numId");
    }

    /// lvlOverride > startOverride:numId 2 与 numId 1 共享 abstractNum 但改写起值。
    #[test]
    fn start_override_lands_on_num_instance() {
        let t = parse(NUMBERING);
        assert_eq!(t.start(1, 0), 1);
        assert_eq!(t.start(2, 0), 5);
    }

    /// numStyleLink 间接(v1 不解)被完整刻画,可被渲染侧探知。
    #[test]
    fn num_style_link_captured() {
        let t = parse(NUMBERING);
        assert!(t.uses_num_style_link(3));
        assert!(!t.uses_num_style_link(1));
    }

    /// 畸形/空输入:容错为空表。
    #[test]
    fn malformed_input_yields_empty_table() {
        assert!(parse("").is_empty());
        assert!(parse("<w:numbering>").is_empty());
        assert!(parse("not xml at all").is_empty());
    }
}
