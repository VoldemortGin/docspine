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

use crate::geom::{Emu, Twips};
use crate::style::{ParaProps, RunProps, StyleTable, Theme};

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
    /// 本节覆盖的正文块区间的**排他性**结束下标(相对 [`Document::body`])。
    /// 本节的块为 `body[上一节.end_block .. 本节.end_block]`,首节从 0 起。
    pub end_block: usize,
}

impl Default for Section {
    fn default() -> Self {
        Section {
            page_width: 12_240,
            page_height: 15_840,
            orientation: Orientation::Portrait,
            margins: PageMargins::default(),
            cols: 1,
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
    /// 直接格式化的 pPr 原始片段(**全 Option**,C-4:spacing / ind / pBdr / shd /
    /// keep 系列;`jc` 与上面的 `align` 便利字段并存,契约不变)。
    /// 渲染侧走 [`crate::style::resolve_para`] 消费本片段。
    pub ppr: ParaProps,
}

impl Paragraph {
    /// 便利:把段内所有 run 的文字拼接成整段文本(分段按 [`TextRun::text`] 折叠)。
    pub fn text(&self) -> String {
        self.runs.iter().map(|r| r.text()).collect()
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
    pub fn text(&self) -> String {
        let mut out = String::new();
        for seg in &self.segments {
            match seg {
                RunSegment::Text(s) => out.push_str(s),
                RunSegment::Tab => out.push('\t'),
                RunSegment::Break(_) => out.push('\n'),
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

/// 一张表格(`w:tbl`)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Table {
    /// 表格网格列定义(`w:tblGrid` > `w:gridCol@w:w`,单位 twip),给出逻辑列数与各列宽。
    pub grid_cols: Vec<Twips>,
    /// 表格行。
    pub rows: Vec<Row>,
    /// 表格样式名(`w:tblPr` > `w:tblStyle@w:val`),原样保留。
    pub style: Option<String>,
}

impl Table {
    /// 逻辑列数:优先取 `w:tblGrid` 的列数;退而取首行单元格 `grid_span` 之和。
    pub fn col_count(&self) -> usize {
        if !self.grid_cols.is_empty() {
            return self.grid_cols.len();
        }
        self.rows
            .first()
            .map(|r| r.cells.iter().map(|c| c.grid_span as usize).sum())
            .unwrap_or(0)
    }
}

/// 表格的一行(`w:tr`)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    /// 行高(twip,`w:trPr` > `w:trHeight@w:val`)。
    pub height: Option<Twips>,
    /// 是否为表头行(`w:trPr` > `w:tblHeader`)。
    pub is_header: bool,
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
    /// 单元格底纹/填充色(`w:tcPr` > `w:shd@w:fill`,`"RRGGBB"`;`"auto"` -> `None`)。
    pub fill: Option<Color>,
}

impl Cell {
    /// 便利:把单元格内**直接段落**的文字按行拼接(忽略嵌套表;嵌套表请遍历 `blocks`)。
    pub fn text(&self) -> String {
        let lines: Vec<String> = self
            .blocks
            .iter()
            .filter_map(|b| match b {
                Block::Paragraph(p) => Some(p.text()),
                Block::Table(_) => None,
            })
            .collect();
        lines.join("\n")
    }

    /// 该单元格是否是被纵向合并“吃掉”的延续格(`w:vMerge` 为 `continue`)。
    pub fn is_vmerge_continuation(&self) -> bool {
        matches!(self.v_merge, VMerge::Continue)
    }
}

/// 纵向合并(`w:vMerge`)状态。
///
/// WordprocessingML 的纵向合并语义:`w:vMerge w:val="restart"`(或省略 `val` 但元素存在)是
/// 合并区的**起始格**(承载内容并向下吞并);`w:val="continue"` 是被吞并的**延续格**(通常空);
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
    /// 显示尺寸 `(cx, cy)`(EMU,来自 `wp:extent`),best-effort。
    pub extent: Option<(Emu, Emu)>,
    /// 图片字节长度(便利字段;字节本身在 media map 里)。
    pub image_bytes_len: usize,
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
        if h.eq_ignore_ascii_case("auto") || h.len() != 6 {
            return None;
        }
        let r = u8::from_str_radix(&h[0..2], 16).ok()?;
        let g = u8::from_str_radix(&h[2..4], 16).ok()?;
        let b = u8::from_str_radix(&h[4..6], 16).ok()?;
        Some(Color { rgb: [r, g, b] })
    }
}
