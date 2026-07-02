//! 解析 `word/styles.xml`(样式部件)-> [`StyleTable`](PDF-EXPORT C-5)。
//!
//! 结构(ECMA-376 §17.7):
//!
//! ```xml
//! <w:styles>
//!   <w:docDefaults>
//!     <w:rPrDefault><w:rPr>…</w:rPr></w:rPrDefault>
//!     <w:pPrDefault><w:pPr>…</w:pPr></w:pPrDefault>
//!   </w:docDefaults>
//!   <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
//!     <w:name w:val="Normal"/> <w:basedOn w:val="…"/> <w:pPr>…</w:pPr> <w:rPr>…</w:rPr>
//!   </w:style> …
//! </w:styles>
//! ```
//!
//! 解析是**机械搬运**:把 docDefaults 与每条 `w:style` 的 id / 种类 / basedOn / default
//! 标志 + rPr/pPr 片段(经共享的 [`props`] 解析器)装进 [`StyleTable`],**不做级联**——
//! 级联合并在 doc-core 的 `style::resolve_*`(纯模型计算)。容错:未知元素跳过、
//! 缺失属性 → 缺省、绝不 panic。

use doc_core::style::{Style, StyleKind, StyleTable};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{attr_of, local_name, props, skip_element};

/// 解析 `word/styles.xml` 文本。部件整体畸形时返回已得部分(最坏空表)。
pub fn parse(xml: &str) -> StyleTable {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    // 先定位到 w:styles 根,再解析其直接子元素。
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"styles" {
                    return parse_styles_children(&mut reader);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    StyleTable::default()
}

/// 解析 `w:styles` 的直接子元素。假定 reader 已消费 `<w:styles>` 起始标签。
fn parse_styles_children<R: std::io::BufRead>(reader: &mut Reader<R>) -> StyleTable {
    let mut table = StyleTable::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"docDefaults" => parse_doc_defaults(reader, &mut table),
                b"style" => {
                    let (id, style) = parse_style(reader, &e);
                    register(&mut table, id, style);
                }
                // 其余(w:latentStyles 等)整体跳过。
                _ => skip_element(reader),
            },
            Ok(Event::Empty(e)) => {
                // 自闭合的退化 <w:style …/>(无子元素):仅属性也照收。
                if local_name(e.name().as_ref()) == b"style" {
                    let (id, kind, default) = style_attrs(&e);
                    register(
                        &mut table,
                        id,
                        Style {
                            kind,
                            default,
                            ..Style::default()
                        },
                    );
                }
            }
            Ok(Event::End(_)) => break, // styles 根结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    table
}

/// 解析 `w:docDefaults`:`rPrDefault > rPr` 与 `pPrDefault > pPr`(壳透明下潜)。
/// 已消费 `<w:docDefaults>` 起始标签。
fn parse_doc_defaults<R: std::io::BufRead>(reader: &mut Reader<R>, table: &mut StyleTable) {
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                // 透明壳:下潜(深度 +1,由对应 End 抵消)。
                b"rPrDefault" | b"pPrDefault" => depth += 1,
                // 片段子 walker 消费到自己的结束标签,深度不受影响。
                b"rPr" => table.doc_default_rpr = props::parse_rpr(reader),
                b"pPr" => table.doc_default_ppr = props::parse_ppr(reader),
                _ => skip_element(reader),
            },
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // docDefaults 自身结束。
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

/// 解析一条 `w:style`。已消费其起始标签(属性经 `start` 传入)。
fn parse_style<R: std::io::BufRead>(reader: &mut Reader<R>, start: &BytesStart) -> (String, Style) {
    let (id, kind, default) = style_attrs(start);
    let mut style = Style {
        kind,
        default,
        ..Style::default()
    };
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_style_child(&e, &mut style),
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"rPr" => style.rpr = props::parse_rpr(reader),
                b"pPr" => style.ppr = props::parse_ppr(reader),
                _ => {
                    apply_style_child(&e, &mut style);
                    skip_element(reader);
                }
            },
            Ok(Event::End(_)) => break, // style 自身结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (id, style)
}

/// `w:style` 的元数据子元素:`w:name@w:val`、`w:basedOn@w:val`(其余忽略)。
fn apply_style_child(e: &BytesStart, style: &mut Style) {
    match local_name(e.name().as_ref()) {
        b"name" => style.name = attr_of(e, b"val").or(style.name.take()),
        b"basedOn" => style.based_on = attr_of(e, b"val").or(style.based_on.take()),
        _ => {}
    }
}

/// 读 `w:style` 的属性三元组:`styleId` / `type`(缺省 paragraph)/ `default`。
fn style_attrs(e: &BytesStart) -> (String, StyleKind, bool) {
    let id = attr_of(e, b"styleId").unwrap_or_default();
    let kind = attr_of(e, b"type")
        .map(|t| StyleKind::from_attr(&t))
        .unwrap_or_default();
    let default = attr_of(e, b"default")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true") || v.eq_ignore_ascii_case("on"))
        .unwrap_or(false);
    (id, kind, default)
}

/// 把一条样式登记进表:无 id 的丢弃;`default` 为真时按种类记缺省样式 id;同 id 后者覆盖。
fn register(table: &mut StyleTable, id: String, style: Style) {
    if id.is_empty() {
        return;
    }
    if style.default {
        match style.kind {
            StyleKind::Paragraph => table.default_para_style = Some(id.clone()),
            StyleKind::Character => table.default_char_style = Some(id.clone()),
            _ => {}
        }
    }
    table.styles.insert(id, style);
}
