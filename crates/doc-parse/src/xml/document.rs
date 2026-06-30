//! 解析 `word/document.xml`(WordprocessingML 主文档)-> `Vec<Block>`。
//!
//! 走 `w:document` > `w:body`,在 body 这一级识别两类块级元素(顺序即文档顺序):
//! - `w:p`   —— 段落(内含带样式的 `w:r` run + 内嵌图片)
//! - `w:tbl` —— 表格(**本轮重点**)
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
use doc_core::model::{Block, Cell, Color, Paragraph, Picture, Row, Table, TextRun, VMerge};
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
pub fn parse(
    xml: &str,
    rels_xml: Option<&str>,
    media_index: &BTreeMap<String, usize>,
) -> Vec<Block> {
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
                    return parse_block_container(&mut reader, &ctx);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    Vec::new()
}

/// 解析一个块容器(`w:body` 或 `w:tc`)的直接子块,直到容器结束标签。
/// 假定 reader 已经消费了容器的起始标签。在这里 `w:p` -> 段落、`w:tbl` -> 表格。
fn parse_block_container<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"p" => blocks.push(Block::Paragraph(parse_paragraph(reader, ctx))),
                    b"tbl" => blocks.push(Block::Table(parse_table(reader, ctx))),
                    // 其它直接子元素(sectPr / tcPr / sdt 容器外壳等)整体跳过。
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

// ============================================================ 段落 (w:p)

/// 解析 `w:p`(段落)。已消费 `<w:p>` 起始标签。
fn parse_paragraph<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Paragraph {
    let mut para = Paragraph::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"pPr" => parse_ppr(reader, &mut para),
                    b"r" => {
                        let run = parse_run(reader, ctx);
                        // 丢掉完全空白且无图片的 run,避免噪声;但保留带图片的空文字 run。
                        if !run.text.is_empty() || !run.pictures.is_empty() {
                            para.runs.push(run);
                        }
                    }
                    // 超链接 `w:hyperlink` / 修订插入 `w:ins` 都是 run 的容器:不要 skip,
                    // 展开解析其中的 run。`w:ins`(修订插入)按“接受修订”语义当作正常正文保留。
                    b"hyperlink" | b"ins" => {
                        for run in parse_run_container(reader, ctx) {
                            if !run.text.is_empty() || !run.pictures.is_empty() {
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
    para
}

/// 解析 `w:pPr`(段落属性):`w:pStyle`、`w:jc`、`w:numPr>w:ilvl`。已消费 `<w:pPr>` 起始标签。
fn parse_ppr<R: std::io::BufRead>(reader: &mut Reader<R>, para: &mut Paragraph) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"pStyle" => para.style = attr_of(&e, b"val").or(para.style.take()),
                    b"jc" => para.align = attr_of(&e, b"val").or(para.align.take()),
                    b"ilvl" => {
                        para.list_level = attr_of(&e, b"val").and_then(|s| s.parse().ok());
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

// ============================================================ run (w:r)

/// 解析一个可能含若干 `w:r` 的容器(如 `w:hyperlink` / `w:ins`)。已消费容器起始标签。
/// 嵌套的 `w:hyperlink` / `w:ins`(接受修订)递归展开其 run;其余(含 `w:del`)整体跳过。
fn parse_run_container<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<TextRun> {
    let mut runs = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"r" => runs.push(parse_run(reader, ctx)),
                    b"hyperlink" | b"ins" => runs.extend(parse_run_container(reader, ctx)),
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

/// 解析 `w:r`(文本 run):`w:rPr`(字体/字号/粗斜/下划线/颜色)+ `w:t`(文字)+ `w:tab`/`w:br`
/// (规整为空白/换行)+ `w:drawing`/`w:pict`(内嵌图片)。已消费 `<w:r>` 起始标签。
fn parse_run<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> TextRun {
    let mut run = TextRun::default();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                match name.as_slice() {
                    b"rPr" => parse_rpr(reader, &mut run),
                    b"t" => run.text.push_str(&read_text(reader)),
                    b"tab" => {
                        run.text.push('\t');
                        skip_element(reader);
                    }
                    b"br" | b"cr" => {
                        run.text.push('\n');
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
                    b"tab" => run.text.push('\t'),
                    b"br" | b"cr" => run.text.push('\n'),
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
                    b"p" => cell
                        .blocks
                        .push(Block::Paragraph(parse_paragraph(reader, ctx))),
                    b"tbl" => cell.blocks.push(Block::Table(parse_table(reader, ctx))),
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
