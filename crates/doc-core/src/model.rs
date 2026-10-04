//! Word 文档(WordprocessingML)结构化解析的结果模型。
//!
//! 目标是**信息无损**:把 `word/document.xml` 里的段落 / 文字 / 表格 / 图片原样搬进这些
//! 朴素的 `struct` / `enum`。本轮派生 `Debug`/`Clone`/`PartialEq`,不要求 serde。
//!
//! **设计要点(表格是重点):**
//! - 文档正文是一串 [`Block`]:要么段落(`w:p`),要么表格(`w:tbl`)。二者在 body 里同级出现。
//! - 表格 [`Cell`] 内部又是一串 [`Block`] —— 所以**嵌套表**(单元格里再放表)天然支持:
//!   它只是某个 cell 的 `blocks` 里出现一个 [`Block::Table`]。
//! - 合并:横向用 [`Cell::grid_span`](`w:gridSpan`),纵向用 [`Cell::v_merge`](`w:vMerge`,
//!   区分 `restart` 起始格与 `continue` 延续格)。

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::geom::{Emu, Twips};
use crate::numbering::NumberingTable;
use crate::page_number::PageNumFormat;
use crate::style::{
    CellBorders, CellMargins, Justification, ParaProps, RunProps, StyleTable, TableBorders, Theme,
};

/// 一份解析好的 Word 文档。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    /// 正文块序列,按 `word/document.xml` 中 `w:body` 的文档顺序排列。
    pub body: Vec<Block>,
    /// 节(section)序列,按文档顺序。每个 `.docx` 至少有一节(body 末尾的 `w:sectPr`);
    /// 节中段落 `w:pPr > w:sectPr` 结束**包含它的**那一节。解析层保证非空(缺失时补
    /// Word 默认节);块归属经 [`Section::end_block`] 划分。
    pub sections: Vec<Section>,
    /// 样式表(`word/styles.xml`:docDefaults + 样式定义)。部件缺失时为空表。
    /// 有效样式经 [`crate::style::resolve_run`] / [`crate::style::resolve_para`] 级联合并。
    pub styles: StyleTable,
    /// 主题(`word/theme/theme1.xml`:fontScheme + clrScheme)。部件缺失时为空主题。
    pub theme: Theme,
    /// 编号表(`word/numbering.xml`:abstractNum 层级 + num 实例)。部件缺失时为空表。
    /// 计数与标签经 [`crate::numbering::ListCounters`] 在渲染侧推进。
    pub numbering: NumberingTable,
    /// 缺省制表位间隔(twip,`word/settings.xml > w:defaultTabStop@w:val`;
    /// 部件/属性缺失时 `None`,渲染侧落 Word 缺省 720 twip = 0.5 英寸)。
    pub default_tab_stop: Option<Twips>,
    /// 页眉 / 页脚部件内容(`word/header*.xml` / `word/footer*.xml` 的块序列),键为主文档
    /// 关系 id([`HeaderFooterRef::rel_id`])。只收有节引用且部件存在的;指向同一部件的
    /// 多个关系 id 已归一成同一个键,所以一个部件只出现一次。
    pub header_footers: BTreeMap<String, Vec<Block>>,
    /// 奇偶页不同页眉页脚(`word/settings.xml > w:evenAndOddHeaders`,缺省 `false`):
    /// 为真时偶数页用 `even` 类型,否则全用 `default`(见 [`Document::header_footer_for_page`])。
    pub even_and_odd_headers: bool,
    /// 脚注(`word/footnotes.xml`):`w:id` -> 内容块。`w:type` 为 `separator` /
    /// `continuationSeparator` / `continuationNotice` 的非内容注已跳过。
    /// 正文里的引用见 [`RunSegment::NoteRef`]。
    pub footnotes: BTreeMap<i64, Vec<Block>>,
    /// 尾注(`word/endnotes.xml`):语义同 [`Document::footnotes`]。
    pub endnotes: BTreeMap<i64, Vec<Block>>,
    /// 批注(`word/comments.xml`):`w:id` -> [`Comment`]。批注是审阅元数据而非正文,
    /// 默认**不进** `to_text` / `to_markdown` / `to_html`;正文里的锚点见
    /// [`RunSegment::CommentRef`]。
    pub comments: BTreeMap<i64, Comment>,
    /// 正文里 `w:altChunk`(外部内容块导入:内嵌的 HTML / RTF / 另一份 docx 等)的个数。
    /// 其内容**不解析**、不进任何导出,所以文档里这部分内容缺失;计数让调用方知道有内容没被
    /// 导入(PDF 导出另有 `alt-chunk-skipped` 告警)。只计主文档部件,页眉页脚 / 注内的不计。
    pub alt_chunk_count: usize,
    /// 解析诊断:内容被静默截断 / 跳过 / 钳制时的结构化记录(见 [`Diagnostic`])。正常文件为空;
    /// 只含种类 / 部件路径 / 计数,**绝不含文档正文**。
    pub diagnostics: Vec<Diagnostic>,
    /// 文档属性(`docProps/core.xml`);部件缺失 / 畸形时全 `None`。**隐私**:不进任何告警 / 诊断 / trace。
    pub core_properties: CoreProperties,
}

/// 文档核心属性(`docProps/core.xml`):标题 / 作者 / 主题 / 关键词 / 创建·修改时间等,
/// 全部原文字符串(日期不解析),缺失 / 空元素为 `None`。字段名与兄弟库 pptspine 的 `core_properties()` 对齐。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CoreProperties {
    /// `dc:title`。
    pub title: Option<String>,
    /// `dc:subject`。
    pub subject: Option<String>,
    /// `dc:creator`(作者)。
    pub creator: Option<String>,
    /// `cp:keywords`。
    pub keywords: Option<String>,
    /// `dc:description`(备注)。
    pub description: Option<String>,
    /// `cp:category`。
    pub category: Option<String>,
    /// `cp:lastModifiedBy`。
    pub last_modified_by: Option<String>,
    /// `cp:revision`。
    pub revision: Option<String>,
    /// `dcterms:created`(W3CDTF 原文)。
    pub created: Option<String>,
    /// `dcterms:modified`(W3CDTF 原文)。
    pub modified: Option<String>,
    /// `dc:language`。
    pub language: Option<String>,
}

/// 解析诊断的种类。`#[non_exhaustive]`:后续可能新增。[`DiagnosticKind::code`] 是稳定的
/// 短横线标识(与渲染告警 `RenderWarning` 的 code 同风格)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum DiagnosticKind {
    /// 某部件 XML 中途损坏 / 被截断:只解析出了前半部分。
    XmlTruncated,
    /// 递归容器嵌套超过深度上限,子树被整棵跳过(计数 = 跳过的子树数)。
    NestingDepthExceeded,
    /// 表格列数被钳到 Word 上限:`w:tblGrid` 多余的列被丢弃、`gridBefore` / `gridAfter` 被钳
    /// (计数 = 丢弃的列数 + 被钳的次数)。
    TableColumnsClamped,
    /// 单元格 `w:gridSpan` 超过列数上限被钳(计数 = 被钳的单元格数)。
    GridSpanClamped,
    /// 编号起值超过 Word 上限(`MAX_LIST_NUMBER`):字母 / 罗马格式回退十进制、计数饱和
    /// (计数 = 超限的层级 / 覆盖数)。
    NumberingValueClamped,
    /// 关系 / 引用指向缺失的部件(页眉页脚、图片等);`part` 是**持有该引用**的部件
    /// (计数 = 悬空引用数)。
    MissingPart,
    /// 遇到 `w:altChunk`(外部内容块)而未导入(计数 = 个数)。
    AltChunkNotImported,
    /// 样式 `basedOn` 链深度超过 `MAX_STYLE_CHAIN`:最基祖先被截断、不参与级联
    /// (计数 = 链过深的样式数)。
    StyleChainTruncated,
    /// 脚注 / 尾注 / 批注部件的条目数超过 [`MAX_NOTES`],多余条目被丢弃(计数 = 丢弃的条目数)。
    NotesTruncated,
    /// 某个字段指令超过 [`MAX_FIELD_INSTR`] 被截断(计数 = 被截断的字段数)。
    FieldInstrTruncated,
    /// 节数超过 [`MAX_SECTIONS`],中间多余的节被并入最后一节(计数 = 被并入的节数)。
    SectionsTruncated,
}

impl DiagnosticKind {
    /// 稳定的短横线 code(如 `"xml-truncated"`)。
    pub fn code(self) -> &'static str {
        match self {
            DiagnosticKind::XmlTruncated => "xml-truncated",
            DiagnosticKind::NestingDepthExceeded => "nesting-depth-exceeded",
            DiagnosticKind::TableColumnsClamped => "table-columns-clamped",
            DiagnosticKind::GridSpanClamped => "grid-span-clamped",
            DiagnosticKind::NumberingValueClamped => "numbering-value-clamped",
            DiagnosticKind::MissingPart => "missing-part",
            DiagnosticKind::AltChunkNotImported => "alt-chunk-not-imported",
            DiagnosticKind::StyleChainTruncated => "style-chain-truncated",
            DiagnosticKind::NotesTruncated => "notes-truncated",
            DiagnosticKind::FieldInstrTruncated => "field-instr-truncated",
            DiagnosticKind::SectionsTruncated => "sections-truncated",
        }
    }
}

/// 一条解析诊断:种类 + 部件路径(如 `word/document.xml`)+ 计数。同一 `(种类, 部件)` 合并成一条。
/// 隐私:不含任何文档正文。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub kind: DiagnosticKind,
    pub part: String,
    pub count: usize,
}

#[cfg(test)]
thread_local! {
    /// 仅测试编译:页眉页脚有效引用解析时访问的节次数。
    static HF_STEPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

impl Document {
    /// 某一页实际生效的 `(页眉, 页脚)` 部件键([`Document::header_footers`] 的键;`None` = 无)。
    ///
    /// `section` 是节序号(0 起),`page_in_section` 是该节内的页序号(0 起),`page_number`
    /// 是**显示页码**(受 `w:pgNumType@w:start` 影响,可为 0;只用于奇偶判定)。纯函数,规则同 Word:
    /// - 本节 `w:titlePg` 为真且是节内首页 → `first`;
    /// - 否则 `w:evenAndOddHeaders` 为真且页码为偶数 → `even`(依据 Word 实际行为:奇偶页看
    ///   页码本身,所以 `w:start` 把节首页设成偶数时首页就用 `even`,与物理页序无关);
    /// - 否则 → `default`。
    ///
    /// 选定类型在本节没有引用时逐节向前回溯同类型引用;一直没有就是无页眉 / 页脚。
    /// 节序号越界返回 `(None, None)`。
    pub fn header_footer_for_page(
        &self,
        section: usize,
        page_in_section: usize,
        page_number: i64,
    ) -> (Option<&str>, Option<&str>) {
        let Some(sect) = self.sections.get(section) else {
            return (None, None);
        };
        let kind = page_kind(
            sect.title_pg,
            self.even_and_odd_headers,
            page_in_section,
            page_number,
        );
        let pick = |refs: fn(&Section) -> &[HeaderFooterRef]| {
            self.sections[..=section].iter().rev().find_map(|s| {
                refs(s)
                    .iter()
                    .find(|r| r.kind == kind)
                    .map(|r| r.rel_id.as_str())
            })
        };
        (pick(|s| &s.headers), pick(|s| &s.footers))
    }
}

/// 某页用哪一类页眉 / 页脚(首页 > 偶数页 > 通用;规则见 [`Document::header_footer_for_page`])。
fn page_kind(
    title_pg: bool,
    even_and_odd_headers: bool,
    page_in_section: usize,
    page_number: i64,
) -> HeaderFooterKind {
    if title_pg && page_in_section == 0 {
        HeaderFooterKind::First
    } else if even_and_odd_headers && page_number.rem_euclid(2) == 0 {
        HeaderFooterKind::Even
    } else {
        HeaderFooterKind::Default
    }
}

/// 逐节预计算的有效页眉 / 页脚引用:按节前缀传播一次(本节没有某类型的引用就沿用上一节的),
/// 之后每页的查询是 O(1),而不是 [`Document::header_footer_for_page`] 那样每页向前回溯 O(节数)。
/// 语义与它完全一致(见单测的逐页等价校验)。
pub struct HeaderFooterIndex<'a> {
    /// 每节、按 `[default, first, even]` 排的有效部件键。
    headers: Vec<[Option<&'a str>; 3]>,
    footers: Vec<[Option<&'a str>; 3]>,
    title_pg: Vec<bool>,
    even_and_odd_headers: bool,
}

fn kind_slot(kind: HeaderFooterKind) -> usize {
    match kind {
        HeaderFooterKind::Default => 0,
        HeaderFooterKind::First => 1,
        HeaderFooterKind::Even => 2,
    }
}

/// 在上一节的有效引用 `prev` 上叠加本节 `refs`:有某类型的引用就覆盖,否则沿用。
/// 同类型多个引用取最先出现的(与逐节 `find` 一致,故从后往前覆盖)。
fn propagate_refs<'a>(
    refs: &'a [HeaderFooterRef],
    prev: Option<&[Option<&'a str>; 3]>,
) -> [Option<&'a str>; 3] {
    let mut eff = prev.copied().unwrap_or([None; 3]);
    for r in refs.iter().rev() {
        eff[kind_slot(r.kind)] = Some(r.rel_id.as_str());
    }
    eff
}

impl Document {
    /// 预计算每节的有效页眉 / 页脚引用(O(节数 x 引用数),只做一次)。
    pub fn header_footer_index(&self) -> HeaderFooterIndex<'_> {
        let mut headers: Vec<[Option<&str>; 3]> = Vec::with_capacity(self.sections.len());
        let mut footers: Vec<[Option<&str>; 3]> = Vec::with_capacity(self.sections.len());
        for sect in &self.sections {
            #[cfg(test)]
            HF_STEPS.with(|c| c.set(c.get() + 1));
            // 本节有某类型的引用则覆盖,否则沿用上一节。
            let h = propagate_refs(&sect.headers, headers.last());
            let f = propagate_refs(&sect.footers, footers.last());
            headers.push(h);
            footers.push(f);
        }
        HeaderFooterIndex {
            headers,
            footers,
            title_pg: self.sections.iter().map(|s| s.title_pg).collect(),
            even_and_odd_headers: self.even_and_odd_headers,
        }
    }
}

impl<'a> HeaderFooterIndex<'a> {
    /// 同 [`Document::header_footer_for_page`],O(1)。
    pub fn for_page(
        &self,
        section: usize,
        page_in_section: usize,
        page_number: i64,
    ) -> (Option<&'a str>, Option<&'a str>) {
        let (Some(h), Some(f), Some(&title_pg)) = (
            self.headers.get(section),
            self.footers.get(section),
            self.title_pg.get(section),
        ) else {
            return (None, None);
        };
        let slot = kind_slot(page_kind(
            title_pg,
            self.even_and_odd_headers,
            page_in_section,
            page_number,
        ));
        (h[slot], f[slot])
    }
}

/// 一条批注(`w:comment`)。作者 / 日期 / 内容属于文档内容,可进模型,但**不得**写进
/// trace / 日志 / 告警。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Comment {
    /// `w:id`(正文 [`RunSegment::CommentRef`] 的键)。
    pub id: i64,
    /// 作者(`w:author`),缺失 `None`。
    pub author: Option<String>,
    /// 日期(`w:date`,ISO 8601 原文,不解析),缺失 `None`。
    pub date: Option<String>,
    /// 作者缩写(`w:initials`),缺失 `None`。
    pub initials: Option<String>,
    /// 批注内容块(段落 / 表格,与正文同一套块级解析)。
    pub blocks: Vec<Block>,
}

/// 页眉 / 页脚的类型(`w:headerReference` / `w:footerReference` 的 `@w:type`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeaderFooterKind {
    /// 通用(`default`;未知取值按它容错)。
    #[default]
    Default,
    /// 首页(`first`,配合 `w:titlePg`)。
    First,
    /// 偶数页(`even`,配合 `w:evenAndOddHeaders`)。
    Even,
}

impl HeaderFooterKind {
    /// 解析 `@w:type`。未知值按 `default` 容错。
    pub fn from_attr(s: &str) -> Self {
        match s {
            "first" => HeaderFooterKind::First,
            "even" => HeaderFooterKind::Even,
            _ => HeaderFooterKind::Default,
        }
    }

    /// 稳定的小写名(`default` / `first` / `even`)。
    pub fn as_str(self) -> &'static str {
        match self {
            HeaderFooterKind::Default => "default",
            HeaderFooterKind::First => "first",
            HeaderFooterKind::Even => "even",
        }
    }
}

/// 一节对某个页眉 / 页脚部件的引用(`w:sectPr > w:headerReference` / `w:footerReference`)。
/// 内容在 [`Document::header_footers`] 里按 `rel_id` 取;解析层只保留能解析到存在部件的引用。
/// 原样记录引用;“缺省则继承上一节”与首页 / 奇偶页的生效判断见 [`Document::header_footer_for_page`]。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderFooterRef {
    pub kind: HeaderFooterKind,
    /// 主文档关系 id(`r:id`;指向同一部件的多个 id 已归一成第一个)。
    pub rel_id: String,
}

/// 注的种类。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// 脚注(`w:footnoteReference`)。
    Footnote,
    /// 尾注(`w:endnoteReference`)。
    Endnote,
}

/// 一节(`w:sectPr`)的页面几何:页面尺寸 / 页边距 / 纸向 / 分栏。
///
/// 缺省值取 Word 的默认页面设置:Letter 纵向(12240 x 15840 twip)、四边 1 英寸
/// (1440 twip)边距、页眉/页脚 720 twip、单栏。
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    /// 页面宽(twip,`w:pgSz@w:w`)。
    pub page_width: Twips,
    /// 页面高(twip,`w:pgSz@w:h`)。
    pub page_height: Twips,
    /// 纸向(`w:pgSz@w:orient`,缺省纵向)。
    pub orientation: Orientation,
    /// 页边距(`w:pgMar`)。
    pub margins: PageMargins,
    /// 分栏数(`w:cols@w:num`,缺省 1)。
    pub cols: u32,
    /// 本节的页眉引用(`w:headerReference`),按出现顺序;内容见 [`Document::header_footers`]。
    pub headers: Vec<HeaderFooterRef>,
    /// 本节的页脚引用(`w:footerReference`),语义同 [`Section::headers`]。
    pub footers: Vec<HeaderFooterRef>,
    /// 首页不同页眉页脚(`w:titlePg`,缺省 `false`):为真时本节首页用 `first` 类型。
    pub title_pg: bool,
    /// 本节首页的页码起始值(`w:pgNumType@w:start`)。`None` = 缺失 / 非法(负数、非数字、
    /// 超 `u32`)= 接续上一节页码;`Some(0)` 合法。
    pub page_number_start: Option<u32>,
    /// 本节页码格式(`w:pgNumType@w:fmt`,缺省 `decimal`;按本节自己的声明,不接续上一节)。
    pub page_number_format: PageNumFormat,
    /// 本节覆盖的正文块区间的**排他性**结束下标(相对 [`Document::body`])。
    /// 本节的块为 `body[上一节.end_block .. 本节.end_block]`,首节从 0 起。
    pub end_block: usize,
}

impl Section {
    /// 本节首页的页码:有 `w:start` 取它,否则接续(`continued` = 上一节末页页码 + 1,
    /// 文档首节传 1)。
    pub fn first_page_number(&self, continued: i64) -> i64 {
        self.page_number_start.map_or(continued, i64::from)
    }
}

impl Default for Section {
    fn default() -> Self {
        Section {
            page_width: 12_240,
            page_height: 15_840,
            orientation: Orientation::Portrait,
            margins: PageMargins::default(),
            cols: 1,
            headers: Vec::new(),
            footers: Vec::new(),
            title_pg: false,
            page_number_start: None,
            page_number_format: PageNumFormat::Decimal,
            end_block: 0,
        }
    }
}

/// 纸向(`w:pgSz@w:orient`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Orientation {
    /// 纵向(缺省)。
    #[default]
    Portrait,
    /// 横向。
    Landscape,
}

/// 页边距(twip,`w:pgMar` 的各属性)。缺省值取 Word 默认:四边 1440、页眉/页脚 720、
/// 装订线 0。`top` / `bottom` 允许为负(Word 语义:负值表示正文可侵入页眉/页脚区)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PageMargins {
    pub top: Twips,
    pub right: Twips,
    pub bottom: Twips,
    pub left: Twips,
    /// 页眉距页顶(`@w:header`)。
    pub header: Twips,
    /// 页脚距页底(`@w:footer`)。
    pub footer: Twips,
    /// 装订线(`@w:gutter`)。
    pub gutter: Twips,
}

impl Default for PageMargins {
    fn default() -> Self {
        PageMargins {
            top: 1_440,
            right: 1_440,
            bottom: 1_440,
            left: 1_440,
            header: 720,
            footer: 720,
            gutter: 0,
        }
    }
}

/// 文档正文(或单元格内)的一个块级元素。段落与表格在 body 里同级出现,顺序即文档顺序。
///
/// (`Paragraph` 自 C-4 携带直接格式化 pPr 片段后天然比 `Table` 宽;正文以段落为
/// 主,把**主流**变体装箱只会给热路径加一次指针跳转,故按语义保持内联。)
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    /// 一个段落(`w:p`)。
    Paragraph(Paragraph),
    /// 一张表格(`w:tbl`)。
    Table(Table),
}

/// 一个段落(`w:p`):带样式的 run 序列 + 段落级属性。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Paragraph {
    pub runs: Vec<TextRun>,
    /// 段落样式名(`w:pPr` > `w:pStyle@w:val`,如 `"Heading1"`),原样保留。
    pub style: Option<String>,
    /// 对齐方式(`w:pPr` > `w:jc@w:val`,如 `"center"`/`"left"`/`"right"`/`"both"`),原样保留。
    pub align: Option<String>,
    /// 列表/大纲层级(`w:pPr` > `w:numPr` > `w:ilvl@w:val`),缺省 `None`。
    pub list_level: Option<u32>,
    /// 编号实例 id(`w:pPr` > `w:numPr` > `w:numId@w:val`),缺省 `None`。
    /// `Some(0)` 是 Word 的「显式去编号」语义;标签经 [`Document::numbering`] 解析。
    pub num_id: Option<u32>,
    /// 直接格式化的 pPr 原始片段(**全 Option**,C-4:spacing / ind / pBdr / shd /
    /// keep 系列;`jc` 与上面的 `align` 便利字段并存,契约不变)。
    /// 渲染侧走 [`crate::style::resolve_para`] 消费本片段。
    pub ppr: ParaProps,
}

impl Paragraph {
    /// 便利:把段内所有 run 的文字拼接成整段文本(分段按 [`TextRun::text`] 折叠)。
    /// 浮动文本框内容不在其中(见 [`Paragraph::text_boxes`])。
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text()).collect()
    }

    /// 同 [`Paragraph::text`],注引用按 `mark` 展开(见 [`TextRun::text_with_notes`])。
    pub fn text_with_notes(&self, mark: &dyn Fn(NoteKind, i64) -> Option<String>) -> String {
        self.runs.iter().map(|r| r.text_with_notes(mark)).collect()
    }

    /// 段内各 run 锚定的浮动文本框,按文档顺序。
    pub fn text_boxes(&self) -> impl Iterator<Item = &TextBox> {
        self.runs.iter().flat_map(|r| &r.text_boxes)
    }
}

/// 一段带样式的文字(`w:r`)。内容是**分段序列**([`RunSegment`]):文字(`w:t`)/
/// 制表(`w:tab`)/ 换行换页(`w:br`、`w:cr`)各自独立成段,不再折叠丢失 `w:br@w:type`。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextRun {
    /// run 内容分段(文字 / 制表 / 换行换页),按文档顺序。
    pub segments: Vec<RunSegment>,
    /// 字体名(`w:rPr` > `w:rFonts@w:ascii`,回退 `@w:hAnsi`)。
    pub font: Option<String>,
    /// 字号(磅;WordprocessingML 的 `w:sz` 以**半磅**存储,解析时已除以 2)。
    pub size_pt: Option<f32>,
    pub bold: bool,
    pub italic: bool,
    /// 下划线(`w:rPr` > `w:u@w:val`,非 `"none"` 即为真)。
    pub underline: bool,
    /// 文字颜色(`w:rPr` > `w:color@w:val`,`"RRGGBB"` 十六进制;`"auto"` -> `None`)。
    pub color: Option<Color>,
    /// 该 run 内嵌的图片(`w:drawing` / `w:pict`);一个 run 通常至多一张。
    pub pictures: Vec<Picture>,
    /// 直接格式化的 rPr 原始片段(**全 Option**,能区分「未设置(继承)」与「显式关」,
    /// toggle XOR 语义依赖这一点)。上面的 `font`/`size_pt`/`bold`/… 是它折叠后的便利字段
    /// (缺省 = false/None,契约不变),导出与 Python 侧继续用便利字段;
    /// 渲染侧走 [`crate::style::resolve_run`] 消费本片段。
    pub rpr: RunProps,
    /// 超链接目标(`w:hyperlink`):外链为 `r:id` 经 `word/_rels` 解出的 URI;
    /// 文档内部书签跳转(`w:hyperlink@w:anchor`)存成 `"#书签名"`(渲染侧只存不画 +
    /// 一次性降级告警)。`None` = 该 run 不在任何超链接内。
    pub link_target: Option<String>,
    /// 该 run 锚定的浮动文本框(DrawingML `wps:txbx` / VML `v:textbox` 内的
    /// `w:txbxContent`)。**只做抽取**:导出侧紧随所在段落之后输出其内容;
    /// PDF 渲染不画(一次性降级告警)。
    pub text_boxes: Vec<TextBox>,
    /// 该 run 是公式(`m:oMath`)的纯文本抽取:只保 `m:t` 文字,不含公式排版;
    /// PDF 渲染按普通文字出(一次性降级告警)。
    pub is_math: bool,
    /// 该 run 属于哪个字段的结果(`w:fldSimple@w:instr` / 复杂字段 `w:instrText` 拼接,
    /// 去首尾空白的原文,如 `"PAGE \* MERGEFORMAT"`);`None` = 不在字段结果里。
    /// 正文与导出照用缓存结果文字;PDF 渲染页眉页脚时 `PAGE` / `NUMPAGES` 换成真实值。
    /// 无缓存结果的字段留一个无分段、只带本标记的 run。
    /// `Arc<str>`:同一字段结果区里的所有 run 共享同一份指令;指令至多 [`MAX_FIELD_INSTR`] 字节。
    pub field: Option<Arc<str>>,
}

/// 一个浮动文本框(`w:txbxContent`)的内容:段落与表格的块序列。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct TextBox {
    pub blocks: Vec<Block>,
}

impl TextRun {
    /// 便利:仅含一段纯文字的 run(测试与轻量构造常用)。
    pub fn from_text(text: impl Into<String>) -> Self {
        TextRun {
            segments: vec![RunSegment::Text(text.into())],
            ..TextRun::default()
        }
    }

    /// 把分段折叠成纯文本:`Tab` -> `'\t'`,`Break`(任意种类)-> `'\n'`。
    /// 与历史上的 `text` 字段语义逐字节一致(导出契约依赖这一点)。
    /// 注引用([`RunSegment::NoteRef`])不产生文字。
    pub fn text(&self) -> String {
        self.text_with_notes(&|_, _| None)
    }

    /// 同 [`TextRun::text`],但注引用在原位置按 `mark(种类, id)` 给出的标记串展开
    /// (`None` = 不出标记)。导出侧用它放 `[1]` / `[^1]` 之类的脚注标记。
    pub fn text_with_notes(&self, mark: &dyn Fn(NoteKind, i64) -> Option<String>) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                RunSegment::Text(s) => out.push_str(s),
                RunSegment::Tab => out.push('\t'),
                RunSegment::Break(_) => out.push('\n'),
                RunSegment::NoteRef { kind, id } => out.extend(mark(*kind, *id)),
                RunSegment::CommentRef { .. } => {}
            }
        }
        out
    }

    /// 追加一段文字:与末尾的 `Text` 段合并,保持“无相邻 Text 段”的最简形态。
    pub fn push_text(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        if let Some(RunSegment::Text(t)) = self.segments.last_mut() {
            t.push_str(s);
        } else {
            self.segments.push(RunSegment::Text(s.to_string()));
        }
    }
}

/// run 内容的一个分段。
#[derive(Debug, Clone, PartialEq)]
pub enum RunSegment {
    /// 一段文字(`w:t` / `w:delText` 等的字符内容)。
    Text(String),
    /// 一个制表符(`w:tab`)。
    Tab,
    /// 一个断行/断页/断栏(`w:br`,`w:cr` 视作换行)。
    Break(BreakKind),
    /// 脚注 / 尾注引用(`w:footnoteReference` / `w:endnoteReference`):`id` 对应
    /// [`Document::footnotes`] / [`Document::endnotes`] 的键(可能悬空)。不产生文字:
    /// [`TextRun::text`] 折叠时忽略,导出侧另行按引用顺序编号。
    NoteRef { kind: NoteKind, id: i64 },
    /// 批注引用(`w:commentReference`):`id` 对应 [`Document::comments`] 的键(可能悬空)。
    /// 只记引用点(`commentRangeStart` / `commentRangeEnd` 是 run 之间的兄弟标记,
    /// 模型不表达范围);不产生文字。
    CommentRef { id: i64 },
}

/// 断的种类(`w:br@w:type`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BreakKind {
    /// 换行(`textWrapping`,缺省;`w:cr` 同义)。
    #[default]
    Line,
    /// 换页(`w:br w:type="page"`)。
    Page,
    /// 换栏(`w:br w:type="column"`;单栏渲染时等效换页)。
    Column,
}

/// 表格总宽(`w:tblPr` > `w:tblW`):dxa 绝对宽或百分比宽。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TableWidth {
    /// 绝对宽(twip,`@w:type="dxa"`)。
    Dxa(Twips),
    /// 百分比宽(0..=100,`@w:type="pct"`,原始值以 1/50 个百分点存储)。
    Pct(f32),
}

/// 一张表格(`w:tbl`)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    /// 表格网格列定义(`w:tblGrid` > `w:gridCol@w:w`,单位 twip),给出逻辑列数与各列宽。
    pub grid_cols: Vec<Twips>,
    /// 表格行。
    pub rows: Vec<Row>,
    /// 表格样式名(`w:tblPr` > `w:tblStyle@w:val`),原样保留。
    pub style: Option<String>,
    /// 表级边框(`w:tblPr` > `w:tblBorders`:外四边 + insideH/insideV)。
    /// 冲突消解(`tcBorders` > `tblBorders` > 表样式)在渲染侧进行。
    pub borders: TableBorders,
    /// 表级单元格缺省边距(`w:tblPr` > `w:tblCellMar`)。
    pub cell_margins: CellMargins,
    /// 表格缩进(twip,`w:tblPr` > `w:tblInd@w:w`,仅 `type="dxa"`)。
    pub indent: Option<Twips>,
    /// 表格整体对齐(`w:tblPr` > `w:jc@w:val`)。
    pub jc: Option<Justification>,
    /// 表格总宽(`w:tblPr` > `w:tblW`)。
    pub width: Option<TableWidth>,
}

/// 表格列数上限:Word 的表格最多 63 列(UI 与二进制格式的硬上限)。解析时 `gridSpan` 与
/// `w:tblGrid` 列数都钳到它,[`Table::col_count`] 也不会超过它——防恶意的巨大 `gridSpan`
/// / 海量 `gridCol` 让按 `列数 × 行数` 分配的渲染映射分配失败而进程中止。
pub const MAX_TABLE_COLS: usize = 63;

/// 单张表格渲染映射的网格位(`列数 × 行数`)预算。渲染按该乘积分配占位网格 / 边表 / 引擎格,
/// 超出时只映射前 `预算 / 列数` 行并告警(`table-over-budget`),不中止进程。取值依据:63 列
/// 满宽时约 4 000 行,覆盖实际文档(几千行 × 十来列),占用约几十 MiB。
pub const MAX_TABLE_CELLS: usize = 250_000;

/// 单个字段指令(`w:instrText` 拼接 / `w:fldSimple@w:instr`)保留的最大字节数,超出截断并记
/// `field-instr-truncated` 诊断。取值依据:渲染只认指令的首词(`PAGE` / `NUMPAGES`)与 `\*` 格式开关,
/// 真实指令至多几百字节(`HYPERLINK` 受 URL 上限约 2 KB 约束);4 KiB 已是最长合法写法的两倍,
/// 同时把「指令长度 × 结果 run 数」之类的放大钉死在常数上。
pub const MAX_FIELD_INSTR: usize = 4096;

/// 节数上限。每节至少一页、各自带页眉页脚引用,排版 / 页眉页脚开销随节数线性增长;取值依据:
/// 合并邮件式文档(每收件人一节)也不过几千节,10 000 留足余量,而几百字节 / 节的合成文件
/// 不会把渲染拖到不可控。超出时中间的节并入最后一节(它携带文档级 `sectPr`),计 `sections-truncated`。
pub const MAX_SECTIONS: usize = 10_000;

/// 单个脚注 / 尾注 / 批注部件最多收录的条目数。超出的条目丢弃并记 `notes-truncated` 诊断。
/// 取值依据:真实长文档(法律文书 / 学术专著)脚注至多几千条,10 万留足 20 倍以上余量;
/// 每条约百字节级的固定开销,封顶后单部件的条目表至多十几 MiB,而不被几百 KB 的合成文件撑到不可控。
pub const MAX_NOTES: usize = 100_000;

impl Table {
    /// 逻辑列数:优先取 `w:tblGrid` 的列数;退而取首行的 `grid_before` + 单元格 `grid_span` 之和 +
    /// `grid_after`(饱和求和)。
    /// 结果不超过 [`MAX_TABLE_COLS`]。
    pub fn col_count(&self) -> usize {
        if !self.grid_cols.is_empty() {
            return self.grid_cols.len().min(MAX_TABLE_COLS);
        }
        self.rows
            .first()
            .map(|r| {
                r.cells.iter().fold(
                    (r.grid_before as usize).saturating_add(r.grid_after as usize),
                    |acc, c| acc.saturating_add(c.grid_span as usize),
                )
            })
            .unwrap_or(0)
            .min(MAX_TABLE_COLS)
    }
}

/// 行高规则(`w:trPr` > `w:trHeight@w:hRule`)。缺省 `atLeast`(内容可长高)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HeightRule {
    /// 至少(缺省):`w:val` 是下限,内容可长高。
    #[default]
    AtLeast,
    /// 精确:`w:val` 即行高(v1 渲染按下限近似,内容不截断)。
    Exact,
    /// 自动:高度随内容,`w:val` 忽略。
    Auto,
}

impl HeightRule {
    /// 解析 `@w:hRule`。未知值按缺省 `atLeast` 容错。
    pub fn from_attr(s: &str) -> Self {
        match s {
            "exact" => HeightRule::Exact,
            "auto" => HeightRule::Auto,
            _ => HeightRule::AtLeast,
        }
    }
}

/// 表格的一行(`w:tr`)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// 行高(twip,`w:trPr` > `w:trHeight@w:val`)。
    pub height: Option<Twips>,
    /// 行高规则(`w:trPr` > `w:trHeight@w:hRule`)。
    pub height_rule: HeightRule,
    /// 是否为表头行(`w:trPr` > `w:tblHeader`)。
    pub is_header: bool,
    /// 行不跨页(`w:trPr` > `w:cantSplit`)。v1 渲染对**所有**行整行挪页
    /// (引擎内建),该标志仅保真刻画。
    pub cant_split: bool,
    /// 行首跳过的网格列数(`w:trPr` > `w:gridBefore@w:val`;钳到 [`MAX_TABLE_COLS`])。
    /// 该行第一个单元格从网格第 `grid_before` 列起排,`vMerge` 配对与导出 / 渲染的列号都计入它。
    pub grid_before: u32,
    /// 行末跳过的网格列数(`w:trPr` > `w:gridAfter@w:val`;钳到 [`MAX_TABLE_COLS`])。
    pub grid_after: u32,
}

/// 表格单元格(`w:tc`)。内容是块序列(段落 + 可嵌套的表),所以嵌套表天然落在这里。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Cell {
    /// 单元格内容:段落与(嵌套)表格的块序列。
    pub blocks: Vec<Block>,
    /// 横向跨列数(`w:tcPr` > `w:gridSpan@w:val`),缺省 1。
    pub grid_span: u32,
    /// 纵向合并状态(`w:tcPr` > `w:vMerge`)。见 [`VMerge`]。
    pub v_merge: VMerge,
    /// 单元格宽度(twip,`w:tcPr` > `w:tcW@w:w`,仅当 `w:type="dxa"` 时为绝对 twip)。
    pub width: Option<Twips>,
    /// 单元格百分比宽(0..=100,`w:tcPr` > `w:tcW@w:type="pct"`;渲染侧对正文宽解析)。
    pub width_pct: Option<f32>,
    /// 单元格底纹/填充色(`w:tcPr` > `w:shd@w:fill`,`"RRGGBB"`;`"auto"` -> `None`)。
    pub fill: Option<Color>,
    /// 单元格边框(`w:tcPr` > `w:tcBorders`)。冲突消解在渲染侧(优先于表级)。
    pub borders: CellBorders,
    /// 单元格内容纵向对齐(`w:tcPr` > `w:vAlign@w:val`)。
    pub v_align: Option<CellVAlign>,
    /// 单元格边距覆盖(`w:tcPr` > `w:tcMar`,逐边覆盖表级 tblCellMar)。
    pub margins: CellMargins,
}

/// 单元格内容纵向对齐(`w:vAlign@w:val`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CellVAlign {
    /// 顶对齐(缺省)。
    #[default]
    Top,
    /// 垂直居中。
    Center,
    /// 底对齐。
    Bottom,
}

impl CellVAlign {
    /// 解析 `w:vAlign@w:val`。未知值 → `None`(容错,当未设置)。
    pub fn from_attr(s: &str) -> Option<Self> {
        Some(match s {
            "top" => CellVAlign::Top,
            "center" => CellVAlign::Center,
            "bottom" => CellVAlign::Bottom,
            _ => return None,
        })
    }
}

impl Cell {
    /// 便利:把单元格内**直接段落**的文字按行拼接(忽略嵌套表;嵌套表请遍历 `blocks`)。
    /// 段落锚定的浮动文本框文字紧随该段之后成行。
    pub fn text(&self) -> String {
        blocks_text(&self.blocks, &|_, _| None)
    }

    /// 同 [`Cell::text`],注引用按 `mark` 展开(见 [`TextRun::text_with_notes`])。
    pub fn text_with_notes(&self, mark: &dyn Fn(NoteKind, i64) -> Option<String>) -> String {
        blocks_text(&self.blocks, mark)
    }

    /// 该单元格是否是被纵向合并“吃掉”的延续格(`w:vMerge` 为 `continue`)。
    pub fn is_vmerge_continuation(&self) -> bool {
        matches!(self.v_merge, VMerge::Continue)
    }
}

/// 块序列的直接段落文字按行拼接(忽略表格);段落锚定的文本框文字紧随该段之后。
fn blocks_text(blocks: &[Block], mark: &dyn Fn(NoteKind, i64) -> Option<String>) -> String {
    let mut lines: Vec<String> = Vec::new();
    for b in blocks {
        if let Block::Paragraph(p) = b {
            lines.push(p.text_with_notes(mark));
            for tb in p.text_boxes() {
                let t = blocks_text(&tb.blocks, mark);
                if !t.is_empty() {
                    lines.push(t);
                }
            }
        }
    }
    lines.join("\n")
}

/// 纵向合并(`w:vMerge`)状态。
///
/// WordprocessingML 的纵向合并语义(ECMA-376 §17.4.85):`w:vMerge w:val="restart"` 是
/// 合并区的**起始格**(承载内容并向下吞并);`w:val="continue"` **或省略 `val` 但元素存在**
/// 是被吞并的**延续格**(通常空——Word 写延续格就是裸 `<w:vMerge/>`);
/// 完全没有 `w:vMerge` 元素则该格不参与纵向合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VMerge {
    /// 不参与纵向合并。
    #[default]
    None,
    /// 纵向合并的起始格(承载内容)。
    Restart,
    /// 纵向合并的延续格(被上方起始格吞并,内容通常为空)。
    Continue,
}

/// 一张内嵌图片(`w:drawing` 内的 `a:blip@r:embed`,或旧式 `w:pict`)。
/// 原始字节存放在解析输出的 media map 里,这里只携带定位信息。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Picture {
    /// `a:blip@r:embed`(或 VML `v:imagedata@r:id`)的关系 id。
    pub rel_id: String,
    /// 经 `word/_rels/document.xml.rels` 解析得到的 `word/media/*` 裸文件名(media map 的键)。
    pub media_name: Option<String>,
    /// 显示尺寸 `(cx, cy)`(EMU,来自 `wp:extent`;VML 从 `style` 宽高折算),best-effort。
    pub extent: Option<(Emu, Emu)>,
    /// 图片字节长度(便利字段;字节本身在 media map 里)。
    pub image_bytes_len: usize,
    /// 替代文字:`wp:docPr@descr`,缺则 `@title`(VML:`v:shape@alt` / `v:imagedata@o:title`);
    /// 空白折叠成单个空格,全空为 `None`。导出的 `![alt]` / `<img alt>` / `[图片: alt]` 用它。
    pub alt: Option<String>,
    /// 放置方式(`wp:inline` 行内 / `wp:anchor` 锚定浮动;C-8)。
    pub placement: Placement,
}

/// 图片放置方式(`wp:inline` / `wp:anchor`,C-8)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// 行内:随文字流参与版式(缺省)。
    #[default]
    Inline,
    /// 锚定浮动:按 `wp:positionH` / `wp:positionV` 的偏移绝对定位
    /// (v1 渲染为无环绕的覆盖层)。
    Anchored {
        /// 水平偏移(EMU,`wp:positionH > wp:posOffset`;缺省 0)。
        x: Emu,
        /// 垂直偏移(EMU,`wp:positionV > wp:posOffset`;缺省 0)。
        y: Emu,
        /// 水平参照系(`wp:positionH@relativeFrom`)。
        rel_h: AnchorRef,
        /// 垂直参照系(`wp:positionV@relativeFrom`)。
        rel_v: AnchorRef,
        /// 衬于文字下方(`wp:anchor@behindDoc`)。
        behind: bool,
    },
}

/// 锚定偏移的参照系(`@relativeFrom` 的常用子集;其余按 `Other` 容错,
/// 渲染侧近似按页边距原点)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AnchorRef {
    /// 页面原点。
    Page,
    /// 页边距原点(缺省)。
    #[default]
    Margin,
    /// 栏(单栏渲染下同 margin)。
    Column,
    /// 所在段落(v1 近似按 margin)。
    Paragraph,
    /// 所在字符(v1 近似按 margin)。
    Character,
    /// 所在行(v1 近似按 margin)。
    Line,
    /// 其余取值(insideMargin / outsideMargin 等;近似按 margin)。
    Other,
}

impl AnchorRef {
    /// 解析 `@relativeFrom`。未知值 → [`AnchorRef::Other`](容错)。
    pub fn from_attr(s: &str) -> Self {
        match s {
            "page" => AnchorRef::Page,
            "margin" => AnchorRef::Margin,
            "column" => AnchorRef::Column,
            "paragraph" => AnchorRef::Paragraph,
            "character" => AnchorRef::Character,
            "line" => AnchorRef::Line,
            _ => AnchorRef::Other,
        }
    }
}

/// 一个 RGB 颜色(来自 `w:color@w:val` / `w:shd@w:fill` 的十六进制)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub rgb: [u8; 3],
}

impl Color {
    pub const fn new(rgb: [u8; 3]) -> Self {
        Color { rgb }
    }

    /// 把 `"RRGGBB"` 十六进制串解析成颜色;`"auto"` / 非法输入返回 `None`。
    pub fn from_hex(hex: &str) -> Option<Self> {
        let h = hex.trim();
        // 非 ASCII 的 6 字节串(如多字节字符)按字节切片会落在非字符边界上 panic(fuzz 发现),
        // 十六进制色值必为 ASCII,先挡掉。
        if h.eq_ignore_ascii_case("auto") || h.len() != 6 || !h.is_ascii() {
            return None;
        }
        let r = u8::from_str_radix(&h[0..2], 16).ok()?;
        let g = u8::from_str_radix(&h[2..4], 16).ok()?;
        let b = u8::from_str_radix(&h[4..6], 16).ok()?;
        Some(Color { rgb: [r, g, b] })
    }
}

#[cfg(test)]
mod color_tests {
    use super::Color;

    #[test]
    fn from_hex_basic() {
        assert_eq!(Color::from_hex("FF8000"), Some(Color::new([255, 128, 0])));
        assert_eq!(Color::from_hex(" 00ff00 "), Some(Color::new([0, 255, 0])));
        assert_eq!(Color::from_hex("auto"), None);
        assert_eq!(Color::from_hex("GG0000"), None);
        assert_eq!(Color::from_hex("FFF"), None);
    }

    /// 回归(cargo-fuzz):6 字节的非 ASCII 串曾在非字符边界切片处 panic。
    #[test]
    fn from_hex_non_ascii_does_not_panic() {
        assert_eq!(Color::from_hex("DD\u{FFFD}\u{FFFD}"), None); // 2 + 3 + 3 = 8 bytes
        assert_eq!(Color::from_hex("D\u{FFFD}D"), None); // 1 + 3 + 1 = 5 bytes
        assert_eq!(Color::from_hex("DD\u{FFFD}D"), None); // 2 + 3 + 1 = 6 bytes: 切点 4 在字符内
        assert_eq!(Color::from_hex("\u{4e2d}\u{6587}"), None); // 6 bytes,切点 2 在字符内
    }
}

#[cfg(test)]
mod header_footer_rule_tests {
    use super::{Document, HeaderFooterKind, HeaderFooterRef, Section};

    fn r(kind: HeaderFooterKind, id: &str) -> HeaderFooterRef {
        HeaderFooterRef {
            kind,
            rel_id: id.to_string(),
        }
    }

    /// 一节:页眉 / 页脚各三种类型都有(键 `h-*` / `f-*`)。
    fn full_section(tag: &str) -> Section {
        let kinds = [
            HeaderFooterKind::Default,
            HeaderFooterKind::First,
            HeaderFooterKind::Even,
        ];
        Section {
            headers: kinds
                .iter()
                .map(|k| r(*k, &format!("h-{tag}-{}", k.as_str())))
                .collect(),
            footers: kinds
                .iter()
                .map(|k| r(*k, &format!("f-{tag}-{}", k.as_str())))
                .collect(),
            ..Section::default()
        }
    }

    fn doc_of(sections: Vec<Section>, even_odd: bool) -> Document {
        Document {
            sections,
            even_and_odd_headers: even_odd,
            ..Document::default()
        }
    }

    #[test]
    fn title_pg_off_uses_default_on_first_page() {
        let d = doc_of(vec![full_section("a")], false);
        assert_eq!(
            d.header_footer_for_page(0, 0, 1),
            (Some("h-a-default"), Some("f-a-default"))
        );
        assert_eq!(
            d.header_footer_for_page(0, 1, 2),
            (Some("h-a-default"), Some("f-a-default"))
        );
    }

    #[test]
    fn title_pg_on_uses_first_only_on_section_first_page() {
        let mut s = full_section("a");
        s.title_pg = true;
        let d = doc_of(vec![s], false);
        assert_eq!(
            d.header_footer_for_page(0, 0, 1),
            (Some("h-a-first"), Some("f-a-first"))
        );
        assert_eq!(
            d.header_footer_for_page(0, 1, 2),
            (Some("h-a-default"), Some("f-a-default"))
        );
    }

    #[test]
    fn even_and_odd_on_uses_even_on_even_page_numbers() {
        let d = doc_of(vec![full_section("a")], true);
        assert_eq!(d.header_footer_for_page(0, 0, 1).0, Some("h-a-default"));
        assert_eq!(d.header_footer_for_page(0, 1, 2).0, Some("h-a-even"));
        assert_eq!(d.header_footer_for_page(0, 2, 3).1, Some("f-a-default"));
        assert_eq!(d.header_footer_for_page(0, 3, 4).1, Some("f-a-even"));
        // 奇偶看全局页码,不看节内序号。
        assert_eq!(d.header_footer_for_page(0, 0, 2).0, Some("h-a-even"));
    }

    #[test]
    fn even_and_odd_off_ignores_even_parts() {
        let d = doc_of(vec![full_section("a")], false);
        assert_eq!(d.header_footer_for_page(0, 1, 2).0, Some("h-a-default"));
    }

    #[test]
    fn title_pg_wins_over_even_on_first_page() {
        let mut s = full_section("a");
        s.title_pg = true;
        let d = doc_of(vec![s], true);
        assert_eq!(d.header_footer_for_page(0, 0, 2).0, Some("h-a-first"));
    }

    /// 本节缺某类型 → 逐节向前回溯同类型;中间节缺也继续往前。
    #[test]
    fn missing_kind_inherits_from_previous_sections() {
        let first = full_section("a");
        let mut second = Section::default();
        second
            .headers
            .push(r(HeaderFooterKind::Default, "h-b-default"));
        second.title_pg = true;
        let third = Section {
            title_pg: true,
            ..Section::default()
        };
        let d = doc_of(vec![first, second, third], true);
        // 第二节:default 页眉自有;页脚全继承第一节。
        assert_eq!(
            d.header_footer_for_page(1, 1, 3),
            (Some("h-b-default"), Some("f-a-default"))
        );
        // 第二节首页(titlePg):first 继承第一节。
        assert_eq!(
            d.header_footer_for_page(1, 0, 3),
            (Some("h-a-first"), Some("f-a-first"))
        );
        // 第三节一无所有:越过第二节继续回溯;偶数页取第一节的 even。
        assert_eq!(
            d.header_footer_for_page(2, 1, 6),
            (Some("h-a-even"), Some("f-a-even"))
        );
        assert_eq!(d.header_footer_for_page(2, 1, 5).0, Some("h-b-default"));
    }

    /// 完全没有引用 / titlePg 开但无 first 引用 / 节序号越界:无页眉页脚。
    #[test]
    fn no_reference_anywhere_yields_none() {
        let d = doc_of(vec![Section::default(), Section::default()], true);
        assert_eq!(d.header_footer_for_page(1, 0, 4), (None, None));

        let mut s = Section::default();
        s.headers.push(r(HeaderFooterKind::Default, "h"));
        s.title_pg = true;
        let d = doc_of(vec![s], false);
        assert_eq!(d.header_footer_for_page(0, 0, 1), (None, None));
        assert_eq!(d.header_footer_for_page(0, 1, 2), (Some("h"), None));
        assert_eq!(d.header_footer_for_page(5, 0, 1), (None, None));
    }

    #[test]
    fn first_page_number_continues_or_restarts() {
        let mut s = Section::default();
        assert_eq!(s.first_page_number(1), 1);
        assert_eq!(s.first_page_number(8), 8);
        s.page_number_start = Some(5);
        assert_eq!(s.first_page_number(8), 5);
        // `w:start="0"` 合法:该节首页是 0,不当作缺失。
        s.page_number_start = Some(0);
        assert_eq!(s.first_page_number(8), 0);
    }

    /// 奇偶判定看**显示页码**:`w:start` 把节首页设成偶数时,首页就用 `even`。
    #[test]
    fn even_page_parity_follows_displayed_page_number() {
        let d = doc_of(vec![full_section("a")], true);
        assert_eq!(d.header_footer_for_page(0, 0, 0).0, Some("h-a-even"));
        assert_eq!(d.header_footer_for_page(0, 0, 1).0, Some("h-a-default"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `sections` 个节,只有第 0 节引用页眉 / 页脚,其余全部继承。
    fn inherit_doc(sections: usize) -> Document {
        let r = |id: &str| {
            vec![HeaderFooterRef {
                kind: HeaderFooterKind::Default,
                rel_id: id.into(),
            }]
        };
        Document {
            sections: (0..sections)
                .map(|i| Section {
                    headers: if i == 0 { r("h") } else { vec![] },
                    footers: if i == 0 { r("f") } else { vec![] },
                    ..Section::default()
                })
                .collect(),
            ..Document::default()
        }
    }

    /// 预计算索引:构建只访问每节一次,之后每页查询 O(1);总访问节数与「节数」同阶、与页数无关
    /// (修复前逐页向前回溯,`pages x sections`)。
    #[test]
    fn index_lookup_steps_are_linear_in_sections() {
        let (sections, pages) = (500usize, 2000usize);
        let doc = inherit_doc(sections);
        HF_STEPS.with(|c| c.set(0));
        let index = doc.header_footer_index();
        for k in 0..pages {
            assert_eq!(
                index.for_page(sections - 1, k, k as i64 + 1),
                (Some("h"), Some("f"))
            );
        }
        let steps = HF_STEPS.with(|c| c.get());
        assert!(steps <= sections, "steps {steps} 应 <= 节数 {sections}");
    }

    /// 与逐页回溯的 `header_footer_for_page` 逐页等价:首页 / 偶数页 / 同类型多引用 / 继承 / 越界。
    #[test]
    fn index_matches_per_page_lookup() {
        let r = |kind, id: &str| HeaderFooterRef {
            kind,
            rel_id: id.into(),
        };
        use HeaderFooterKind::{Default as D, Even as E, First as F};
        let mut doc = Document {
            sections: vec![
                Section {
                    headers: vec![r(D, "h0"), r(F, "h0f"), r(D, "h0-dup")],
                    footers: vec![r(E, "f0e")],
                    title_pg: true,
                    ..Section::default()
                },
                Section::default(),
                Section {
                    headers: vec![r(E, "h2e")],
                    footers: vec![r(D, "f2"), r(F, "f2f")],
                    title_pg: true,
                    ..Section::default()
                },
                Section::default(),
            ],
            ..Document::default()
        };
        for even in [false, true] {
            doc.even_and_odd_headers = even;
            let index = doc.header_footer_index();
            for section in 0..6 {
                for k in 0..4 {
                    for number in -2..5 {
                        assert_eq!(
                            index.for_page(section, k, number),
                            doc.header_footer_for_page(section, k, number),
                            "section {section} k {k} number {number} even {even}"
                        );
                    }
                }
            }
        }
    }
}
