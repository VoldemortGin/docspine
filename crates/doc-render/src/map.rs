//! doc-core IR → pdf-typeset 输入模型的**纯映射**(块遍历 + 有效样式驱动)。
//!
//! 每段每 run 经 C-5 的 `resolve_para` / `resolve_run`(表格内 `*_in_table` 变体)
//! 得到**最终值**,再折进引擎的 [`Block`](pdf_typeset::Block) / [`Run`] /
//! [`RunStyle`] / [`ParaProps`](pdf_typeset::ParaProps):
//!
//! - run 分段:`Text` 拼接、`Tab` → `'\t'`(引擎按空格排,制表位是 C-9)、
//!   `Break(Line)` → `'\n'`;`Break(Page/Column)` 把段落切成多段,中间插
//!   [`Block::PageBreak`](pdf_typeset::Block::PageBreak)(单栏渲染下换栏 = 换页)。
//! - **空段落占一行高**:无(可见)run 的段落传一个携带段落标记样式的空文本 run
//!   (引擎约定:无 run 的段落零高)。
//! - `EffectiveRunProps.font_east_asia` 有值时按字符类切分子 run:CJK 字符喂
//!   eastAsia 字体、其余喂 ascii 字体(引擎的 `RunStyle` 只有单一 family 槽;
//!   ea 缺省时整段用 ascii 字体,CJK 字符由引擎的逐字回退链兜底)。
//! - caps/smallCaps → 文本大写(smallCaps 不缩小,v1 近似);vanish → 跳过(隐藏);
//!   上/下标 → 字号 ×0.65 近似(引擎无基线偏移)。
//! - `pageBreakBefore` → 段前插 `Block::PageBreak`(节首块除外——本就新起一页)。
//! - **列表(C-6)**:带 `numPr` 的段落按文档顺序推进 [`ListCounters`],最终标签串
//!   喂引擎的 [`ListLabel`](标签右对齐到正文起点前 `gutter` 处);悬挂缩进折成
//!   gutter(文本对齐 left 缩进,与 Word 的列表版式一致)。
//! - 段落边框 / 底纹 / 图片:解析保真,渲染降级 + 一次性告警(C-8 落地图片)。

use std::collections::BTreeMap;

use doc_core::geom::emu_to_points;
use doc_core::model::{
    Block as DocBlock, BreakKind, Document, Paragraph, Picture, Placement, RunSegment,
    Table as DocTable, TextRun,
};
use doc_core::numbering::ListCounters;
use doc_core::style::{
    resolve_para, resolve_para_in_table, resolve_run, resolve_run_in_table, EffectiveLineSpacing,
    EffectiveParaProps, EffectiveRunProps, Justification, VertAlign,
};
use pdf_typeset::{
    Align, Block, BorderEdge, CellBorders, ColumnWidth, ImageSpec, LineSpacing, ListLabel,
    PageGeom, ParaProps, Rgb, Run, RunStyle, TableCell, TableRow, TableSpec,
};

use crate::section::page_geom;
use crate::table::map_table;
use crate::warn::RenderWarning;

/// 上/下标的字号近似缩放(引擎无基线偏移,v1 只缩字号)。
const VERT_ALIGN_SCALE: f64 = 0.65;

/// 不支持矢量图(EMF/WMF)占位框的浅灰填充与描边(观感对齐 pptspine 的图表灰框)。
const PLACEHOLDER_FILL: Rgb = Rgb::new(0.90, 0.90, 0.90);
const PLACEHOLDER_STROKE: Rgb = Rgb::new(0.60, 0.60, 0.60);
const PLACEHOLDER_STROKE_PT: f64 = 0.75;

/// 列表标签与正文起点的缺省空隙(磅;层级无悬挂缩进时用)。
const DEFAULT_LIST_GUTTER: f64 = 4.0;

/// 悬挂缩进折算标签空隙的比例(引擎不知标签宽,标签右对齐到正文起点前 gutter 处;
/// 取 0.35×hanging 并夹在 2..=10pt——Word 缺省 360twip=18pt 悬挂 → 6.3pt 空隙,
/// 标签左缘落点与 Word 的首行缩进位相近)。
const LIST_GUTTER_RATIO: f64 = 0.35;

/// 一节的渲染计划:页面几何 + 引擎块序列。
pub(crate) struct SectionPlan {
    pub(crate) geom: PageGeom,
    pub(crate) blocks: Vec<Block>,
}

/// 整篇文档的映射结果。
pub(crate) struct MappedDoc {
    pub(crate) sections: Vec<SectionPlan>,
    pub(crate) warnings: Vec<RenderWarning>,
}

/// 映射期上下文:告警收集(docspine 侧的降级每种只报一次,引擎侧自带去重)+
/// 列表计数器(C-6,按文档顺序推进)+ 当前节的正文框架(C-7,表格 pct 宽解析
/// 与 RowTooTall 检测用)。
pub(crate) struct MapCtx {
    list: Vec<RenderWarning>,
    /// per-numId per-level 列表计数(一份文档一实例,含表格内段落,文档序推进)。
    counters: ListCounters,
    /// 当前节的正文区宽(磅:页面宽 − 左右边距)。
    body_w: f64,
    /// 当前节的正文区高(磅:页面高 − 上下边距)。
    body_h: f64,
    border_warned: bool,
    shading_warned: bool,
    picture_warned: bool,
    unsupported_format_warned: bool,
    floating_warned: bool,
    valign_warned: bool,
    row_warned: bool,
    numbering_warned: bool,
}

impl MapCtx {
    fn new() -> Self {
        let mut ctx = MapCtx {
            list: Vec::new(),
            counters: ListCounters::new(),
            body_w: 1.0,
            body_h: 1.0,
            border_warned: false,
            shading_warned: false,
            picture_warned: false,
            unsupported_format_warned: false,
            floating_warned: false,
            valign_warned: false,
            row_warned: false,
            numbering_warned: false,
        };
        ctx.set_frame(&page_geom(&doc_core::model::Section::default()));
        ctx
    }

    /// 切换到一节的正文框架(每节映射前调用)。
    fn set_frame(&mut self, geom: &PageGeom) {
        self.body_w = (geom.width - geom.margin_left - geom.margin_right).max(1.0);
        self.body_h = (geom.height - geom.margin_top - geom.margin_bottom).max(1.0);
    }

    /// 当前节的正文区宽(磅)。
    pub(crate) fn body_width(&self) -> f64 {
        self.body_w
    }

    /// 当前节的正文区高(磅)。
    pub(crate) fn body_height(&self) -> f64 {
        self.body_h
    }

    fn para_border(&mut self) {
        if !self.border_warned {
            self.border_warned = true;
            self.list.push(RenderWarning::ParaBorderOmitted);
        }
    }

    fn para_shading(&mut self) {
        if !self.shading_warned {
            self.shading_warned = true;
            self.list.push(RenderWarning::ParaShadingOmitted);
        }
    }

    fn picture(&mut self) {
        if !self.picture_warned {
            self.picture_warned = true;
            self.list.push(RenderWarning::PictureSkipped);
        }
    }

    fn unsupported_format(&mut self) {
        if !self.unsupported_format_warned {
            self.unsupported_format_warned = true;
            self.list.push(RenderWarning::UnsupportedImageFormat);
        }
    }

    fn floating_image(&mut self) {
        if !self.floating_warned {
            self.floating_warned = true;
            self.list.push(RenderWarning::FloatingImageInlined);
        }
    }

    fn numbering_indirection(&mut self) {
        if !self.numbering_warned {
            self.numbering_warned = true;
            self.list.push(RenderWarning::NumberingIndirectionSkipped);
        }
    }

    /// 单元格 vAlign(center/bottom)按顶对齐渲染的一次性降级(table.rs 调用)。
    pub(crate) fn cell_valign(&mut self) {
        if !self.valign_warned {
            self.valign_warned = true;
            self.list.push(RenderWarning::CellVAlignIgnored);
        }
    }

    /// 行高超过一页正文高度的一次性降级(table.rs 调用)。
    pub(crate) fn row_too_tall(&mut self) {
        if !self.row_warned {
            self.row_warned = true;
            self.list.push(RenderWarning::RowTooTall);
        }
    }
}

/// 把整篇文档映射成按节分组的引擎块序列(纯计算,不触引擎状态)。
/// 无 media 的便利入口(测试用;图片一律跳过)。生产路径走
/// [`map_document_with_media`]。
#[cfg(test)]
pub(crate) fn map_document(doc: &Document) -> MappedDoc {
    map_document_with_media(doc, &BTreeMap::new())
}

pub(crate) fn map_document_with_media(
    doc: &Document,
    media: &BTreeMap<String, Vec<u8>>,
) -> MappedDoc {
    let mut ctx = MapCtx::new();
    // 渲染前体检一次样式表:basedOn 环 / 悬空引用浮成告警(解析自身带防环)。
    for sw in doc.styles.validate() {
        ctx.list.push(RenderWarning::Style(sw));
    }

    let mut sections = Vec::new();
    let mut start = 0usize;
    for sect in &doc.sections {
        if sect.cols > 1 {
            ctx.list
                .push(RenderWarning::MultiColumnFlattened { cols: sect.cols });
        }
        let end = sect.end_block.min(doc.body.len()).max(start);
        let geom = page_geom(sect);
        ctx.set_frame(&geom);
        sections.push(SectionPlan {
            geom,
            blocks: map_blocks(doc, &doc.body[start..end], None, &mut ctx, media),
        });
        start = end;
    }
    // 防御:节序列缺失 / 未覆盖全部块(解析层保证不发生)也不丢内容。
    if start < doc.body.len() {
        let tail = map_blocks(doc, &doc.body[start..], None, &mut ctx, media);
        match sections.last_mut() {
            Some(last) => last.blocks.extend(tail),
            None => sections.push(SectionPlan {
                geom: page_geom(&doc_core::model::Section::default()),
                blocks: tail,
            }),
        }
    }
    if sections.is_empty() {
        sections.push(SectionPlan {
            geom: page_geom(&doc_core::model::Section::default()),
            blocks: Vec::new(),
        });
    }
    MappedDoc {
        sections,
        warnings: ctx.list,
    }
}

/// 映射一串同级块(正文一节的切片,或单元格内容)。`table` 是表格上下文
/// (在表格内时段落/ run 走 `*_in_table` 解析,表格样式链参与级联)。
pub(crate) fn map_blocks(
    doc: &Document,
    blocks: &[DocBlock],
    table: Option<&DocTable>,
    ctx: &mut MapCtx,
    media: &BTreeMap<String, Vec<u8>>,
) -> Vec<Block> {
    let mut out = Vec::new();
    for block in blocks {
        match block {
            DocBlock::Paragraph(p) => map_paragraph(doc, p, table, ctx, &mut out, media),
            DocBlock::Table(t) => out.push(Block::Table(map_table(doc, t, ctx, media))),
        }
    }
    out
}

/// 映射一个段落:有效样式 → 引擎段落属性;run 分段折叠;段内换页把段落切成多段;
/// 带 `numPr` 的段落推进列表计数产出标签(C-6)。
fn map_paragraph(
    doc: &Document,
    para: &Paragraph,
    table: Option<&DocTable>,
    ctx: &mut MapCtx,
    out: &mut Vec<Block>,
    media: &BTreeMap<String, Vec<u8>>,
) {
    let eff = match table {
        Some(t) => resolve_para_in_table(doc, t, para),
        None => resolve_para(doc, para),
    };
    if eff.borders.any() {
        ctx.para_border();
    }
    if eff.shading.is_some() {
        ctx.para_shading();
    }
    // 段前分页:引擎支持,直接插 PageBreak(节首块除外——该页本就新起)。
    if eff.page_break_before && !out.is_empty() {
        out.push(Block::PageBreak);
    }
    let mut props = para_props(&eff);

    // 列表标签(C-6):按文档顺序推进计数;numId=0 / 层级无定义 / numFmt=none 不产
    // 标签(缩进仍经层级 pPr 级联生效);numStyleLink 间接 v1 不解 → 一次性告警。
    if let Some(num_id) = para.num_id {
        if doc.numbering.uses_num_style_link(num_id) {
            ctx.numbering_indirection();
        }
        let ilvl = para.list_level.unwrap_or(0);
        if let Some(text) = ctx.counters.advance(&doc.numbering, num_id, ilvl) {
            // Word 列表版式:标签画在首行缩进位(left − hanging),正文**含首行**对齐
            // left。引擎把标签右对齐到首行文本起点前 gutter 处——因此清零负首行缩进
            // (文本全部落在 left),悬挂量按比例折成 gutter。
            let hanging = f64::from(-eff.first_line_indent_pt).max(0.0);
            let gutter = if hanging > 0.0 {
                (hanging * LIST_GUTTER_RATIO).clamp(2.0, 10.0)
            } else {
                DEFAULT_LIST_GUTTER
            };
            props.first_line_indent = props.first_line_indent.max(0.0);
            props.list = Some(ListLabel::new(text, gutter));
        }
    }

    // 内嵌图片(C-8):按文档顺序收集成块级块。有 media 字节且有 wp:extent 尺寸
    // 的光栅图渲染成 Block::Image;EMF/WMF 等矢量格式画等大的浅灰占位框(表格原语)
    // + UnsupportedImageFormat 告警;缺字节/缺尺寸的图跳过 + PictureSkipped 告警;
    // 浮动(锚定)图按块级内联近似(不做绝对定位/文字环绕)+ 一次性告警。
    let mut image_blocks: Vec<Block> = Vec::new();
    for run in &para.runs {
        for pic in &run.pictures {
            let anchored = matches!(pic.placement, Placement::Anchored { .. });
            match picture_block(pic, media) {
                PicOutcome::Raster(block) => {
                    if anchored {
                        ctx.floating_image();
                    }
                    image_blocks.push(block);
                }
                PicOutcome::Placeholder(block) => {
                    if anchored {
                        ctx.floating_image();
                    }
                    ctx.unsupported_format();
                    image_blocks.push(block);
                }
                PicOutcome::Skip => ctx.picture(),
            }
        }
    }

    // run 序列 → 若干「段落片」,段内换页/换栏是切分点。
    let mut parts: Vec<Vec<Run>> = vec![Vec::new()];
    for run in &para.runs {
        let eff_r = match table {
            Some(t) => resolve_run_in_table(doc, t, para, run),
            None => resolve_run(doc, para, run),
        };
        if eff_r.vanish {
            continue; // 隐藏文字:忠实于 Word 的不渲染。
        }
        let mut text = String::new();
        for seg in &run.segments {
            match seg {
                RunSegment::Text(s) => text.push_str(s),
                RunSegment::Tab => text.push('\t'),
                RunSegment::Break(BreakKind::Line) => text.push('\n'),
                // 单栏渲染:换栏等效换页(C-2 声明语义)。
                RunSegment::Break(BreakKind::Page) | RunSegment::Break(BreakKind::Column) => {
                    push_runs(parts.last_mut().expect("parts non-empty"), &text, &eff_r);
                    text.clear();
                    parts.push(Vec::new());
                }
            }
        }
        push_runs(parts.last_mut().expect("parts non-empty"), &text, &eff_r);
    }

    // 空段落片(含整段无 run)传空文本 run,携带段落标记样式占一行高。
    let mark_style = || {
        let eff_r = match table {
            Some(t) => resolve_run_in_table(doc, t, para, &TextRun::default()),
            None => resolve_run(doc, para, &TextRun::default()),
        };
        run_style(&eff_r, &eff_r.font_ascii)
    };
    // 图片独占段落(无可见文字)时跳过「空段落占一行」,只 emit 图片——避免图前
    // 多一条空行(真实 docx 里图片常独占一段)。有文字则文字先排、图片紧随其后。
    let has_text = parts.iter().any(|part| !part.is_empty());
    if has_text || image_blocks.is_empty() {
        for (i, part) in parts.into_iter().enumerate() {
            if i > 0 {
                out.push(Block::PageBreak);
            }
            let runs = if part.is_empty() {
                // 空段落 / 换页后的空尾片:仍占一行(Word:段落标记独占一行)。
                vec![Run::new("", mark_style())]
            } else {
                part
            };
            let mut p = props.clone();
            if i > 0 {
                p.list = None; // 标签只画在首片(段内换页不重复编号)。
            }
            out.push(Block::Paragraph(p, runs));
        }
    }
    for block in image_blocks {
        out.push(block);
    }
}

/// 一张内嵌图片的映射去向。
enum PicOutcome {
    /// 可解码的光栅图:块级 [`Block::Image`]。
    Raster(Block),
    /// 不支持的矢量格式(EMF/WMF):等大的浅灰占位框(块级表格)。
    Placeholder(Block),
    /// 跳过(缺 media 名 / 字节 / 尺寸,或尺寸非法):调用方发 PictureSkipped 告警。
    Skip,
}

/// 把一张内嵌图片分类映射。缺 `media_name` / media 字节 / `wp:extent` 尺寸,或尺寸
/// 非法(≤0),→ [`PicOutcome::Skip`]。EMF/WMF(名后缀或魔数命中)→
/// [`PicOutcome::Placeholder`];其余光栅图 → [`PicOutcome::Raster`]。引擎按可用列宽
/// 等比缩小(绝不放大),故传图片的自然显示尺寸(EMU → pt)即可。
fn picture_block(pic: &Picture, media: &BTreeMap<String, Vec<u8>>) -> PicOutcome {
    let Some(name) = pic.media_name.as_ref() else {
        return PicOutcome::Skip;
    };
    let Some(bytes) = media.get(name) else {
        return PicOutcome::Skip;
    };
    let Some((w_emu, h_emu)) = pic.extent else {
        return PicOutcome::Skip;
    };
    let (w, h) = (emu_to_points(w_emu), emu_to_points(h_emu));
    if !(w > 0.0 && h > 0.0 && w.is_finite() && h.is_finite()) {
        return PicOutcome::Skip;
    }
    if is_unsupported_vector(name, bytes) {
        return PicOutcome::Placeholder(placeholder_box(w, h));
    }
    PicOutcome::Raster(Block::Image(ImageSpec::new(bytes.clone(), w, h)))
}

/// EMF/WMF 识别(名后缀 + 魔数双保险):
/// - 后缀 `.emf` / `.wmf`(忽略大小写);
/// - EMF 魔数:首个 `EMR_HEADER` 记录类型 `0x00000001` + 偏移 40 处 " EMF" 签名;
/// - WMF 魔数:可置放头 `0xD7 0xCD 0xC6 0x9A`,或标准 METAHEADER(`mtType`∈{1,2}、
///   `mtHeaderSize = 0x0009`)。
fn is_unsupported_vector(name: &str, bytes: &[u8]) -> bool {
    let lower = name.to_ascii_lowercase();
    if lower.ends_with(".emf") || lower.ends_with(".wmf") {
        return true;
    }
    is_emf_magic(bytes) || is_wmf_magic(bytes)
}

/// EMF 魔数:`iType == 1` 且偏移 40..44 为 ASCII " EMF"(`0x20 0x45 0x4D 0x46`)。
fn is_emf_magic(b: &[u8]) -> bool {
    b.len() >= 44 && b[0..4] == [0x01, 0x00, 0x00, 0x00] && b[40..44] == [0x20, 0x45, 0x4D, 0x46]
}

/// WMF 魔数:可置放头,或标准内存/磁盘 METAHEADER(`mtHeaderSize` 恒为 9)。
fn is_wmf_magic(b: &[u8]) -> bool {
    if b.len() >= 4 && b[0..4] == [0xD7, 0xCD, 0xC6, 0x9A] {
        return true; // 可置放 WMF。
    }
    b.len() >= 4 && (b[0..2] == [0x01, 0x00] || b[0..2] == [0x02, 0x00]) && b[2..4] == [0x09, 0x00]
}

/// 不支持的矢量图 → 与图片显示尺寸等大的浅灰占位框:单列单行表(浅灰填充 + 四边
/// 细线),引擎按可用列宽等比收窄。家族占位画法对齐 pptspine 的灰框——那里是形状
/// Op,这里落成流式表格原语。
fn placeholder_box(width: f64, height: f64) -> Block {
    let edge = BorderEdge {
        width: PLACEHOLDER_STROKE_PT,
        color: PLACEHOLDER_STROKE,
    };
    let mut cell = TableCell::new(Vec::new());
    cell.fill = Some(PLACEHOLDER_FILL);
    cell.borders = CellBorders {
        top: Some(edge),
        right: Some(edge),
        bottom: Some(edge),
        left: Some(edge),
    };
    let mut row = TableRow::new(vec![cell]);
    row.min_height = Some(height);
    Block::Table(TableSpec::new(vec![ColumnWidth::Fixed(width)], vec![row]))
}

/// 有效段落属性 → 引擎段落属性。
fn para_props(eff: &EffectiveParaProps) -> ParaProps {
    let mut p = ParaProps::new();
    p.align = match eff.align {
        Justification::Left => Align::Left,
        Justification::Center => Align::Center,
        Justification::Right => Align::Right,
        // distribute(两端撑满含末行)近似按 justify。
        Justification::Justify | Justification::Distribute => Align::Justify,
    };
    p.spacing = match eff.line_spacing {
        EffectiveLineSpacing::Multiple(m) => LineSpacing::Multiple(f64::from(m)),
        EffectiveLineSpacing::Exact(h) => LineSpacing::Exact(f64::from(h)),
        // TS-8 起引擎有真 atLeast:行高 = max(自然行高, 给定值)。
        EffectiveLineSpacing::AtLeast(h) => LineSpacing::AtLeast(f64::from(h)),
    };
    p.space_before = f64::from(eff.space_before_pt);
    p.space_after = f64::from(eff.space_after_pt);
    p.indent_left = f64::from(eff.indent_left_pt);
    p.indent_right = f64::from(eff.indent_right_pt);
    p.first_line_indent = f64::from(eff.first_line_indent_pt);
    p
}

/// 把一段文本按有效 run 属性折成引擎 run(caps 大写、eastAsia 字体按字符类切分)。
fn push_runs(runs: &mut Vec<Run>, text: &str, eff: &EffectiveRunProps) {
    if text.is_empty() {
        return;
    }
    let text = if eff.caps || eff.small_caps {
        // smallCaps 不缩小(v1 近似:与 caps 同渲染)。
        text.to_uppercase()
    } else {
        text.to_string()
    };
    let Some(ea) = eff.font_east_asia.as_deref() else {
        runs.push(Run::new(text, run_style(eff, &eff.font_ascii)));
        return;
    };
    // eastAsia 槽有值:CJK 字符段喂 ea 字体,其余喂 ascii 字体;空白等中性字符
    // 跟随当前段,避免碎片化。
    let mut cur = String::new();
    let mut cur_cjk: Option<bool> = None;
    for ch in text.chars() {
        let class = if is_east_asian(ch) {
            Some(true)
        } else if ch.is_whitespace() {
            None // 中性:跟随当前段。
        } else {
            Some(false)
        };
        match (class, cur_cjk) {
            (Some(c), Some(prev)) if c != prev => {
                let family = if prev { ea } else { &eff.font_ascii };
                runs.push(Run::new(std::mem::take(&mut cur), run_style(eff, family)));
                cur_cjk = Some(c);
            }
            (Some(c), None) => cur_cjk = Some(c),
            _ => {}
        }
        cur.push(ch);
    }
    if !cur.is_empty() {
        let family = if cur_cjk == Some(true) {
            ea
        } else {
            &eff.font_ascii
        };
        runs.push(Run::new(cur, run_style(eff, family)));
    }
}

/// 有效 run 属性 + 选定 family → 引擎 run 样式。
pub(crate) fn run_style(eff: &EffectiveRunProps, family: &str) -> RunStyle {
    let scale = match eff.vert_align {
        VertAlign::Baseline => 1.0,
        VertAlign::Superscript | VertAlign::Subscript => VERT_ALIGN_SCALE,
    };
    let mut s = RunStyle::new(family, f64::from(eff.size_pt) * scale);
    s.bold = eff.bold;
    s.italic = eff.italic;
    s.underline = eff.underline;
    s.strike = eff.strike;
    s.color = eff.color.map(rgb).unwrap_or(Rgb::BLACK);
    s.highlight = eff.highlight.map(rgb);
    s
}

/// doc-core 颜色 → 引擎 RGB(0..=1 浮点)。
pub(crate) fn rgb(c: doc_core::model::Color) -> Rgb {
    Rgb::new(
        f64::from(c.rgb[0]) / 255.0,
        f64::from(c.rgb[1]) / 255.0,
        f64::from(c.rgb[2]) / 255.0,
    )
}

/// 该字符是否按东亚(CJK)字体渲染(与引擎的断行分类同一套区段)。
fn is_east_asian(ch: char) -> bool {
    let cp = ch as u32;
    (0x1100..=0x11FF).contains(&cp)           // 谚文字母
        || (0x2E80..=0x9FFF).contains(&cp)    // CJK 部首 … 统一表意(含假名)
        || (0xAC00..=0xD7AF).contains(&cp)    // 谚文音节
        || (0xF900..=0xFAFF).contains(&cp)    // 兼容表意
        || (0xFF00..=0xFFEF).contains(&cp)    // 全角/半角形式
        || (0x20000..=0x3FFFF).contains(&cp) // 扩展平面
}

// ============================================================ 单测:映射的每条语义

#[cfg(test)]
mod tests {
    use super::*;
    use doc_core::model::{Cell, Color, Paragraph, Row, Section, Table};
    use doc_core::style::{ColorRef, Highlight};
    use pdf_typeset::ColumnWidth;

    fn para_with_text(text: &str) -> Paragraph {
        Paragraph {
            runs: vec![TextRun::from_text(text)],
            ..Paragraph::default()
        }
    }

    fn doc_with_body(body: Vec<DocBlock>) -> Document {
        let end = body.len();
        Document {
            body,
            sections: vec![Section {
                end_block: end,
                ..Section::default()
            }],
            ..Document::default()
        }
    }

    fn para_with_picture(
        media_name: &str,
        extent: Option<(i64, i64)>,
        placement: Placement,
    ) -> Paragraph {
        let mut run = TextRun::default();
        run.pictures.push(Picture {
            rel_id: "rId1".into(),
            media_name: Some(media_name.into()),
            extent,
            image_bytes_len: 3,
            placement,
        });
        Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        }
    }

    /// C-8:有 media 字节 + wp:extent 的内嵌图 → 块级 `Block::Image`,尺寸 EMU→pt;
    /// 图片独占段落时不产多余空段落。
    #[test]
    fn inline_image_renders_as_block_image() {
        // 914400 EMU = 1 in = 72 pt;457200 EMU = 0.5 in = 36 pt。
        let doc = doc_with_body(vec![DocBlock::Paragraph(para_with_picture(
            "img.png",
            Some((914_400, 457_200)),
            Placement::Inline,
        ))]);
        let mut media = BTreeMap::new();
        media.insert("img.png".to_string(), vec![1u8, 2, 3]);
        let blocks = &map_document_with_media(&doc, &media).sections[0].blocks;
        assert_eq!(blocks.len(), 1, "图片独占段应只 emit 图片本身,无空段落");
        match &blocks[0] {
            Block::Image(spec) => {
                assert!((spec.width - 72.0).abs() < 0.01);
                assert!((spec.height - 36.0).abs() < 0.01);
            }
            other => panic!("expected Block::Image, got {other:?}"),
        }
    }

    /// 缺 media 字节 → 跳过图片 + 一次性 picture-skipped 告警(不产 Block::Image)。
    #[test]
    fn missing_media_skips_image_with_warning() {
        let doc = doc_with_body(vec![DocBlock::Paragraph(para_with_picture(
            "gone.png",
            Some((914_400, 457_200)),
            Placement::Inline,
        ))]);
        let mapped = map_document_with_media(&doc, &BTreeMap::new());
        assert!(!mapped.sections[0]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Image(_))));
        assert!(mapped
            .warnings
            .iter()
            .any(|w| w.kind() == "picture-skipped"));
    }

    /// 浮动/锚定图片:仍按块级渲染,并发一次 floating-image-inlined 告警。
    #[test]
    fn floating_image_renders_inline_with_warning() {
        let placement = Placement::Anchored {
            x: 0,
            y: 0,
            rel_h: Default::default(),
            rel_v: Default::default(),
            behind: false,
        };
        let doc = doc_with_body(vec![DocBlock::Paragraph(para_with_picture(
            "f.png",
            Some((914_400, 914_400)),
            placement,
        ))]);
        let mut media = BTreeMap::new();
        media.insert("f.png".to_string(), vec![9u8]);
        let mapped = map_document_with_media(&doc, &media);
        assert!(mapped.sections[0]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Image(_))));
        assert!(mapped
            .warnings
            .iter()
            .any(|w| w.kind() == "floating-image-inlined"));
    }

    /// C-8:EMF/WMF 矢量图无法解码 → 画等大的浅灰占位框(块级表格:浅灰填充 +
    /// 四边细线、行高 = 显示高),两张只发一次 unsupported-image-format 告警。
    #[test]
    fn unsupported_vector_image_draws_placeholder_box_once() {
        let doc = doc_with_body(vec![
            DocBlock::Paragraph(para_with_picture(
                "chart.emf",
                Some((914_400, 457_200)), // 1in × 0.5in = 72 × 36 pt。
                Placement::Inline,
            )),
            DocBlock::Paragraph(para_with_picture(
                "diagram.wmf",
                Some((914_400, 914_400)),
                Placement::Inline,
            )),
        ]);
        let mut media = BTreeMap::new();
        media.insert("chart.emf".to_string(), vec![0u8; 4]);
        media.insert("diagram.wmf".to_string(), vec![0u8; 4]);
        let mapped = map_document_with_media(&doc, &media);
        let blocks = &mapped.sections[0].blocks;
        // 无光栅图;两张矢量图各出一个占位表格。
        assert!(!blocks.iter().any(|b| matches!(b, Block::Image(_))));
        let tables: Vec<&TableSpec> = blocks
            .iter()
            .filter_map(|b| match b {
                Block::Table(t) => Some(t),
                _ => None,
            })
            .collect();
        assert_eq!(tables.len(), 2, "两张矢量图各一个占位框");
        let first = tables[0];
        assert_eq!(first.columns, vec![ColumnWidth::Fixed(72.0)]);
        assert_eq!(first.rows[0].min_height, Some(36.0));
        let cell = &first.rows[0].cells[0];
        assert!(cell.fill.is_some(), "占位框有填充(get_drawings 可见)");
        assert!(cell.borders.top.is_some() && cell.borders.left.is_some());
        let kinds: Vec<&str> = mapped.warnings.iter().map(RenderWarning::kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == "unsupported-image-format")
                .count(),
            1,
            "两张矢量图同类去重,只报一次"
        );
    }

    /// 魔数兜底:名后缀非 emf/wmf,但字节是 EMF 头(iType=1 + 偏移 40 " EMF")→
    /// 仍识别为矢量占位。
    #[test]
    fn emf_magic_bytes_trigger_placeholder_without_suffix() {
        let mut bytes = vec![0u8; 44];
        bytes[0] = 0x01; // iType = EMR_HEADER
        bytes[40..44].copy_from_slice(b" EMF"); // 签名。
        let doc = doc_with_body(vec![DocBlock::Paragraph(para_with_picture(
            "image1.dat",
            Some((914_400, 914_400)),
            Placement::Inline,
        ))]);
        let mut media = BTreeMap::new();
        media.insert("image1.dat".to_string(), bytes);
        let mapped = map_document_with_media(&doc, &media);
        assert!(mapped.sections[0]
            .blocks
            .iter()
            .any(|b| matches!(b, Block::Table(_))));
        assert!(mapped
            .warnings
            .iter()
            .any(|w| w.kind() == "unsupported-image-format"));
    }

    /// 默认节:Letter 纵向 612x792 pt、四边 72 pt(1440 twip)边距。
    #[test]
    fn default_section_maps_to_letter_geometry() {
        let doc = doc_with_body(vec![DocBlock::Paragraph(para_with_text("hi"))]);
        let mapped = map_document(&doc);
        assert_eq!(mapped.sections.len(), 1);
        let g = mapped.sections[0].geom;
        assert_eq!((g.width, g.height), (612.0, 792.0));
        assert_eq!(g.margin_left, 72.0);
        assert!(mapped.warnings.is_empty());
    }

    /// 两节文档:各节独立的块序列与几何;多栏节告警。
    #[test]
    fn sections_split_blocks_and_flag_multi_column() {
        let mut doc = doc_with_body(vec![
            DocBlock::Paragraph(para_with_text("one")),
            DocBlock::Paragraph(para_with_text("two")),
        ]);
        doc.sections = vec![
            Section {
                end_block: 1,
                ..Section::default()
            },
            Section {
                page_width: 16_838, // A4 横向
                page_height: 11_906,
                cols: 2,
                end_block: 2,
                ..Section::default()
            },
        ];
        let mapped = map_document(&doc);
        assert_eq!(mapped.sections.len(), 2);
        assert_eq!(mapped.sections[0].blocks.len(), 1);
        assert_eq!(mapped.sections[1].blocks.len(), 1);
        let g = mapped.sections[1].geom;
        assert!((g.width - 841.9).abs() < 0.05 && (g.height - 595.3).abs() < 0.05);
        assert!(mapped
            .warnings
            .iter()
            .any(|w| w.kind() == "multi-column-flattened"));
    }

    /// 段内 `w:br@page`:段落切成两片,中间是 PageBreak。
    #[test]
    fn page_break_inside_run_splits_paragraph() {
        let mut run = TextRun::from_text("before");
        run.segments.push(RunSegment::Break(BreakKind::Page));
        run.segments.push(RunSegment::Text("after".into()));
        let doc = doc_with_body(vec![DocBlock::Paragraph(Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        })]);
        let blocks = &map_document(&doc).sections[0].blocks;
        assert_eq!(blocks.len(), 3);
        let Block::Paragraph(_, runs) = &blocks[0] else {
            panic!("first part should be a paragraph");
        };
        assert_eq!(runs[0].text, "before");
        assert!(matches!(blocks[1], Block::PageBreak));
        let Block::Paragraph(_, runs) = &blocks[2] else {
            panic!("second part should be a paragraph");
        };
        assert_eq!(runs[0].text, "after");
    }

    /// 空段落:传空文本 run(段落标记样式)占一行高,不是零 run。
    #[test]
    fn empty_paragraph_gets_mark_style_empty_run() {
        let mut doc = doc_with_body(vec![DocBlock::Paragraph(Paragraph::default())]);
        doc.styles.doc_default_rpr.sz = Some(14.0);
        let blocks = &map_document(&doc).sections[0].blocks;
        let Block::Paragraph(_, runs) = &blocks[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].text, "");
        assert_eq!(runs[0].style.size, 14.0, "空 run 携带段落标记样式的字号");
    }

    /// 有效样式驱动:直格 bold/italic/color/highlight/下划线/删除线落进引擎样式;
    /// vanish 跳过;caps 大写;上标缩字号。
    #[test]
    fn run_styles_map_decorations_and_transforms() {
        let mut styled = TextRun::from_text("word");
        styled.rpr.b = Some(true);
        styled.rpr.i = Some(true);
        styled.rpr.u = Some(doc_core::style::UnderlineKind::Single);
        styled.rpr.strike = Some(true);
        styled.rpr.color = Some(ColorRef::Rgb(Color::new([0xFF, 0x00, 0x00])));
        styled.rpr.highlight = Some(Highlight::On(Color::new([0xFF, 0xFF, 0x00])));
        let mut hidden = TextRun::from_text("gone");
        hidden.rpr.vanish = Some(true);
        let mut caps = TextRun::from_text("shout");
        caps.rpr.caps = Some(true);
        let mut sup = TextRun::from_text("2");
        sup.rpr.vert_align = Some(VertAlign::Superscript);
        sup.rpr.sz = Some(10.0);

        let doc = doc_with_body(vec![DocBlock::Paragraph(Paragraph {
            runs: vec![styled, hidden, caps, sup],
            ..Paragraph::default()
        })]);
        let blocks = &map_document(&doc).sections[0].blocks;
        let Block::Paragraph(_, runs) = &blocks[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(runs.len(), 3, "vanish run 被跳过");
        let s = &runs[0].style;
        assert!(s.bold && s.italic && s.underline && s.strike);
        assert_eq!(s.color, Rgb::new(1.0, 0.0, 0.0));
        assert_eq!(s.highlight, Some(Rgb::new(1.0, 1.0, 0.0)));
        assert_eq!(runs[1].text, "SHOUT");
        assert!((runs[2].style.size - 6.5).abs() < 1e-9, "上标 10pt × 0.65");
    }

    /// eastAsia 字体切分:CJK 字符段喂 ea 字体,拉丁段喂 ascii 字体;中性空白跟随。
    #[test]
    fn east_asia_font_slot_splits_runs_by_char_class() {
        let mut run = TextRun::from_text("AB 中文 CD");
        run.rpr.fonts.ascii = Some(doc_core::style::FontRef::Named("Helvetica".into()));
        run.rpr.fonts.east_asia = Some(doc_core::style::FontRef::Named("Songti SC".into()));
        let doc = doc_with_body(vec![DocBlock::Paragraph(Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        })]);
        let blocks = &map_document(&doc).sections[0].blocks;
        let Block::Paragraph(_, runs) = &blocks[0] else {
            panic!("expected a paragraph");
        };
        let fams: Vec<(&str, &str)> = runs
            .iter()
            .map(|r| (r.text.as_str(), r.style.family.as_str()))
            .collect();
        assert_eq!(
            fams,
            vec![
                ("AB ", "Helvetica"),
                ("中文 ", "Songti SC"),
                ("CD", "Helvetica"),
            ]
        );
    }

    /// pageBreakBefore:段前插 PageBreak;节首块不插(本就新起一页)。
    #[test]
    fn page_break_before_inserts_break_except_at_section_start() {
        let mut first = para_with_text("first");
        first.ppr.page_break_before = Some(true);
        let mut second = para_with_text("second");
        second.ppr.page_break_before = Some(true);
        let doc = doc_with_body(vec![
            DocBlock::Paragraph(first),
            DocBlock::Paragraph(second),
        ]);
        let blocks = &map_document(&doc).sections[0].blocks;
        assert_eq!(blocks.len(), 3);
        assert!(matches!(blocks[0], Block::Paragraph(..)), "节首不插");
        assert!(matches!(blocks[1], Block::PageBreak));
        assert!(matches!(blocks[2], Block::Paragraph(..)));
    }

    /// 段落 spacing / ind / jc 落进引擎段落属性(hanging = 负首行缩进)。
    #[test]
    fn para_props_map_spacing_indents_and_align() {
        let mut p = para_with_text("x");
        p.ppr.jc = Some(Justification::Justify);
        p.ppr.space_before = Some(240);
        p.ppr.space_after = Some(120);
        p.ppr.ind_left = Some(720);
        p.ppr.ind_hanging = Some(360);
        p.ppr.line = Some(doc_core::style::LineSpacingRule::Auto(360));
        let doc = doc_with_body(vec![DocBlock::Paragraph(p)]);
        let blocks = &map_document(&doc).sections[0].blocks;
        let Block::Paragraph(props, _) = &blocks[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(props.align, Align::Justify);
        assert_eq!(props.space_before, 12.0);
        assert_eq!(props.space_after, 6.0);
        assert_eq!(props.indent_left, 36.0);
        assert_eq!(props.first_line_indent, -18.0);
        assert_eq!(props.spacing, LineSpacing::Multiple(1.5));
    }

    /// 段落边框 / 底纹 / 图片:一次性降级告警(每种一次)。
    #[test]
    fn degradations_warn_once_per_kind() {
        let mut bordered = para_with_text("b");
        bordered.ppr.borders.top = Some(doc_core::style::Border {
            val: "single".into(),
            sz_eighth_pt: 4,
            space_pt: 0,
            color: None,
        });
        let mut bordered2 = bordered.clone();
        bordered2.ppr.shd_fill = Some(ColorRef::Rgb(Color::new([1, 2, 3])));
        let mut with_pic = para_with_text("p");
        with_pic.runs[0]
            .pictures
            .push(doc_core::model::Picture::default());
        let doc = doc_with_body(vec![
            DocBlock::Paragraph(bordered),
            DocBlock::Paragraph(bordered2),
            DocBlock::Paragraph(with_pic),
        ]);
        let warnings = map_document(&doc).warnings;
        let kinds: Vec<&str> = warnings.iter().map(RenderWarning::kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == "para-border-omitted")
                .count(),
            1,
            "两段带边框只报一次"
        );
        assert!(kinds.contains(&"para-shading-omitted"));
        assert!(kinds.contains(&"picture-skipped"));
    }

    /// 表格映射:grid 列宽 dxa→pt 的 Fixed 列;gridSpan 补占位格;底纹;表格内段落
    /// 走 `*_in_table` 解析(表格样式链参与级联)。
    #[test]
    fn table_maps_grid_span_fill_and_table_style_context() {
        let mut doc = doc_with_body(Vec::new());
        // 表格样式:字号 9pt(表格内段落应吃到)。
        doc.styles.styles.insert(
            "TStyle".into(),
            doc_core::style::Style {
                kind: doc_core::style::StyleKind::Table,
                rpr: doc_core::style::RunProps {
                    sz: Some(9.0),
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        let table = Table {
            grid_cols: vec![2400, 2400, 2400],
            style: Some("TStyle".into()),
            rows: vec![Row {
                cells: vec![
                    Cell {
                        blocks: vec![DocBlock::Paragraph(para_with_text("wide"))],
                        grid_span: 2,
                        fill: Some(Color::new([0xFF, 0xCC, 0x00])),
                        ..Cell::default()
                    },
                    Cell {
                        blocks: vec![DocBlock::Paragraph(para_with_text("c3"))],
                        grid_span: 1,
                        ..Cell::default()
                    },
                ],
                height: Some(400),
                ..Row::default()
            }],
            ..Table::default()
        };
        doc.body = vec![DocBlock::Table(table)];
        doc.sections[0].end_block = 1;

        let blocks = &map_document(&doc).sections[0].blocks;
        let Block::Table(spec) = &blocks[0] else {
            panic!("expected a table");
        };
        assert_eq!(
            spec.columns,
            vec![
                ColumnWidth::Fixed(120.0),
                ColumnWidth::Fixed(120.0),
                ColumnWidth::Fixed(120.0)
            ]
        );
        let row = &spec.rows[0];
        assert_eq!(row.cells.len(), 3, "gridSpan=2 后补一个占位格");
        assert_eq!(row.min_height, Some(20.0));
        assert_eq!(row.cells[0].fill, Some(Rgb::new(1.0, 0.8, 0.0)));
        assert!(row.cells[1].blocks.is_empty(), "占位格无内容");
        let Block::Paragraph(_, runs) = &row.cells[0].blocks[0] else {
            panic!("cell content should be a paragraph");
        };
        assert_eq!(runs[0].style.size, 9.0, "表格样式链在表内生效");
    }

    // ------------------------------------------------------------ C-6:列表标签

    /// 造一张两级编号表:lvl0 = 十进制 `%1.`(ind 720/hanging 360),
    /// lvl1 = 小写字母 `%2.`(ind 1440/hanging 360);numId 1、2 都指向它。
    fn install_numbering(doc: &mut Document) {
        use doc_core::numbering::{AbstractNum, Num, NumFmt, NumLevel};
        let lvl = |fmt: NumFmt, text: &str, left: i64| NumLevel {
            fmt,
            lvl_text: Some(text.to_string()),
            ppr: doc_core::style::ParaProps {
                ind_left: Some(left),
                ind_hanging: Some(360),
                ..Default::default()
            },
            ..NumLevel::default()
        };
        doc.numbering.abstracts.insert(
            0,
            AbstractNum {
                levels: [
                    (0u32, lvl(NumFmt::Decimal, "%1.", 720)),
                    (1u32, lvl(NumFmt::LowerLetter, "%2.", 1440)),
                ]
                .into_iter()
                .collect(),
                ..AbstractNum::default()
            },
        );
        for id in [1u32, 2u32] {
            doc.numbering.nums.insert(
                id,
                doc_core::numbering::Num {
                    abstract_id: 0,
                    ..Num::default()
                },
            );
        }
    }

    fn list_para(text: &str, num_id: u32, ilvl: u32) -> DocBlock {
        let mut p = para_with_text(text);
        p.num_id = Some(num_id);
        p.list_level = Some(ilvl);
        DocBlock::Paragraph(p)
    }

    /// 两级嵌套计数与重置、独立 numId 各自计数;悬挂缩进折成 gutter、
    /// 文本(含首行)对齐 left 缩进;层级缩进经级联生效。
    #[test]
    fn list_labels_count_reset_and_fold_hanging_into_gutter() {
        let mut doc = doc_with_body(vec![
            list_para("one", 1, 0),
            list_para("sub a", 1, 1),
            list_para("sub b", 1, 1),
            list_para("two", 1, 0),
            list_para("fresh", 2, 0),
            DocBlock::Paragraph(para_with_text("plain")),
        ]);
        install_numbering(&mut doc);
        let blocks = &map_document(&doc).sections[0].blocks;
        let labels: Vec<Option<String>> = blocks
            .iter()
            .map(|b| {
                let Block::Paragraph(props, _) = b else {
                    panic!("expected paragraphs only");
                };
                props.list.as_ref().map(|l| l.text.clone())
            })
            .collect();
        assert_eq!(
            labels,
            vec![
                Some("1.".into()),
                Some("a.".into()),
                Some("b.".into()),
                Some("2.".into()),
                Some("1.".into()), // 新 numId:独立计数。
                None,              // 无 numPr 的普通段。
            ]
        );
        let Block::Paragraph(props, _) = &blocks[0] else {
            panic!("expected a paragraph");
        };
        assert_eq!(props.indent_left, 36.0, "层级 pPr 缩进 720 twip");
        assert_eq!(
            props.first_line_indent, 0.0,
            "悬挂折进 gutter,文本对齐 left"
        );
        let label = props.list.as_ref().expect("list label");
        assert!((label.gutter - 6.3).abs() < 1e-6, "0.35 × 18pt 悬挂");
        let Block::Paragraph(props, _) = &blocks[1] else {
            panic!("expected a paragraph");
        };
        assert_eq!(props.indent_left, 72.0, "二级缩进 1440 twip");
    }

    /// 段内换页把列表段切成多片:标签只画在首片。
    #[test]
    fn list_label_only_on_first_part_after_page_break() {
        let mut run = TextRun::from_text("before");
        run.segments.push(RunSegment::Break(BreakKind::Page));
        run.segments.push(RunSegment::Text("after".into()));
        let mut p = Paragraph {
            runs: vec![run],
            ..Paragraph::default()
        };
        p.num_id = Some(1);
        p.list_level = Some(0);
        let mut doc = doc_with_body(vec![DocBlock::Paragraph(p)]);
        install_numbering(&mut doc);
        let blocks = &map_document(&doc).sections[0].blocks;
        let Block::Paragraph(first, _) = &blocks[0] else {
            panic!("expected a paragraph");
        };
        assert!(first.list.is_some(), "首片带标签");
        let Block::Paragraph(second, _) = &blocks[2] else {
            panic!("expected a paragraph");
        };
        assert!(second.list.is_none(), "换页后的尾片不重复编号");
    }

    /// numId=0(显式去编号)与未登记 numId:不产标签;numStyleLink 间接:
    /// 一次性降级告警。
    #[test]
    fn list_degradations_no_label_and_num_style_link_warning() {
        let mut doc = doc_with_body(vec![
            list_para("off", 0, 0),
            list_para("dangling", 9, 0),
            list_para("linked", 3, 0),
            list_para("linked again", 3, 0),
        ]);
        install_numbering(&mut doc);
        doc.numbering.abstracts.insert(
            1,
            doc_core::numbering::AbstractNum {
                num_style_link: Some("ListStyle".into()),
                ..Default::default()
            },
        );
        doc.numbering.nums.insert(
            3,
            doc_core::numbering::Num {
                abstract_id: 1,
                ..Default::default()
            },
        );
        let mapped = map_document(&doc);
        for b in &mapped.sections[0].blocks {
            let Block::Paragraph(props, _) = b else {
                panic!("expected paragraphs only");
            };
            assert!(props.list.is_none());
        }
        let kinds: Vec<&str> = mapped.warnings.iter().map(RenderWarning::kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|k| **k == "numbering-indirection-skipped")
                .count(),
            1,
            "两个 linked 段只报一次"
        );
    }
}
