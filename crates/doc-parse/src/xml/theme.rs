//! 解析 `word/theme/theme1.xml`(DrawingML 主题部件)-> [`Theme`](PDF-EXPORT C-5)。
//!
//! 只关心两个子树(其余 fmtScheme / extraClrSchemeLst 等整体跳过——特别是
//! `a:extraClrSchemeLst` 里还有别的 `a:clrScheme`,不能误收):
//!
//! ```xml
//! <a:theme><a:themeElements>
//!   <a:clrScheme>  <a:dk1><a:sysClr lastClr="000000"/></a:dk1>
//!                  <a:accent1><a:srgbClr val="4472C4"/></a:accent1> … </a:clrScheme>
//!   <a:fontScheme> <a:majorFont><a:latin typeface="Calibri Light"/><a:ea …/><a:cs …/></a:majorFont>
//!                  <a:minorFont>…</a:minorFont> </a:fontScheme>
//! </a:themeElements></a:theme>
//! ```
//!
//! rFonts 的 `asciiTheme="minorHAnsi"` 等间接引用与 `w:color@themeColor` 就是在
//! doc-core 的解析器里查这里的 fontScheme / clrScheme 解成实际值。

use doc_core::model::Color;
use doc_core::style::{FontScheme, FontSet, Theme, ThemeColor};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{attr_of, local_name, skip_element};

/// 解析主题部件文本。畸形输入返回已得部分(最坏空主题)。
pub fn parse(xml: &str) -> Theme {
    let mut theme = Theme::default();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                // 只经 theme / themeElements 两层壳下潜,其余子树整体跳过。
                b"theme" | b"themeElements" => {}
                b"clrScheme" => parse_clr_scheme(&mut reader, &mut theme),
                b"fontScheme" => parse_font_scheme(&mut reader, &mut theme.fonts),
                _ => skip_element(&mut reader),
            },
            // 壳的结束标签:忽略,直到文档结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    theme
}

/// 解析 `a:clrScheme`:直接子元素是槽位(`a:dk1` / `a:accent1` …),槽位内
/// `a:srgbClr@val` 或 `a:sysClr@lastClr` 给出 RGB。已消费起始标签。
fn parse_clr_scheme<R: std::io::BufRead>(reader: &mut Reader<R>, theme: &mut Theme) {
    let mut slot: Option<ThemeColor> = None;
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if depth == 0 {
                    // clrScheme 的直接子元素 = 槽位名。
                    slot = slot_of(&e);
                } else if let (Some(s), Some(c)) = (slot, color_of(&e)) {
                    theme.colors.set(s, c);
                }
                depth += 1;
            }
            Ok(Event::Empty(e)) => {
                if depth > 0 {
                    if let (Some(s), Some(c)) = (slot, color_of(&e)) {
                        theme.colors.set(s, c);
                    }
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // clrScheme 自身结束。
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

/// 槽位元素本地名 -> 主题色槽位。
fn slot_of(e: &BytesStart) -> Option<ThemeColor> {
    let name = e.name();
    let local = String::from_utf8_lossy(local_name(name.as_ref())).into_owned();
    ThemeColor::from_scheme_element(&local)
}

/// 颜色值元素 -> RGB:`a:srgbClr@val`(十六进制)或 `a:sysClr@lastClr`(系统色的
/// 最近解析值)。其余(alpha 修饰等)忽略。
fn color_of(e: &BytesStart) -> Option<Color> {
    match local_name(e.name().as_ref()) {
        b"srgbClr" => attr_of(e, b"val").and_then(|v| Color::from_hex(&v)),
        b"sysClr" => attr_of(e, b"lastClr").and_then(|v| Color::from_hex(&v)),
        _ => None,
    }
}

/// 解析 `a:fontScheme`:`a:majorFont` / `a:minorFont` 各含 `a:latin` / `a:ea` / `a:cs`
/// (`@typeface`,空串按缺失)。per-script 的 `a:font` 覆盖表忽略。已消费起始标签。
fn parse_font_scheme<R: std::io::BufRead>(reader: &mut Reader<R>, fonts: &mut FontScheme) {
    let mut major = false;
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                match local_name(e.name().as_ref()) {
                    b"majorFont" => major = true,
                    b"minorFont" => major = false,
                    other => apply_font_slot(other, &e, major, fonts),
                }
                depth += 1;
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                apply_font_slot(&name, &e, major, fonts);
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // fontScheme 自身结束。
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

/// 把 `a:latin` / `a:ea` / `a:cs` 的 `@typeface` 写进对应槽位(空串跳过)。
fn apply_font_slot(name: &[u8], e: &BytesStart, major: bool, fonts: &mut FontScheme) {
    let set: &mut FontSet = if major {
        &mut fonts.major
    } else {
        &mut fonts.minor
    };
    let slot = match name {
        b"latin" => &mut set.latin,
        b"ea" => &mut set.east_asia,
        b"cs" => &mut set.cs,
        _ => return,
    };
    if let Some(t) = attr_of(e, b"typeface") {
        if !t.is_empty() {
            *slot = Some(t);
        }
    }
}
