//! 解析 `word/document.xml`(WordprocessingML 主文档)-> `Vec<Block>`。
//!
//! 走 `w:document` > `w:body`,在 body 这一级识别块级元素(顺序即文档顺序):
//! - `w:p`   —— 段落(内含带样式的 `w:r` run + 内嵌图片;`w:pPr > w:sectPr` 是节边界)
//! - `w:tbl` —— 表格(**本轮重点**)
//! - `w:sdt` —— 结构化文档标签,内容透明展开(封面/目录文字不丢)
//! - `w:sectPr`(body 末尾)—— 最后一节的页面几何(尺寸/边距/纸向/分栏)
//!
//! **表格解析做扎实**:
//! - `w:tblGrid` > `w:gridCol` 给出逻辑列定义(列数 + 各列宽 twip)。
//! - `w:tr` 行;`w:tc` 单元格。单元格属性 `w:tcPr` 里:
//!   - `w:gridSpan@w:val` —— 横向跨列合并。
//!   - `w:vMerge`         —— 纵向合并(`restart` 起始 / `continue` 延续 / 省略 val 视作 restart)。
//!   - `w:tcW@w:w`(`type="dxa"`)—— 单元格绝对宽度 twip。
//!   - `w:shd@w:fill`     —— 单元格底纹填充色。
//! - **嵌套表**天然支持:单元格内容是块序列,里头再出现 `w:tbl` 就递归成 [`Block::Table`]。
//! - **单元格内段落**完整解析(同正文段落)。
//!
//! 实现是**递归下降**的 quick-xml 事件遍历:每个 `parse_*` 子函数在收到对应起始标签后,
//! 一路消费到其匹配的结束标签为止,期间填充模型。容错:未知元素跳过、缺失属性 → 缺省、绝不 panic。

use std::collections::BTreeMap;

use doc_core::geom::{Emu, Twips};
use doc_core::model::{
    Block, BreakKind, Cell, Color, Orientation, Paragraph, Picture, Row, RunSegment, Section,
    Table, TextRun, VMerge,
};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{
    attr_of, attr_string, local_name, media_name_from_target, on_off_val, parse_rels, Relationship,
};

/// 解析期的只读上下文:rel 映射(图片 r:id -> media 名) + media 长度索引。
struct Ctx<'a> {
    rels: &'a BTreeMap<String, Relationship>,
    media_index: &'a BTreeMap<String, usize>,
}

/// 解析 `word/document.xml`。`rels_xml` 是主文档 `.rels` 文本(把图片 `r:embed/r:id`
/// 映射到 media 名);`media_index` 是 `裸文件名 -> 字节长度`,用于回填 `image_bytes_len`。
/// 返回 `(正文块序列, 节序列)`;节序列保证非空(无任何 `w:sectPr` 时补 Word 默认节)。
pub fn parse(
    xml: &str,
    rels_xml: Option<&str>,
    media_index: &BTreeMap<String, usize>,
) -> (Vec<Block>, Vec<Section>) {
    let rels = rels_xml.map(parse_rels).unwrap_or_default();
    let ctx = Ctx {
        rels: &rels,
        media_index,
    };
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    // 先定位到 w:body,再解析其直接子块。
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"body" {
                    return parse_body(&mut reader, &ctx);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (Vec::new(), vec![Section::default()])
}

/// 解析 `w:body` 的直接子块 + 节序列。假定 reader 已经消费了 `<w:body>` 起始标签。
///
/// 节的归属语义(WordprocessingML):段落 `w:pPr > w:sectPr` 结束**包含该段落**的那一节
/// (该段落属于这一节);body 末尾的直接子元素 `w:sectPr` 定义最后一节。容错:整篇没有
/// 任何 `w:sectPr` 时补一个覆盖全部块的 Word 默认节,保证节序列非空。
fn parse_body<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    ctx: &Ctx,
) -> (Vec<Block>, Vec<Section>) {
    let mut blocks = Vec::new();
    let mut sections: Vec<Section> = Vec::new();
    let mut trailing: Option<Section> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"p" => {
                        let (para, sect) = parse_paragraph(reader, ctx);
                        blocks.push(Block::Paragraph(para));
                        // 段内 sectPr:结束包含它的这一节(该段落含在内)。
                        if let Some(mut s) = sect {
                            s.end_block = blocks.len();
                            sections.push(s);
                        }
                    }
                    b"tbl" => blocks.push(Block::Table(parse_table(reader, ctx))),
                    // 结构化文档标签:内容在 w:sdtContent 里,透明展开(修复内容丢失)。
                    b"sdt" => blocks.extend(parse_sdt_blocks(reader, ctx)),
                    // body 末尾的 sectPr:最后一节的页面几何。
                    b"sectPr" => trailing = Some(parse_sectpr(reader)),
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"sectPr" {
                    trailing = Some(Section::default());
                }
            }
            Ok(Event::End(_)) => break, // body 结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    // 收尾:body 末尾 sectPr 定义最后一节;完全没有 sectPr 时补默认节;畸形输入
    // (有段内 sectPr、缺 body 末尾 sectPr)也让余下块归入一个默认节。
    match trailing {
        Some(mut s) => {
            s.end_block = blocks.len();
            sections.push(s);
        }
        None => {
            let covered = sections.last().map(|s| s.end_block).unwrap_or(0);
            if sections.is_empty() || covered < blocks.len() {
                sections.push(Section {
                    end_block: blocks.len(),
                    ..Section::default()
                });
            }
        }
    }
    (blocks, sections)
}

/// 解析一个块容器(`w:tc` / `w:sdtContent`)的直接子块,直到容器结束标签。
/// 假定 reader 已经消费了容器的起始标签。在这里 `w:p` -> 段落、`w:tbl` -> 表格、
/// `w:sdt` -> 透明展开。(段内 sectPr 在这些容器里不合法,忽略。)
fn parse_block_container<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"p" => {
                        let (para, _) = parse_paragraph(reader, ctx);
                        blocks.push(Block::Paragraph(para));
                    }
                    b"tbl" => blocks.push(Block::Table(parse_table(reader, ctx))),
                    b"sdt" => blocks.extend(parse_sdt_blocks(reader, ctx)),
                    // 其它直接子元素(tcPr 等)整体跳过。
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break, // 容器结束。
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    blocks
}

// ============================================================ 节 (w:sectPr)

/// 解析 `w:sectPr`(节属性):`w:pgSz`(页面尺寸/纸向)、`w:pgMar`(页边距)、
/// `w:cols@w:num`(分栏数)。已消费 `<w:sectPr>` 起始标签。未知子元素跳过;
/// 缺失属性一律落到 Word 默认值([`Section::default`])。`end_block` 由调用方回填。
fn parse_sectpr<R: std::io::BufRead>(reader: &mut Reader<R>) -> Section {
    let mut sect = Section::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_sectpr_prop(&e, &mut sect),
            Ok(Event::Start(e)) => {
                // 带子树的形式(如 w:cols 内嵌 w:col)先取属性,再整体跳过子树。
                apply_sectpr_prop(&e, &mut sect);
                skip_element(reader);
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    sect
}

/// 把一个 sectPr 子元素的属性应用到 [`Section`] 上。
fn apply_sectpr_prop(e: &BytesStart, sect: &mut Section) {
    match local_name(e.name().as_ref()) {
        b"pgSz" => {
            if let Some(w) = attr_of(e, b"w").and_then(|s| s.parse().ok()) {
                sect.page_width = w;
            }
            if let Some(h) = attr_of(e, b"h").and_then(|s| s.parse().ok()) {
                sect.page_height = h;
            }
            if let Some(o) = attr_of(e, b"orient") {
                if o.eq_ignore_ascii_case("landscape") {
                    sect.orientation = Orientation::Landscape;
                }
            }
        }
        b"pgMar" => {
            let m = &mut sect.margins;
            for (key, slot) in [
                (&b"top"[..], &mut m.top),
                (&b"right"[..], &mut m.right),
                (&b"bottom"[..], &mut m.bottom),
                (&b"left"[..], &mut m.left),
                (&b"header"[..], &mut m.header),
                (&b"footer"[..], &mut m.footer),
                (&b"gutter"[..], &mut m.gutter),
            ] {
                if let Some(v) = attr_of(e, key).and_then(|s| s.parse().ok()) {
                    *slot = v;
                }
            }
        }
        b"cols" => {
            if let Some(n) = attr_of(e, b"num").and_then(|s| s.parse().ok()) {
                sect.cols = n;
            }
        }
        _ => {}
    }
}

// ============================================================ 段落 (w:p)

/// 解析 `w:p`(段落)。已消费 `<w:p>` 起始标签。返回 `(段落, 段内 sectPr 的节)`:
/// 段落 `w:pPr > w:sectPr` 是**节边界**(该段落是所在节的最后一块),交由 body 层归属。
fn parse_paragraph<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    ctx: &Ctx,
) -> (Paragraph, Option<Section>) {
    let mut para = Paragraph::default();
    let mut sect = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"pPr" => sect = parse_ppr(reader, &mut para).or(sect),
                    b"r" => {
                        let run = parse_run(reader, ctx);
                        // 丢掉完全空白且无图片的 run,避免噪声;但保留带图片的空文字 run。
                        if !run.segments.is_empty() || !run.pictures.is_empty() {
                            para.runs.push(run);
                        }
                    }
                    // 超链接 `w:hyperlink` / 修订插入 `w:ins` / 字段 `w:fldSimple`(缓存的
                    // 字段结果)都是 run 的容器:不要 skip,展开解析其中的 run。`w:ins`
                    // (修订插入)按“接受修订”语义当作正常正文保留。
                    b"hyperlink" | b"ins" | b"fldSimple" => {
                        for run in parse_run_container(reader, ctx) {
                            if !run.segments.is_empty() || !run.pictures.is_empty() {
                                para.runs.push(run);
                            }
                        }
                    }
                    // 行内结构化文档标签:内容在 w:sdtContent 里,透明展开(修复内容丢失)。
                    b"sdt" => {
                        for run in parse_sdt_runs(reader, ctx) {
                            if !run.segments.is_empty() || !run.pictures.is_empty() {
                                para.runs.push(run);
                            }
                        }
                    }
                    // 其余元素跳过。其中修订删除 `w:del`(其内 run 用 `w:delText`)按
                    // “接受修订”语义整段丢弃、不输出文字,正好走这里被 skip。
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (para, sect)
}

/// 解析 `w:pPr`(段落属性):`w:pStyle`、`w:jc`、`w:numPr>w:ilvl`、`w:sectPr`(节边界,
/// 作为返回值交给段落层)。已消费 `<w:pPr>` 起始标签。
///
/// 与 [`parse_rpr`] 同构地做**深度计数**:嵌套容器(如 `w:numPr`)的结束标签不会再把
/// pPr 的遍历提前打断——修复过去 `w:numPr` / `w:sectPr` 之后的属性(如 `w:jc`)丢失、
/// 甚至整段后续正文被截断的内容丢失缺陷。
fn parse_ppr<R: std::io::BufRead>(reader: &mut Reader<R>, para: &mut Paragraph) -> Option<Section> {
    let mut sect = None;
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"sectPr" {
                    // 自闭合 <w:sectPr/>:全默认值的节。
                    sect = Some(Section::default());
                } else {
                    apply_ppr_prop(&e, para);
                }
            }
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"sectPr" {
                    // 子 walker 消费整个 sectPr 子树,深度不受影响。
                    sect = Some(parse_sectpr(reader));
                } else {
                    apply_ppr_prop(&e, para);
                    depth += 1;
                }
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
    sect
}

/// 把一个 pPr 子元素(`Empty` 或 `Start`)的属性应用到段落上。
fn apply_ppr_prop(e: &BytesStart, para: &mut Paragraph) {
    match local_name(e.name().as_ref()) {
        b"pStyle" => para.style = attr_of(e, b"val").or(para.style.take()),
        b"jc" => para.align = attr_of(e, b"val").or(para.align.take()),
        b"ilvl" => {
            para.list_level = attr_of(e, b"val").and_then(|s| s.parse().ok());
        }
        _ => {}
    }
}

// ============================================================ run (w:r)

/// 解析一个可能含若干 `w:r` 的容器(如 `w:hyperlink` / `w:ins` / `w:fldSimple`)。已消费
/// 容器起始标签。嵌套的同类容器与行内 `w:sdt` 递归展开其 run;其余(含 `w:del`)整体跳过。
fn parse_run_container<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"r" => runs.push(parse_run(reader, ctx)),
                    b"hyperlink" | b"ins" | b"fldSimple" => {
                        runs.extend(parse_run_container(reader, ctx))
                    }
                    b"sdt" => runs.extend(parse_sdt_runs(reader, ctx)),
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    runs
}

/// 解析 `w:r`(文本 run):`w:rPr`(字体/字号/粗斜/下划线/颜色)+ 内容分段
/// (`w:t` -> `Text`、`w:tab` -> `Tab`、`w:br`/`w:cr` -> `Break`,`w:br@w:type` 区分
/// 换行/换页/换栏)+ `w:drawing`/`w:pict`(内嵌图片)。已消费 `<w:r>` 起始标签。
fn parse_run<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> TextRun {
    let mut run = TextRun::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"rPr" => parse_rpr(reader, &mut run),
                    b"t" => run.push_text(&read_text(reader)),
                    b"tab" => {
                        run.segments.push(RunSegment::Tab);
                        skip_element(reader);
                    }
                    b"br" => {
                        run.segments.push(RunSegment::Break(break_kind(&e)));
                        skip_element(reader);
                    }
                    b"cr" => {
                        run.segments.push(RunSegment::Break(BreakKind::Line));
                        skip_element(reader);
                    }
                    b"drawing" => {
                        if let Some(pic) = parse_drawing(reader, ctx) {
                            run.pictures.push(pic);
                        }
                    }
                    b"pict" | b"object" => {
                        if let Some(pic) = parse_vml_pict(reader, ctx) {
                            run.pictures.push(pic);
                        }
                    }
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"tab" => run.segments.push(RunSegment::Tab),
                    b"br" => run.segments.push(RunSegment::Break(break_kind(&e))),
                    b"cr" => run.segments.push(RunSegment::Break(BreakKind::Line)),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    run
}

/// 读 `w:br@w:type` 的断种类:`page` 换页、`column` 换栏、其余(含缺省 `textWrapping`)换行。
fn break_kind(e: &BytesStart) -> BreakKind {
    match attr_of(e, b"type") {
        Some(t) if t.eq_ignore_ascii_case("page") => BreakKind::Page,
        Some(t) if t.eq_ignore_ascii_case("column") => BreakKind::Column,
        _ => BreakKind::Line,
    }
}

// ============================================================ 结构化文档标签 (w:sdt)

/// 解析**块级** `w:sdt`(结构化文档标签,如封面 / 目录容器):跳过 `w:sdtPr` / `w:sdtEndPr`
/// 外壳,把 `w:sdtContent` 里的块透明展开(嵌套 sdt 经 [`parse_block_container`] 递归)。
/// 已消费 `<w:sdt>` 起始标签。
fn parse_sdt_blocks<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"sdtContent" {
                    blocks.extend(parse_block_container(reader, ctx));
                } else {
                    skip_element(reader);
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    blocks
}

/// 解析**行内** `w:sdt`(段落内的结构化文档标签,如日期选择器):跳过外壳,把
/// `w:sdtContent` 里的 run 透明展开(嵌套容器经 [`parse_run_container`] 递归)。
/// 已消费 `<w:sdt>` 起始标签。
fn parse_sdt_runs<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"sdtContent" {
                    runs.extend(parse_run_container(reader, ctx));
                } else {
                    skip_element(reader);
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    runs
}

/// 解析 `w:rPr`(run 属性):字体/字号/粗体/斜体/下划线/颜色。已消费 `<w:rPr>` 起始标签。
/// rPr 子元素几乎都是自闭合的开关/属性元素;`Empty` 与 `Start` 形式按本地名统一处理,Start
/// 形式带子树的(罕见)以深度计数兜底,不会错位(见 [`skip_element`] 的循环结构)。
fn parse_rpr<R: std::io::BufRead>(reader: &mut Reader<R>, run: &mut TextRun) {
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_rpr_prop(&e, run),
            Ok(Event::Start(e)) => {
                apply_rpr_prop(&e, run);
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
}

/// 把一个 rPr 子元素(`Empty` 或 `Start`)的属性应用到 run 上。
fn apply_rpr_prop(e: &BytesStart, run: &mut TextRun) {
    match local_name(e.name().as_ref()) {
        b"rFonts" => {
            run.font = attr_of(e, b"ascii")
                .or_else(|| attr_of(e, b"hAnsi"))
                .or_else(|| attr_of(e, b"cs"))
                .or(run.font.take());
        }
        b"sz" => {
            // w:sz 以半磅存储;除以 2 得磅。
            run.size_pt = attr_of(e, b"val")
                .and_then(|s| s.parse::<f32>().ok())
                .map(|v| v / 2.0)
                .or(run.size_pt.take());
        }
        b"b" => run.bold = on_off_val(e),
        b"i" => run.italic = on_off_val(e),
        b"u" => {
            // 下划线:val 非 "none" 即为真。
            run.underline = attr_of(e, b"val")
                .map(|v| !v.eq_ignore_ascii_case("none"))
                .unwrap_or(true);
        }
        b"color" => {
            run.color = attr_of(e, b"val")
                .and_then(|h| Color::from_hex(&h))
                .or(run.color.take());
        }
        _ => {}
    }
}

// ============================================================ 表格 (w:tbl)

/// 解析 `w:tbl`(表格)。已消费 `<w:tbl>` 起始标签。
fn parse_table<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Table {
    let mut table = Table::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"tblPr" => parse_tblpr(reader, &mut table),
                    b"tblGrid" => table.grid_cols = parse_tbl_grid(reader),
                    b"tr" => table.rows.push(parse_table_row(reader, ctx)),
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    table
}

/// 解析 `w:tblPr`(表格属性):目前取 `w:tblStyle@w:val`。已消费 `<w:tblPr>` 起始标签。
fn parse_tblpr<R: std::io::BufRead>(reader: &mut Reader<R>, table: &mut Table) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                if name.as_slice() == b"tblStyle" {
                    table.style = attr_of(&e, b"val").or(table.style.take());
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// 解析 `w:tblGrid` -> 各列宽(twip)。已消费 `<w:tblGrid>` 起始标签。
fn parse_tbl_grid<R: std::io::BufRead>(reader: &mut Reader<R>) -> Vec<Twips> {
    let mut cols = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"gridCol" {
                    let w: Twips = attr_of(&e, b"w").and_then(|s| s.parse().ok()).unwrap_or(0);
                    cols.push(w);
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    cols
}

/// 解析 `w:tr`(表格行)。已消费 `<w:tr>` 起始标签。
fn parse_table_row<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Row {
    let mut row = Row::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"trPr" => parse_trpr(reader, &mut row),
                    b"tc" => row.cells.push(parse_table_cell(reader, ctx)),
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    row
}

/// 解析 `w:trPr`(行属性):`w:trHeight@w:val`、`w:tblHeader`。已消费 `<w:trPr>` 起始标签。
fn parse_trpr<R: std::io::BufRead>(reader: &mut Reader<R>, row: &mut Row) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"trHeight" => {
                        row.height = attr_of(&e, b"val")
                            .and_then(|s| s.parse().ok())
                            .or(row.height);
                    }
                    b"tblHeader" => row.is_header = on_off_val(&e),
                    _ => {}
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

/// 解析 `w:tc`(单元格):`w:tcPr`(合并/宽度/填充)+ 内容块(段落 + 嵌套表)。
/// 已消费 `<w:tc>` 起始标签。**内容块复用 [`parse_block_container`],所以嵌套表天然支持。**
fn parse_table_cell<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Cell {
    let mut cell = Cell {
        grid_span: 1,
        ..Cell::default()
    };
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"tcPr" => parse_tcpr(reader, &mut cell),
                    b"p" => {
                        let (para, _) = parse_paragraph(reader, ctx);
                        cell.blocks.push(Block::Paragraph(para));
                    }
                    b"tbl" => cell.blocks.push(Block::Table(parse_table(reader, ctx))),
                    b"sdt" => cell.blocks.extend(parse_sdt_blocks(reader, ctx)),
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    cell
}

/// 解析 `w:tcPr`(单元格属性):`w:gridSpan`(横向合并)、`w:vMerge`(纵向合并)、
/// `w:tcW`(宽度)、`w:shd`(填充)。已消费 `<w:tcPr>` 起始标签。
fn parse_tcpr<R: std::io::BufRead>(reader: &mut Reader<R>, cell: &mut Cell) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"gridSpan" => {
                        cell.grid_span = attr_of(&e, b"val")
                            .and_then(|s| s.parse().ok())
                            .unwrap_or(1);
                    }
                    b"vMerge" => {
                        // val="restart" 起始;val="continue" 延续;省略 val 视作 restart。
                        cell.v_merge = match attr_of(&e, b"val") {
                            Some(v) if v.eq_ignore_ascii_case("continue") => VMerge::Continue,
                            _ => VMerge::Restart,
                        };
                    }
                    b"tcW" => {
                        // 仅当 type="dxa"(绝对 twip)时记宽度;pct/auto 不记。
                        let is_dxa = attr_of(&e, b"type")
                            .map(|t| t.eq_ignore_ascii_case("dxa"))
                            .unwrap_or(true);
                        if is_dxa {
                            cell.width = attr_of(&e, b"w")
                                .and_then(|s| s.parse().ok())
                                .or(cell.width);
                        }
                    }
                    b"shd" => {
                        cell.fill = attr_of(&e, b"fill")
                            .and_then(|h| Color::from_hex(&h))
                            .or(cell.fill.take());
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}

// ============================================================ 图片 (w:drawing / w:pict)

/// 解析 `w:drawing`(DrawingML 内嵌/浮动图片):取 `wp:extent@cx/cy` + `a:blip@r:embed`。
/// 已消费 `<w:drawing>` 起始标签。无 blip 引用则返回 `None`。
fn parse_drawing<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Option<Picture> {
    let mut rel_id = String::new();
    let mut extent: Option<(Emu, Emu)> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"extent" => {
                        let cx: Emu = attr_of(&e, b"cx").and_then(|s| s.parse().ok()).unwrap_or(0);
                        let cy: Emu = attr_of(&e, b"cy").and_then(|s| s.parse().ok()).unwrap_or(0);
                        extent = Some((cx, cy));
                    }
                    b"blip" => {
                        // r:embed 属性(命名空间前缀按本地名匹配)。
                        for attr in e.attributes().flatten() {
                            if local_name(attr.key.as_ref()) == b"embed" {
                                rel_id = attr_string(&attr);
                            }
                        }
                    }
                    _ => {}
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    resolve_picture(ctx, rel_id, extent)
}

/// 解析旧式 VML `w:pict`(以及 `w:object` 内的 VML):取 `v:imagedata@r:id`。
/// 已消费起始标签。无引用则返回 `None`。
fn parse_vml_pict<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Option<Picture> {
    let mut rel_id = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"imagedata" {
                    for attr in e.attributes().flatten() {
                        if local_name(attr.key.as_ref()) == b"id" {
                            rel_id = attr_string(&attr);
                        }
                    }
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    resolve_picture(ctx, rel_id, None)
}

/// 把图片的 rel id 经 rels 映射到 media 裸文件名,回填字节长度,组装 [`Picture`]。
/// rel id 为空(没找到引用)则返回 `None`。
fn resolve_picture(ctx: &Ctx, rel_id: String, extent: Option<(Emu, Emu)>) -> Option<Picture> {
    if rel_id.is_empty() {
        return None;
    }
    let media_name = ctx
        .rels
        .get(&rel_id)
        .map(|r| media_name_from_target(&r.target));
    let image_bytes_len = media_name
        .as_ref()
        .and_then(|n| ctx.media_index.get(n).copied())
        .unwrap_or(0);
    Some(Picture {
        rel_id,
        media_name,
        extent,
        image_bytes_len,
    })
}

// ============================================================ 通用小工具

/// 读取当前已打开元素的纯文本内容,直到其结束标签。已消费该元素的起始标签。
fn read_text<R: std::io::BufRead>(reader: &mut Reader<R>) -> String {
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(t)) => {
                if let Ok(s) = t.unescape() {
                    out.push_str(&s);
                }
            }
            Ok(Event::CData(c)) => out.push_str(&String::from_utf8_lossy(&c)),
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out
}

/// 跳过当前已打开元素的全部内容,直到其匹配的结束标签。已消费该元素的起始标签。
/// 通过深度计数处理同名嵌套。
fn skip_element<R: std::io::BufRead>(reader: &mut Reader<R>) {
    let mut depth = 1usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}
