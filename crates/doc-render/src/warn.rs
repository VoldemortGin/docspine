//! [`RenderWarning`] —— docspine 侧的导出降级告警,与引擎告警合流。
//!
//! pdf-typeset 的 [`ExportWarning`] 覆盖引擎内的降级(字体替换、字形回退、图片解码
//! 失败等);docspine 自己的降级(多栏压平、段落边框/底纹本批不画、图片待 C-8)在
//! 这里枚举。py-bindings 按 [`RenderWarning::kind`] 去重,每种只 `warnings.warn` 一次。

use std::fmt;

use doc_core::style::StyleWarning;
use pdf_typeset::ExportWarning;

/// 一条导出降级告警(引擎侧或 docspine 侧)。
#[derive(Clone, Debug, PartialEq)]
pub enum RenderWarning {
    /// 排版引擎的降级(字体替换 / 样式近似 / 字形回退 / 图片丢弃等)。
    Engine(ExportWarning),
    /// 样式表体检告警(`basedOn` 环 / 悬空引用;解析已截断,不悬挂)。
    Style(StyleWarning),
    /// 多栏节(`w:cols@num > 1`)压平为单栏渲染(v1 声明降级)。
    MultiColumnFlattened {
        /// 声明的栏数。
        cols: u32,
    },
    /// 段落边框(`w:pBdr`)已解析但本批不绘制(引擎段落属性暂无边框槽位)。
    ParaBorderOmitted,
    /// 段落底纹(`w:pPr > w:shd`)已解析但本批不绘制。
    ParaShadingOmitted,
    /// 内嵌图片本批不渲染(图片布局是 C-8)。
    PictureSkipped,
    /// 列表编号落在未解的 `styleLink`/`numStyleLink` 间接上(C-6 声明降级):
    /// 该 numId 无自有层级定义,段落按普通段渲染(缩进照常级联)。
    NumberingIndirectionSkipped,
    /// 单元格纵向对齐(`w:vAlign` center/bottom)按顶对齐渲染(引擎单元格
    /// 暂无 vAlign 槽;解析保真,C-7 声明降级)。
    CellVAlignIgnored,
    /// 表格行高超过一页正文高度:行不跨页(整行挪页)语义下该行溢出页面
    /// (引擎“行不分割”的 v1 限制)。
    RowTooTall,
}

impl RenderWarning {
    /// 稳定的告警类别标签(py-bindings 按它去重,每种只浮出一次)。
    pub fn kind(&self) -> &'static str {
        match self {
            RenderWarning::Engine(w) => match w {
                ExportWarning::FontSubstituted { .. } => "font-substituted",
                ExportWarning::StyleApproximated { .. } => "style-approximated",
                ExportWarning::GlyphFallback { .. } => "glyph-fallback",
                ExportWarning::PresetDegraded { .. } => "preset-degraded",
                ExportWarning::GradientDegraded { .. } => "gradient-degraded",
                ExportWarning::BoxOverflowClipped { .. } => "box-overflow-clipped",
                ExportWarning::ImageDropped { .. } => "image-dropped",
                // 引擎枚举 #[non_exhaustive]:后续 TS 阶段的新变体先归到统称。
                _ => "engine",
            },
            RenderWarning::Style(w) => match w {
                StyleWarning::BasedOnCycle { .. } => "style-based-on-cycle",
                StyleWarning::UnknownBasedOn { .. } => "style-unknown-based-on",
            },
            RenderWarning::MultiColumnFlattened { .. } => "multi-column-flattened",
            RenderWarning::ParaBorderOmitted => "para-border-omitted",
            RenderWarning::ParaShadingOmitted => "para-shading-omitted",
            RenderWarning::PictureSkipped => "picture-skipped",
            RenderWarning::NumberingIndirectionSkipped => "numbering-indirection-skipped",
            RenderWarning::CellVAlignIgnored => "cell-valign-ignored",
            RenderWarning::RowTooTall => "row-too-tall",
        }
    }
}

impl fmt::Display for RenderWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderWarning::Engine(w) => write!(f, "{w}"),
            RenderWarning::Style(StyleWarning::BasedOnCycle { style_id }) => {
                write!(
                    f,
                    "style '{style_id}' sits on a basedOn cycle; chain truncated"
                )
            }
            RenderWarning::Style(StyleWarning::UnknownBasedOn { style_id, based_on }) => {
                write!(
                    f,
                    "style '{style_id}' is basedOn unknown style '{based_on}'; chain truncated"
                )
            }
            RenderWarning::MultiColumnFlattened { cols } => {
                write!(f, "{cols}-column section flattened to a single column")
            }
            RenderWarning::ParaBorderOmitted => {
                write!(f, "paragraph borders are not drawn in this version")
            }
            RenderWarning::ParaShadingOmitted => {
                write!(f, "paragraph shading is not drawn in this version")
            }
            RenderWarning::PictureSkipped => {
                write!(f, "embedded pictures are not rendered yet (planned)")
            }
            RenderWarning::NumberingIndirectionSkipped => {
                write!(
                    f,
                    "numbering styleLink/numStyleLink indirection is not resolved; \
                     affected list paragraphs render without labels"
                )
            }
            RenderWarning::CellVAlignIgnored => {
                write!(
                    f,
                    "table cell vertical alignment (vAlign) renders as top in this version"
                )
            }
            RenderWarning::RowTooTall => {
                write!(
                    f,
                    "a table row is taller than the page body; rows never split across pages"
                )
            }
        }
    }
}
