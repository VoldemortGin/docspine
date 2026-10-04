//! 解析 `word/document.xml`(WordprocessingML 主文档)-> `Vec<Block>`。
//!
//! 走 `w:document` > `w:body`,在 body 这一级识别块级元素(顺序即文档顺序):
//! - `w:p`   —— 段落(内含带样式的 `w:r` run + 内嵌图片;`w:pPr > w:sectPr` 是节边界)
//! - `w:tbl` —— 表格(**本轮重点**)
//! - `w:sdt` —— 结构化文档标签,内容透明展开(封面/目录文字不丢)
//! - `w:customXml` —— 自定义 XML 标记,内容透明展开(块级与行内同理;表格里的行级 / 单元格级同理)
//! - `m:oMath` / `m:oMathPara` —— 公式,只抽 `m:t` 纯文本(见 [`parse_math`])
//! - `mc:AlternateContent` —— 取第一个产出非空内容的 `mc:Choice`,否则取 `mc:Fallback`
//!   (块级 / run 容器级 / run 内三层同一策略,见 [`parse_alternate_content`])
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
    AnchorRef, Block, BreakKind, Cell, CellVAlign, Color, Comment, HeaderFooterKind,
    HeaderFooterRef, HeightRule, NoteKind, Orientation, Paragraph, Picture, Placement, Row,
    RunSegment, Section, Table, TableWidth, TextBox, TextRun, VMerge, MAX_TABLE_COLS,
};
use doc_core::page_number::PageNumFormat;
use doc_core::style::{ColorRef, FontRef, Justification, RunProps};
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

use super::{
    attr_of, attr_string, is_prop_change, local_name, media_name_from_target, on_off_val,
    parse_rels, props, skip_element, Relationship,
};

/// 递归容器(`w:tbl` / 块级与行内 `w:sdt`·`w:customXml` / `w:hyperlink`·`w:ins`·`w:moveTo`·
/// `w:fldSimple`·`w:smartTag` / `mc:AlternateContent` / 文本框 `w:txbxContent`)的嵌套深度
/// 上限。恶意输入可以把表格套几千层,递归下降会栈溢出 abort;超限的子树整棵
/// [`skip_element`](迭代、不递归)静默跳过,与“未知元素跳过”的容错策略一致。
const MAX_NEST_DEPTH: u32 = 64;

/// 解析期上下文:rel 映射(图片 r:id -> media 名) + media 长度索引 + 嵌套深度计数 +
/// 复杂字段状态(`w:fldChar` 跨 run、跨段落,一个部件一份)。
struct Ctx<'a> {
    rels: &'a BTreeMap<String, Relationship>,
    media_index: &'a BTreeMap<String, usize>,
    depth: std::cell::Cell<u32>,
    fields: std::cell::RefCell<FieldStack>,
    /// 遇到的 `w:altChunk` 个数(内容不解析,只计数;见 `Document::alt_chunk_count`)。
    alt_chunks: std::cell::Cell<usize>,
}

/// 复杂字段(`w:fldChar` begin / separate / end)的嵌套栈。
#[derive(Default, Clone)]
struct FieldStack {
    frames: Vec<FieldFrame>,
    /// 仍处在指令区(未遇 `separate`)的帧数:为 0 且栈非空时当前位置是可见的字段结果。
    hidden: usize,
}

/// 一层复杂字段:指令文字(`w:instrText` 拼接)+ 是否已进入结果区。
#[derive(Clone)]
struct FieldFrame {
    instr: String,
    in_result: bool,
}

impl FieldStack {
    /// 当前位置若是(各层都已进入结果区的)可见字段结果,返回最内层字段的指令。
    fn visible_instr(&self) -> Option<String> {
        if self.hidden > 0 {
            return None;
        }
        self.frames.last().map(|f| f.instr.trim().to_string())
    }

    /// 当前位置是否可见(不在任何字段的指令区里)。
    fn is_visible(&self) -> bool {
        self.hidden == 0
    }
}

/// 一层递归容器的深度占位;离开作用域时深度减一。
struct DepthGuard<'c> {
    depth: &'c std::cell::Cell<u32>,
}

impl Drop for DepthGuard<'_> {
    fn drop(&mut self) {
        self.depth.set(self.depth.get() - 1);
    }
}

impl Ctx<'_> {
    /// 记一个 `w:altChunk`(外部内容块,不解析)。
    fn count_alt_chunk(&self) {
        self.alt_chunks.set(self.alt_chunks.get().saturating_add(1));
    }

    /// 进入一层递归容器。超过 [`MAX_NEST_DEPTH`] 时返回 `None`,调用方应
    /// [`skip_element`] 整体跳过该子树。
    fn enter(&self) -> Option<DepthGuard<'_>> {
        let d = self.depth.get() + 1;
        if d > MAX_NEST_DEPTH {
            return None;
        }
        self.depth.set(d);
        Some(DepthGuard { depth: &self.depth })
    }
}

/// 解析 `word/document.xml`。`rels_xml` 是主文档 `.rels` 文本(把图片 `r:embed/r:id`
/// 映射到 media 名);`media_index` 是 `裸文件名 -> 字节长度`,用于回填 `image_bytes_len`。
/// 返回 `(正文块序列, 节序列, altChunk 个数)`;节序列保证非空(无任何 `w:sectPr` 时补 Word 默认节)。
/// 递归容器嵌套超过 [`MAX_NEST_DEPTH`] 的子树被跳过。
pub fn parse(
    xml: &str,
    rels_xml: Option<&str>,
    media_index: &BTreeMap<String, usize>,
) -> (Vec<Block>, Vec<Section>, usize) {
    let rels = rels_xml.map(parse_rels).unwrap_or_default();
    let ctx = Ctx {
        rels: &rels,
        media_index,
        depth: std::cell::Cell::new(0),
        fields: Default::default(),
        alt_chunks: Default::default(),
    };
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();

    // 先定位到 w:body,再解析其直接子块。
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"body" {
                    let (blocks, sections) = parse_body(&mut reader, &ctx);
                    return (blocks, sections, ctx.alt_chunks.get());
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    (Vec::new(), vec![Section::default()], 0)
}

/// 解析页眉 / 页脚部件(`w:hdr` / `w:ftr`):根元素的直接子块,与正文同一套块级解析
/// (段落 / 表格 / 透明容器,共享 [`MAX_NEST_DEPTH`] 深度守卫)。`rels_xml` 是该部件自己的
/// `.rels`(图片 `r:embed`)。畸形 XML 按容错风格降级:返回已解析出的部分。
pub fn parse_hdr_ftr(
    xml: &str,
    rels_xml: Option<&str>,
    media_index: &BTreeMap<String, usize>,
) -> Vec<Block> {
    let rels = rels_xml.map(parse_rels).unwrap_or_default();
    let ctx = Ctx {
        rels: &rels,
        media_index,
        depth: std::cell::Cell::new(0),
        fields: Default::default(),
        alt_chunks: Default::default(),
    };
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    if !enter_root(&mut reader) {
        return Vec::new();
    }
    parse_block_container(&mut reader, &ctx)
}

/// 解析脚注 / 尾注部件(`w:footnotes` / `w:endnotes`):`note_tag`(`footnote` / `endnote`)
/// 子元素按 `w:id` 建表,内容走块级解析。`w:type` 为 `separator` / `continuationSeparator` /
/// `continuationNotice` 的非内容注、缺 / 非法 `w:id` 的注跳过;重复 id 以先出现者为准。
pub fn parse_notes(
    xml: &str,
    rels_xml: Option<&str>,
    media_index: &BTreeMap<String, usize>,
    note_tag: &[u8],
) -> BTreeMap<i64, Vec<Block>> {
    let rels = rels_xml.map(parse_rels).unwrap_or_default();
    let ctx = Ctx {
        rels: &rels,
        media_index,
        depth: std::cell::Cell::new(0),
        fields: Default::default(),
        alt_chunks: Default::default(),
    };
    let mut notes = BTreeMap::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    if !enter_root(&mut reader) {
        return notes;
    }
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == note_tag => {
                let id = attr_of(&e, b"id").and_then(|s| s.trim().parse::<i64>().ok());
                let is_content = !matches!(
                    attr_of(&e, b"type").as_deref(),
                    Some("separator" | "continuationSeparator" | "continuationNotice")
                );
                match id {
                    Some(id) if is_content => {
                        let blocks = parse_block_container(&mut reader, &ctx);
                        notes.entry(id).or_insert(blocks);
                    }
                    _ => skip_element(&mut reader),
                }
            }
            Ok(Event::Empty(e)) if local_name(e.name().as_ref()) == note_tag => {
                if let Some(id) = attr_of(&e, b"id").and_then(|s| s.trim().parse::<i64>().ok()) {
                    notes.entry(id).or_insert_with(Vec::new);
                }
            }
            Ok(Event::Start(_)) => skip_element(&mut reader),
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    notes
}

/// 解析批注部件(`w:comments`):`w:comment` 子元素按 `w:id` 建表,记录 `w:author` /
/// `w:date` / `w:initials`(缺失 `None`),内容走块级解析(共享 [`MAX_NEST_DEPTH`] 深度守卫)。
/// 缺 / 非法 `w:id` 的批注跳过;重复 id 以先出现者为准。畸形 XML 返回已解析出的部分。
pub fn parse_comments(
    xml: &str,
    rels_xml: Option<&str>,
    media_index: &BTreeMap<String, usize>,
) -> BTreeMap<i64, Comment> {
    let rels = rels_xml.map(parse_rels).unwrap_or_default();
    let ctx = Ctx {
        rels: &rels,
        media_index,
        depth: std::cell::Cell::new(0),
        fields: Default::default(),
        alt_chunks: Default::default(),
    };
    let mut comments = BTreeMap::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    if !enter_root(&mut reader) {
        return comments;
    }
    let head = |e: &BytesStart| {
        let id = attr_of(e, b"id").and_then(|s| s.trim().parse::<i64>().ok())?;
        Some(Comment {
            id,
            author: attr_of(e, b"author"),
            date: attr_of(e, b"date"),
            initials: attr_of(e, b"initials"),
            blocks: Vec::new(),
        })
    };
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) if local_name(e.name().as_ref()) == b"comment" => match head(&e) {
                Some(mut c) => {
                    c.blocks = parse_block_container(&mut reader, &ctx);
                    comments.entry(c.id).or_insert(c);
                }
                None => skip_element(&mut reader),
            },
            Ok(Event::Empty(e)) if local_name(e.name().as_ref()) == b"comment" => {
                if let Some(c) = head(&e) {
                    comments.entry(c.id).or_insert(c);
                }
            }
            Ok(Event::Start(_)) => skip_element(&mut reader),
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    comments
}

/// 读到部件根元素的起始标签并消费它;找不到(空 / 畸形)返回 `false`。
fn enter_root<R: std::io::BufRead>(reader: &mut Reader<R>) -> bool {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => return true,
            Ok(Event::Eof) | Err(_) => return false,
            _ => {}
        }
        buf.clear();
    }
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
                    b"tbl" => blocks.extend(parse_table(reader, ctx).map(Block::Table)),
                    // 结构化文档标签:内容在 w:sdtContent 里,透明展开(修复内容丢失)。
                    b"sdt" => blocks.extend(parse_sdt_blocks(reader, ctx)),
                    b"customXml" => blocks.extend(parse_custom_xml_blocks(reader, ctx)),
                    b"AlternateContent" => blocks.extend(parse_alt_content_blocks(reader, ctx)),
                    b"altChunk" => {
                        ctx.count_alt_chunk();
                        skip_element(reader);
                    }
                    // body 末尾的 sectPr:最后一节的页面几何。
                    b"sectPr" => trailing = Some(parse_sectpr(reader)),
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(e)) => match local_name(e.name().as_ref()) {
                b"sectPr" => trailing = Some(Section::default()),
                // 自闭合 <w:p/>:空段落(Word 对空段的常见写法),占一个块(渲染占一行)。
                b"p" => blocks.push(Block::Paragraph(Paragraph::default())),
                b"altChunk" => ctx.count_alt_chunk(),
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

/// 解析一个块容器(`w:sdtContent` / 块级 `w:customXml` / `mc:Choice`·`mc:Fallback` /
/// `w:txbxContent`)的直接子块,直到容器结束标签。假定 reader 已经消费了容器的起始标签。
/// 在这里 `w:p` -> 段落、`w:tbl` -> 表格、`w:sdt`·`w:customXml`·`mc:AlternateContent`
/// -> 透明展开。(段内 sectPr 在这些容器里不合法,忽略。)
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
                    b"tbl" => blocks.extend(parse_table(reader, ctx).map(Block::Table)),
                    b"sdt" => blocks.extend(parse_sdt_blocks(reader, ctx)),
                    b"customXml" => blocks.extend(parse_custom_xml_blocks(reader, ctx)),
                    b"AlternateContent" => blocks.extend(parse_alt_content_blocks(reader, ctx)),
                    b"altChunk" => {
                        ctx.count_alt_chunk();
                        skip_element(reader);
                    }
                    // 其它直接子元素(tcPr / customXmlPr 等)整体跳过。
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(e)) => {
                // 自闭合 <w:p/>:空段落照收(占一行)。
                match local_name(e.name().as_ref()) {
                    b"p" => blocks.push(Block::Paragraph(Paragraph::default())),
                    b"altChunk" => ctx.count_alt_chunk(),
                    _ => {}
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
/// `w:cols@w:num`(分栏数)、`w:pgNumType`(页码起始值 / 格式)。已消费 `<w:sectPr>` 起始标签。未知子元素跳过;
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
        b"titlePg" => sect.title_pg = on_off_val(e),
        // 页码设置:`w:start` 负数 / 非数字 / 超 u32 一律按缺失(接续上一节),`0` 合法。
        b"pgNumType" => {
            sect.page_number_start = attr_of(e, b"start").and_then(|s| s.parse().ok());
            sect.page_number_format = attr_of(e, b"fmt")
                .map(|f| PageNumFormat::from_attr(&f))
                .unwrap_or_default();
        }
        // 页眉 / 页脚引用:先只记 `r:id` + 类型,部件由 lib.rs 经 rels 解析并归一。
        name @ (b"headerReference" | b"footerReference") => {
            if let Some(rel_id) = attr_of(e, b"id").filter(|id| !id.is_empty()) {
                let kind = attr_of(e, b"type")
                    .map(|t| HeaderFooterKind::from_attr(&t))
                    .unwrap_or_default();
                let list = if name == b"headerReference" {
                    &mut sect.headers
                } else {
                    &mut sect.footers
                };
                list.push(HeaderFooterRef { kind, rel_id });
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
                        // 丢掉完全空白且无图片/文本框的 run,避免噪声;但保留带图片的空文字 run。
                        if has_content(&run) {
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
                            if has_content(&run) {
                                para.runs.push(run);
                            }
                        }
                    }
                    // 修订插入 `w:ins` / 修订移动目标 `w:moveTo` / 字段 `w:fldSimple`(缓存的
                    // 字段结果,run 盖上字段指令)/ 智能标记 `w:smartTag` / 行内 `w:customXml`
                    // 与双向文本 `w:dir` / `w:bdo` 也是 run 容器:展开其中的 run。`w:ins`·`w:moveTo` 按“接受修订”语义保留正文。
                    b"fldSimple" => {
                        para.runs.extend(
                            parse_fld_simple(reader, &e, ctx)
                                .into_iter()
                                .filter(has_content),
                        );
                    }
                    b"ins" | b"moveTo" | b"smartTag" | b"customXml" | b"dir" | b"bdo" => {
                        para.runs.extend(
                            parse_run_container(reader, ctx)
                                .into_iter()
                                .filter(has_content),
                        );
                    }
                    // 行内结构化文档标签:内容在 w:sdtContent 里,透明展开(修复内容丢失)。
                    b"sdt" => {
                        para.runs
                            .extend(parse_sdt_runs(reader, ctx).into_iter().filter(has_content));
                    }
                    // 公式 `m:oMath` / `m:oMathPara`:只抽 `m:t` 纯文本,不做排版。
                    b"oMath" | b"oMathPara" => para.runs.extend(parse_math(reader, &name)),
                    b"AlternateContent" => {
                        para.runs.extend(
                            parse_alt_content_runs(reader, ctx)
                                .into_iter()
                                .filter(has_content),
                        );
                    }
                    // 其余元素跳过。其中修订删除 `w:del` / 修订移动来源 `w:moveFrom`(其内
                    // run 用 `w:delText`)按“接受修订”语义整段丢弃、不输出文字,正好走这里
                    // 被 skip;`w:moveFromRangeStart` 等范围标记是空元素,本就无内容。
                    _ => skip_element(reader),
                }
            }
            // 自闭合 `w:fldSimple`:无缓存结果的字段,留一个带标记的空 run。
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"fldSimple" {
                    para.runs.extend(empty_field_run(&e, ctx));
                }
            }
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
                // 修订前的旧 pPr(w:pPrChange)整体跳过,不得覆盖现值。
                n if is_prop_change(n) => skip_element(reader),
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

/// 解析一个可能含若干 `w:r` 的容器(如 `w:hyperlink` / `w:ins` / `w:moveTo` / `w:fldSimple` /
/// `w:smartTag` / 行内 `w:customXml` / `mc:Choice`)。已消费容器起始标签。嵌套的同类容器、
/// 行内 `w:sdt` 与 `mc:AlternateContent` 递归展开其 run;其余(含 `w:del`·`w:moveFrom`、
/// `w:smartTagPr`·`w:customXmlPr` 属性外壳)整体跳过。
fn parse_run_container<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<TextRun> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return Vec::new();
    };
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
                    b"fldSimple" => runs.extend(parse_fld_simple(reader, &e, ctx)),
                    b"ins" | b"moveTo" | b"smartTag" | b"customXml" | b"dir" | b"bdo" => {
                        runs.extend(parse_run_container(reader, ctx));
                    }
                    b"sdt" => runs.extend(parse_sdt_runs(reader, ctx)),
                    b"oMath" | b"oMathPara" => runs.extend(parse_math(reader, &name)),
                    b"AlternateContent" => runs.extend(parse_alt_content_runs(reader, ctx)),
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(e)) => {
                if local_name(e.name().as_ref()) == b"fldSimple" {
                    runs.extend(empty_field_run(&e, ctx));
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    runs
}

/// 解析 `w:fldSimple`(已消费起始标签 `e`):展开其中的缓存结果 run,并给尚未带字段标记的
/// run 盖上 `@w:instr`(去首尾空白)。没有任何缓存结果时留一个带标记的空 run。
/// 位于外层复杂字段指令区里(不可见)时不盖标记。
fn parse_fld_simple<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    e: &BytesStart,
    ctx: &Ctx,
) -> Vec<TextRun> {
    let instr = fld_simple_instr(e, ctx);
    let mut runs = parse_run_container(reader, ctx);
    if let Some(instr) = instr {
        if !runs.iter().any(has_content) {
            runs.push(TextRun {
                field: Some(instr),
                ..TextRun::default()
            });
        } else {
            for run in &mut runs {
                if run.field.is_none() && has_content(run) {
                    run.field = Some(instr.clone());
                }
            }
        }
    }
    runs
}

/// 自闭合 `w:fldSimple`:无缓存结果,留一个只带字段标记的空 run(指令为空 / 不可见则无)。
fn empty_field_run(e: &BytesStart, ctx: &Ctx) -> Option<TextRun> {
    fld_simple_instr(e, ctx).map(|instr| TextRun {
        field: Some(instr),
        ..TextRun::default()
    })
}

/// `w:fldSimple@w:instr` 去首尾空白;空指令或位于不可见的字段指令区时 `None`。
fn fld_simple_instr(e: &BytesStart, ctx: &Ctx) -> Option<String> {
    if !ctx.fields.borrow().is_visible() {
        return None;
    }
    attr_of(e, b"instr")
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 复杂字段字符 `w:fldChar@w:fldCharType`:`begin` 入栈、`separate` 进入结果区、`end` 出栈。
/// 孤立的 `separate` / `end` 忽略。无结果区的字段在可见位置结束时,给当前 run 盖上该字段的
/// 指令(渲染侧据此在页眉页脚里现算 `PAGE` / `NUMPAGES`)。
fn field_char(e: &BytesStart, ctx: &Ctx, run: &mut TextRun) {
    let mut st = ctx.fields.borrow_mut();
    match attr_of(e, b"fldCharType").as_deref() {
        Some("begin") => {
            st.frames.push(FieldFrame {
                instr: String::new(),
                in_result: false,
            });
            st.hidden += 1;
        }
        Some("separate") => {
            if let Some(top) = st.frames.last_mut() {
                if !top.in_result {
                    top.in_result = true;
                    st.hidden -= 1;
                }
            }
        }
        Some("end") => {
            if let Some(top) = st.frames.pop() {
                if !top.in_result {
                    st.hidden -= 1;
                    let instr = top.instr.trim();
                    if st.is_visible() && !instr.is_empty() && run.field.is_none() {
                        run.field = Some(instr.to_string());
                    }
                }
            }
        }
        _ => {}
    }
}

/// 解析 `w:r`(文本 run):`w:rPr`(字体/字号/粗斜/下划线/颜色)+ 内容分段
/// (`w:t` -> `Text`、`w:tab`/`w:ptab` -> `Tab`、`w:br`/`w:cr` -> `Break`,`w:br@w:type`
/// 区分换行/换页/换栏;`w:sym` / `w:softHyphen` / `w:noBreakHyphen` -> 对应字符)+
/// `w:drawing`/`w:pict`(内嵌图片与浮动文本框)+ run 内 `mc:AlternateContent`。
/// 已消费 `<w:r>` 起始标签。字段指令 `w:instrText` 不进正文(只留缓存结果),只累积进
/// 复杂字段栈;处在可见字段结果里的 run 盖上该字段指令([`TextRun::field`])。
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
                    b"instrText" => {
                        let text = read_text(reader);
                        let mut st = ctx.fields.borrow_mut();
                        if let Some(top) = st.frames.last_mut().filter(|f| !f.in_result) {
                            top.instr.push_str(&text);
                        }
                    }
                    b"fldChar" => {
                        field_char(&e, ctx, &mut run);
                        skip_element(reader);
                    }
                    b"drawing" => {
                        if let Some(pic) = parse_drawing(reader, ctx, &mut run.text_boxes) {
                            run.pictures.push(pic);
                        }
                    }
                    b"pict" | b"object" => {
                        if let Some(pic) = parse_vml_pict(reader, ctx, &mut run.text_boxes) {
                            run.pictures.push(pic);
                        }
                    }
                    b"ruby" => parse_ruby(reader, ctx, &mut run),
                    // run 内的 AlternateContent(如 wps 形状 / VML 回退):选中分支的内容
                    // 并入本 run(分段 / 图片 / 文本框)。
                    b"AlternateContent" => {
                        if let Some(alt) =
                            parse_alternate_content(reader, ctx, parse_run, |r| !has_content(r))
                        {
                            run.segments.extend(alt.segments);
                            run.pictures.extend(alt.pictures);
                            run.text_boxes.extend(alt.text_boxes);
                        }
                    }
                    _ => {
                        // 自闭合惯用的内容元素(w:tab / w:br / w:sym …)偶见写成起止对。
                        push_run_char(&mut run, &e, &name);
                        skip_element(reader);
                    }
                }
            }
            Ok(Event::Empty(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                if name == b"fldChar" {
                    field_char(&e, ctx, &mut run);
                } else {
                    push_run_char(&mut run, &e, &name);
                }
            }
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    if run.field.is_none() && has_content(&run) {
        run.field = ctx.fields.borrow().visible_instr();
    }
    run
}

/// 解析 `w:ruby`(注音 / 拼音指南)。已消费起始标签。只取基字 `w:rubyBase` 里的 run 并入当前
/// run(分段 / 图片 / 文本框,按序);注音 `w:rt`(与 `w:rubyPr`)不进正文——避免“漢かん”式重复,
/// 模型里没有自然位置放它,不加字段。基字里的 run 经 [`parse_run_container`],深度受守卫约束。
fn parse_ruby<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx, run: &mut TextRun) {
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"rubyBase" {
                    for base in parse_run_container(reader, ctx) {
                        for seg in base.segments {
                            match seg {
                                RunSegment::Text(t) => run.push_text(&t),
                                other => run.segments.push(other),
                            }
                        }
                        run.pictures.extend(base.pictures);
                        run.text_boxes.extend(base.text_boxes);
                    }
                } else {
                    skip_element(reader);
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

/// 自闭合 run 内容元素 -> 分段:`w:tab`/`w:ptab` -> 制表、`w:br`/`w:cr` -> 断,
/// `w:footnoteReference` / `w:endnoteReference` -> 带 id 的注引用段,`w:commentReference` -> 批注引用段,
/// `w:sym` -> `@w:char` 十六进制码点(Symbol/Wingdings 等的 `U+F0xx` 私有区码点原样
/// 保留,不做字体映射)、`w:softHyphen` -> U+00AD、`w:noBreakHyphen` -> U+2011。
/// 其余元素忽略。
fn push_run_char(run: &mut TextRun, e: &BytesStart, name: &[u8]) {
    match name {
        b"tab" | b"ptab" => run.segments.push(RunSegment::Tab),
        b"br" => run.segments.push(RunSegment::Break(break_kind(e))),
        b"cr" => run.segments.push(RunSegment::Break(BreakKind::Line)),
        b"sym" => {
            if let Some(c) = attr_of(e, b"char")
                .and_then(|h| u32::from_str_radix(h.trim(), 16).ok())
                .and_then(char::from_u32)
            {
                run.push_text(c.encode_utf8(&mut [0; 4]));
            }
        }
        // 脚注 / 尾注引用:在 run 序列里留一个带 id 的定位点(无 / 非法 id 忽略)。
        b"footnoteReference" | b"endnoteReference" => {
            if let Some(id) = attr_of(e, b"id").and_then(|s| s.trim().parse().ok()) {
                let kind = if name == b"footnoteReference" {
                    NoteKind::Footnote
                } else {
                    NoteKind::Endnote
                };
                run.segments.push(RunSegment::NoteRef { kind, id });
            }
        }
        // 批注引用:留一个带 id 的定位点(无 / 非法 id 忽略)。
        b"commentReference" => {
            if let Some(id) = attr_of(e, b"id").and_then(|s| s.trim().parse().ok()) {
                run.segments.push(RunSegment::CommentRef { id });
            }
        }
        b"softHyphen" => run.push_text("\u{00AD}"),
        b"noBreakHyphen" => run.push_text("\u{2011}"),
        _ => {}
    }
}

/// run 是否值得保留:有内容分段、图片、浮动文本框,或是无缓存结果字段的标记 run。
fn has_content(run: &TextRun) -> bool {
    !run.segments.is_empty()
        || !run.pictures.is_empty()
        || !run.text_boxes.is_empty()
        || run.field.is_some()
}

/// 读 `w:br@w:type` 的断种类:`page` 换页、`column` 换栏、其余(含缺省 `textWrapping`)换行。
fn break_kind(e: &BytesStart) -> BreakKind {
    match attr_of(e, b"type") {
        Some(t) if t.eq_ignore_ascii_case("page") => BreakKind::Page,
        Some(t) if t.eq_ignore_ascii_case("column") => BreakKind::Column,
        _ => BreakKind::Line,
    }
}

// ============================================================ 公式 (m:oMath)

/// 公式里需要线性化的结构(其余结构保持文字纯拼接)。
#[derive(Clone, Copy, PartialEq)]
enum MathStruct {
    /// `m:f` 分式(`m:num` / `m:den`)-> `分子/分母`。
    Frac,
    /// `m:sSup` 上标(`m:e` / `m:sup`)-> `底^上`。
    Sup,
    /// `m:sSub` 下标(`m:e` / `m:sub`)-> `底_下`。
    Sub,
    /// `m:rad` 根号(`m:deg` / `m:e`)-> `sqrt(x)`;带次数时 `root(次,x)`。
    Rad,
}

/// 结构内的槽位元素。
#[derive(Clone, Copy, PartialEq)]
enum MathSlot {
    Num,
    Den,
    Base,
    Sup,
    Sub,
    Deg,
}

/// 公式遍历栈上的一帧:容器 / 结构 / 槽位各自攒一份文字。其余嵌套元素不开帧,只在帧内
/// 记 `other_depth`(外壳不挡住其内的结构),文字直接并入当前帧(保持纯拼接且不随深度反复复制)。
struct MathFrame {
    /// 本帧是哪种结构(`None` = 容器或槽位)。
    kind: Option<MathStruct>,
    /// 本帧若是槽位,它是哪个槽。
    slot: Option<MathSlot>,
    text: String,
    /// 本帧内 `m:t` 文字片段数(复合子项记 2),决定线性化时是否加括号。
    frags: usize,
    /// 结构帧:已收齐的槽位 `(槽, 文字, 片段数)`。
    slots: Vec<(MathSlot, String, usize)>,
    /// 帧内未开帧的嵌套元素深度。
    other_depth: usize,
}

impl MathFrame {
    fn new(kind: Option<MathStruct>, slot: Option<MathSlot>) -> Self {
        MathFrame {
            kind,
            slot,
            text: String::new(),
            frags: 0,
            slots: Vec::new(),
            other_depth: 0,
        }
    }
}

/// 结构元素本地名 -> 结构种类。
fn math_struct_of(name: &[u8]) -> Option<MathStruct> {
    match name {
        b"f" => Some(MathStruct::Frac),
        b"sSup" => Some(MathStruct::Sup),
        b"sSub" => Some(MathStruct::Sub),
        b"rad" => Some(MathStruct::Rad),
        _ => None,
    }
}

/// 槽位元素本地名 -> 在给定结构里的槽位(不属于该结构的名字不算槽位)。
fn math_slot_of(kind: MathStruct, name: &[u8]) -> Option<MathSlot> {
    match (kind, name) {
        (MathStruct::Frac, b"num") => Some(MathSlot::Num),
        (MathStruct::Frac, b"den") => Some(MathSlot::Den),
        (MathStruct::Sup, b"e") | (MathStruct::Sub, b"e") | (MathStruct::Rad, b"e") => {
            Some(MathSlot::Base)
        }
        (MathStruct::Sup, b"sup") => Some(MathSlot::Sup),
        (MathStruct::Sub, b"sub") => Some(MathSlot::Sub),
        (MathStruct::Rad, b"deg") => Some(MathSlot::Deg),
        _ => None,
    }
}

/// 多于一个文字片段(或复合子项)时用括号包起来。
fn math_wrap(text: &str, frags: usize) -> String {
    if frags > 1 {
        format!("({text})")
    } else {
        text.to_string()
    }
}

/// 把一个结构帧收拢成线性记法文本;所有槽位都为空时返回空串。
fn math_linearize(kind: MathStruct, slots: &[(MathSlot, String, usize)]) -> String {
    if slots.iter().all(|(_, t, _)| t.is_empty()) {
        return String::new();
    }
    let slot = |want: MathSlot| {
        slots
            .iter()
            .find(|(s, _, _)| *s == want)
            .map(|(_, t, n)| (t.as_str(), *n))
            .unwrap_or(("", 0))
    };
    let wrapped = |want: MathSlot| {
        let (t, n) = slot(want);
        math_wrap(t, n)
    };
    match kind {
        MathStruct::Frac => format!("{}/{}", wrapped(MathSlot::Num), wrapped(MathSlot::Den)),
        MathStruct::Sup => format!("{}^{}", wrapped(MathSlot::Base), wrapped(MathSlot::Sup)),
        MathStruct::Sub => format!("{}_{}", wrapped(MathSlot::Base), wrapped(MathSlot::Sub)),
        MathStruct::Rad => {
            let (base, _) = slot(MathSlot::Base);
            match slot(MathSlot::Deg) {
                ("", _) => format!("sqrt({base})"),
                (deg, _) => format!("root({deg},{base})"),
            }
        }
    }
}

/// 把结束的帧并入父帧:槽位 -> 父结构的槽表;结构 -> 线性化文字(作复合项,记 2 片段)。
fn math_merge(parent: &mut MathFrame, done: MathFrame) {
    if let (Some(slot), true) = (done.slot, parent.kind.is_some()) {
        parent.slots.push((slot, done.text, done.frags));
    } else if let Some(kind) = done.kind {
        let out = math_linearize(kind, &done.slots);
        if !out.is_empty() {
            parent.text.push_str(&out);
            parent.frags += 2;
        }
    }
}

/// 解析 `m:oMath` / `m:oMathPara`:按文档顺序抽取其中所有文字元素(`m:t`,亦含 run 内
/// 偶见的 `w:t`)拼成一个 `is_math` run。仅四种结构按最朴素的线性记法消歧,免得相邻
/// 数字被拼错(1/2 不再成 `12`、x² 不再成 `x2`):分式 `分子/分母`、上标 `x^2`、下标
/// `x_i`、根号 `sqrt(x)`(带次数 `root(3,x)`);某一项含多于一个文字片段(或本身是复合
/// 结构)时加括号,如 `(a+b)/c`。其它结构(定界符 / 求和 / 矩阵等)仍纯拼接,不做 LaTeX。
/// `m:oMathPara` 内多个 `m:oMath` 以空格分隔。修订删除 `w:del` / `w:moveFrom` 子树按
/// “接受修订”丢弃。已消费起始标签;**迭代**遍历(显式栈,不递归),结构帧深度受
/// [`MAX_NEST_DEPTH`] 约束(更深的结构退化为纯拼接),深嵌套不会栈溢出。
/// 抽不出文字时返回 `None`。
fn parse_math<R: std::io::BufRead>(reader: &mut Reader<R>, container: &[u8]) -> Option<TextRun> {
    let is_para = container == b"oMathPara";
    let mut stack = vec![MathFrame::new(None, None)];
    let mut struct_depth = 0u32;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                let name = name.as_slice();
                // 栈永不为空(根帧只在容器结束时才处理),下面的 last_mut 都成立。
                let Some(top) = stack.last_mut() else { break };
                match name {
                    b"del" | b"moveFrom" => skip_element(reader),
                    b"t" => {
                        let t = read_text(reader);
                        if !t.is_empty() {
                            top.text.push_str(&t);
                            top.frags += 1;
                        }
                    }
                    _ if top.other_depth == 0 && top.kind.is_some() => {
                        // 结构帧的直接子元素:认得的槽位开新帧,其余当普通嵌套。
                        match top.kind.and_then(|k| math_slot_of(k, name)) {
                            Some(slot) => stack.push(MathFrame::new(None, Some(slot))),
                            None => top.other_depth += 1,
                        }
                    }
                    // 容器 / 槽位帧(`kind` 为 `None`)里不论隔了几层透明外壳(`m:oMath` /
                    // `m:d` / `m:e` / `m:nary` / `m:func` 等)都认结构;外壳只记 `other_depth`。
                    _ if top.kind.is_none() && struct_depth < MAX_NEST_DEPTH => {
                        match math_struct_of(name) {
                            Some(kind) => {
                                struct_depth += 1;
                                stack.push(MathFrame::new(Some(kind), None));
                            }
                            None => {
                                if name == b"oMath" && is_para && !top.text.is_empty() {
                                    top.text.push(' ');
                                }
                                top.other_depth += 1;
                            }
                        }
                    }
                    _ => {
                        if name == b"oMath" && is_para && !top.text.is_empty() {
                            top.text.push(' ');
                        }
                        top.other_depth += 1;
                    }
                }
            }
            Ok(Event::End(_)) => {
                let Some(top) = stack.last_mut() else { break };
                if top.other_depth > 0 {
                    top.other_depth -= 1;
                } else if stack.len() == 1 {
                    break; // 容器自身结束。
                } else if let Some(done) = stack.pop() {
                    if done.kind.is_some() {
                        struct_depth -= 1;
                    }
                    if let Some(parent) = stack.last_mut() {
                        math_merge(parent, done);
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    // 畸形输入(Eof 时栈未回到根):自内向外把未闭合的帧并入父帧,文字不丢。
    while stack.len() > 1 {
        let Some(done) = stack.pop() else { break };
        if let Some(parent) = stack.last_mut() {
            math_merge(parent, done);
        }
    }
    let text = stack.pop().map(|f| f.text).unwrap_or_default();
    if text.is_empty() {
        return None;
    }
    let mut run = TextRun::from_text(&text);
    run.is_math = true;
    Some(run)
}

// ============================================================ 结构化文档标签 (w:sdt)

/// 解析**块级** `w:sdt`(结构化文档标签,如封面 / 目录容器):跳过 `w:sdtPr` / `w:sdtEndPr`
/// 外壳,把 `w:sdtContent` 里的块透明展开(嵌套 sdt 经 [`parse_block_container`] 递归)。
/// 已消费 `<w:sdt>` 起始标签。
fn parse_sdt_blocks<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Block> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return Vec::new();
    };
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
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return Vec::new();
    };
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

/// 解析**块级** `w:customXml`(自定义 XML 标记):跳过 `w:customXmlPr` 外壳,子块透明
/// 展开(可嵌套)。已消费 `<w:customXml>` 起始标签。
fn parse_custom_xml_blocks<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Block> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return Vec::new();
    };
    parse_block_container(reader, ctx)
}

/// 解析**行级**容器(`w:sdtContent` / 行级 `w:customXml`)的直接子节点:`w:tr` -> 行,
/// 嵌套 `w:sdt`·`w:customXml` 递归展开,其余(`w:customXmlPr` 等)跳过。已消费容器起始标签。
fn parse_row_container<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"tr" => rows.push(parse_table_row(reader, ctx)),
                b"sdt" => rows.extend(parse_sdt_rows(reader, ctx)),
                b"customXml" => rows.extend(parse_custom_xml_rows(reader, ctx)),
                _ => skip_element(reader),
            },
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    rows
}

/// 解析**单元格级**容器(`w:sdtContent` / 单元格级 `w:customXml`)的直接子节点:`w:tc`
/// -> 单元格,嵌套 `w:sdt`·`w:customXml` 递归展开,其余跳过。已消费容器起始标签。
fn parse_cell_container<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Cell> {
    let mut cells = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"tc" => cells.push(parse_table_cell(reader, ctx)),
                b"sdt" => cells.extend(parse_sdt_cells(reader, ctx)),
                b"customXml" => cells.extend(parse_custom_xml_cells(reader, ctx)),
                _ => skip_element(reader),
            },
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    cells
}

/// 解析 `w:sdt` 的外壳:跳过 `w:sdtPr` / `w:sdtEndPr`,把 `w:sdtContent` 交给 `parse`
/// 展开。受 [`MAX_NEST_DEPTH`] 约束,超限整棵跳过。已消费 `<w:sdt>` 起始标签。
fn parse_sdt_with<R: std::io::BufRead, T>(
    reader: &mut Reader<R>,
    ctx: &Ctx,
    parse: fn(&mut Reader<R>, &Ctx) -> Vec<T>,
) -> Vec<T> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"sdtContent" {
                    out.extend(parse(reader, ctx));
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
    out
}

/// **行级** `w:sdt`(`w:tbl > w:sdt > w:sdtContent > w:tr`):透明展开其中的行。
fn parse_sdt_rows<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Row> {
    parse_sdt_with(reader, ctx, parse_row_container)
}

/// **单元格级** `w:sdt`(`w:tr > w:sdt > w:sdtContent > w:tc`):透明展开其中的单元格。
fn parse_sdt_cells<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Cell> {
    parse_sdt_with(reader, ctx, parse_cell_container)
}

/// **行级** `w:customXml`:跳过 `w:customXmlPr` 外壳,子行透明展开(可嵌套)。
fn parse_custom_xml_rows<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Row> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return Vec::new();
    };
    parse_row_container(reader, ctx)
}

/// **单元格级** `w:customXml`:跳过 `w:customXmlPr` 外壳,子单元格透明展开(可嵌套)。
fn parse_custom_xml_cells<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Cell> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return Vec::new();
    };
    parse_cell_container(reader, ctx)
}

// ============================================================ 多选内容 (mc:AlternateContent)

/// 解析 `mc:AlternateContent`(Markup Compatibility,ECMA-376 Part 3)。已消费起始标签。
///
/// **选择策略(试解析)**:不按 `mc:Choice@Requires` 的命名空间前缀(`wps` / `w14` /
/// `w16se` …)查白名单,而是按文档顺序把每个 `mc:Choice` 的子内容交给 `parse`
/// 试解析,第一个 `is_empty` 判否(即本解析器认得其中元素、产出了内容)的分支即被
/// 选中;全部为空则取 `mc:Fallback` 的解析结果。流式 reader 无法回退,故各分支都
/// 顺序解析一遍、选中后其余丢弃——同一内容的 Choice/Fallback 两份绝不重复输出。
/// 复杂字段栈([`FieldStack`])同理只认选中分支:试解析落选的 Choice 推进过的字段状态在
/// 落选时回滚,否则它残留的 `begin` / `separate` 会把后续正文误标成字段(或卡在指令区)。
/// 三层调用方共用本策略:块级([`parse_block_container`])、run 容器级
/// ([`parse_run_container`])、run 内([`parse_run`])。
fn parse_alternate_content<R, T>(
    reader: &mut Reader<R>,
    ctx: &Ctx,
    parse: fn(&mut Reader<R>, &Ctx) -> T,
    is_empty: impl Fn(&T) -> bool,
) -> Option<T>
where
    R: std::io::BufRead,
{
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return None;
    };
    let mut chosen: Option<T> = None;
    let mut fallback: Option<T> = None;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                b"Choice" if chosen.is_none() => {
                    // 病态的超深字段栈不做快照(防止对抗输入下的二次方拷贝)。
                    let snapshot = Some(ctx.fields.borrow())
                        .filter(|st| st.frames.len() <= MAX_NEST_DEPTH as usize)
                        .map(|st| st.clone());
                    let v = parse(reader, ctx);
                    if !is_empty(&v) {
                        chosen = Some(v);
                    } else if let Some(snapshot) = snapshot {
                        *ctx.fields.borrow_mut() = snapshot;
                    }
                }
                b"Fallback" if chosen.is_none() && fallback.is_none() => {
                    fallback = Some(parse(reader, ctx));
                }
                _ => skip_element(reader),
            },
            Ok(Event::Empty(_)) => {}
            Ok(Event::End(_)) => break,
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    chosen.or(fallback)
}

/// 块级 `mc:AlternateContent` -> 选中分支的块序列(策略见 [`parse_alternate_content`])。
fn parse_alt_content_blocks<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<Block> {
    parse_alternate_content(reader, ctx, parse_block_container, |b: &Vec<Block>| {
        b.is_empty()
    })
    .unwrap_or_default()
}

/// 段落内 `mc:AlternateContent`(包着 `w:r` 等)-> 选中分支的 run 序列。
fn parse_alt_content_runs<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Vec<TextRun> {
    parse_alternate_content(reader, ctx, parse_run_container, |runs: &Vec<TextRun>| {
        !runs.iter().any(has_content)
    })
    .unwrap_or_default()
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

/// 解析 `w:tbl`(表格)。已消费 `<w:tbl>` 起始标签。嵌套超过 [`MAX_NEST_DEPTH`] 时整表
/// 跳过并返回 `None`。
fn parse_table<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Option<Table> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return None;
    };
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
                    b"sdt" => table.rows.extend(parse_sdt_rows(reader, ctx)),
                    b"customXml" => table.rows.extend(parse_custom_xml_rows(reader, ctx)),
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
    Some(table)
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
                // 修订前的旧 tblPr(w:tblPrChange)整体跳过,不得覆盖现值。
                n if is_prop_change(n) => skip_element(reader),
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
/// `w:tblGridChange`(修订前的旧网格,内嵌一份 `w:tblGrid`)整体跳过;展开写法
/// `<w:gridCol></w:gridCol>` 的结束标签也要随之消费,否则会被误当成 `tblGrid` 的结束。
fn parse_tbl_grid<R: std::io::BufRead>(reader: &mut Reader<R>) -> Vec<Twips> {
    let mut cols = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) => push_grid_col(&e, &mut cols),
            Ok(Event::Start(e)) => {
                push_grid_col(&e, &mut cols);
                skip_element(reader);
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

/// `w:gridCol` 记一列宽(其它元素忽略)。
fn push_grid_col(e: &BytesStart, cols: &mut Vec<Twips>) {
    // 超过 Word 列数上限的 gridCol 丢弃(与 `gridSpan` 同一上限)。
    if local_name(e.name().as_ref()) == b"gridCol" && cols.len() < MAX_TABLE_COLS {
        cols.push(attr_of(e, b"w").and_then(|s| s.parse().ok()).unwrap_or(0));
    }
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
                    b"sdt" => row.cells.extend(parse_sdt_cells(reader, ctx)),
                    b"customXml" => row.cells.extend(parse_custom_xml_cells(reader, ctx)),
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
            // 修订前的旧 trPr(w:trPrChange)整体跳过,不得覆盖现值。
            Ok(Event::Start(e)) if is_prop_change(local_name(e.name().as_ref())) => {
                skip_element(reader)
            }
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
                    b"tbl" => cell
                        .blocks
                        .extend(parse_table(reader, ctx).map(Block::Table)),
                    b"sdt" => cell.blocks.extend(parse_sdt_blocks(reader, ctx)),
                    b"customXml" => cell.blocks.extend(parse_custom_xml_blocks(reader, ctx)),
                    b"AlternateContent" => {
                        cell.blocks.extend(parse_alt_content_blocks(reader, ctx));
                    }
                    b"altChunk" => {
                        ctx.count_alt_chunk();
                        skip_element(reader);
                    }
                    _ => skip_element(reader),
                }
            }
            Ok(Event::Empty(e)) => {
                // 自闭合 <w:p/>:空段落照收(单元格常见,渲染占一行)。
                match local_name(e.name().as_ref()) {
                    b"p" => cell.blocks.push(Block::Paragraph(Paragraph::default())),
                    b"altChunk" => ctx.count_alt_chunk(),
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
                // 修订前的旧 tcPr(w:tcPrChange)整体跳过,不得覆盖现值。
                n if is_prop_change(n) => skip_element(reader),
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
            // 钳到 Word 的表格列数上限;非数字 / 负数按缺省 1,纯数字但溢出 u64 视作超大。
            cell.grid_span = attr_of(e, b"val").map_or(1, |s| {
                let s = s.trim();
                match s.parse::<u64>() {
                    Ok(n) => n.min(MAX_TABLE_COLS as u64) as u32,
                    Err(_) if !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()) => {
                        MAX_TABLE_COLS as u32
                    }
                    Err(_) => 1,
                }
            });
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
/// 形状里的文本框(`wps:txbx > w:txbxContent`,含组合形状内的)抽取进 `text_boxes`。
fn parse_drawing<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    ctx: &Ctx,
    text_boxes: &mut Vec<TextBox>,
) -> Option<Picture> {
    let mut rel_id = String::new();
    let mut extent: Option<(Emu, Emu)> = None;
    let mut alt = AltText::default();
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
                if name.as_slice() == b"txbxContent" {
                    // 子 walker 消费整个文本框内容,深度不变。
                    text_boxes.extend(parse_text_box(reader, ctx));
                } else if name.as_slice() == b"posOffset" {
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
                        (&mut rel_id, &mut extent, &mut alt),
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
                    (&mut rel_id, &mut extent, &mut alt),
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
    let mut pic = resolve_picture(ctx, rel_id, extent, alt)?;
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
    (rel_id, extent, alt): (&mut String, &mut Option<(Emu, Emu)>, &mut AltText),
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
        b"docPr" => alt.set(attr_of(e, b"descr"), attr_of(e, b"title")),
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
/// (C-8;VML 无 wp:extent)。`v:textbox > w:txbxContent` 抽取进 `text_boxes`。
/// 已消费起始标签,深度计数消费到其结束标签。无引用则返回 `None`。
fn parse_vml_pict<R: std::io::BufRead>(
    reader: &mut Reader<R>,
    ctx: &Ctx,
    text_boxes: &mut Vec<TextBox>,
) -> Option<Picture> {
    let mut rel_id = String::new();
    let mut extent: Option<(Emu, Emu)> = None;
    let mut alt = AltText::default();
    let mut depth = 0usize;
    let mut buf = Vec::new();
    loop {
        let event = reader.read_event_into(&mut buf);
        match event {
            Ok(Event::Start(ref e)) if local_name(e.name().as_ref()) == b"txbxContent" => {
                // 子 walker 消费整个文本框内容,深度不变。
                text_boxes.extend(parse_text_box(reader, ctx));
            }
            Ok(Event::Empty(ref e)) | Ok(Event::Start(ref e)) => {
                if matches!(event, Ok(Event::Start(_))) {
                    depth += 1;
                }
                if local_name(e.name().as_ref()) == b"imagedata" {
                    for attr in e.attributes().flatten() {
                        if local_name(attr.key.as_ref()) == b"id" {
                            rel_id = attr_string(&attr);
                        }
                    }
                    alt.set(None, attr_of(e, b"title"));
                } else if extent.is_none() {
                    // v:shape / v:rect 等形状元素:style="width:36pt;height:24pt;…"。
                    if let Some(style) = attr_of(e, b"style") {
                        extent = vml_style_extent(&style);
                    }
                }
                alt.set(attr_of(e, b"alt"), None);
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break; // w:pict / w:object 自身结束。
                }
                depth -= 1;
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    resolve_picture(ctx, rel_id, extent, alt)
}

/// 解析一个文本框内容 `w:txbxContent`(段落 / 表格块序列)。已消费起始标签。
/// 嵌套超过 [`MAX_NEST_DEPTH`] 时整体跳过并返回 `None`。
fn parse_text_box<R: std::io::BufRead>(reader: &mut Reader<R>, ctx: &Ctx) -> Option<TextBox> {
    let Some(_guard) = ctx.enter() else {
        skip_element(reader);
        return None;
    };
    Some(TextBox {
        blocks: parse_block_container(reader, ctx),
    })
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

/// 图片替代文字的收集器:`descr`(优先)与 `title`(兜底)各取首个非空值。
#[derive(Default)]
struct AltText {
    descr: Option<String>,
    title: Option<String>,
}

impl AltText {
    fn set(&mut self, descr: Option<String>, title: Option<String>) {
        let norm = |s: String| {
            let t = s.split_whitespace().collect::<Vec<_>>().join(" ");
            (!t.is_empty()).then_some(t)
        };
        if self.descr.is_none() {
            self.descr = descr.and_then(norm);
        }
        if self.title.is_none() {
            self.title = title.and_then(norm);
        }
    }

    fn finish(self) -> Option<String> {
        self.descr.or(self.title)
    }
}

/// 把图片的 rel id 经 rels 映射到 media 裸文件名,回填字节长度,组装 [`Picture`]。
/// rel id 为空(没找到引用)则返回 `None`。
fn resolve_picture(
    ctx: &Ctx,
    rel_id: String,
    extent: Option<(Emu, Emu)>,
    alt: AltText,
) -> Option<Picture> {
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
        alt: alt.finish(),
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
