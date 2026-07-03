//! 共享的 rPr / pPr **属性片段**解析器(PRD C-4/C-5 的可复用 prop-struct 解析器)。
//!
//! `w:rPr` / `w:pPr` 的语法在 `word/document.xml`(直接格式化)与 `word/styles.xml`
//! (docDefaults / 样式定义)里同构,所以片段解析只写一份:输出 doc-core 的
//! [`RunProps`] / [`ParaProps`](全 Option,能区分「未设置」与「显式关」)。
//!
//! walker 结构与 document.rs 的属性遍历同构:`Empty` 与 `Start` 形式统一按本地名处理,
//! `Start` 带子树的以深度计数兜底,不会错位、绝不 panic。两个例外子树:
//! `w:pBdr`(段落边框,子元素 `w:top` 等会与别的属性名撞车)走专用子 walker;
//! pPr 内嵌的 `w:rPr`(段落标记符的 run 属性,内含 `w:spacing` 字符间距等**同名异义**
//! 元素)整体跳过,防止污染段落属性。

use doc_core::model::Color;
use doc_core::style::{
    Border, CellBorders, CellMargins, ColorRef, FontRef, Highlight, Justification, LineSpacingRule,
    ParaProps, RunProps, TableBorders, TableProps, ThemeColor, ThemeFont, UnderlineKind, VertAlign,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{attr_of, local_name, on_off_val, skip_element};

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
        b"szCs" => {
            if let Some(v) = attr_of(e, b"val").and_then(|s| s.parse::<f32>().ok()) {
                p.sz_cs = Some(v / 2.0);
            }
        }
        b"b" => p.b = Some(on_off_val(e)),
        b"i" => p.i = Some(on_off_val(e)),
        b"caps" => p.caps = Some(on_off_val(e)),
        b"smallCaps" => p.small_caps = Some(on_off_val(e)),
        b"strike" => p.strike = Some(on_off_val(e)),
        b"vanish" => p.vanish = Some(on_off_val(e)),
        b"u" => {
            // 下划线:val 缺省按单线;"none" 是「显式关」(能盖掉样式链);
            // 未知非 none 值容错为单线(种类近似,开关保真)。
            p.u = Some(
                attr_of(e, b"val")
                    .map(|v| UnderlineKind::from_attr(&v))
                    .unwrap_or(UnderlineKind::Single),
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
        b"highlight" => {
            // 具名高亮色;"none" 显式关;未知名容错为未设置。
            if let Some(h) = attr_of(e, b"val").and_then(|s| Highlight::from_attr(&s)) {
                p.highlight = Some(h);
            }
        }
        b"vertAlign" => {
            if let Some(v) = attr_of(e, b"val").and_then(|s| VertAlign::from_attr(&s)) {
                p.vert_align = Some(v);
            }
        }
        b"rStyle" => {
            p.r_style = attr_of(e, b"val").or(p.r_style.take());
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
/// pPr 因还要处理 pStyle/sectPr/numPr 走 document.rs 的专用 walker,但其每个子元素
/// 也会经 [`apply_ppr_prop`] 写进共享片段)。已消费 `<w:pPr>` 起始标签。
pub fn parse_ppr<R: std::io::BufRead>(reader: &mut Reader<R>) -> ParaProps {
    let mut props = ParaProps::default();
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_ppr_prop(&e, &mut props),
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                // 段落边框子树:子元素名(top/bottom/…)会与其它属性撞车,专用子 walker。
                b"pBdr" => parse_pbdr(reader, &mut props),
                // 段落标记符的 run 属性:内含同名异义元素(如字符间距 w:spacing),整体跳过。
                b"rPr" => skip_element(reader),
                _ => {
                    apply_ppr_prop(&e, &mut props);
                    depth += 1;
                }
            },
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

/// 把一个 pPr 子元素(`Empty` 或 `Start`)的属性写进片段(pBdr / rPr 除外,
/// 见 [`parse_ppr`] / [`parse_pbdr`])。document.rs 的直接 pPr walker 也复用本函数。
pub fn apply_ppr_prop(e: &BytesStart, p: &mut ParaProps) {
    match local_name(e.name().as_ref()) {
        b"jc" => {
            if let Some(j) = attr_of(e, b"val").and_then(|s| Justification::from_attr(&s)) {
                p.jc = Some(j);
            }
        }
        b"spacing" => {
            // 段前/段后(twip)+ 行距(line + lineRule)。
            if let Some(v) = attr_of(e, b"before").and_then(|s| s.parse().ok()) {
                p.space_before = Some(v);
            }
            if let Some(v) = attr_of(e, b"after").and_then(|s| s.parse().ok()) {
                p.space_after = Some(v);
            }
            if let Some(line) = attr_of(e, b"line").and_then(|s| s.parse().ok()) {
                let rule = attr_of(e, b"lineRule");
                p.line = Some(match rule.as_deref() {
                    Some("exact") => LineSpacingRule::Exact(line),
                    Some("atLeast") => LineSpacingRule::AtLeast(line),
                    // "auto" 与缺省:line 单位 1/240 行。
                    _ => LineSpacingRule::Auto(line),
                });
            }
        }
        b"ind" => {
            // left/right 兼 start/end 别名(LTR 语义等价);firstLine 与 hanging 并存
            // 时由解析器决(hanging 胜),这里全量保真。
            for (keys, slot) in [
                (&[&b"left"[..], &b"start"[..]][..], &mut p.ind_left),
                (&[&b"right"[..], &b"end"[..]][..], &mut p.ind_right),
                (&[&b"firstLine"[..]][..], &mut p.ind_first_line),
                (&[&b"hanging"[..]][..], &mut p.ind_hanging),
            ] {
                for key in keys {
                    if let Some(v) = attr_of(e, key).and_then(|s| s.parse().ok()) {
                        *slot = Some(v);
                    }
                }
            }
        }
        b"shd" => {
            // 底纹填充:themeFill 间接引用优先于 fill;"auto" → 显式无底纹。
            if let Some(tc) = attr_of(e, b"themeFill").and_then(|s| ThemeColor::from_attr(&s)) {
                p.shd_fill = Some(ColorRef::Theme(tc));
            } else if let Some(v) = attr_of(e, b"fill") {
                p.shd_fill = Some(match Color::from_hex(&v) {
                    Some(c) => ColorRef::Rgb(c),
                    None => ColorRef::Auto,
                });
            }
        }
        b"keepNext" => p.keep_next = Some(on_off_val(e)),
        b"keepLines" => p.keep_lines = Some(on_off_val(e)),
        b"pageBreakBefore" => p.page_break_before = Some(on_off_val(e)),
        b"widowControl" => p.widow_control = Some(on_off_val(e)),
        b"contextualSpacing" => p.contextual_spacing = Some(on_off_val(e)),
        _ => {}
    }
}

/// 解析 `w:pBdr`(段落边框)子树:`w:top` / `w:bottom` / `w:left` / `w:right` /
/// `w:between` 各一条 [`Border`]。已消费 `<w:pBdr>` 起始标签,消费到其结束标签为止。
/// document.rs 的直接 pPr walker 也复用本函数。
pub fn parse_pbdr<R: std::io::BufRead>(reader: &mut Reader<R>, p: &mut ParaProps) {
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_border_edge(&e, p),
            Ok(Event::Start(e)) => {
                apply_border_edge(&e, p);
                depth += 1;
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // pBdr 自身结束。
                }
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// 把 pBdr 的一条边元素解析成 [`Border`] 并写进对应槽位。
fn apply_border_edge(e: &BytesStart, p: &mut ParaProps) {
    let slot = match local_name(e.name().as_ref()) {
        b"top" => &mut p.borders.top,
        b"bottom" => &mut p.borders.bottom,
        b"left" => &mut p.borders.left,
        b"right" => &mut p.borders.right,
        b"between" => &mut p.borders.between,
        _ => return, // w:bar 等其余边忽略(v1)。
    };
    *slot = Some(parse_border(e));
}

/// 解析 `w:tblBorders`(表级边框)子树:外四边 + `w:insideH` / `w:insideV`。
/// 已消费起始标签,消费到其结束标签为止。`left/right` 兼 `start/end` 别名。
pub fn parse_tbl_borders<R: std::io::BufRead>(reader: &mut Reader<R>) -> TableBorders {
    let mut borders = TableBorders::default();
    walk_border_edges(reader, |name, e| {
        let slot = match name {
            b"top" => &mut borders.top,
            b"bottom" => &mut borders.bottom,
            b"left" | b"start" => &mut borders.left,
            b"right" | b"end" => &mut borders.right,
            b"insideH" => &mut borders.inside_h,
            b"insideV" => &mut borders.inside_v,
            _ => return,
        };
        *slot = Some(parse_border(e));
    });
    borders
}

/// 解析 `w:tcBorders`(单元格边框)子树:四边(对角线 tl2br/tr2bl v1 忽略)。
/// 已消费起始标签,消费到其结束标签为止。
pub fn parse_tc_borders<R: std::io::BufRead>(reader: &mut Reader<R>) -> CellBorders {
    let mut borders = CellBorders::default();
    walk_border_edges(reader, |name, e| {
        let slot = match name {
            b"top" => &mut borders.top,
            b"bottom" => &mut borders.bottom,
            b"left" | b"start" => &mut borders.left,
            b"right" | b"end" => &mut borders.right,
            _ => return,
        };
        *slot = Some(parse_border(e));
    });
    borders
}

/// 解析 `w:tblCellMar` / `w:tcMar` 子树:四边 `@w:w`(twip,仅 `type="dxa"`,
/// 缺省按 dxa)。已消费起始标签,消费到其结束标签为止。
pub fn parse_cell_margins<R: std::io::BufRead>(reader: &mut Reader<R>) -> CellMargins {
    let mut margins = CellMargins::default();
    walk_border_edges(reader, |name, e| {
        let slot = match name {
            b"top" => &mut margins.top,
            b"bottom" => &mut margins.bottom,
            b"left" | b"start" => &mut margins.left,
            b"right" | b"end" => &mut margins.right,
            _ => return,
        };
        let is_dxa = attr_of(e, b"type")
            .map(|t| t.eq_ignore_ascii_case("dxa"))
            .unwrap_or(true);
        if is_dxa {
            if let Some(v) = attr_of(e, b"w").and_then(|s| s.parse().ok()) {
                *slot = Some(v);
            }
        }
    });
    margins
}

/// 解析样式定义里的 `w:tblPr` 片段(`w:style > w:tblPr` 的 C-7 子集:`w:tblBorders` +
/// `w:tblCellMar`;其余属性 v1 不入级联)。已消费起始标签,消费到其结束标签为止。
pub fn parse_style_tblpr<R: std::io::BufRead>(reader: &mut Reader<R>) -> TableProps {
    let mut props = TableProps::default();
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(_)) => {}
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"tblBorders" => props.borders = parse_tbl_borders(reader),
                b"tblCellMar" => props.cell_margins = parse_cell_margins(reader),
                _ => depth += 1,
            },
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // tblPr 自身结束。
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

/// 遍历一个「边集合」子树(tblBorders/tcBorders/tblCellMar/tcMar 同构:直接子元素
/// 按本地名分派,`Empty` 与 `Start` 统一处理,`Start` 子树以深度计数兜底)。
fn walk_border_edges<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    mut apply: impl FnMut(&[u8], &BytesStart),
) {
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply(local_name(e.name().as_ref()), &e),
            Ok(Event::Start(e)) => {
                apply(local_name(e.name().as_ref()), &e);
                depth += 1;
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // 集合自身结束。
                }
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// 解析一条 CT_Border 边(`@val` / `@sz` / `@space` / `@color` / `@themeColor`)。
pub fn parse_border(e: &BytesStart) -> Border {
    let color = if let Some(tc) = attr_of(e, b"themeColor").and_then(|s| ThemeColor::from_attr(&s))
    {
        Some(ColorRef::Theme(tc))
    } else {
        attr_of(e, b"color").map(|v| match Color::from_hex(&v) {
            Some(c) => ColorRef::Rgb(c),
            None => ColorRef::Auto,
        })
    };
    Border {
        val: attr_of(e, b"val").unwrap_or_else(|| "single".to_string()),
        sz_eighth_pt: attr_of(e, b"sz").and_then(|s| s.parse().ok()).unwrap_or(0),
        space_pt: attr_of(e, b"space")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0),
        color,
    }
}
