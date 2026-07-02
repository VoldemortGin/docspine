//! 共享的 rPr / pPr **属性片段**解析器(PRD C-4/C-5 的可复用 prop-struct 解析器)。
//!
//! `w:rPr` / `w:pPr` 的语法在 `word/document.xml`(直接格式化)与 `word/styles.xml`
//! (docDefaults / 样式定义)里同构,所以片段解析只写一份:输出 doc-core 的
//! [`RunProps`] / [`ParaProps`](全 Option,能区分「未设置」与「显式关」)。
//!
//! walker 结构与 document.rs 的属性遍历同构:`Empty` 与 `Start` 形式统一按本地名处理,
//! `Start` 带子树的以深度计数兜底,不会错位、绝不 panic。

use doc_core::model::Color;
use doc_core::style::{
    ColorRef, FontRef, Justification, ParaProps, RunProps, ThemeColor, ThemeFont,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{attr_of, local_name, on_off_val};

/// 解析一个 `w:rPr` 片段。已消费 `<w:rPr>` 起始标签,消费到其匹配的结束标签为止。
pub fn parse_rpr<R: std::io::BufRead>(reader: &mut Reader<R>) -> RunProps {
    let mut props = RunProps::default();
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_rpr_prop(&e, &mut props),
            Ok(Event::Start(e)) => {
                apply_rpr_prop(&e, &mut props);
                depth += 1;
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // rPr 自身结束。
                }
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    props
}

/// 把一个 rPr 子元素(`Empty` 或 `Start`)的属性写进片段。
fn apply_rpr_prop(e: &BytesStart, p: &mut RunProps) {
    match local_name(e.name().as_ref()) {
        b"rFonts" => apply_rfonts(e, p),
        b"sz" => {
            // w:sz 以半磅存储;除以 2 得磅。
            if let Some(v) = attr_of(e, b"val").and_then(|s| s.parse::<f32>().ok()) {
                p.sz = Some(v / 2.0);
            }
        }
        b"b" => p.b = Some(on_off_val(e)),
        b"i" => p.i = Some(on_off_val(e)),
        b"caps" => p.caps = Some(on_off_val(e)),
        b"smallCaps" => p.small_caps = Some(on_off_val(e)),
        b"strike" => p.strike = Some(on_off_val(e)),
        b"vanish" => p.vanish = Some(on_off_val(e)),
        b"u" => {
            // 下划线:val 非 "none" 即为真;显式 "none" 是「显式关」(能盖掉样式链)。
            p.u = Some(
                attr_of(e, b"val")
                    .map(|v| !v.eq_ignore_ascii_case("none"))
                    .unwrap_or(true),
            );
        }
        b"color" => {
            // themeColor 间接引用优先于 val(ECMA-376 §17.3.2.6)。
            if let Some(tc) = attr_of(e, b"themeColor").and_then(|s| ThemeColor::from_attr(&s)) {
                p.color = Some(ColorRef::Theme(tc));
            } else if let Some(v) = attr_of(e, b"val") {
                p.color = Some(match Color::from_hex(&v) {
                    Some(c) => ColorRef::Rgb(c),
                    None => ColorRef::Auto, // "auto"(或非法值容错):自动色。
                });
            }
        }
        _ => {}
    }
}

/// `w:rFonts`:四槽显名 + 四槽 theme 间接引用;同槽位上 theme 属性优先于显名
/// (ECMA-376 §17.3.2.26)。空串按缺失处理。
fn apply_rfonts(e: &BytesStart, p: &mut RunProps) {
    let f = &mut p.fonts;
    for (key, slot) in [
        (&b"ascii"[..], &mut f.ascii),
        (&b"hAnsi"[..], &mut f.h_ansi),
        (&b"eastAsia"[..], &mut f.east_asia),
        (&b"cs"[..], &mut f.cs),
    ] {
        if let Some(name) = attr_of(e, key) {
            if !name.is_empty() {
                *slot = Some(FontRef::Named(name));
            }
        }
    }
    let f = &mut p.fonts;
    for (key, slot) in [
        (&b"asciiTheme"[..], &mut f.ascii),
        (&b"hAnsiTheme"[..], &mut f.h_ansi),
        (&b"eastAsiaTheme"[..], &mut f.east_asia),
        (&b"cstheme"[..], &mut f.cs),
    ] {
        if let Some(t) = attr_of(e, key).and_then(|s| ThemeFont::from_attr(&s)) {
            *slot = Some(FontRef::Theme(t));
        }
    }
}

/// 解析一个 `w:pPr` 片段(styles.xml 的 docDefaults / 样式定义用;document.xml 的直接
/// pPr 因还要处理 pStyle/sectPr/numPr 走 document.rs 的专用 walker)。
/// 本轮语法:`w:jc`;C-4 在此扩展 spacing / ind / pBdr / shd / keep 系列。
/// 已消费 `<w:pPr>` 起始标签。
pub fn parse_ppr<R: std::io::BufRead>(reader: &mut Reader<R>) -> ParaProps {
    let mut props = ParaProps::default();
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_ppr_prop(&e, &mut props),
            Ok(Event::Start(e)) => {
                apply_ppr_prop(&e, &mut props);
                depth += 1;
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // pPr 自身结束。
                }
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    props
}

/// 把一个 pPr 子元素的属性写进片段。
fn apply_ppr_prop(e: &BytesStart, p: &mut ParaProps) {
    if local_name(e.name().as_ref()) == b"jc" {
        if let Some(j) = attr_of(e, b"val").and_then(|s| Justification::from_attr(&s)) {
            p.jc = Some(j);
        }
    }
}
