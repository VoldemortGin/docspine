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
//!   - `w:vMerge`         —— 纵向合并(`restart` 起始 / `continue` **或省略 val** 延续)。
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
    AnchorRef, Block, BreakKind, Cell, CellVAlign, Color, HeightRule, Orientation, Paragraph,
    Picture, Placement, Row, RunSegment, Section, Table, TableWidth, TextRun, VMerge,
};
use doc_core::style::{ColorRef, FontRef, Justification, RunProps};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{
    attr_of, attr_string, local_name, media_name_from_target, on_off_val, parse_rels, props,
    skip_element, Relationship,
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
            Ok(Event::Empty(e)) => match local_name(e.name().as_ref()) {
                b"sectPr" => trailing = Some(Section::default()),
                // 自闭合 <w:p/>:空段落(Word 对空段的常见写法),占一个块(渲染占一行)。
                b"p" => blocks.push(Block::Paragraph(Paragraph::default())),
                _ => {}
            },
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
            Ok(Event::Empty(e)) => {
                // 自闭合 <w:p/>:空段落照收(占一行)。
                if local_name(e.name().as_ref()) == b"p" {
                    blocks.push(Block::Paragraph(Paragraph::default()));
                }
            }
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
                    // 超链接 `w:hyperlink`:run 容器,展开其中 run 并盖上链接目标
                    // (外链 URI / 内部书签 "#anchor";§3j)。
                    b"hyperlink" => {
                        let link = hyperlink_target(&e, ctx);
                        for mut run in parse_run_container(reader, ctx) {
                            if run.link_target.is_none() {
                                run.link_target = link.clone();
                            }
                            if !run.segments.is_empty() || !run.pictures.is_empty() {
                                para.runs.push(run);
                            }
                        }
                    }
                    // 修订插入 `w:ins` / 字段 `w:fldSimple`(缓存的字段结果)也是 run
                    // 容器:展开其中的 run。`w:ins`(修订插入)按“接受修订”语义保留正文。
                    b"ins" | b"fldSimple" => {
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
/// 作为返回值交给段落层),以及经共享的 [`props::apply_ppr_prop`] 写进 `para.ppr` 的
/// 直接格式化片段(spacing / ind / pBdr / shd / keep 系列,C-4)。已消费 `<w:pPr>` 起始标签。
///
/// 与 [`parse_rpr`] 同构地做**深度计数**:嵌套容器(如 `w:numPr`)的结束标签不会再把
/// pPr 的遍历提前打断——修复过去 `w:numPr` / `w:sectPr` 之后的属性(如 `w:jc`)丢失、
/// 甚至整段后续正文被截断的内容丢失缺陷。两个例外子树与共享解析器一致:`w:pBdr` 走
/// 专用子 walker;pPr 内嵌的 `w:rPr`(段落标记符属性,内含同名异义元素)整体跳过。
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
                    props::apply_ppr_prop(&e, &mut para.ppr);
                }
            }
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                // 子 walker 消费整个 sectPr 子树,深度不受影响。
                b"sectPr" => sect = Some(parse_sectpr(reader)),
                b"pBdr" => props::parse_pbdr(reader, &mut para.ppr),
                b"rPr" => skip_element(reader),
                _ => {
                    apply_ppr_prop(&e, para);
                    props::apply_ppr_prop(&e, &mut para.ppr);
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
    sect
}

/// 把一个 pPr 子元素(`Empty` 或 `Start`)的**便利字段**应用到段落上
/// (pStyle / 原样 jc / ilvl;归一化属性走共享的 [`props::apply_ppr_prop`])。
fn apply_ppr_prop(e: &BytesStart, para: &mut Paragraph) {
    match local_name(e.name().as_ref()) {
        b"pStyle" => para.style = attr_of(e, b"val").or(para.style.take()),
        b"jc" => para.align = attr_of(e, b"val").or(para.align.take()),
        b"ilvl" => {
            para.list_level = attr_of(e, b"val").and_then(|s| s.parse().ok());
        }
        b"numId" => {
            para.num_id = attr_of(e, b"val").and_then(|s| s.parse().ok());
        }
        _ => {}
    }
}

// ============================================================ run (w:r)

/// 求一个 `w:hyperlink` 的链接目标:优先外链(`r:id` 经 `word/_rels` 解出 `Target`
/// URI),否则文档内部书签跳转(`w:anchor` → `"#书签名"`);都无则 `None`。
fn hyperlink_target(e: &BytesStart, ctx: &Ctx) -> Option<String> {
    // 外链:`r:id`(本地名 "id")→ rels 的 Target(External 关系即目标 URL)。
    if let Some(rid) = attr_of(e, b"id") {
        if let Some(rel) = ctx.rels.get(&rid) {
            if !rel.target.is_empty() {
                return Some(rel.target.clone());
            }
        }
    }
    // 内部锚点:`w:anchor` → "#书签名"(渲染侧只存不画 + 一次性降级告警)。
    match attr_of(e, b"anchor") {
        Some(anchor) if !anchor.is_empty() => Some(format!("#{anchor}")),
        _ => None,
    }
}

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
                    b"hyperlink" => {
                        let link = hyperlink_target(&e, ctx);
                        for mut run in parse_run_container(reader, ctx) {
                            if run.link_target.is_none() {
                                run.link_target = link.clone();
                            }
                            runs.push(run);
                        }
                    }
                    b"ins" | b"fldSimple" => runs.extend(parse_run_container(reader, ctx)),
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
                    b"rPr" => apply_direct_rpr(&mut run, props::parse_rpr(reader)),
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

/// 把直接格式化的 rPr 片段(经共享的 [`props::parse_rpr`])装上 run:原始片段存进
/// `run.rpr`(供有效样式解析器,能区分「未设置」与「显式关」),同时折叠出历史便利字段
/// (`font`/`size_pt`/`bold`/…,缺省 = false/None)——折叠语义与旧 walker 逐字节一致,
/// 导出/Python 契约不变。
fn apply_direct_rpr(run: &mut TextRun, rpr: RunProps) {
    run.font = named_font(&rpr.fonts.ascii)
        .or_else(|| named_font(&rpr.fonts.h_ansi))
        .or_else(|| named_font(&rpr.fonts.cs));
    run.size_pt = rpr.sz;
    run.bold = rpr.b == Some(true);
    run.italic = rpr.i == Some(true);
    run.underline = matches!(rpr.u, Some(k) if k.is_on());
    run.color = match rpr.color {
        Some(ColorRef::Rgb(c)) => Some(c),
        _ => None, // auto / theme 引用:便利字段维持 None(有效色走解析器)。
    };
    run.rpr = rpr;
}

/// 便利字段只认显名字体(theme 间接引用旧 walker 本就不读,行为保持)。
fn named_font(font: &Option<FontRef>) -> Option<String> {
    match font {
        Some(FontRef::Named(name)) => Some(name.clone()),
        _ => None,
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

/// 解析 `w:tblPr`(表格属性,C-7):`w:tblStyle`、`w:tblBorders`、`w:tblCellMar`、
/// `w:tblInd`、`w:tblW`、`w:jc`。已消费 `<w:tblPr>` 起始标签。边框/边距子树走
/// 专用子 walker(其子元素名 top/left/… 会与别的属性撞车);其余以深度计数兜底。
fn parse_tblpr<R: std::io::BufRead>(reader: &mut Reader<R>, table: &mut Table) {
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_tblpr_prop(&e, table),
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"tblBorders" => table.borders = props::parse_tbl_borders(reader),
                b"tblCellMar" => table.cell_margins = props::parse_cell_margins(reader),
                _ => {
                    apply_tblpr_prop(&e, table);
                    depth += 1;
                }
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
}

/// 把一个 tblPr 子元素的属性应用到 [`Table`] 上(边框/边距子树除外)。
fn apply_tblpr_prop(e: &BytesStart, table: &mut Table) {
    match local_name(e.name().as_ref()) {
        b"tblStyle" => table.style = attr_of(e, b"val").or(table.style.take()),
        b"tblInd" => {
            // 仅 type="dxa"(缺省按 dxa)记表格缩进。
            let is_dxa = attr_of(e, b"type")
                .map(|t| t.eq_ignore_ascii_case("dxa"))
                .unwrap_or(true);
            if is_dxa {
                table.indent = attr_of(e, b"w")
                    .and_then(|s| s.parse().ok())
                    .or(table.indent);
            }
        }
        b"tblW" => table.width = parse_measure(e).or(table.width),
        b"jc" => {
            if let Some(j) = attr_of(e, b"val").and_then(|s| Justification::from_attr(&s)) {
                table.jc = Some(j);
            }
        }
        _ => {}
    }
}

/// 解析一个 CT_TblWidth 度量(`@w:w` + `@w:type`):`dxa` → 绝对 twip;`pct` →
/// 百分比(原始值 1/50 个百分点,兼容 `"50%"` 后缀形);`auto`/其它 → `None`。
fn parse_measure(e: &BytesStart) -> Option<TableWidth> {
    let w = attr_of(e, b"w")?;
    match attr_of(e, b"type").as_deref() {
        Some("pct") => {
            let pct = match w.strip_suffix('%') {
                Some(p) => p.trim().parse::<f32>().ok()?,
                None => w.parse::<f32>().ok()? / 50.0,
            };
            Some(TableWidth::Pct(pct))
        }
        Some("dxa") | None => w.parse().ok().map(TableWidth::Dxa),
        _ => None, // "auto" / "nil" 等:不定宽。
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

/// 解析 `w:trPr`(行属性,C-7):`w:trHeight@w:val/@w:hRule`、`w:tblHeader`、
/// `w:cantSplit`。已消费 `<w:trPr>` 起始标签。深度计数兜底嵌套子树。
fn parse_trpr<R: std::io::BufRead>(reader: &mut Reader<R>, row: &mut Row) {
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_trpr_prop(&e, row),
            Ok(Event::Start(e)) => {
                apply_trpr_prop(&e, row);
                depth += 1;
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // trPr 自身结束。
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

/// 把一个 trPr 子元素的属性应用到 [`Row`] 上。
fn apply_trpr_prop(e: &BytesStart, row: &mut Row) {
    match local_name(e.name().as_ref()) {
        b"trHeight" => {
            row.height = attr_of(e, b"val")
                .and_then(|s| s.parse().ok())
                .or(row.height);
            if let Some(r) = attr_of(e, b"hRule") {
                row.height_rule = HeightRule::from_attr(&r);
            }
        }
        b"tblHeader" => row.is_header = on_off_val(e),
        b"cantSplit" => row.cant_split = on_off_val(e),
        _ => {}
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
            Ok(Event::Empty(e)) => {
                // 自闭合 <w:p/>:空段落照收(单元格常见,渲染占一行)。
                if local_name(e.name().as_ref()) == b"p" {
                    cell.blocks.push(Block::Paragraph(Paragraph::default()));
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    cell
}

/// 解析 `w:tcPr`(单元格属性,C-7):`w:gridSpan`(横向合并)、`w:vMerge`(纵向
/// 合并)、`w:tcW`(dxa 绝对宽 / pct 百分比宽)、`w:shd`(填充)、`w:tcBorders`、
/// `w:vAlign`、`w:tcMar`。已消费 `<w:tcPr>` 起始标签。边框/边距子树走专用子
/// walker(其子元素名 top/left/… 会与别的属性撞车);其余以深度计数兜底。
fn parse_tcpr<R: std::io::BufRead>(reader: &mut Reader<R>, cell: &mut Cell) {
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => apply_tcpr_prop(&e, cell),
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"tcBorders" => cell.borders = props::parse_tc_borders(reader),
                b"tcMar" => cell.margins = props::parse_cell_margins(reader),
                _ => {
                    apply_tcpr_prop(&e, cell);
                    depth += 1;
                }
            },
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // tcPr 自身结束。
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

/// 把一个 tcPr 子元素的属性应用到 [`Cell`] 上(边框/边距子树除外)。
fn apply_tcpr_prop(e: &BytesStart, cell: &mut Cell) {
    match local_name(e.name().as_ref()) {
        b"gridSpan" => {
            cell.grid_span = attr_of(e, b"val").and_then(|s| s.parse().ok()).unwrap_or(1);
        }
        b"vMerge" => {
            // val="restart" 起始;val="continue" **或省略 val** 是延续
            // (ECMA-376 §17.4.85:缺省 continue——Word 写延续格就是裸 <w:vMerge/>)。
            cell.v_merge = match attr_of(e, b"val") {
                Some(v) if v.eq_ignore_ascii_case("restart") => VMerge::Restart,
                _ => VMerge::Continue,
            };
        }
        b"tcW" => match attr_of(e, b"type").as_deref() {
            // pct:原始值 1/50 个百分点(兼容 "50%" 后缀形),对正文宽在渲染侧解析。
            Some("pct") => {
                if let Some(w) = attr_of(e, b"w") {
                    cell.width_pct = match w.strip_suffix('%') {
                        Some(p) => p.trim().parse().ok(),
                        None => w.parse::<f32>().ok().map(|v| v / 50.0),
                    };
                }
            }
            // dxa(缺省按 dxa):绝对 twip。"auto"/其它不记。
            Some("dxa") | None => {
                cell.width = attr_of(e, b"w").and_then(|s| s.parse().ok()).or(cell.width);
            }
            _ => {}
        },
        b"shd" => {
            cell.fill = attr_of(e, b"fill")
                .and_then(|h| Color::from_hex(&h))
                .or(cell.fill.take());
        }
        b"vAlign" => {
            if let Some(v) = attr_of(e, b"val").and_then(|s| CellVAlign::from_attr(&s)) {
                cell.v_align = Some(v);
            }
        }
        _ => {}
    }
}

// ============================================================ 图片 (w:drawing / w:pict)

/// 解析 `w:drawing`(DrawingML 内嵌/浮动图片):取 `wp:extent@cx/cy` +
/// `a:blip@r:embed`,并区分 `wp:inline` / `wp:anchor`(C-8:锚定的取
/// `wp:positionH/V@relativeFrom` + `wp:posOffset` 偏移与 `@behindDoc`)。
/// 已消费 `<w:drawing>` 起始标签,深度计数消费到其结束标签。无 blip 引用则返回 `None`。
fn parse_drawing<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Option<Picture> {
    let mut rel_id = String::new();
    let mut extent: Option<(Emu, Emu)> = None;
    let mut anchored = false;
    let mut behind = false;
    let (mut x, mut y): (Emu, Emu) = (0, 0);
    let mut rel_h = AnchorRef::default();
    let mut rel_v = AnchorRef::default();
    // 当前打开的定位轴:Some(true) = positionH、Some(false) = positionV,
    // 其内的 posOffset 文本归该轴。
    let mut axis: Option<bool> = None;
    let mut depth = 1usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                if name.as_slice() == b"posOffset" {
                    // 子 walker 消费整个 posOffset(文本 + 结束标签),深度不变。
                    if let (Some(h), Ok(v)) = (axis, read_text(reader).trim().parse::<Emu>()) {
                        if h {
                            x = v;
                        } else {
                            y = v;
                        }
                    }
                } else {
                    apply_drawing_elem(
                        &e,
                        &name,
                        (&mut rel_id, &mut extent),
                        (&mut anchored, &mut behind),
                        (&mut rel_h, &mut rel_v, &mut axis),
                    );
                    depth += 1;
                }
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                apply_drawing_elem(
                    &e,
                    &name,
                    (&mut rel_id, &mut extent),
                    (&mut anchored, &mut behind),
                    (&mut rel_h, &mut rel_v, &mut axis),
                );
                if matches!(name.as_slice(), b"positionH" | b"positionV") {
                    axis = None; // 自闭合定位轴:无 posOffset,偏移取 0。
                }
            }
            Ok(Event::End(e)) => {
                if matches!(local_name(e.name().as_ref()), b"positionH" | b"positionV") {
                    axis = None;
                }
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
    let mut pic = resolve_picture(ctx, rel_id, extent)?;
    if anchored {
        pic.placement = Placement::Anchored {
            x,
            y,
            rel_h,
            rel_v,
            behind,
        };
    }
    Some(pic)
}

/// 把 drawing 子元素的属性写进解析状态(posOffset 文本另在调用方处理)。
fn apply_drawing_elem(
    e: &BytesStart,
    name: &[u8],
    (rel_id, extent): (&mut String, &mut Option<(Emu, Emu)>),
    (anchored, behind): (&mut bool, &mut bool),
    (rel_h, rel_v, axis): (&mut AnchorRef, &mut AnchorRef, &mut Option<bool>),
) {
    match name {
        b"anchor" => {
            *anchored = true;
            if let Some(b) = attr_of(e, b"behindDoc") {
                *behind = !(b == "0" || b.eq_ignore_ascii_case("false"));
            }
        }
        b"extent" => {
            let cx: Emu = attr_of(e, b"cx").and_then(|s| s.parse().ok()).unwrap_or(0);
            let cy: Emu = attr_of(e, b"cy").and_then(|s| s.parse().ok()).unwrap_or(0);
            *extent = Some((cx, cy));
        }
        b"positionH" => {
            *axis = Some(true);
            if let Some(r) = attr_of(e, b"relativeFrom") {
                *rel_h = AnchorRef::from_attr(&r);
            }
        }
        b"positionV" => {
            *axis = Some(false);
            if let Some(r) = attr_of(e, b"relativeFrom") {
                *rel_v = AnchorRef::from_attr(&r);
            }
        }
        b"blip" => {
            // r:embed 属性(命名空间前缀按本地名匹配)。
            for attr in e.attributes().flatten() {
                if local_name(attr.key.as_ref()) == b"embed" {
                    *rel_id = attr_string(&attr);
                }
            }
        }
        _ => {}
    }
}

/// 解析旧式 VML `w:pict`(以及 `w:object` 内的 VML):取 `v:imagedata@r:id`;
/// 形状 `style` 属性里的 `width:`/`height:`(pt/px/in/cm/mm/pc)折算成 EMU 尺寸
/// (C-8;VML 无 wp:extent)。已消费起始标签。无引用则返回 `None`。
fn parse_vml_pict<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Option<Picture> {
    let mut rel_id = String::new();
    let mut extent: Option<(Emu, Emu)> = None;
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
                } else if extent.is_none() {
                    // v:shape / v:rect 等形状元素:style="width:36pt;height:24pt;…"。
                    if let Some(style) = attr_of(&e, b"style") {
                        extent = vml_style_extent(&style);
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
    resolve_picture(ctx, rel_id, extent)
}

/// 从 VML `style` 属性解析 `(width, height)` → EMU。两者都在才算(单边尺寸交
/// 渲染侧按位图固有尺寸兜底)。
fn vml_style_extent(style: &str) -> Option<(Emu, Emu)> {
    let mut w = None;
    let mut h = None;
    for decl in style.split(';') {
        let Some((key, value)) = decl.split_once(':') else {
            continue;
        };
        match key.trim() {
            "width" => w = css_length_emu(value),
            "height" => h = css_length_emu(value),
            _ => {}
        }
    }
    Some((w?, h?))
}

/// 把一个 CSS 风格长度(`36pt` / `48px` / `1in` / `2.54cm` / `25.4mm` / `3pc`;
/// 裸数字按 px)折算成 EMU。非正值 / 未知单位 → `None`。
fn css_length_emu(value: &str) -> Option<Emu> {
    let v = value.trim();
    let split = v
        .char_indices()
        .find(|(_, c)| !(c.is_ascii_digit() || *c == '.' || *c == '-' || *c == '+'))
        .map(|(i, _)| i)
        .unwrap_or(v.len());
    let n: f64 = v[..split].parse().ok()?;
    let pt = match v[split..].trim() {
        "pt" => n,
        "px" | "" => n * 0.75,
        "in" => n * 72.0,
        "cm" => n * 72.0 / 2.54,
        "mm" => n * 72.0 / 25.4,
        "pc" => n * 12.0,
        _ => return None,
    };
    if pt <= 0.0 || !pt.is_finite() {
        return None;
    }
    Some((pt * doc_core::geom::EMU_PER_POINT).round() as Emu)
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
        // 缺省行内;锚定浮动由调用方在解析 wp:anchor 后覆盖(C-8)。
        placement: Placement::Inline,
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
