//! 页眉 / 页脚渲染:每页按 [`Document::header_footer_for_page`] 选部件,部件块走正文同一条
//! 映射路径([`PartMapper`] → 引擎块),由引擎 `layout_text_box`(与正文共用段落 / 表格 /
//! 图片排版)排进页眉区 / 页脚区。
//!
//! - **位置**:页眉从 `w:pgMar@w:header`(距页顶)往下排;页脚底边落在 `@w:footer`(距页底)
//!   处、向上长;左右与正文同宽。
//! - **正文避让(先量后排)**:排正文前用引擎度量 API 量出本节可能用到的各部件高度;页眉底
//!   (页眉距 + 内容高)超过上边距时该页正文起点下推到页眉底,页脚同理上推正文底边。负的
//!   上 / 下边距是 Word 的「正文固定起点」语义,不避让。推完正文区不足下限时该页退回原边距 +
//!   一次 [`RenderWarning::HeaderFooterOverflow`]。
//! - **字段**:`PAGE` / `NUMPAGES` 现算——正文单遍排完即知总页数,再逐页排页眉页脚;部件高度
//!   按缓存结果量(页码位数变化不回流正文)。其余字段用缓存结果文字。
//! - 只有空段落的部件视同无页眉 / 页脚(不画、不占高、不告警)。

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use doc_core::geom::twips_to_points;
use doc_core::model::{Block as DocBlock, Document, HeaderFooterIndex, RunSegment, Section};
use doc_core::{format_page_number, PageNumFormat};
use pdf_typeset::{ImageSpec, Op, PageGeom, PageOps, Rect, TextBoxSpec, Typesetter, VAnchor};

use crate::map::PartMapper;
use crate::warn::RenderWarning;

#[cfg(test)]
thread_local! {
    /// 仅测试编译:部件被映射 + 量高的次数(`measure_section`)与被排版的次数(`part_ops`)。
    static MEASURES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static LAYOUTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 页眉页脚避让后正文区高度的下限(磅;原正文区更矮时取原高)。
const MIN_BODY_PT: f64 = 72.0;

/// 页面几何的比较键(各字段的位模式):同一部件在几何相同的节里映射 / 量高 / 排版的结果相同,
/// 缓存按它去重,而不是按节序号(继承来的同一部件不必每节重做)。
type GeomSig = [u64; 6];

fn geom_sig(g: &PageGeom) -> GeomSig {
    [
        g.width,
        g.height,
        g.margin_top,
        g.margin_right,
        g.margin_bottom,
        g.margin_left,
    ]
    .map(f64::to_bits)
}

/// 排版缓存键:`(部件键, 页面几何, 页眉 / 页脚距, 是否页脚)`——部件排版结果的全部输入。
type LayoutKey = (String, GeomSig, u64, bool);

/// 一次渲染的页眉页脚状态:部件高度(先量)、排版缓存与图片 id 归一(后画)。
pub(crate) struct HeaderFooters<'a> {
    doc: &'a Document,
    /// 每节有效页眉页脚引用的预计算(前缀传播一次,逐页查询 O(1))。
    index: HeaderFooterIndex<'a>,
    media: &'a BTreeMap<String, Vec<u8>>,
    mapper: PartMapper,
    /// 每节的基础几何(节序号为下标;未避让)。
    geoms: Vec<PageGeom>,
    /// `(部件键, 几何)` → 内容高(磅,按缓存结果量)。
    heights: BTreeMap<(String, GeomSig), f64>,
    /// 不含页码字段的部件:排版键 → 排好的 ops(跨页 / 跨节复用)。
    cache: BTreeMap<LayoutKey, Vec<Op>>,
    /// 排版键 → 首次排版的图片 id 序列(逐页重排时归一,PDF 只嵌一次)。
    image_ids: BTreeMap<LayoutKey, Vec<usize>>,
    warnings: Vec<RenderWarning>,
    overflow_warned: bool,
}

impl<'a> HeaderFooters<'a> {
    pub(crate) fn new(doc: &'a Document, media: &'a BTreeMap<String, Vec<u8>>) -> Self {
        HeaderFooters {
            doc,
            index: doc.header_footer_index(),
            media,
            mapper: PartMapper::new(),
            geoms: Vec::new(),
            heights: BTreeMap::new(),
            cache: BTreeMap::new(),
            image_ids: BTreeMap::new(),
            warnings: Vec::new(),
            overflow_warned: false,
        }
    }

    /// 记下第 `si` 节(须按节序逐个调用)的基础几何,并量出本节可能用到的各部件高度
    /// (首页 / 非首页 × 奇 / 偶页的全部组合)。
    pub(crate) fn measure_section(&mut self, ts: &mut Typesetter, si: usize, geom: PageGeom) {
        self.geoms.push(geom);
        let doc = self.doc;
        let mut keys = BTreeSet::new();
        // 页码奇偶两种、首页 / 非首页两种,共四种组合(与 `w:start` 取值无关)。
        for (k, n) in [(0, 1), (0, 2), (1, 1), (1, 2)] {
            let (h, f) = self.index.for_page(si, k, n);
            keys.extend(h.into_iter().chain(f));
        }
        let width = (geom.width - geom.margin_left - geom.margin_right).max(1.0);
        let sig = geom_sig(&geom);
        for key in keys {
            // 继承来的同一部件在几何相同的节里只量一次。
            if self.heights.contains_key(&(key.to_string(), sig)) {
                continue;
            }
            let Some(blocks) = doc.header_footers.get(key).filter(|b| has_content(b)) else {
                continue;
            };
            #[cfg(test)]
            MEASURES.with(|c| c.set(c.get() + 1));
            let (mapped, _) = self.mapper.map(doc, blocks, &geom, self.media);
            let height = ts.measure_blocks(&mapped, width, true).height;
            self.heights.insert((key.to_string(), sig), height);
        }
    }

    /// 第 `si` 节节内第 `k` 页(显示页码 `page_number`)的正文几何:按该页页眉 / 页脚的
    /// 内容高下推正文起点、上推正文底边。
    pub(crate) fn body_geom(&mut self, si: usize, k: usize, page_number: i64) -> PageGeom {
        let doc = self.doc;
        let base = self
            .geoms
            .get(si)
            .copied()
            .unwrap_or_else(|| crate::section::page_geom(&Section::default()));
        let Some(sect) = doc.sections.get(si) else {
            return base;
        };
        let (h, f) = self.index.for_page(si, k, page_number);
        let sig = geom_sig(&base);
        let height_of = |key: Option<&str>| {
            key.and_then(|key| self.heights.get(&(key.to_string(), sig)).copied())
        };
        let mut g = base;
        if let (true, Some(hh)) = (sect.margins.top >= 0, height_of(h)) {
            g.margin_top = g.margin_top.max(header_dist(sect) + hh);
        }
        if let (true, Some(fh)) = (sect.margins.bottom >= 0, height_of(f)) {
            g.margin_bottom = g.margin_bottom.max(footer_dist(sect) + fh);
        }
        let min_body = (base.height - base.margin_top - base.margin_bottom).min(MIN_BODY_PT);
        if g.height - g.margin_top - g.margin_bottom < min_body {
            if !self.overflow_warned {
                self.overflow_warned = true;
                self.warnings.push(RenderWarning::HeaderFooterOverflow);
            }
            return base;
        }
        g
    }

    /// 正文全部排完后逐页画页眉页脚(衬在该页其余内容之下)。`placement[i]` 是第 `i` 页的
    /// `(节序号, 节内页序号)`,`numbers[i]` 是其显示页码(`PAGE`、奇偶页判定用);总页数即
    /// `pages.len()`(`NUMPAGES`)。
    pub(crate) fn draw(
        &mut self,
        ts: &mut Typesetter,
        pages: &mut [PageOps],
        placement: &[(usize, usize)],
        numbers: &[i64],
    ) {
        let total = pages.len();
        for ((page, &(si, k)), &number) in pages.iter_mut().zip(placement).zip(numbers) {
            let (h, f) = self.index.for_page(si, k, number);
            let mut ops = Vec::new();
            for (key, footer) in [(h, false), (f, true)] {
                if let Some(key) = key {
                    ops.extend(self.part_ops(ts, si, key, footer, number, total));
                }
            }
            page.ops.splice(0..0, ops);
        }
    }

    /// 部件映射与引擎排版累积的降级告警。
    pub(crate) fn into_warnings(self) -> Vec<RenderWarning> {
        let mut out = self.warnings;
        out.extend(self.mapper.into_warnings());
        out
    }

    /// 一个部件在某页上的 ops(页坐标):含页码字段的逐页现算重排,否则排一次跨页复用。
    fn part_ops(
        &mut self,
        ts: &mut Typesetter,
        si: usize,
        key: &str,
        footer: bool,
        page_number: i64,
        total: usize,
    ) -> Vec<Op> {
        let doc = self.doc;
        let (Some(blocks), Some(&geom), Some(sect)) = (
            doc.header_footers.get(key).filter(|b| has_content(b)),
            self.geoms.get(si),
            doc.sections.get(si),
        ) else {
            return Vec::new();
        };
        let dist = if footer {
            footer_dist(sect)
        } else {
            header_dist(sect)
        };
        let cache_key: LayoutKey = (key.to_string(), geom_sig(&geom), dist.to_bits(), footer);
        let dynamic = uses_page_fields(blocks);
        if !dynamic {
            if let Some(ops) = self.cache.get(&cache_key) {
                return ops.clone();
            }
        }
        let substituted;
        let blocks = if dynamic {
            substituted = substitute_fields(blocks, page_number, total, sect.page_number_format);
            &substituted
        } else {
            blocks
        };
        let (mapped, overlays) = self.mapper.map(doc, blocks, &geom, self.media);
        let x0 = geom.margin_left;
        let x1 = (geom.width - geom.margin_right).max(x0 + 1.0);
        let mut spec = if footer {
            TextBoxSpec::new(
                Rect::new(x0, 0.0, x1, geom.height - footer_dist(sect)),
                mapped,
            )
        } else {
            TextBoxSpec::new(Rect::new(x0, header_dist(sect), x1, geom.height), mapped)
        };
        if footer {
            spec.v_anchor = VAnchor::Bottom;
        }
        #[cfg(test)]
        LAYOUTS.with(|c| c.set(c.get() + 1));
        let flow = ts.layout_text_box(&spec);
        // 部件里的锚定图:衬底的画在部件内容之前,其余之后。
        let mut ops = Vec::new();
        let mut front = Vec::new();
        for img in overlays {
            if let Some(id) = ts.add_image(&ImageSpec::new(img.bytes, img.w, img.h)) {
                let op = Op::Image {
                    id,
                    x: img.x,
                    y: img.y,
                    w: img.w,
                    h: img.h,
                };
                if img.behind {
                    ops.push(op);
                } else {
                    front.push(op);
                }
            }
        }
        ops.extend(flow);
        ops.extend(front);
        let ids = self.image_ids.entry(cache_key.clone()).or_default();
        canon_images(&mut ops, ids, &mut 0);
        if !dynamic {
            self.cache.insert(cache_key, ops.clone());
        }
        ops
    }
}

/// 页眉距页顶(磅;负值按 0)。
fn header_dist(sect: &Section) -> f64 {
    twips_to_points(sect.margins.header).max(0.0)
}

/// 页脚距页底(磅;负值按 0)。
fn footer_dist(sect: &Section) -> f64 {
    twips_to_points(sect.margins.footer).max(0.0)
}

/// 部件有实际内容(段落有 run / 含表格):只有空段落的部件视同不存在。
fn has_content(blocks: &[DocBlock]) -> bool {
    blocks.iter().any(|b| match b {
        DocBlock::Paragraph(p) => !p.runs.is_empty(),
        DocBlock::Table(_) => true,
    })
}

/// 字段指令里的页码格式开关 `\* roman|ROMAN|alphabetic|ALPHABETIC|Arabic`(大小写按 Word:
/// 罗马 / 字母的大小写决定输出大小写,`Arabic` 不分大小写);未知开关(`MERGEFORMAT` …)忽略,
/// 取第一个认得的。
fn switch_format(instr: &str) -> Option<PageNumFormat> {
    let mut tokens = instr.split_whitespace();
    while let Some(tok) = tokens.next() {
        let Some(rest) = tok.strip_prefix("\\*") else {
            continue;
        };
        let arg = if rest.is_empty() {
            tokens.next()?
        } else {
            rest
        };
        match arg {
            "roman" => return Some(PageNumFormat::LowerRoman),
            "ROMAN" => return Some(PageNumFormat::UpperRoman),
            "alphabetic" => return Some(PageNumFormat::LowerLetter),
            "ALPHABETIC" => return Some(PageNumFormat::UpperLetter),
            a if a.eq_ignore_ascii_case("arabic") => return Some(PageNumFormat::Decimal),
            _ => {}
        }
    }
    None
}

/// 字段指令 → 现算值:`PAGE` → 显示页码(格式:字段 `\*` 开关优先,否则本节 `sect_fmt`),
/// `NUMPAGES` → 物理总页数(恒为阿拉伯数字);其余字段 `None`(用缓存结果)。
fn field_value(
    instr: &str,
    page_number: i64,
    total: usize,
    sect_fmt: PageNumFormat,
) -> Option<String> {
    let name = instr.split_whitespace().next()?;
    if name.eq_ignore_ascii_case("PAGE") {
        let fmt = switch_format(instr).unwrap_or(sect_fmt);
        Some(format_page_number(page_number, fmt))
    } else if name.eq_ignore_ascii_case("NUMPAGES") {
        Some(total.to_string())
    } else {
        None
    }
}

/// 部件里是否有需要逐页现算的字段(含表格单元格内)。
fn uses_page_fields(blocks: &[DocBlock]) -> bool {
    blocks.iter().any(|b| match b {
        DocBlock::Paragraph(p) => p.runs.iter().any(|r| {
            r.field
                .as_deref()
                .is_some_and(|f| field_value(f, 0, 0, PageNumFormat::Decimal).is_some())
        }),
        DocBlock::Table(t) => t
            .rows
            .iter()
            .flat_map(|row| &row.cells)
            .any(|c| uses_page_fields(&c.blocks)),
    })
}

/// 克隆部件块并把 `PAGE` / `NUMPAGES` 字段结果换成现算值。同一字段结果跨相邻多个 run 时
/// 只在第一个 run 放值,其余清空。
fn substitute_fields(
    blocks: &[DocBlock],
    page_number: i64,
    total: usize,
    sect_fmt: PageNumFormat,
) -> Vec<DocBlock> {
    let mut out = blocks.to_vec();
    substitute_in(&mut out, page_number, total, sect_fmt);
    out
}

fn substitute_in(blocks: &mut [DocBlock], page_number: i64, total: usize, sect_fmt: PageNumFormat) {
    for block in blocks {
        match block {
            DocBlock::Paragraph(p) => {
                let mut prev: Option<Arc<str>> = None;
                for run in &mut p.runs {
                    let value = run
                        .field
                        .as_deref()
                        .and_then(|f| field_value(f, page_number, total, sect_fmt));
                    if let Some(v) = value {
                        if prev.is_some() && prev == run.field {
                            run.segments.clear();
                        } else {
                            run.segments = vec![RunSegment::Text(v)];
                        }
                    }
                    prev = run.field.clone();
                }
            }
            DocBlock::Table(t) => {
                for cell in t.rows.iter_mut().flat_map(|row| &mut row.cells) {
                    substitute_in(&mut cell.blocks, page_number, total, sect_fmt);
                }
            }
        }
    }
}

/// 把 ops 里第 n 张图的 id 换成该部件首次排版时第 n 张图的 id(首次则记录)。
fn canon_images(ops: &mut [Op], ids: &mut Vec<usize>, nth: &mut usize) {
    for op in ops {
        match op {
            Op::Image { id, .. } => {
                match ids.get(*nth) {
                    Some(&first) => *id = first,
                    None => ids.push(*id),
                }
                *nth += 1;
            }
            Op::Group { ops, .. } => canon_images(ops, ids, nth),
            _ => {}
        }
    }
}

// ============================================================ 单测:每页页眉页脚

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use doc_core::model::{
        Block as DocBlock, BreakKind, Cell, Document, HeaderFooterKind, HeaderFooterRef, Paragraph,
        Picture, Placement, Row, RunSegment, Section, Table, TextRun,
    };
    use doc_core::PageNumFormat;
    use pdf_typeset::{FontResolver, Op, PageOps, Typesetter};

    use super::{LAYOUTS, MEASURES};
    use crate::{layout_document, render_with, RenderOptions};

    /// 合法的 1×1 RGB PNG(引擎可解码)。
    const PNG_1X1: [u8; 69] = [
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0xC9, 0xFE, 0x92, 0xEF, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// 缺省节几何(Letter 612×792pt,四边 72pt,页眉 / 页脚距 36pt)。
    const PAGE_H: f64 = 792.0;

    fn deterministic() -> Typesetter {
        Typesetter::new(FontResolver::without_system_fonts())
    }

    fn para(text: &str) -> DocBlock {
        DocBlock::Paragraph(Paragraph {
            runs: vec![TextRun::from_text(text)],
            ..Paragraph::default()
        })
    }

    /// 一段:`n` 页正文(段内 `w:br@page` 切页),每页文字 `Body{i}`。
    fn pages_body(n: usize) -> DocBlock {
        let mut run = TextRun::default();
        for i in 1..=n {
            if i > 1 {
                run.segments.push(RunSegment::Break(BreakKind::Page));
            }
            run.segments.push(RunSegment::Text(format!("Body{i}")));
        }
        DocBlock::Paragraph(Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        })
    }

    fn r(kind: HeaderFooterKind, id: &str) -> HeaderFooterRef {
        HeaderFooterRef {
            kind,
            rel_id: id.to_string(),
        }
    }

    /// 单节文档 + 部件表。
    fn doc_with(
        body: Vec<DocBlock>,
        headers: Vec<HeaderFooterRef>,
        footers: Vec<HeaderFooterRef>,
        parts: Vec<(&str, Vec<DocBlock>)>,
    ) -> Document {
        let end = body.len();
        Document {
            body,
            sections: vec![Section {
                headers,
                footers,
                end_block: end,
                ..Section::default()
            }],
            header_footers: parts.into_iter().map(|(k, v)| (k.to_string(), v)).collect(),
            ..Document::default()
        }
    }

    fn layout(doc: &Document, media: &BTreeMap<String, Vec<u8>>) -> (Vec<PageOps>, Vec<String>) {
        let mut ts = deterministic();
        let (pages, warnings) = layout_document(&mut ts, doc, media);
        let kinds = warnings.iter().map(|w| w.kind().to_string()).collect();
        (pages, kinds)
    }

    /// 页上全部文字 op `(文字, 基线 y)`(递归进 Group)。
    fn texts(page: &PageOps) -> Vec<(String, f64)> {
        fn walk(ops: &[Op], out: &mut Vec<(String, f64)>) {
            for op in ops {
                match op {
                    Op::Text { text, baseline, .. } => out.push((text.clone(), *baseline)),
                    Op::Group { ops, .. } => walk(ops, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&page.ops, &mut out);
        out
    }

    /// 页上全部图片 op `(id, y)`(递归进 Group)。
    fn images(page: &PageOps) -> Vec<(usize, f64)> {
        fn walk(ops: &[Op], out: &mut Vec<(usize, f64)>) {
            for op in ops {
                match op {
                    Op::Image { id, y, .. } => out.push((*id, *y)),
                    Op::Group { ops, .. } => walk(ops, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&page.ops, &mut out);
        out
    }

    /// 页眉区(页顶到上边距 72pt)内的文字,去空白拼接。
    fn header_text(page: &PageOps) -> String {
        texts(page)
            .into_iter()
            .filter(|(_, y)| *y < 72.0)
            .map(|(t, _)| t)
            .collect::<String>()
            .split_whitespace()
            .collect()
    }

    /// 页脚区(下边距 72pt 到页底)内的文字,去空白拼接。
    fn footer_text(page: &PageOps) -> String {
        texts(page)
            .into_iter()
            .filter(|(_, y)| *y > PAGE_H - 72.0)
            .map(|(t, _)| t)
            .collect::<String>()
            .split_whitespace()
            .collect()
    }

    fn has_text(page: &PageOps, needle: &str) -> bool {
        texts(page).iter().any(|(t, _)| t.contains(needle))
    }

    /// 带字段标记的 run(缓存结果 `cached`)。
    fn field_run(instr: &str, cached: &str) -> TextRun {
        let mut run = TextRun::from_text(cached);
        if cached.is_empty() {
            run.segments.clear();
        }
        run.field = Some(instr.into());
        run
    }

    /// `Page {PAGE} of {NUMPAGES}` 页脚段落(PAGE 带过时缓存 "9",NUMPAGES 无缓存结果)。
    fn page_x_of_y() -> DocBlock {
        DocBlock::Paragraph(Paragraph {
            runs: vec![
                TextRun::from_text("Page "),
                field_run(r"PAGE \* MERGEFORMAT", "9"),
                TextRun::from_text(" of "),
                field_run("NUMPAGES", ""),
            ],
            ..Paragraph::default()
        })
    }

    #[test]
    fn single_section_every_page_has_header_and_footer() {
        let doc = doc_with(
            vec![pages_body(3)],
            vec![r(HeaderFooterKind::Default, "h")],
            vec![r(HeaderFooterKind::Default, "f")],
            vec![("h", vec![para("HEAD")]), ("f", vec![para("FOOT")])],
        );
        let (pages, warnings) = layout(&doc, &BTreeMap::new());
        assert_eq!(pages.len(), 3);
        for page in &pages {
            assert_eq!(header_text(page), "HEAD");
            assert_eq!(footer_text(page), "FOOT");
            // 页眉从页眉距(36pt)往下排,页脚底边落在页底上方 36pt 处。
            let (_, hy) = texts(page).into_iter().find(|(t, _)| t == "HEAD").unwrap();
            assert!(hy > 36.0 && hy < 72.0, "header baseline {hy}");
            let (_, fy) = texts(page).into_iter().find(|(t, _)| t == "FOOT").unwrap();
            assert!(
                fy > PAGE_H - 72.0 && fy < PAGE_H - 36.0,
                "footer baseline {fy}"
            );
        }
        assert!(
            !warnings.iter().any(|k| k.starts_with("header-footer")),
            "{warnings:?}"
        );
    }

    #[test]
    fn title_pg_gives_first_page_its_own_header() {
        let mut doc = doc_with(
            vec![pages_body(3)],
            vec![
                r(HeaderFooterKind::Default, "h"),
                r(HeaderFooterKind::First, "h1"),
            ],
            vec![r(HeaderFooterKind::First, "f1")],
            vec![
                ("h", vec![para("DEFH")]),
                ("h1", vec![para("FIRSTH")]),
                ("f1", vec![para("FIRSTF")]),
            ],
        );
        doc.sections[0].title_pg = true;
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let heads: Vec<String> = pages.iter().map(header_text).collect();
        assert_eq!(heads, ["FIRSTH", "DEFH", "DEFH"]);
        let foots: Vec<String> = pages.iter().map(footer_text).collect();
        assert_eq!(foots, ["FIRSTF", "", ""], "无 default 页脚:后续页无页脚");

        // titlePg 关:first 部件不用。
        doc.sections[0].title_pg = false;
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let heads: Vec<String> = pages.iter().map(header_text).collect();
        assert_eq!(heads, ["DEFH", "DEFH", "DEFH"]);
    }

    #[test]
    fn even_and_odd_headers_alternate() {
        let mut doc = doc_with(
            vec![pages_body(4)],
            vec![
                r(HeaderFooterKind::Default, "h"),
                r(HeaderFooterKind::Even, "he"),
            ],
            vec![],
            vec![("h", vec![para("ODDH")]), ("he", vec![para("EVENH")])],
        );
        doc.even_and_odd_headers = true;
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let heads: Vec<String> = pages.iter().map(header_text).collect();
        assert_eq!(heads, ["ODDH", "EVENH", "ODDH", "EVENH"]);

        doc.even_and_odd_headers = false;
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let heads: Vec<String> = pages.iter().map(header_text).collect();
        assert_eq!(heads, ["ODDH", "ODDH", "ODDH", "ODDH"]);
    }

    fn counts() -> (usize, usize) {
        (MEASURES.with(|c| c.get()), LAYOUTS.with(|c| c.get()))
    }

    /// `n` 个节,只有第 0 节引用页眉 `h`,其余全部继承;每节一段正文。`tweak(i, &mut Section)` 调整第 i 节。
    fn inherited_header_doc(n: usize, tweak: impl Fn(usize, &mut Section)) -> Document {
        let mut doc = Document {
            body: (0..n).map(|i| para(&format!("S{i}"))).collect(),
            sections: (0..n)
                .map(|i| {
                    let mut s = Section {
                        end_block: i + 1,
                        ..Section::default()
                    };
                    if i == 0 {
                        s.headers = vec![r(HeaderFooterKind::Default, "h")];
                    }
                    tweak(i, &mut s);
                    s
                })
                .collect(),
            ..Document::default()
        };
        doc.header_footers.insert("h".into(), vec![para("HEAD")]);
        doc
    }

    /// 继承来的同一页眉部件,在版面参数相同的各节之间只映射 / 量高 / 排版一次(按部件 + 几何缓存),
    /// 而不是每节重做;几何不同的节才各自重做。
    #[test]
    fn inherited_header_is_measured_and_laid_out_once_per_geometry() {
        let doc = inherited_header_doc(300, |_, _| {});
        MEASURES.with(|c| c.set(0));
        LAYOUTS.with(|c| c.set(0));
        let (pages, _) = layout(&doc, &BTreeMap::new());
        assert_eq!(pages.len(), 300);
        assert!(pages.iter().all(|p| header_text(p) == "HEAD"));
        assert_eq!(counts(), (1, 1), "(量高次数, 排版次数)");

        // 偶数节左页边距不同 -> 两种几何,各量一次、各排一次。
        let doc = inherited_header_doc(300, |i, s| {
            if i % 2 == 1 {
                s.margins.left = 2000;
            }
        });
        MEASURES.with(|c| c.set(0));
        LAYOUTS.with(|c| c.set(0));
        let (pages, _) = layout(&doc, &BTreeMap::new());
        assert!(pages.iter().all(|p| header_text(p) == "HEAD"));
        assert_eq!(counts(), (2, 2));
    }

    /// 两节各自的页眉;第三节无引用 → 继承第二节。
    #[test]
    fn sections_use_their_own_or_inherited_headers() {
        let mut doc = Document {
            body: vec![pages_body(2), para("Second"), para("Third")],
            sections: vec![
                Section {
                    headers: vec![r(HeaderFooterKind::Default, "h1")],
                    end_block: 1,
                    ..Section::default()
                },
                Section {
                    headers: vec![r(HeaderFooterKind::Default, "h2")],
                    end_block: 2,
                    ..Section::default()
                },
                Section {
                    end_block: 3,
                    ..Section::default()
                },
            ],
            ..Document::default()
        };
        doc.header_footers.insert("h1".into(), vec![para("SECONE")]);
        doc.header_footers.insert("h2".into(), vec![para("SECTWO")]);
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let heads: Vec<String> = pages.iter().map(header_text).collect();
        assert_eq!(heads, ["SECONE", "SECONE", "SECTWO", "SECTWO"]);
    }

    #[test]
    fn header_with_table_draws_cells_on_every_page() {
        let cell = |t: &str| Cell {
            blocks: vec![para(t)],
            grid_span: 1,
            ..Cell::default()
        };
        let table = DocBlock::Table(Table {
            grid_cols: vec![2000, 2000],
            rows: vec![Row {
                cells: vec![cell("LEFTCELL"), cell("RIGHTCELL")],
                ..Row::default()
            }],
            ..Table::default()
        });
        let doc = doc_with(
            vec![pages_body(2)],
            vec![r(HeaderFooterKind::Default, "h")],
            vec![],
            vec![("h", vec![table])],
        );
        let (pages, _) = layout(&doc, &BTreeMap::new());
        for page in &pages {
            let h = header_text(page);
            assert!(h.contains("LEFTCELL") && h.contains("RIGHTCELL"), "{h}");
        }
    }

    /// 页眉图片每页都画在页眉区;同一部件跨页复用同一个图片 id(PDF 只嵌一次)。
    #[test]
    fn header_with_image_draws_on_every_page_once_embedded() {
        let mut run = TextRun::default();
        run.pictures.push(Picture {
            rel_id: "rId1".into(),
            media_name: Some("logo.png".into()),
            extent: Some((228_600, 228_600)), // 18pt × 18pt
            image_bytes_len: PNG_1X1.len(),
            alt: None,
            placement: Placement::Inline,
        });
        let logo = DocBlock::Paragraph(Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        });
        let mut media = BTreeMap::new();
        media.insert("logo.png".to_string(), PNG_1X1.to_vec());
        // 带页码字段:逐页重排也不应重复嵌图。
        let doc = doc_with(
            vec![pages_body(3)],
            vec![r(HeaderFooterKind::Default, "h")],
            vec![],
            vec![("h", vec![logo, page_x_of_y()])],
        );
        let (pages, _) = layout(&doc, &media);
        let mut ids = Vec::new();
        for page in &pages {
            let imgs = images(page);
            assert_eq!(imgs.len(), 1, "每页一张页眉图");
            let (id, y) = imgs[0];
            assert!((36.0..72.0).contains(&y), "image top {y}");
            ids.push(id);
        }
        assert!(
            ids.iter().all(|&i| i == ids[0]),
            "图片 id 跨页复用: {ids:?}"
        );

        let res =
            render_with(deterministic(), &doc, &media, &RenderOptions::default()).expect("render");
        let xobjects = res
            .pdf
            .windows(b"/Subtype /Image".len())
            .filter(|w| *w == b"/Subtype /Image")
            .count();
        assert_eq!(xobjects, 1, "页眉图只嵌一次");
    }

    #[test]
    fn page_field_counts_up_and_numpages_is_total() {
        let doc = doc_with(
            vec![pages_body(3)],
            vec![],
            vec![r(HeaderFooterKind::Default, "f")],
            vec![("f", vec![page_x_of_y()])],
        );
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let foots: Vec<String> = pages.iter().map(footer_text).collect();
        assert_eq!(foots, ["Page1of3", "Page2of3", "Page3of3"]);
    }

    /// 正文里的 PAGE 字段不现算:照用缓存结果文字。
    #[test]
    fn body_fields_keep_cached_text() {
        let doc = doc_with(
            vec![DocBlock::Paragraph(Paragraph {
                runs: vec![field_run("PAGE", "77")],
                ..Paragraph::default()
            })],
            vec![],
            vec![],
            vec![],
        );
        let (pages, _) = layout(&doc, &BTreeMap::new());
        assert!(has_text(&pages[0], "77"));
    }

    /// 页眉内容高过上边距:正文起点按 Word 行为下推到页眉底之下。
    #[test]
    fn tall_header_pushes_body_down() {
        let lines: Vec<DocBlock> = (0..6).map(|i| para(&format!("H{i}"))).collect();
        let doc = doc_with(
            vec![para("BODYTOP")],
            vec![r(HeaderFooterKind::Default, "h")],
            vec![],
            vec![("h", lines)],
        );
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let all = texts(&pages[0]);
        let last_header = all
            .iter()
            .filter(|(t, _)| t.starts_with('H'))
            .map(|(_, y)| *y)
            .fold(0.0, f64::max);
        let body = all.iter().find(|(t, _)| t == "BODYTOP").unwrap().1;
        assert!(
            last_header > 72.0,
            "6 行页眉应超出 72pt 上边距: {last_header}"
        );
        assert!(
            body > last_header,
            "正文 {body} 应在页眉末行 {last_header} 之下"
        );
    }

    /// 页脚内容高过下边距:正文底边上推,正文文字不进入页脚区。
    #[test]
    fn tall_footer_pushes_body_bottom_up() {
        let lines: Vec<DocBlock> = (0..6).map(|i| para(&format!("F{i}"))).collect();
        let body: Vec<DocBlock> = (0..80).map(|i| para(&format!("B{i}"))).collect();
        let doc = doc_with(
            body,
            vec![],
            vec![r(HeaderFooterKind::Default, "f")],
            vec![("f", lines)],
        );
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let all = texts(&pages[0]);
        let first_footer = all
            .iter()
            .filter(|(t, _)| t.starts_with('F'))
            .map(|(_, y)| *y)
            .fold(f64::INFINITY, f64::min);
        let last_body = all
            .iter()
            .filter(|(t, _)| t.starts_with('B'))
            .map(|(_, y)| *y)
            .fold(0.0, f64::max);
        assert!(first_footer < PAGE_H - 72.0, "6 行页脚应超出下边距");
        assert!(
            last_body < first_footer,
            "正文末行 {last_body} 应在页脚首行 {first_footer} 之上"
        );
    }

    /// 超高页眉(比整页还高):不 panic,正文退回按原边距排,发一次溢出告警。
    #[test]
    fn oversized_header_does_not_panic_and_warns_once() {
        let lines: Vec<DocBlock> = (0..120).map(|i| para(&format!("H{i}"))).collect();
        let doc = doc_with(
            vec![pages_body(2)],
            vec![r(HeaderFooterKind::Default, "h")],
            vec![],
            vec![("h", lines)],
        );
        let (pages, warnings) = layout(&doc, &BTreeMap::new());
        assert_eq!(pages.len(), 2);
        assert!(has_text(&pages[0], "Body1") && has_text(&pages[1], "Body2"));
        let n = warnings
            .iter()
            .filter(|k| *k == "header-footer-overflow")
            .count();
        assert_eq!(n, 1, "{warnings:?}");
        let res = render_with(
            deterministic(),
            &doc,
            &BTreeMap::new(),
            &RenderOptions::default(),
        )
        .expect("render");
        assert!(res.pdf.starts_with(b"%PDF-"));
    }

    /// 只有空段落的页眉部件:不画(与无页眉逐 op 相同),也不告警。
    #[test]
    fn empty_header_part_draws_nothing_and_does_not_warn() {
        let with_empty = doc_with(
            vec![pages_body(2)],
            vec![r(HeaderFooterKind::Default, "h")],
            vec![r(HeaderFooterKind::Default, "f")],
            vec![
                ("h", vec![DocBlock::Paragraph(Paragraph::default())]),
                ("f", vec![]),
            ],
        );
        let without = doc_with(vec![pages_body(2)], vec![], vec![], vec![]);
        let (a, warnings) = layout(&with_empty, &BTreeMap::new());
        let (b, _) = layout(&without, &BTreeMap::new());
        assert_eq!(a, b);
        assert!(
            !warnings.iter().any(|k| k.starts_with("header-footer")),
            "{warnings:?}"
        );
    }

    /// 确定性:同输入两次渲染(页眉页脚 + 字段 + 图片)字节一致。
    #[test]
    fn rendering_with_headers_is_deterministic() {
        let mut media = BTreeMap::new();
        media.insert("logo.png".to_string(), PNG_1X1.to_vec());
        let mut run = TextRun::default();
        run.pictures.push(Picture {
            rel_id: "rId1".into(),
            media_name: Some("logo.png".into()),
            extent: Some((228_600, 228_600)),
            image_bytes_len: PNG_1X1.len(),
            alt: None,
            placement: Placement::Inline,
        });
        let mut doc = doc_with(
            vec![pages_body(4)],
            vec![
                r(HeaderFooterKind::Default, "h"),
                r(HeaderFooterKind::Even, "he"),
            ],
            vec![r(HeaderFooterKind::Default, "f")],
            vec![
                (
                    "h",
                    vec![DocBlock::Paragraph(Paragraph {
                        runs: vec![run],
                        ..Paragraph::default()
                    })],
                ),
                ("he", vec![para("EVEN")]),
                ("f", vec![page_x_of_y()]),
            ],
        );
        doc.even_and_odd_headers = true;
        let render = || {
            render_with(deterministic(), &doc, &media, &RenderOptions::default())
                .expect("render")
                .pdf
        };
        assert_eq!(render(), render());
    }

    // -------------------------------------------------------- 页码起始值 / 格式

    /// 多节文档:`sects` 每项 `(本节页数, w:start, w:fmt)`,每节一段 `pages_body`;只有首节
    /// 引用页脚部件 `f`(后面的节继承),奇偶页眉开关 `even_odd` 时另挂 `he` 偶数页眉。
    fn numbered(sects: &[(usize, Option<u32>, PageNumFormat)], footer: DocBlock) -> Document {
        let mut doc = Document {
            header_footers: [("f".to_string(), vec![footer])].into_iter().collect(),
            ..Document::default()
        };
        for (i, &(pages, start, fmt)) in sects.iter().enumerate() {
            doc.body.push(pages_body(pages));
            doc.sections.push(Section {
                footers: if i == 0 {
                    vec![r(HeaderFooterKind::Default, "f")]
                } else {
                    vec![]
                },
                page_number_start: start,
                page_number_format: fmt,
                end_block: doc.body.len(),
                ..Section::default()
            });
        }
        doc
    }

    fn page_only(instr: &str) -> DocBlock {
        DocBlock::Paragraph(Paragraph {
            runs: vec![field_run(instr, "9")],
            ..Paragraph::default()
        })
    }

    fn feet(doc: &Document) -> (Vec<String>, Vec<String>) {
        let (pages, warnings) = layout(doc, &BTreeMap::new());
        (pages.iter().map(footer_text).collect(), warnings)
    }

    #[test]
    fn section_start_resets_page_numbers() {
        let doc = numbered(&[(3, Some(5), PageNumFormat::Decimal)], page_only("PAGE"));
        assert_eq!(feet(&doc).0, ["5", "6", "7"]);
        // `w:start="0"` 合法:首页印 0。
        let doc = numbered(&[(2, Some(0), PageNumFormat::Decimal)], page_only("PAGE"));
        assert_eq!(feet(&doc).0, ["0", "1"]);
    }

    /// 1 万节以上(邮件合并:每封信一节、页码各自从 1 重起):逐页页码都是 1,与节数无关。
    #[test]
    fn ten_thousand_restarting_sections_number_every_page_from_one() {
        let sects = vec![(1, Some(1), PageNumFormat::Decimal); 10_005];
        let (foots, _) = feet(&numbered(&sects, page_only("PAGE")));
        assert_eq!(foots.len(), 10_005);
        assert!(foots.iter().all(|f| f == "1"));
    }

    #[test]
    fn missing_start_continues_from_previous_section_and_start_restarts() {
        let d = PageNumFormat::Decimal;
        // 封面不计页:第一节 start=0,正文节 start=1 重新起算,第三节接续。
        let doc = numbered(
            &[(1, Some(0), d), (2, Some(1), d), (2, None, d)],
            page_only("PAGE"),
        );
        assert_eq!(feet(&doc).0, ["0", "1", "2", "3", "4"]);
        // 全部缺失 = 物理页序。
        let doc = numbered(&[(2, None, d), (1, None, d)], page_only("PAGE"));
        assert_eq!(feet(&doc).0, ["1", "2", "3"]);
        // 中途重置后再接续。
        let doc = numbered(
            &[(2, None, d), (2, Some(10), d), (1, None, d)],
            page_only("PAGE"),
        );
        assert_eq!(feet(&doc).0, ["1", "2", "10", "11", "12"]);
    }

    #[test]
    fn section_format_applies_per_section() {
        let doc = numbered(
            &[
                (3, None, PageNumFormat::LowerRoman),
                (2, Some(1), PageNumFormat::Decimal),
                (2, None, PageNumFormat::UpperLetter),
            ],
            page_only("PAGE"),
        );
        assert_eq!(feet(&doc).0, ["i", "ii", "iii", "1", "2", "C", "D"]);
        // 超出罗马数字范围(<=0)降级为阿拉伯数字。
        let doc = numbered(
            &[(2, Some(0), PageNumFormat::UpperRoman)],
            page_only("PAGE"),
        );
        assert_eq!(feet(&doc).0, ["0", "I"]);
    }

    #[test]
    fn numpages_stays_physical_total_in_arabic() {
        let doc = numbered(&[(3, Some(7), PageNumFormat::LowerRoman)], page_x_of_y());
        assert_eq!(feet(&doc).0, ["Pageviiof3", "Pageviiiof3", "Pageixof3"]);
    }

    #[test]
    fn field_format_switch_beats_section_format() {
        let sect = |f| numbered(&[(2, None, f)], page_only(r"PAGE \* roman"));
        assert_eq!(feet(&sect(PageNumFormat::Decimal)).0, ["i", "ii"]);
        let cases = [
            (r"PAGE \* ROMAN", PageNumFormat::Decimal, ["I", "II"]),
            (r"PAGE \* alphabetic", PageNumFormat::Decimal, ["a", "b"]),
            (r"PAGE \* ALPHABETIC", PageNumFormat::Decimal, ["A", "B"]),
            (r"PAGE \* Arabic", PageNumFormat::UpperRoman, ["1", "2"]),
            // 未知开关忽略:落回节格式。
            (
                r"PAGE \* MERGEFORMAT",
                PageNumFormat::LowerLetter,
                ["a", "b"],
            ),
            (r"PAGE \* Bogus", PageNumFormat::UpperRoman, ["I", "II"]),
            // 未知开关不挡后面的已知开关。
            (
                r"PAGE \* MERGEFORMAT \* ROMAN",
                PageNumFormat::Decimal,
                ["I", "II"],
            ),
        ];
        for (instr, fmt, want) in cases {
            let doc = numbered(&[(2, None, fmt)], page_only(instr));
            assert_eq!(feet(&doc).0, want, "{instr}");
        }
    }

    /// 奇偶页眉按显示页码的奇偶(`w:start`=2 → 节首页算偶数页)。
    #[test]
    fn even_odd_headers_follow_displayed_page_number() {
        let mut doc = numbered(&[(3, Some(2), PageNumFormat::Decimal)], para("F"));
        doc.sections[0].headers = vec![
            r(HeaderFooterKind::Default, "h"),
            r(HeaderFooterKind::Even, "he"),
        ];
        doc.header_footers.insert("h".into(), vec![para("ODDH")]);
        doc.header_footers.insert("he".into(), vec![para("EVENH")]);
        doc.even_and_odd_headers = true;
        let (pages, _) = layout(&doc, &BTreeMap::new());
        let heads: Vec<String> = pages.iter().map(header_text).collect();
        assert_eq!(heads, ["EVENH", "ODDH", "EVENH"]);
    }

    #[test]
    fn unsupported_format_prints_arabic_and_warns_once() {
        let doc = numbered(
            &[
                (2, None, PageNumFormat::Other),
                (1, None, PageNumFormat::Other),
            ],
            page_only("PAGE"),
        );
        let (foots, warnings) = feet(&doc);
        assert_eq!(foots, ["1", "2", "3"]);
        let n = warnings
            .iter()
            .filter(|k| *k == "page-number-format-unsupported")
            .count();
        assert_eq!(n, 1, "{warnings:?}");
        // 支持的格式不告警。
        let ok = numbered(&[(1, None, PageNumFormat::LowerRoman)], page_only("PAGE"));
        assert!(!feet(&ok).1.iter().any(|k| k.contains("page-number")));
    }
}
