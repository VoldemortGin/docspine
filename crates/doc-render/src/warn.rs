//! [`RenderWarning`] —— docspine 侧的导出降级告警,与引擎告警合流。
//!
//! pdf-typeset 的 [`ExportWarning`] 覆盖引擎内的降级(字体替换、字形回退、图片解码
//! 失败等);docspine 自己的降级(多栏压平、段落 `between` 边框不画、图片降级等)在
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
    /// 段落边框的段间横线(`w:pBdr > w:between`)已解析但不绘制:引擎原生段落边框
    /// (`ParaProps.borders`)无 between 槽位,且不得近似成两个矩形边。四周边与底纹
    /// 照常原生绘制(含跨页片段)。
    ParaBorderOmitted,
    /// 内嵌图片无法渲染(缺 media 字节 / 缺 `wp:extent` 尺寸 / 尺寸非法):
    /// 该图跳过,其余内容照常(C-8:有字节且有尺寸的图已按块级渲染)。
    PictureSkipped,
    /// 浮动/锚定图片(`wp:anchor`)按 `positionH/V` 的 posOffset 绝对定位成覆盖层
    /// 绘制,但**不做文字环绕**(周绕排除区 v1 不实现;C-8 声明降级)。
    FloatingNoWrap,
    /// 不支持的矢量图格式(EMF/WMF):引擎无法解码,改画一个与显示尺寸等大的
    /// 浅灰占位框(C-8 声明降级;文本/表格照常)。
    UnsupportedImageFormat,
    /// 列表编号落在未解的 `styleLink`/`numStyleLink` 间接上(C-6 声明降级):
    /// 该 numId 无自有层级定义,段落按普通段渲染(缩进照常级联)。
    NumberingIndirectionSkipped,
    /// 文档内部书签跳转的超链接(`w:hyperlink@w:anchor`,如目录/交叉引用):目标存成
    /// `"#书签名"` 但**不渲染**成 PDF 链接注解(v1 只发外链注解;§3j 声明降级)。
    InternalLinkNotRendered,
    /// 表格行高超过一页正文高度:行不跨页(整行挪页)语义下该行溢出页面
    /// (引擎“行不分割”的 v1 限制)。
    RowTooTall,
    /// 段落声明了自定义制表位(`w:tabs > w:tab`,含 pos/leader/对齐):v1 只按
    /// 缺省制表位间隔(`defaultTabStop`)等距推进 `\t`,自定义停位/前导符/对齐被
    /// 忽略(C-9 声明降级;正文照常)。
    CustomTabStopsIgnored,
    /// 浮动文本框(`wps:txbx` / VML `v:textbox` 的 `w:txbxContent`)只做抽取(进
    /// `to_text` / `to_markdown` / `to_html`),PDF **不绘制**(v1 声明降级;正文照常)。
    TextBoxNotRendered,
    /// 公式(`m:oMath` / `m:oMathPara`)只抽取 `m:t` 纯文本:PDF 按普通文字出,不做
    /// 公式排版(分式 / 上下标 / 根号等结构丢失;v1 声明降级;文字不丢)。
    MathFlattened,
    /// 页眉 / 页脚内容过高:按 Word 行为下推 / 上推正文会让正文区小于下限(72pt 或原正文高),
    /// 该页正文退回按原上 / 下边距排,页眉页脚照画(可能与正文重叠)。
    HeaderFooterOverflow,
    /// 脚注 / 尾注只抽取(进 `to_text` / `to_markdown`),PDF **不绘制**注文,正文里的引用
    /// 标记也不画(v1 声明降级;正文照常)。
    NotesNotRendered,
    /// 页码格式(`w:pgNumType@w:fmt`)取了不支持的值(`ordinal` / 各语种计数法 …):
    /// 该节页眉页脚里的 `PAGE` 按阿拉伯数字输出(字段自带 `\*` 开关时以开关为准)。
    PageNumFormatUnsupported,
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
                ExportWarning::SignedSpacingFallback { .. } => "signed-spacing-fallback",
                // 引擎枚举 #[non_exhaustive]:后续 TS 阶段的新变体先归到统称。
                _ => "engine",
            },
            RenderWarning::Style(w) => match w {
                StyleWarning::BasedOnCycle { .. } => "style-based-on-cycle",
                StyleWarning::UnknownBasedOn { .. } => "style-unknown-based-on",
            },
            RenderWarning::MultiColumnFlattened { .. } => "multi-column-flattened",
            RenderWarning::ParaBorderOmitted => "para-border-omitted",
            RenderWarning::PictureSkipped => "picture-skipped",
            RenderWarning::FloatingNoWrap => "floating-no-wrap",
            RenderWarning::UnsupportedImageFormat => "unsupported-image-format",
            RenderWarning::NumberingIndirectionSkipped => "numbering-indirection-skipped",
            RenderWarning::InternalLinkNotRendered => "internal-link-not-rendered",
            RenderWarning::RowTooTall => "row-too-tall",
            RenderWarning::CustomTabStopsIgnored => "custom-tab-stops-ignored",
            RenderWarning::TextBoxNotRendered => "text-box-not-rendered",
            RenderWarning::MathFlattened => "math-flattened",
            RenderWarning::HeaderFooterOverflow => "header-footer-overflow",
            RenderWarning::NotesNotRendered => "notes-not-rendered",
            RenderWarning::PageNumFormatUnsupported => "page-number-format-unsupported",
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
                write!(
                    f,
                    "paragraph 'between' borders (w:between) are not drawn in this version"
                )
            }
            RenderWarning::PictureSkipped => {
                write!(
                    f,
                    "an embedded picture was skipped (missing media bytes or size)"
                )
            }
            RenderWarning::FloatingNoWrap => {
                write!(
                    f,
                    "a floating (anchored) image is placed at its absolute anchor \
                     offset; text does not wrap around it in this version"
                )
            }
            RenderWarning::UnsupportedImageFormat => {
                write!(
                    f,
                    "an unsupported vector image format (EMF/WMF) is shown as a \
                     placeholder box; it cannot be decoded in this version"
                )
            }
            RenderWarning::NumberingIndirectionSkipped => {
                write!(
                    f,
                    "numbering styleLink/numStyleLink indirection is not resolved; \
                     affected list paragraphs render without labels"
                )
            }
            RenderWarning::InternalLinkNotRendered => {
                write!(
                    f,
                    "an internal-anchor hyperlink is not rendered as a PDF link in this version"
                )
            }
            RenderWarning::RowTooTall => {
                write!(
                    f,
                    "a table row is taller than the page body; rows never split across pages"
                )
            }
            RenderWarning::CustomTabStopsIgnored => {
                write!(
                    f,
                    "custom tab stops (w:tabs) are ignored; tabs advance by the \
                     default interval in this version"
                )
            }
            RenderWarning::TextBoxNotRendered => {
                write!(
                    f,
                    "floating text boxes are extracted to text/markdown/html but not \
                     drawn in the PDF in this version"
                )
            }
            RenderWarning::MathFlattened => {
                write!(
                    f,
                    "equations (m:oMath) are rendered as plain text only; math layout \
                     (fractions, scripts, radicals) is not reproduced in this version"
                )
            }
            RenderWarning::HeaderFooterOverflow => {
                write!(
                    f,
                    "a header or footer is too tall to push the body clear of it; the body \
                     keeps the page margins and may overlap the header or footer"
                )
            }
            RenderWarning::NotesNotRendered => {
                write!(
                    f,
                    "footnotes and endnotes are extracted to text/markdown but not drawn \
                     in the PDF (nor their reference marks) in this version"
                )
            }
            RenderWarning::PageNumFormatUnsupported => {
                write!(
                    f,
                    "an unsupported page number format (w:pgNumType w:fmt) is shown as \
                     decimal digits in this version"
                )
            }
        }
    }
}
