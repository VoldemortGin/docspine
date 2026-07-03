//! 有效样式解析器(PDF-EXPORT C-5):styles.xml 样式表 + theme1.xml 主题 + 级联合并。
//!
//! WordprocessingML 的**有效格式**是一条级联链(ECMA-376 §17.7.2):
//!
//! ```text
//! docDefaults(rPrDefault/pPrDefault)
//!   → 表格样式(段落在表格内时,tblStyle 的 basedOn 链)
//!   → 段落样式(pStyle 的 basedOn 链,根先、派生后)
//!   → 直接格式化(w:pPr / w:rPr),后者覆盖前者
//! ```
//!
//! **toggle 属性(b/i/caps/smallCaps/strike/vanish)** 例外(ECMA-376 §17.7.3):
//! 直接格式化是绝对开关;否则以 docDefaults 为基值,与样式链上 `true` 出现次数的
//! **奇偶**做异或(样式里显式的 `false` 不参与计数)。字符样式(`w:rStyle`,C-4 起
//! 经 [`RunProps::r_style`] 捕获)的 basedOn 链插在段落样式链之后、直接格式化之前,
//! 同样进入异或计数。
//!
//! **theme 间接引用**:`rFonts@asciiTheme="minorHAnsi"` 等经 [`Theme::fonts`]
//! (fontScheme)解成实际 family 名;`w:color@themeColor="accent1"` 经
//! [`Theme::colors`](clrScheme)解成 RGB。
//!
//! 本模块是纯「模型 → 模型」计算:无 IO / zip / XML,输入全部来自 [`Document`] 上的
//! [`StyleTable`] / [`Theme`](由 doc-parse 机械填充),供 doc-render(C-1)消费。

use std::collections::{BTreeMap, BTreeSet};

use crate::geom::{twips_to_points, Twips};
use crate::model::{Color, Document, Paragraph, Table, TextRun};

// ============================================================ Word 内置缺省(硬编码兜底)

/// Word 内置缺省正文西文字体。出处:Office 默认主题「Office」的 minorFont/latin =
/// "Calibri"(Word 2007+ 默认模板);ECMA-376 未规定实现缺省,这里对齐 Word 实际行为。
/// 仅当 docDefaults / 样式链 / theme 都给不出字体时才落到这里。
pub const DEFAULT_LATIN_FONT: &str = "Calibri";

/// Word 内置缺省标题西文字体(majorFont/latin)。出处:Office 2013+ 默认主题「Office」的
/// majorFont/latin = "Calibri Light"。仅当 theme 部件缺失/槽位为空时兜底。
pub const DEFAULT_MAJOR_LATIN_FONT: &str = "Calibri Light";

/// Word 内置缺省字号(磅)。出处:Word 默认模板 docDefaults 为 `w:sz w:val="22"`
/// (22 半磅 = 11pt)。仅当 docDefaults 与样式链都未给出字号时兜底。
pub const DEFAULT_SIZE_PT: f32 = 11.0;

// ============================================================ theme(theme1.xml)

/// 主题(`word/theme/theme1.xml`):fontScheme(字体方案)+ clrScheme(配色方案)。
/// 部件缺失时为全空缺省,解析器落到硬编码兜底。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Theme {
    /// 字体方案(`a:fontScheme`):major(标题)/ minor(正文)各一组。
    pub fonts: FontScheme,
    /// 配色方案(`a:clrScheme`):12 个具名槽位。
    pub colors: ColorScheme,
}

/// 字体方案(`a:fontScheme`)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FontScheme {
    /// 标题字体组(`a:majorFont`)。
    pub major: FontSet,
    /// 正文字体组(`a:minorFont`)。
    pub minor: FontSet,
}

/// 一组主题字体(`a:majorFont` / `a:minorFont` 的 latin / ea / cs 槽)。
/// `typeface=""`(空串)按缺失处理 → `None`。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct FontSet {
    /// 西文(`a:latin@typeface`)。
    pub latin: Option<String>,
    /// 东亚(`a:ea@typeface`)。
    pub east_asia: Option<String>,
    /// 复杂文种(`a:cs@typeface`)。
    pub cs: Option<String>,
}

/// 配色方案(`a:clrScheme`)的 12 个槽位;取 `a:srgbClr@val` 或 `a:sysClr@lastClr`。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ColorScheme {
    pub dk1: Option<Color>,
    pub lt1: Option<Color>,
    pub dk2: Option<Color>,
    pub lt2: Option<Color>,
    pub accent1: Option<Color>,
    pub accent2: Option<Color>,
    pub accent3: Option<Color>,
    pub accent4: Option<Color>,
    pub accent5: Option<Color>,
    pub accent6: Option<Color>,
    pub hlink: Option<Color>,
    pub fol_hlink: Option<Color>,
}

impl ColorScheme {
    /// 按主题色槽位取色。`text1/dark1 → dk1`、`background1/light1 → lt1` 等映射见
    /// [`ThemeColor::from_attr`]。
    pub fn get(&self, slot: ThemeColor) -> Option<Color> {
        match slot {
            ThemeColor::Dark1 => self.dk1,
            ThemeColor::Light1 => self.lt1,
            ThemeColor::Dark2 => self.dk2,
            ThemeColor::Light2 => self.lt2,
            ThemeColor::Accent1 => self.accent1,
            ThemeColor::Accent2 => self.accent2,
            ThemeColor::Accent3 => self.accent3,
            ThemeColor::Accent4 => self.accent4,
            ThemeColor::Accent5 => self.accent5,
            ThemeColor::Accent6 => self.accent6,
            ThemeColor::Hyperlink => self.hlink,
            ThemeColor::FollowedHyperlink => self.fol_hlink,
        }
    }

    /// 按槽位写色(供 theme1.xml walker 填充)。
    pub fn set(&mut self, slot: ThemeColor, color: Color) {
        let field = match slot {
            ThemeColor::Dark1 => &mut self.dk1,
            ThemeColor::Light1 => &mut self.lt1,
            ThemeColor::Dark2 => &mut self.dk2,
            ThemeColor::Light2 => &mut self.lt2,
            ThemeColor::Accent1 => &mut self.accent1,
            ThemeColor::Accent2 => &mut self.accent2,
            ThemeColor::Accent3 => &mut self.accent3,
            ThemeColor::Accent4 => &mut self.accent4,
            ThemeColor::Accent5 => &mut self.accent5,
            ThemeColor::Accent6 => &mut self.accent6,
            ThemeColor::Hyperlink => &mut self.hlink,
            ThemeColor::FollowedHyperlink => &mut self.fol_hlink,
        };
        *field = Some(color);
    }
}

/// 主题色槽位(`w:color@w:themeColor` 的取值域,ECMA-376 ST_ThemeColor)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeColor {
    Dark1,
    Light1,
    Dark2,
    Light2,
    Accent1,
    Accent2,
    Accent3,
    Accent4,
    Accent5,
    Accent6,
    Hyperlink,
    FollowedHyperlink,
}

impl ThemeColor {
    /// 解析 `w:themeColor` 属性值。`text1/dark1` 都指 dk1、`background1/light1` 都指
    /// lt1(text/background 是 dk/lt 的用途别名,ECMA-376 §20.1.6.2)。未知值 → `None`。
    pub fn from_attr(s: &str) -> Option<Self> {
        Some(match s {
            "dark1" | "text1" => ThemeColor::Dark1,
            "light1" | "background1" => ThemeColor::Light1,
            "dark2" | "text2" => ThemeColor::Dark2,
            "light2" | "background2" => ThemeColor::Light2,
            "accent1" => ThemeColor::Accent1,
            "accent2" => ThemeColor::Accent2,
            "accent3" => ThemeColor::Accent3,
            "accent4" => ThemeColor::Accent4,
            "accent5" => ThemeColor::Accent5,
            "accent6" => ThemeColor::Accent6,
            "hyperlink" => ThemeColor::Hyperlink,
            "followedHyperlink" => ThemeColor::FollowedHyperlink,
            _ => return None,
        })
    }

    /// 解析 clrScheme 子元素的本地名(`a:dk1` / `a:accent1` / `a:folHlink` …)。
    pub fn from_scheme_element(name: &str) -> Option<Self> {
        Some(match name {
            "dk1" => ThemeColor::Dark1,
            "lt1" => ThemeColor::Light1,
            "dk2" => ThemeColor::Dark2,
            "lt2" => ThemeColor::Light2,
            "accent1" => ThemeColor::Accent1,
            "accent2" => ThemeColor::Accent2,
            "accent3" => ThemeColor::Accent3,
            "accent4" => ThemeColor::Accent4,
            "accent5" => ThemeColor::Accent5,
            "accent6" => ThemeColor::Accent6,
            "hlink" => ThemeColor::Hyperlink,
            "folHlink" => ThemeColor::FollowedHyperlink,
            _ => return None,
        })
    }
}

/// 主题字体槽位(`rFonts@asciiTheme` 等的取值域,ECMA-376 ST_Theme)。
/// `major*` 指 fontScheme 的 majorFont,`minor*` 指 minorFont;
/// `*Ascii`/`*HAnsi` 落 latin 槽、`*EastAsia` 落 ea 槽、`*Bidi` 落 cs 槽。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeFont {
    MajorAscii,
    MajorHAnsi,
    MajorEastAsia,
    MajorBidi,
    MinorAscii,
    MinorHAnsi,
    MinorEastAsia,
    MinorBidi,
}

impl ThemeFont {
    /// 解析 `asciiTheme` / `hAnsiTheme` / `eastAsiaTheme` / `cstheme` 的属性值。
    pub fn from_attr(s: &str) -> Option<Self> {
        Some(match s {
            "majorAscii" => ThemeFont::MajorAscii,
            "majorHAnsi" => ThemeFont::MajorHAnsi,
            "majorEastAsia" => ThemeFont::MajorEastAsia,
            "majorBidi" => ThemeFont::MajorBidi,
            "minorAscii" => ThemeFont::MinorAscii,
            "minorHAnsi" => ThemeFont::MinorHAnsi,
            "minorEastAsia" => ThemeFont::MinorEastAsia,
            "minorBidi" => ThemeFont::MinorBidi,
            _ => return None,
        })
    }

    fn is_major(self) -> bool {
        matches!(
            self,
            ThemeFont::MajorAscii
                | ThemeFont::MajorHAnsi
                | ThemeFont::MajorEastAsia
                | ThemeFont::MajorBidi
        )
    }
}

// ============================================================ 属性片段(rPr / pPr)

/// 一处字体引用:显名,或主题间接引用(解引在有效样式解析时进行)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontRef {
    /// 显式 family 名(`rFonts@ascii` 等)。
    Named(String),
    /// 主题间接引用(`rFonts@asciiTheme` 等;同槽位上 theme 属性优先于显名,
    /// ECMA-376 §17.3.2.26)。
    Theme(ThemeFont),
}

/// `rFonts` 的四槽字体(ascii / hAnsi / eastAsia / cs),每槽独立参与级联。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FontSlots {
    pub ascii: Option<FontRef>,
    pub h_ansi: Option<FontRef>,
    pub east_asia: Option<FontRef>,
    pub cs: Option<FontRef>,
}

/// 一处颜色引用(`w:color`):显式 RGB、主题间接引用、或 `auto`(自动,渲染侧按黑)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorRef {
    Rgb(Color),
    Theme(ThemeColor),
    Auto,
}

/// 下划线样式(`w:u@w:val`,ECMA-376 ST_Underline 的常用子集)。
/// `None` 是「显式关」(能盖掉样式链);未知的非 `none` 取值容错为 [`UnderlineKind::Single`]
/// (渲染侧 v1 只画单线,种类保真供后续扩展)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UnderlineKind {
    /// 显式无下划线(`val="none"`)。
    None,
    /// 单线(缺省)。
    #[default]
    Single,
    Double,
    Thick,
    Dotted,
    Dash,
    DotDash,
    DotDotDash,
    Wave,
    /// 仅词下划线(空格不画)。
    Words,
}

impl UnderlineKind {
    /// 解析 `w:u@w:val`。未知非 `none` 值容错为 `Single`(下划线开着,种类近似)。
    pub fn from_attr(s: &str) -> Self {
        match s {
            "none" => UnderlineKind::None,
            "double" => UnderlineKind::Double,
            "thick" => UnderlineKind::Thick,
            "dotted" | "dottedHeavy" => UnderlineKind::Dotted,
            "dash" | "dashedHeavy" | "dashLong" | "dashLongHeavy" => UnderlineKind::Dash,
            "dotDash" | "dashDotHeavy" => UnderlineKind::DotDash,
            "dotDotDash" | "dashDotDotHeavy" => UnderlineKind::DotDotDash,
            "wave" | "wavyHeavy" | "wavyDouble" => UnderlineKind::Wave,
            "words" => UnderlineKind::Words,
            _ => UnderlineKind::Single, // "single" 与未知值。
        }
    }

    /// 该种类是否画下划线(`None` 以外都画)。
    pub fn is_on(self) -> bool {
        self != UnderlineKind::None
    }
}

/// 纵向对齐(`w:vertAlign@w:val`):上标 / 下标 / 基线。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VertAlign {
    #[default]
    Baseline,
    Superscript,
    Subscript,
}

impl VertAlign {
    /// 解析 `w:vertAlign@w:val`。未知值 → `None`(容错,当未设置)。
    pub fn from_attr(s: &str) -> Option<Self> {
        Some(match s {
            "baseline" => VertAlign::Baseline,
            "superscript" => VertAlign::Superscript,
            "subscript" => VertAlign::Subscript,
            _ => return None,
        })
    }
}

/// 高亮(`w:highlight@w:val`,ST_HighlightColor 具名色)。`Off`(`val="none"`)是
/// 「显式关」,能盖掉样式链上继承的高亮。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Highlight {
    /// 显式无高亮(`val="none"`)。
    Off,
    /// 具名高亮色(解析时已折成 RGB)。
    On(Color),
}

impl Highlight {
    /// 解析 `w:highlight@w:val` 的具名色(ECMA-376 §17.18.40 ST_HighlightColor)。
    /// 未知名容错为 `None`(当未设置)。
    pub fn from_attr(s: &str) -> Option<Self> {
        let rgb: [u8; 3] = match s {
            "none" => return Some(Highlight::Off),
            "black" => [0x00, 0x00, 0x00],
            "blue" => [0x00, 0x00, 0xFF],
            "cyan" => [0x00, 0xFF, 0xFF],
            "darkBlue" => [0x00, 0x00, 0x8B],
            "darkCyan" => [0x00, 0x8B, 0x8B],
            "darkGray" => [0xA9, 0xA9, 0xA9],
            "darkGreen" => [0x00, 0x64, 0x00],
            "darkMagenta" => [0x80, 0x00, 0x80],
            "darkRed" => [0x8B, 0x00, 0x00],
            "darkYellow" => [0x80, 0x80, 0x00],
            "green" => [0x00, 0xFF, 0x00],
            "lightGray" => [0xD3, 0xD3, 0xD3],
            "magenta" => [0xFF, 0x00, 0xFF],
            "red" => [0xFF, 0x00, 0x00],
            "white" => [0xFF, 0xFF, 0xFF],
            "yellow" => [0xFF, 0xFF, 0x00],
            _ => return None,
        };
        Some(Highlight::On(Color::new(rgb)))
    }
}

/// 一条边框(CT_Border:`w:pBdr` 的 `w:top` 等;C-7 起表格边框复用)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Border {
    /// 线型(`@w:val`,`"single"`/`"double"`/`"none"` … 原样保留,容错)。
    pub val: String,
    /// 线宽(`@w:sz`,单位 1/8 磅)。
    pub sz_eighth_pt: u32,
    /// 到正文的留白(`@w:space`,磅)。
    pub space_pt: u32,
    /// 线色(`@w:color` / `@w:themeColor`)。
    pub color: Option<ColorRef>,
}

/// 段落边框(`w:pPr > w:pBdr`)的各边;每边独立参与级联(就近覆盖)。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParaBorders {
    pub top: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    pub right: Option<Border>,
    /// 相邻同边框段落之间的横线(`w:between`)。
    pub between: Option<Border>,
}

impl ParaBorders {
    /// 是否任意一边有(非 `none` 线型的)边框。
    pub fn any(&self) -> bool {
        [
            &self.top,
            &self.bottom,
            &self.left,
            &self.right,
            &self.between,
        ]
        .into_iter()
        .any(|b| b.as_ref().is_some_and(|b| b.val != "none"))
    }
}

/// 表级边框(`w:tblBorders` / 表样式 `w:tblPr > w:tblBorders`)的六槽:
/// 外四边 + 内横线(insideH)+ 内竖线(insideV)。每边独立参与级联。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableBorders {
    pub top: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    pub right: Option<Border>,
    /// 行与行之间的内横线(`w:insideH`)。
    pub inside_h: Option<Border>,
    /// 列与列之间的内竖线(`w:insideV`)。
    pub inside_v: Option<Border>,
}

impl TableBorders {
    /// 逐边就近覆盖合并(`other` 是级联链上更靠近直接格式化的一层)。
    pub fn overlay(&mut self, other: &TableBorders) {
        for (slot, o) in [
            (&mut self.top, &other.top),
            (&mut self.bottom, &other.bottom),
            (&mut self.left, &other.left),
            (&mut self.right, &other.right),
            (&mut self.inside_h, &other.inside_h),
            (&mut self.inside_v, &other.inside_v),
        ] {
            if o.is_some() {
                *slot = o.clone();
            }
        }
    }
}

/// 单元格边框(`w:tcBorders`)的四槽(对角线 tl2br/tr2bl v1 忽略)。
/// 冲突消解(`tcBorders` > `tblBorders` > 表样式;共享边归并)在渲染侧进行。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CellBorders {
    pub top: Option<Border>,
    pub bottom: Option<Border>,
    pub left: Option<Border>,
    pub right: Option<Border>,
}

/// 单元格边距(`w:tblCellMar` / `w:tcMar` 的 top/left/bottom/right,twip,仅
/// `type="dxa"`)。逐边独立级联:tcMar 覆盖表级 tblCellMar 覆盖 Word 缺省
/// (左右 108 twip、上下 0)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CellMargins {
    pub top: Option<Twips>,
    pub left: Option<Twips>,
    pub bottom: Option<Twips>,
    pub right: Option<Twips>,
}

impl CellMargins {
    /// 逐边就近覆盖合并。
    pub fn overlay(&mut self, other: &CellMargins) {
        for (slot, o) in [
            (&mut self.top, &other.top),
            (&mut self.left, &other.left),
            (&mut self.bottom, &other.bottom),
            (&mut self.right, &other.right),
        ] {
            if o.is_some() {
                *slot = *o;
            }
        }
    }
}

/// 样式定义里的表格属性片段(`w:style > w:tblPr` 的 C-7 子集)。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct TableProps {
    /// 表级边框(`w:tblBorders`)。
    pub borders: TableBorders,
    /// 表级单元格缺省边距(`w:tblCellMar`)。
    pub cell_margins: CellMargins,
}

/// 行距规则(`w:spacing@w:line` + `@w:lineRule`)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineSpacingRule {
    /// 行高倍数(`lineRule="auto"` 或缺省;`line` 单位 1/240 行,240 = 单倍)。
    Auto(i64),
    /// 最小行高(`lineRule="atLeast"`,twip)。
    AtLeast(Twips),
    /// 精确行高(`lineRule="exact"`,twip)。
    Exact(Twips),
}

/// 一个 rPr 片段:**全 Option**,能区分「未设置(继承)」与「显式关」。
/// docDefaults / 样式定义 / 直接格式化共用同一形状(C-4/C-5 的共享 prop-struct)。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RunProps {
    /// 四槽字体(`w:rFonts`)。
    pub fonts: FontSlots,
    /// 字号(磅;`w:sz` 半磅已除 2)。值属性:就近覆盖。
    pub sz: Option<f32>,
    /// 复杂文种字号(磅;`w:szCs` 半磅已除 2)。值属性。
    pub sz_cs: Option<f32>,
    /// 粗体(`w:b`)。**toggle 属性**:XOR 语义。
    pub b: Option<bool>,
    /// 斜体(`w:i`)。toggle。
    pub i: Option<bool>,
    /// 全大写(`w:caps`)。toggle。
    pub caps: Option<bool>,
    /// 小型大写(`w:smallCaps`)。toggle。
    pub small_caps: Option<bool>,
    /// 单删除线(`w:strike`)。toggle。
    pub strike: Option<bool>,
    /// 隐藏文字(`w:vanish`)。toggle。
    pub vanish: Option<bool>,
    /// 下划线(`w:u`;`val="none"` → `Some(UnderlineKind::None)` = 显式关)。值属性。
    pub u: Option<UnderlineKind>,
    /// 文字颜色(`w:color`)。值属性。
    pub color: Option<ColorRef>,
    /// 高亮(`w:highlight`;`val="none"` → `Some(Highlight::Off)` = 显式关)。值属性。
    pub highlight: Option<Highlight>,
    /// 纵向对齐(`w:vertAlign`:上标/下标/基线)。值属性。
    pub vert_align: Option<VertAlign>,
    /// 字符样式引用(`w:rStyle@w:val`;仅直接格式化侧有意义)。其 basedOn 链插在
    /// 段落样式链之后、直接格式化之前参与级联与 toggle 计数(C-4)。
    pub r_style: Option<String>,
}

impl RunProps {
    /// 值属性的就近覆盖合并:`other`(级联链上更靠近正文的一层)的 `Some` 覆盖本层。
    /// **toggle 属性(b/i/caps/smallCaps/strike/vanish)不在此处理**(XOR 语义,
    /// 见 [`resolve_run`]);`r_style` 也不搬运(样式定义里无意义)。
    fn overlay_values(&mut self, other: &RunProps) {
        for (slot, o) in [
            (&mut self.fonts.ascii, &other.fonts.ascii),
            (&mut self.fonts.h_ansi, &other.fonts.h_ansi),
            (&mut self.fonts.east_asia, &other.fonts.east_asia),
            (&mut self.fonts.cs, &other.fonts.cs),
        ] {
            if o.is_some() {
                *slot = o.clone();
            }
        }
        if other.sz.is_some() {
            self.sz = other.sz;
        }
        if other.sz_cs.is_some() {
            self.sz_cs = other.sz_cs;
        }
        if other.u.is_some() {
            self.u = other.u;
        }
        if other.color.is_some() {
            self.color = other.color;
        }
        if other.highlight.is_some() {
            self.highlight = other.highlight;
        }
        if other.vert_align.is_some() {
            self.vert_align = other.vert_align;
        }
    }
}

/// 一个 pPr 片段(样式定义 / docDefaults / 直接格式化共用):**全 Option**,每个
/// 子属性独立参与级联(就近覆盖)。C-4 起覆盖 spacing / ind / pBdr / shd / keep 系列。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ParaProps {
    /// 对齐(`w:jc@w:val`,归一化枚举)。值属性。
    pub jc: Option<Justification>,
    /// 段前间距(`w:spacing@w:before`,twip)。
    pub space_before: Option<Twips>,
    /// 段后间距(`w:spacing@w:after`,twip)。
    pub space_after: Option<Twips>,
    /// 行距(`w:spacing@w:line` + `@w:lineRule`)。
    pub line: Option<LineSpacingRule>,
    /// 左缩进(`w:ind@w:left`,兼 `@w:start`,twip)。
    pub ind_left: Option<Twips>,
    /// 右缩进(`w:ind@w:right`,兼 `@w:end`,twip)。
    pub ind_right: Option<Twips>,
    /// 首行缩进(`w:ind@w:firstLine`,twip;与 hanging 互斥,hanging 优先)。
    pub ind_first_line: Option<Twips>,
    /// 悬挂缩进(`w:ind@w:hanging`,twip:首行相对 left **回退**该量)。
    pub ind_hanging: Option<Twips>,
    /// 段落边框(`w:pBdr`),每边独立级联。
    pub borders: ParaBorders,
    /// 段落底纹填充(`w:shd@w:fill` / `@w:themeFill`;`"auto"` → `Auto` = 显式无底纹)。
    pub shd_fill: Option<ColorRef>,
    /// 与下段同页(`w:keepNext`)。
    pub keep_next: Option<bool>,
    /// 段中不分页(`w:keepLines`)。
    pub keep_lines: Option<bool>,
    /// 段前分页(`w:pageBreakBefore`)。
    pub page_break_before: Option<bool>,
    /// 孤行控制(`w:widowControl`,Word 缺省开)。
    pub widow_control: Option<bool>,
    /// 同样式相邻段落间不加间距(`w:contextualSpacing`)。
    pub contextual_spacing: Option<bool>,
}

impl ParaProps {
    /// 值属性的就近覆盖合并(与 [`RunProps::overlay_values`] 同构;pPr 无 toggle 属性)。
    fn overlay_values(&mut self, other: &ParaProps) {
        macro_rules! ov {
            ($($field:ident),+ $(,)?) => {
                $(if other.$field.is_some() { self.$field = other.$field.clone(); })+
            };
        }
        ov!(
            jc,
            space_before,
            space_after,
            line,
            ind_left,
            ind_right,
            ind_first_line,
            ind_hanging,
            shd_fill,
            keep_next,
            keep_lines,
            page_break_before,
            widow_control,
            contextual_spacing,
        );
        for (slot, o) in [
            (&mut self.borders.top, &other.borders.top),
            (&mut self.borders.bottom, &other.borders.bottom),
            (&mut self.borders.left, &other.borders.left),
            (&mut self.borders.right, &other.borders.right),
            (&mut self.borders.between, &other.borders.between),
        ] {
            if o.is_some() {
                *slot = o.clone();
            }
        }
    }
}

/// 归一化的段落对齐(`w:jc`)。`left/start → Left`、`right/end → Right`、
/// `both → Justify`。缺省 Left(LTR 文档的 Word 缺省)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Justification {
    #[default]
    Left,
    Center,
    Right,
    Justify,
    Distribute,
}

impl Justification {
    /// 解析 `w:jc@w:val`。未知值 → `None`(容错,当未设置)。
    pub fn from_attr(s: &str) -> Option<Self> {
        Some(match s {
            "left" | "start" => Justification::Left,
            "center" => Justification::Center,
            "right" | "end" => Justification::Right,
            "both" => Justification::Justify,
            "distribute" => Justification::Distribute,
            _ => return None,
        })
    }
}

// ============================================================ 样式表(styles.xml)

/// 样式种类(`w:style@w:type`)。缺省 paragraph(ECMA-376 §17.7.4.17)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StyleKind {
    #[default]
    Paragraph,
    Character,
    Table,
    Numbering,
}

impl StyleKind {
    /// 解析 `w:style@w:type`。未知值按缺省 paragraph 容错。
    pub fn from_attr(s: &str) -> Self {
        match s {
            "character" => StyleKind::Character,
            "table" => StyleKind::Table,
            "numbering" => StyleKind::Numbering,
            _ => StyleKind::Paragraph,
        }
    }
}

/// 一条样式定义(`w:style`):名字 / 种类 / basedOn 父样式 / 是否该类缺省样式 +
/// rPr / pPr 属性片段。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Style {
    /// 显示名(`w:name@w:val`,如 `"heading 1"`)。级联用的是 styleId(表键),名字仅刻画。
    pub name: Option<String>,
    /// 种类(`w:style@w:type`)。
    pub kind: StyleKind,
    /// 父样式 id(`w:basedOn@w:val`)。
    pub based_on: Option<String>,
    /// 是否该种类的缺省样式(`w:style@w:default`)。
    pub default: bool,
    /// run 属性片段(`w:style > w:rPr`)。
    pub rpr: RunProps,
    /// 段落属性片段(`w:style > w:pPr`)。
    pub ppr: ParaProps,
    /// 表格属性片段(`w:style > w:tblPr`,仅表样式有意义)。
    pub tblpr: TableProps,
}

/// 样式表(`word/styles.xml`):docDefaults + `styleId → Style` 映射 + 各类缺省样式 id。
/// 部件缺失时为空表,解析器落到 Word 内置缺省。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct StyleTable {
    /// 文档级 run 属性缺省(`w:docDefaults > w:rPrDefault > w:rPr`)。
    pub doc_default_rpr: RunProps,
    /// 文档级段落属性缺省(`w:docDefaults > w:pPrDefault > w:pPr`)。
    pub doc_default_ppr: ParaProps,
    /// 样式定义(键 = `w:style@w:styleId`)。
    pub styles: BTreeMap<String, Style>,
    /// 缺省段落样式 id(`w:type="paragraph"` 且 `w:default` 为真;通常是 `Normal`)。
    /// 段落无 `pStyle` 时按 Word 语义应用它。
    pub default_para_style: Option<String>,
    /// 缺省字符样式 id(通常是 `DefaultParagraphFont`,按惯例无属性)。
    /// 本轮解析器不消费(rStyle 在 C-4 捕获),保留以完整刻画。
    pub default_char_style: Option<String>,
}

impl StyleTable {
    /// 体检样式表:`basedOn` 环(会让朴素解析器死循环)与悬空 `basedOn` 引用。
    /// 环上的**每个**成员各报一次(确定性:BTreeMap/BTreeSet 序)。
    /// 解析函数([`resolve_run`] 等)自身带 visited-set 防环,遇环终止不悬挂;
    /// 这里的告警供 doc-render 汇入 ExportWarning。
    pub fn validate(&self) -> Vec<StyleWarning> {
        let mut warnings = Vec::new();
        let mut cycle_members: BTreeSet<&str> = BTreeSet::new();
        for (id, style) in &self.styles {
            if let Some(base) = style.based_on.as_deref() {
                if !self.styles.contains_key(base) {
                    warnings.push(StyleWarning::UnknownBasedOn {
                        style_id: id.clone(),
                        based_on: base.to_string(),
                    });
                }
            }
            // 从每个样式出发沿 basedOn 走:重访到出发点即证明它在环上。
            let mut visited: BTreeSet<&str> = BTreeSet::new();
            let mut cur = Some(id.as_str());
            while let Some(sid) = cur {
                if !visited.insert(sid) {
                    if sid == id {
                        cycle_members.insert(sid);
                    }
                    break;
                }
                cur = self.styles.get(sid).and_then(|s| s.based_on.as_deref());
            }
        }
        warnings.extend(
            cycle_members
                .into_iter()
                .map(|id| StyleWarning::BasedOnCycle {
                    style_id: id.to_string(),
                }),
        );
        warnings
    }
}

/// 样式表体检告警(供 doc-render 转成 ExportWarning 浮出)。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StyleWarning {
    /// `basedOn` 链成环(样式 `style_id` 在环上)。解析已按 visited-set 截断,不悬挂。
    BasedOnCycle { style_id: String },
    /// `basedOn` 指向不存在的样式 id。级联按链在此截断处理。
    UnknownBasedOn { style_id: String, based_on: String },
}

// ============================================================ 有效属性(解析输出)

/// 一个 run 的**有效** run 属性:级联合并 + theme 解引 + Word 内置兜底之后的最终值。
/// doc-render 直接按此排版,不再看原始片段。
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveRunProps {
    /// 西文字体 family(兜底 [`DEFAULT_LATIN_FONT`])。
    pub font_ascii: String,
    /// 高 ANSI 字体(未设时随 `font_ascii`)。
    pub font_h_ansi: String,
    /// 东亚字体(无兜底:`None` 交渲染侧字体回退链)。
    pub font_east_asia: Option<String>,
    /// 复杂文种字体(无兜底)。
    pub font_cs: Option<String>,
    /// 字号(磅,兜底 [`DEFAULT_SIZE_PT`])。
    pub size_pt: f32,
    /// 复杂文种字号(磅;未设时随 `size_pt`)。
    pub size_cs_pt: f32,
    pub bold: bool,
    pub italic: bool,
    /// 是否画下划线(`underline_kind` 非 `None`)。
    pub underline: bool,
    /// 下划线种类(缺省 `None` = 不画;渲染侧 v1 一律画单线)。
    pub underline_kind: UnderlineKind,
    pub strike: bool,
    pub caps: bool,
    pub small_caps: bool,
    pub vanish: bool,
    /// 文字颜色;`None` = auto/未设(渲染侧按黑)。
    pub color: Option<Color>,
    /// 高亮底色;`None` = 无高亮。
    pub highlight: Option<Color>,
    /// 纵向对齐(上标/下标/基线,缺省基线)。
    pub vert_align: VertAlign,
}

/// 有效行距(twip 已换算成磅)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EffectiveLineSpacing {
    /// 行高倍数(1.0 = 单倍,缺省)。
    Multiple(f32),
    /// 最小行高(磅)。
    AtLeast(f32),
    /// 精确行高(磅)。
    Exact(f32),
}

impl Default for EffectiveLineSpacing {
    fn default() -> Self {
        EffectiveLineSpacing::Multiple(1.0)
    }
}

/// 一个段落的**有效**段落属性(长度已换算成磅)。
#[derive(Debug, Clone, PartialEq)]
pub struct EffectiveParaProps {
    /// 对齐(缺省 Left)。
    pub align: Justification,
    /// 段前间距(磅,缺省 0)。
    pub space_before_pt: f32,
    /// 段后间距(磅,缺省 0)。
    pub space_after_pt: f32,
    /// 行距(缺省单倍)。
    pub line_spacing: EffectiveLineSpacing,
    /// 左缩进(磅,缺省 0)。
    pub indent_left_pt: f32,
    /// 右缩进(磅,缺省 0)。
    pub indent_right_pt: f32,
    /// 首行相对左缩进的额外缩进(磅):正 = firstLine,**负 = hanging**,缺省 0。
    pub first_line_indent_pt: f32,
    /// 段落边框(级联合并后的原始各边;渲染侧 v1 降级为告警)。
    pub borders: ParaBorders,
    /// 段落底纹填充色(theme 已解引;`None` = 无底纹)。
    pub shading: Option<Color>,
    /// 与下段同页。
    pub keep_next: bool,
    /// 段中不分页。
    pub keep_lines: bool,
    /// 段前分页。
    pub page_break_before: bool,
    /// 孤行控制(Word 缺省开)。
    pub widow_control: bool,
    /// 同样式相邻段落间不加间距。
    pub contextual_spacing: bool,
}

// ============================================================ 解析器(级联合并)

/// 解析一个正文段落的有效段落属性(非表格内;表格内用 [`resolve_para_in_table`])。
pub fn resolve_para(doc: &Document, para: &Paragraph) -> EffectiveParaProps {
    resolve_para_props(doc, None, para)
}

/// 解析表格内段落的有效段落属性:表格样式(`tblStyle` 链)插在 docDefaults 与段落样式之间。
pub fn resolve_para_in_table(
    doc: &Document,
    table: &Table,
    para: &Paragraph,
) -> EffectiveParaProps {
    resolve_para_props(doc, table.style.as_deref(), para)
}

/// 解析一个 run 的有效 run 属性(非表格内;表格内用 [`resolve_run_in_table`])。
pub fn resolve_run(doc: &Document, para: &Paragraph, run: &TextRun) -> EffectiveRunProps {
    resolve_run_props(doc, None, para, run)
}

/// 一张表格的**有效**表级属性:表样式链(basedOn 递归,根先)→ 直接 tblPr,逐边合并。
/// 单元格级冲突消解(tcBorders 优先、共享边归并)在渲染侧做,这里只出表级底座。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EffectiveTableProps {
    /// 表级边框(六槽,级联合并后)。
    pub borders: TableBorders,
    /// 表级单元格缺省边距(级联合并后;Word 缺省兜底由渲染侧给)。
    pub cell_margins: CellMargins,
}

/// 解析一张表格的有效表级属性(表样式链 → 直接 tblPr)。
pub fn resolve_table(doc: &Document, table: &Table) -> EffectiveTableProps {
    let mut eff = EffectiveTableProps::default();
    for s in style_chain(&doc.styles, table.style.as_deref()) {
        eff.borders.overlay(&s.tblpr.borders);
        eff.cell_margins.overlay(&s.tblpr.cell_margins);
    }
    eff.borders.overlay(&table.borders);
    eff.cell_margins.overlay(&table.cell_margins);
    eff
}

/// 解析表格内 run 的有效 run 属性(表格样式链参与级联与 toggle 计数)。
pub fn resolve_run_in_table(
    doc: &Document,
    table: &Table,
    para: &Paragraph,
    run: &TextRun,
) -> EffectiveRunProps {
    resolve_run_props(doc, table.style.as_deref(), para, run)
}

/// 组装完整样式链:表格样式链(如在表格内)+ 段落样式链,各自 basedOn 递归、根先派生后。
/// 段落无 `pStyle` 时应用缺省段落样式(Word 语义:每个段落都有样式)。
fn full_chain<'a>(
    table: &'a StyleTable,
    table_style: Option<&str>,
    para: &'a Paragraph,
) -> Vec<&'a Style> {
    let mut chain = style_chain(table, table_style);
    let para_style = para
        .style
        .as_deref()
        .or(table.default_para_style.as_deref());
    chain.extend(style_chain(table, para_style));
    chain
}

/// 沿 `basedOn` 走出一条样式链,**根(最基)在前**。visited-set 防环:重访即截断,
/// 有限步终止(环告警见 [`StyleTable::validate`]);未知 id 处链截断。
fn style_chain<'a>(table: &'a StyleTable, id: Option<&str>) -> Vec<&'a Style> {
    let mut chain = Vec::new();
    let mut visited: BTreeSet<&str> = BTreeSet::new();
    let mut cur = id;
    while let Some(sid) = cur {
        if !visited.insert(sid) {
            break; // basedOn 成环:截断。
        }
        let Some(style) = table.styles.get(sid) else {
            break; // 悬空引用:截断。
        };
        chain.push(style);
        cur = style.based_on.as_deref();
    }
    chain.reverse();
    chain
}

/// toggle 属性的有效值(ECMA-376 §17.7.3):直接格式化是绝对开关;否则以 docDefaults
/// 为基值,与样式链上 `Some(true)` 出现次数的奇偶异或(显式 `false` 不参与计数)。
fn resolve_toggle(
    direct: Option<bool>,
    doc_default: Option<bool>,
    chain: &[&Style],
    get: impl Fn(&RunProps) -> Option<bool>,
) -> bool {
    if let Some(v) = direct {
        return v;
    }
    let base = doc_default == Some(true);
    let odd = chain.iter().filter(|s| get(&s.rpr) == Some(true)).count() % 2 == 1;
    base ^ odd
}

/// 把一处字体引用解成实际 family 名:显名直接用;主题引用查 fontScheme,latin 槽在
/// theme 缺失/为空时落硬编码兜底(ea/cs 槽无兜底,交渲染侧回退链)。
fn deref_font(font: &FontRef, theme: &Theme) -> Option<String> {
    match font {
        FontRef::Named(name) => Some(name.clone()),
        FontRef::Theme(slot) => {
            let set = if slot.is_major() {
                &theme.fonts.major
            } else {
                &theme.fonts.minor
            };
            let resolved = match slot {
                ThemeFont::MajorAscii
                | ThemeFont::MajorHAnsi
                | ThemeFont::MinorAscii
                | ThemeFont::MinorHAnsi => set.latin.as_ref(),
                ThemeFont::MajorEastAsia | ThemeFont::MinorEastAsia => set.east_asia.as_ref(),
                ThemeFont::MajorBidi | ThemeFont::MinorBidi => set.cs.as_ref(),
            };
            match resolved {
                Some(name) => Some(name.clone()),
                None => match slot {
                    // latin 槽兜底 Office 默认主题;ea/cs 无兜底。
                    ThemeFont::MajorAscii | ThemeFont::MajorHAnsi => {
                        Some(DEFAULT_MAJOR_LATIN_FONT.to_string())
                    }
                    ThemeFont::MinorAscii | ThemeFont::MinorHAnsi => {
                        Some(DEFAULT_LATIN_FONT.to_string())
                    }
                    _ => None,
                },
            }
        }
    }
}

fn resolve_run_props(
    doc: &Document,
    table_style: Option<&str>,
    para: &Paragraph,
    run: &TextRun,
) -> EffectiveRunProps {
    let st = &doc.styles;
    let mut chain = full_chain(st, table_style, para);
    // 字符样式(`w:rStyle`)链:插在段落样式链之后、直接格式化之前(ECMA-376 §17.7.2),
    // 同样根先派生后,并进入 toggle 的 XOR 计数。
    chain.extend(style_chain(st, run.rpr.r_style.as_deref()));

    // 值属性:docDefaults → 样式链(根先)→ 直接格式化,后者的 Some 覆盖前者。
    let mut merged = st.doc_default_rpr.clone();
    for s in &chain {
        merged.overlay_values(&s.rpr);
    }
    merged.overlay_values(&run.rpr);

    // toggle 属性:XOR 语义(直接格式化绝对开关)。
    let dd = &st.doc_default_rpr;
    let bold = resolve_toggle(run.rpr.b, dd.b, &chain, |r| r.b);
    let italic = resolve_toggle(run.rpr.i, dd.i, &chain, |r| r.i);
    let caps = resolve_toggle(run.rpr.caps, dd.caps, &chain, |r| r.caps);
    let small_caps = resolve_toggle(run.rpr.small_caps, dd.small_caps, &chain, |r| r.small_caps);
    let strike = resolve_toggle(run.rpr.strike, dd.strike, &chain, |r| r.strike);
    let vanish = resolve_toggle(run.rpr.vanish, dd.vanish, &chain, |r| r.vanish);

    // theme 解引 + Word 内置兜底。
    let font_ascii = merged
        .fonts
        .ascii
        .as_ref()
        .and_then(|f| deref_font(f, &doc.theme))
        .unwrap_or_else(|| DEFAULT_LATIN_FONT.to_string());
    let font_h_ansi = merged
        .fonts
        .h_ansi
        .as_ref()
        .and_then(|f| deref_font(f, &doc.theme))
        .unwrap_or_else(|| font_ascii.clone());
    let font_east_asia = merged
        .fonts
        .east_asia
        .as_ref()
        .and_then(|f| deref_font(f, &doc.theme));
    let font_cs = merged
        .fonts
        .cs
        .as_ref()
        .and_then(|f| deref_font(f, &doc.theme));
    let color = match merged.color {
        Some(ColorRef::Rgb(c)) => Some(c),
        Some(ColorRef::Theme(slot)) => doc.theme.colors.get(slot),
        Some(ColorRef::Auto) | None => None, // auto/未设:渲染侧按黑。
    };

    let size_pt = merged.sz.unwrap_or(DEFAULT_SIZE_PT);
    let underline_kind = merged.u.unwrap_or(UnderlineKind::None);
    EffectiveRunProps {
        font_ascii,
        font_h_ansi,
        font_east_asia,
        font_cs,
        size_pt,
        size_cs_pt: merged.sz_cs.unwrap_or(size_pt),
        bold,
        italic,
        underline: underline_kind.is_on(),
        underline_kind,
        strike,
        caps,
        small_caps,
        vanish,
        color,
        highlight: match merged.highlight {
            Some(Highlight::On(c)) => Some(c),
            Some(Highlight::Off) | None => None,
        },
        vert_align: merged.vert_align.unwrap_or_default(),
    }
}

fn resolve_para_props(
    doc: &Document,
    table_style: Option<&str>,
    para: &Paragraph,
) -> EffectiveParaProps {
    let st = &doc.styles;
    let chain = full_chain(st, table_style, para);

    // 值属性:docDefaults → 样式链(根先)→ numbering 层级 pPr → 直接格式化
    // (`Paragraph.ppr` 共享片段)。numbering.xml 的层级缩进插在样式层之下、
    // 直格之上(ECMA-376 §17.9.5 的实践序,C-6)。
    let mut merged = st.doc_default_ppr.clone();
    for s in &chain {
        merged.overlay_values(&s.ppr);
    }
    if let Some(num_id) = para.num_id {
        if let Some(level) = doc.numbering.level(num_id, para.list_level.unwrap_or(0)) {
            merged.overlay_values(&level.ppr);
        }
    }
    merged.overlay_values(&para.ppr);

    let pt = |t: Option<Twips>| t.map(|v| twips_to_points(v) as f32).unwrap_or(0.0);
    // 首行缩进:hanging 优先于 firstLine(ECMA-376 §17.3.1.12:两者互斥,hanging 胜)。
    let first_line_indent_pt = match (merged.ind_hanging, merged.ind_first_line) {
        (Some(h), _) => -(twips_to_points(h) as f32),
        (None, Some(f)) => twips_to_points(f) as f32,
        (None, None) => 0.0,
    };
    let line_spacing = match merged.line {
        // line 单位 1/240 行;非正值容错为单倍。
        Some(LineSpacingRule::Auto(l)) if l > 0 => EffectiveLineSpacing::Multiple(l as f32 / 240.0),
        Some(LineSpacingRule::AtLeast(t)) if t > 0 => {
            EffectiveLineSpacing::AtLeast(twips_to_points(t) as f32)
        }
        Some(LineSpacingRule::Exact(t)) if t > 0 => {
            EffectiveLineSpacing::Exact(twips_to_points(t) as f32)
        }
        _ => EffectiveLineSpacing::default(),
    };
    let shading = match merged.shd_fill {
        Some(ColorRef::Rgb(c)) => Some(c),
        Some(ColorRef::Theme(slot)) => doc.theme.colors.get(slot),
        Some(ColorRef::Auto) | None => None,
    };

    EffectiveParaProps {
        align: merged.jc.unwrap_or_default(),
        space_before_pt: pt(merged.space_before),
        space_after_pt: pt(merged.space_after),
        line_spacing,
        indent_left_pt: pt(merged.ind_left),
        indent_right_pt: pt(merged.ind_right),
        first_line_indent_pt,
        borders: merged.borders,
        shading,
        keep_next: merged.keep_next == Some(true),
        keep_lines: merged.keep_lines == Some(true),
        page_break_before: merged.page_break_before == Some(true),
        widow_control: merged.widow_control.unwrap_or(true),
        contextual_spacing: merged.contextual_spacing == Some(true),
    }
}

// ============================================================ 单测:每条合并语义分支

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Document, Paragraph, Table, TextRun};

    /// 便利:造一条只带 rPr 片段的段落样式。
    fn para_style(based_on: Option<&str>, rpr: RunProps, ppr: ParaProps) -> Style {
        Style {
            kind: StyleKind::Paragraph,
            based_on: based_on.map(str::to_string),
            rpr,
            ppr,
            ..Style::default()
        }
    }

    fn doc_with_styles(styles: Vec<(&str, Style)>) -> Document {
        let mut doc = Document::default();
        for (id, s) in styles {
            doc.styles.styles.insert(id.to_string(), s);
        }
        doc
    }

    fn styled_para(style: Option<&str>) -> Paragraph {
        Paragraph {
            style: style.map(str::to_string),
            ..Paragraph::default()
        }
    }

    /// numbering 层级 pPr 的级联位置(C-6):样式层之下、直接格式化之上。
    /// 样式 ind_left=300 被层级 720 盖过;直格 900 再盖过层级。
    #[test]
    fn numbering_level_ppr_sits_between_style_and_direct() {
        use crate::numbering::{AbstractNum, Num, NumLevel};
        let mut doc = doc_with_styles(vec![(
            "Listy",
            para_style(
                None,
                RunProps::default(),
                ParaProps {
                    ind_left: Some(300),
                    ind_right: Some(150),
                    ..ParaProps::default()
                },
            ),
        )]);
        doc.numbering.abstracts.insert(
            0,
            AbstractNum {
                levels: [(
                    0u32,
                    NumLevel {
                        ppr: ParaProps {
                            ind_left: Some(720),
                            ind_hanging: Some(360),
                            ..ParaProps::default()
                        },
                        ..NumLevel::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..AbstractNum::default()
            },
        );
        doc.numbering.nums.insert(
            1,
            Num {
                abstract_id: 0,
                ..Num::default()
            },
        );
        let mut para = styled_para(Some("Listy"));
        para.num_id = Some(1);
        para.list_level = Some(0);
        let eff = resolve_para(&doc, &para);
        assert_eq!(eff.indent_left_pt, 36.0, "层级 720 twip 盖过样式 300");
        assert_eq!(eff.indent_right_pt, 7.5, "层级未设的槽位从样式继承");
        assert_eq!(eff.first_line_indent_pt, -18.0, "层级 hanging 360 生效");
        // 直接格式化盖过层级。
        para.ppr.ind_left = Some(900);
        let eff = resolve_para(&doc, &para);
        assert_eq!(eff.indent_left_pt, 45.0, "直格 900 twip 盖过层级 720");
        // 无 numPr 的段落不受层级影响。
        let plain = styled_para(Some("Listy"));
        assert_eq!(resolve_para(&doc, &plain).indent_left_pt, 15.0);
    }

    /// 表样式链的表级属性级联(C-7):basedOn 根先、派生后,直接 tblPr 最末。
    #[test]
    fn resolve_table_cascades_style_chain_then_direct() {
        let base_borders = TableBorders {
            top: Some(Border {
                val: "single".into(),
                sz_eighth_pt: 4,
                space_pt: 0,
                color: None,
            }),
            inside_h: Some(Border {
                val: "single".into(),
                sz_eighth_pt: 4,
                space_pt: 0,
                color: None,
            }),
            ..TableBorders::default()
        };
        let mut doc = doc_with_styles(vec![
            (
                "TBase",
                Style {
                    kind: StyleKind::Table,
                    tblpr: TableProps {
                        borders: base_borders,
                        cell_margins: CellMargins {
                            left: Some(200),
                            ..CellMargins::default()
                        },
                    },
                    ..Style::default()
                },
            ),
            (
                "TDerived",
                Style {
                    kind: StyleKind::Table,
                    based_on: Some("TBase".into()),
                    tblpr: TableProps {
                        borders: TableBorders {
                            top: Some(Border {
                                val: "single".into(),
                                sz_eighth_pt: 16,
                                space_pt: 0,
                                color: None,
                            }),
                            ..TableBorders::default()
                        },
                        ..TableProps::default()
                    },
                    ..Style::default()
                },
            ),
        ]);
        doc.styles.styles.get_mut("TBase").expect("TBase").kind = StyleKind::Table;
        let mut table = Table {
            style: Some("TDerived".into()),
            ..Table::default()
        };
        table.cell_margins.left = Some(288);
        let eff = resolve_table(&doc, &table);
        assert_eq!(
            eff.borders.top.as_ref().map(|b| b.sz_eighth_pt),
            Some(16),
            "派生样式盖过基样式的 top"
        );
        assert_eq!(
            eff.borders.inside_h.as_ref().map(|b| b.sz_eighth_pt),
            Some(4),
            "未覆盖的 insideH 从基样式继承"
        );
        assert_eq!(eff.cell_margins.left, Some(288), "直接 tblCellMar 最末覆盖");
    }

    /// 空文档 + 无任何样式/直接格式化:落 Word 内置缺省(Calibri 11pt,全开关关)。
    #[test]
    fn hardcoded_word_defaults_when_everything_absent() {
        let doc = Document::default();
        let para = Paragraph::default();
        let run = TextRun::default();
        let eff = resolve_run(&doc, &para, &run);
        assert_eq!(eff.font_ascii, DEFAULT_LATIN_FONT);
        assert_eq!(eff.font_h_ansi, DEFAULT_LATIN_FONT);
        assert_eq!(eff.font_east_asia, None);
        assert_eq!(eff.size_pt, DEFAULT_SIZE_PT);
        assert!(!eff.bold && !eff.italic && !eff.underline && !eff.strike);
        assert_eq!(eff.color, None);
        assert_eq!(resolve_para(&doc, &para).align, Justification::Left);
    }

    /// docDefaults 覆盖内置缺省:rPrDefault 的字号/字体、pPrDefault 的对齐生效。
    #[test]
    fn doc_defaults_apply_to_unstyled_content() {
        let mut doc = Document::default();
        doc.styles.doc_default_rpr.sz = Some(12.0);
        doc.styles.doc_default_rpr.fonts.ascii = Some(FontRef::Named("Aptos".into()));
        doc.styles.doc_default_ppr.jc = Some(Justification::Justify);
        let para = Paragraph::default();
        let eff = resolve_run(&doc, &para, &TextRun::default());
        assert_eq!(eff.size_pt, 12.0);
        assert_eq!(eff.font_ascii, "Aptos");
        // h_ansi 未设:随 ascii。
        assert_eq!(eff.font_h_ansi, "Aptos");
        assert_eq!(resolve_para(&doc, &para).align, Justification::Justify);
    }

    /// basedOn 链:派生样式覆盖基样式的值属性;未覆盖的槽位从基样式继承。
    #[test]
    fn based_on_chain_value_override_and_inherit() {
        let normal = para_style(
            None,
            RunProps {
                sz: Some(11.0),
                fonts: FontSlots {
                    ascii: Some(FontRef::Named("Base".into())),
                    east_asia: Some(FontRef::Named("宋体".into())),
                    ..FontSlots::default()
                },
                ..RunProps::default()
            },
            ParaProps {
                jc: Some(Justification::Left),
                ..ParaProps::default()
            },
        );
        let heading = para_style(
            Some("Normal"),
            RunProps {
                sz: Some(16.0),
                ..RunProps::default()
            },
            ParaProps {
                jc: Some(Justification::Center),
                ..ParaProps::default()
            },
        );
        let doc = doc_with_styles(vec![("Normal", normal), ("Heading1", heading)]);
        let para = styled_para(Some("Heading1"));
        let eff = resolve_run(&doc, &para, &TextRun::default());
        assert_eq!(eff.size_pt, 16.0); // 派生覆盖
        assert_eq!(eff.font_ascii, "Base"); // 槽位继承
        assert_eq!(eff.font_east_asia.as_deref(), Some("宋体"));
        assert_eq!(resolve_para(&doc, &para).align, Justification::Center);
    }

    /// 段落无 pStyle 时应用缺省段落样式(Word 语义:每个段落都有样式)。
    #[test]
    fn default_paragraph_style_applies_when_pstyle_absent() {
        let mut normal = para_style(
            None,
            RunProps {
                sz: Some(10.5),
                ..RunProps::default()
            },
            ParaProps::default(),
        );
        normal.default = true;
        let mut doc = doc_with_styles(vec![("Normal", normal)]);
        doc.styles.default_para_style = Some("Normal".into());
        let eff = resolve_run(&doc, &Paragraph::default(), &TextRun::default());
        assert_eq!(eff.size_pt, 10.5);
    }

    /// toggle XOR:样式链上 b=true 出现奇数次 → 开;偶数次 → 关(相互抵消)。
    #[test]
    fn toggle_xor_along_style_chain() {
        let bold_on = RunProps {
            b: Some(true),
            ..RunProps::default()
        };
        // 奇数次(1 层):开。
        let doc = doc_with_styles(vec![(
            "S1",
            para_style(None, bold_on.clone(), ParaProps::default()),
        )]);
        let eff = resolve_run(&doc, &styled_para(Some("S1")), &TextRun::default());
        assert!(eff.bold, "b=true x1(奇数)→ 开");

        // 偶数次(2 层,basedOn 链):关。
        let doc = doc_with_styles(vec![
            (
                "Normal",
                para_style(None, bold_on.clone(), ParaProps::default()),
            ),
            (
                "Heading1",
                para_style(Some("Normal"), bold_on.clone(), ParaProps::default()),
            ),
        ]);
        let eff = resolve_run(&doc, &styled_para(Some("Heading1")), &TextRun::default());
        assert!(!eff.bold, "b=true x2(偶数)→ 关");

        // 样式里显式 false 不参与计数:true x1 + false x1 → 仍开。
        let doc = doc_with_styles(vec![
            (
                "Normal",
                para_style(None, bold_on.clone(), ParaProps::default()),
            ),
            (
                "Derived",
                para_style(
                    Some("Normal"),
                    RunProps {
                        b: Some(false),
                        ..RunProps::default()
                    },
                    ParaProps::default(),
                ),
            ),
        ]);
        let eff = resolve_run(&doc, &styled_para(Some("Derived")), &TextRun::default());
        assert!(eff.bold, "显式 false 不计入 XOR");
    }

    /// toggle:直接格式化是绝对开关(b=0 恒关、b=1 恒开,无视链上奇偶)。
    #[test]
    fn toggle_direct_formatting_is_absolute() {
        let bold_on = RunProps {
            b: Some(true),
            ..RunProps::default()
        };
        let doc = doc_with_styles(vec![(
            "S1",
            para_style(None, bold_on.clone(), ParaProps::default()),
        )]);
        let para = styled_para(Some("S1"));

        // 链上奇数次(开)+ direct b=0 → 恒关。
        let mut run = TextRun::default();
        run.rpr.b = Some(false);
        assert!(!resolve_run(&doc, &para, &run).bold);

        // 链上偶数次(关)+ direct b=1 → 恒开。
        let doc2 = doc_with_styles(vec![
            (
                "Normal",
                para_style(None, bold_on.clone(), ParaProps::default()),
            ),
            (
                "H",
                para_style(Some("Normal"), bold_on, ParaProps::default()),
            ),
        ]);
        let mut run = TextRun::default();
        run.rpr.b = Some(true);
        assert!(resolve_run(&doc2, &styled_para(Some("H")), &run).bold);
    }

    /// toggle:docDefaults 是基值,与链上奇偶再异或(docDefault=true + 链上 x1 → 关)。
    #[test]
    fn toggle_doc_default_is_xor_base() {
        let mut doc = doc_with_styles(vec![(
            "S1",
            para_style(
                None,
                RunProps {
                    i: Some(true),
                    ..RunProps::default()
                },
                ParaProps::default(),
            ),
        )]);
        doc.styles.doc_default_rpr.i = Some(true);
        let eff = resolve_run(&doc, &styled_para(Some("S1")), &TextRun::default());
        assert!(!eff.italic, "docDefault(真)XOR 链上奇数次(真)→ 关");
        // 无样式链时:docDefault 直接生效。
        let eff = resolve_run(&doc, &Paragraph::default(), &TextRun::default());
        assert!(eff.italic);
    }

    /// basedOn 成环:解析有限步终止(不悬挂),validate 对环上成员告警。
    #[test]
    fn based_on_cycle_terminates_and_warns() {
        let doc = doc_with_styles(vec![
            (
                "A",
                para_style(
                    Some("B"),
                    RunProps {
                        sz: Some(14.0),
                        ..RunProps::default()
                    },
                    ParaProps::default(),
                ),
            ),
            (
                "B",
                para_style(Some("A"), RunProps::default(), ParaProps::default()),
            ),
        ]);
        // 终止且 A 的值属性仍生效。
        let eff = resolve_run(&doc, &styled_para(Some("A")), &TextRun::default());
        assert_eq!(eff.size_pt, 14.0);
        // 环上成员各报一次。
        let warnings = doc.styles.validate();
        assert!(warnings.contains(&StyleWarning::BasedOnCycle {
            style_id: "A".into()
        }));
        assert!(warnings.contains(&StyleWarning::BasedOnCycle {
            style_id: "B".into()
        }));
    }

    /// 悬空 basedOn:链截断 + UnknownBasedOn 告警,不 panic。
    #[test]
    fn unknown_based_on_truncates_and_warns() {
        let doc = doc_with_styles(vec![(
            "S1",
            para_style(
                Some("Ghost"),
                RunProps {
                    sz: Some(13.0),
                    ..RunProps::default()
                },
                ParaProps::default(),
            ),
        )]);
        let eff = resolve_run(&doc, &styled_para(Some("S1")), &TextRun::default());
        assert_eq!(eff.size_pt, 13.0);
        assert!(doc
            .styles
            .validate()
            .contains(&StyleWarning::UnknownBasedOn {
                style_id: "S1".into(),
                based_on: "Ghost".into()
            }));
    }

    /// theme 字体间接引用:asciiTheme=minorHAnsi → fontScheme minor latin 实际名;
    /// eastAsiaTheme=minorEastAsia → minor ea;theme 缺槽时 latin 落硬编码兜底。
    #[test]
    fn theme_font_indirection_resolves_family_names() {
        let mut doc = Document::default();
        doc.theme.fonts.minor.latin = Some("Aptos".into());
        doc.theme.fonts.minor.east_asia = Some("DengXian".into());
        doc.theme.fonts.major.latin = Some("Aptos Display".into());
        doc.styles.doc_default_rpr.fonts = FontSlots {
            ascii: Some(FontRef::Theme(ThemeFont::MinorHAnsi)),
            h_ansi: Some(FontRef::Theme(ThemeFont::MinorHAnsi)),
            east_asia: Some(FontRef::Theme(ThemeFont::MinorEastAsia)),
            cs: Some(FontRef::Theme(ThemeFont::MinorBidi)),
        };
        let eff = resolve_run(&doc, &Paragraph::default(), &TextRun::default());
        assert_eq!(eff.font_ascii, "Aptos");
        assert_eq!(eff.font_east_asia.as_deref(), Some("DengXian"));
        assert_eq!(eff.font_cs, None, "theme cs 槽为空:无兜底");

        // majorHAnsi 引用 → major latin。
        let mut run = TextRun::default();
        run.rpr.fonts.ascii = Some(FontRef::Theme(ThemeFont::MajorHAnsi));
        let eff = resolve_run(&doc, &Paragraph::default(), &run);
        assert_eq!(eff.font_ascii, "Aptos Display");

        // theme 部件缺失:latin 槽落硬编码兜底(Calibri / Calibri Light)。
        let mut bare = Document::default();
        bare.styles.doc_default_rpr.fonts.ascii = Some(FontRef::Theme(ThemeFont::MinorHAnsi));
        let eff = resolve_run(&bare, &Paragraph::default(), &TextRun::default());
        assert_eq!(eff.font_ascii, DEFAULT_LATIN_FONT);
        let mut run = TextRun::default();
        run.rpr.fonts.ascii = Some(FontRef::Theme(ThemeFont::MajorHAnsi));
        let eff = resolve_run(&bare, &Paragraph::default(), &run);
        assert_eq!(eff.font_ascii, DEFAULT_MAJOR_LATIN_FONT);
    }

    /// theme 颜色间接引用:color@themeColor=accent1 → clrScheme 的 RGB;显式 RGB 覆盖;
    /// auto → None。
    #[test]
    fn theme_color_indirection_and_auto() {
        let mut doc = Document::default();
        doc.theme
            .colors
            .set(ThemeColor::Accent1, Color::new([0x44, 0x72, 0xC4]));
        doc.styles.doc_default_rpr.color = Some(ColorRef::Theme(ThemeColor::Accent1));
        let eff = resolve_run(&doc, &Paragraph::default(), &TextRun::default());
        assert_eq!(eff.color, Some(Color::new([0x44, 0x72, 0xC4])));

        // 直接格式化的显式 RGB 覆盖主题引用。
        let mut run = TextRun::default();
        run.rpr.color = Some(ColorRef::Rgb(Color::new([1, 2, 3])));
        let eff = resolve_run(&doc, &Paragraph::default(), &run);
        assert_eq!(eff.color, Some(Color::new([1, 2, 3])));

        // auto:显式设置但解析为 None(渲染侧按黑),仍覆盖继承色。
        let mut run = TextRun::default();
        run.rpr.color = Some(ColorRef::Auto);
        let eff = resolve_run(&doc, &Paragraph::default(), &run);
        assert_eq!(eff.color, None);
    }

    /// 直接格式化优先:direct sz / jc 覆盖样式链与 docDefaults。
    #[test]
    fn direct_formatting_wins_for_value_props() {
        let mut doc = doc_with_styles(vec![(
            "S1",
            para_style(
                None,
                RunProps {
                    sz: Some(16.0),
                    ..RunProps::default()
                },
                ParaProps {
                    jc: Some(Justification::Center),
                    ..ParaProps::default()
                },
            ),
        )]);
        doc.styles.doc_default_rpr.sz = Some(11.0);
        let mut para = styled_para(Some("S1"));
        para.ppr.jc = Some(Justification::Right);
        let mut run = TextRun::default();
        run.rpr.sz = Some(9.0);
        assert_eq!(resolve_run(&doc, &para, &run).size_pt, 9.0);
        assert_eq!(resolve_para(&doc, &para).align, Justification::Right);
    }

    /// 表格样式 overlay:tblStyle 链插在 docDefaults 与段落样式之间;段落样式仍可覆盖;
    /// 表格样式的 toggle 参与 XOR 计数。
    #[test]
    fn table_style_overlay_sits_between_defaults_and_para_style() {
        let doc = doc_with_styles(vec![
            (
                "TStyle",
                Style {
                    kind: StyleKind::Table,
                    rpr: RunProps {
                        sz: Some(9.0),
                        b: Some(true),
                        ..RunProps::default()
                    },
                    ppr: ParaProps {
                        jc: Some(Justification::Center),
                        ..ParaProps::default()
                    },
                    ..Style::default()
                },
            ),
            (
                "CellPara",
                para_style(
                    None,
                    RunProps {
                        sz: Some(10.0),
                        ..RunProps::default()
                    },
                    ParaProps::default(),
                ),
            ),
        ]);
        let table = Table {
            style: Some("TStyle".into()),
            ..Table::default()
        };

        // 无段落样式:表格样式生效(字号/对齐/粗体)。
        let para = Paragraph::default();
        let eff = resolve_run_in_table(&doc, &table, &para, &TextRun::default());
        assert_eq!(eff.size_pt, 9.0);
        assert!(eff.bold, "表格样式的 b=true 计入 XOR(奇数)");
        assert_eq!(
            resolve_para_in_table(&doc, &table, &para).align,
            Justification::Center
        );

        // 段落样式覆盖表格样式的值属性。
        let para = styled_para(Some("CellPara"));
        let eff = resolve_run_in_table(&doc, &table, &para, &TextRun::default());
        assert_eq!(eff.size_pt, 10.0);
    }

    /// 字符样式(rStyle)链:值属性插在段落样式之后、直接格式化之前;toggle 计入 XOR。
    #[test]
    fn r_style_chain_overlays_values_and_counts_in_toggle_xor() {
        let char_base = Style {
            kind: StyleKind::Character,
            rpr: RunProps {
                sz: Some(9.0),
                b: Some(true),
                ..RunProps::default()
            },
            ..Style::default()
        };
        let char_derived = Style {
            kind: StyleKind::Character,
            based_on: Some("CharBase".into()),
            rpr: RunProps {
                color: Some(ColorRef::Rgb(Color::new([0xFF, 0x00, 0x00]))),
                b: Some(true),
                ..RunProps::default()
            },
            ..Style::default()
        };
        let para_style_ = para_style(
            None,
            RunProps {
                sz: Some(12.0),
                b: Some(true),
                ..RunProps::default()
            },
            ParaProps::default(),
        );
        let doc = doc_with_styles(vec![
            ("CharBase", char_base),
            ("Emphasis", char_derived),
            ("P", para_style_),
        ]);
        let para = styled_para(Some("P"));
        let mut run = TextRun::default();
        run.rpr.r_style = Some("Emphasis".into());
        let eff = resolve_run(&doc, &para, &run);
        // 值:字符样式链(9pt)覆盖段落样式(12pt);颜色来自派生字符样式。
        assert_eq!(eff.size_pt, 9.0);
        assert_eq!(eff.color, Some(Color::new([0xFF, 0x00, 0x00])));
        // toggle:段落样式 b=1 + 字符链 b=1 x2 → 共 3 次(奇数)→ 开。
        assert!(eff.bold);
        // 直接格式化仍是绝对开关。
        run.rpr.b = Some(false);
        assert!(!resolve_run(&doc, &para, &run).bold);
        // 未知 rStyle:链截断,不影响其余解析。
        let mut run = TextRun::default();
        run.rpr.r_style = Some("Ghost".into());
        assert_eq!(resolve_run(&doc, &para, &run).size_pt, 12.0);
    }

    /// 下划线种类:样式链给 Double;直接 `val="none"` 显式关(盖掉链)。
    #[test]
    fn underline_kind_cascades_and_explicit_none_wins() {
        let doc = doc_with_styles(vec![(
            "S1",
            para_style(
                None,
                RunProps {
                    u: Some(UnderlineKind::Double),
                    ..RunProps::default()
                },
                ParaProps::default(),
            ),
        )]);
        let para = styled_para(Some("S1"));
        let eff = resolve_run(&doc, &para, &TextRun::default());
        assert!(eff.underline);
        assert_eq!(eff.underline_kind, UnderlineKind::Double);

        let mut run = TextRun::default();
        run.rpr.u = Some(UnderlineKind::None);
        let eff = resolve_run(&doc, &para, &run);
        assert!(!eff.underline);
        assert_eq!(eff.underline_kind, UnderlineKind::None);

        // from_attr:none / 已知 / 未知(容错 Single)。
        assert_eq!(UnderlineKind::from_attr("none"), UnderlineKind::None);
        assert_eq!(UnderlineKind::from_attr("wave"), UnderlineKind::Wave);
        assert_eq!(UnderlineKind::from_attr("wat"), UnderlineKind::Single);
    }

    /// 高亮 / 纵向对齐 / szCs:值属性级联;highlight none 显式关;szCs 未设随 sz。
    #[test]
    fn highlight_vert_align_and_szcs_resolve() {
        let doc = doc_with_styles(vec![(
            "S1",
            para_style(
                None,
                RunProps {
                    sz: Some(10.0),
                    highlight: Highlight::from_attr("yellow"),
                    vert_align: Some(VertAlign::Superscript),
                    ..RunProps::default()
                },
                ParaProps::default(),
            ),
        )]);
        let para = styled_para(Some("S1"));
        let eff = resolve_run(&doc, &para, &TextRun::default());
        assert_eq!(eff.highlight, Some(Color::new([0xFF, 0xFF, 0x00])));
        assert_eq!(eff.vert_align, VertAlign::Superscript);
        assert_eq!(eff.size_cs_pt, 10.0, "szCs 未设:随 sz");

        // 直接 highlight=none 显式关;szCs 显式值生效。
        let mut run = TextRun::default();
        run.rpr.highlight = Some(Highlight::Off);
        run.rpr.sz_cs = Some(14.0);
        let eff = resolve_run(&doc, &para, &run);
        assert_eq!(eff.highlight, None);
        assert_eq!(eff.size_cs_pt, 14.0);

        // 未知高亮名容错为未设置。
        assert_eq!(Highlight::from_attr("wat"), None);
    }

    /// 段落 spacing / ind:每个子属性独立级联;hanging 优先并给出**负**首行缩进。
    #[test]
    fn para_spacing_and_indent_merge_per_attribute() {
        let doc = doc_with_styles(vec![(
            "S1",
            para_style(
                None,
                RunProps::default(),
                ParaProps {
                    space_before: Some(240), // 12pt
                    ind_left: Some(720),     // 36pt
                    ind_first_line: Some(360),
                    ..ParaProps::default()
                },
            ),
        )]);
        let mut para = styled_para(Some("S1"));
        // 直接格式化只给 space_after 与 hanging:其余从样式继承;hanging 盖掉 firstLine。
        para.ppr.space_after = Some(120); // 6pt
        para.ppr.ind_hanging = Some(360); // -18pt 首行
        let eff = resolve_para(&doc, &para);
        assert_eq!(eff.space_before_pt, 12.0);
        assert_eq!(eff.space_after_pt, 6.0);
        assert_eq!(eff.indent_left_pt, 36.0);
        assert_eq!(eff.first_line_indent_pt, -18.0);

        // 无 hanging 时 firstLine 为正首行缩进。
        let para2 = styled_para(Some("S1"));
        assert_eq!(resolve_para(&doc, &para2).first_line_indent_pt, 18.0);
    }

    /// 行距规则:auto(1/240 行)→ 倍数;exact/atLeast(twip)→ 磅;非正值容错单倍。
    #[test]
    fn line_spacing_rules_resolve() {
        let mut para = Paragraph::default();
        let doc = Document::default();
        para.ppr.line = Some(LineSpacingRule::Auto(360));
        assert_eq!(
            resolve_para(&doc, &para).line_spacing,
            EffectiveLineSpacing::Multiple(1.5)
        );
        para.ppr.line = Some(LineSpacingRule::Exact(480));
        assert_eq!(
            resolve_para(&doc, &para).line_spacing,
            EffectiveLineSpacing::Exact(24.0)
        );
        para.ppr.line = Some(LineSpacingRule::AtLeast(240));
        assert_eq!(
            resolve_para(&doc, &para).line_spacing,
            EffectiveLineSpacing::AtLeast(12.0)
        );
        para.ppr.line = Some(LineSpacingRule::Auto(0));
        assert_eq!(
            resolve_para(&doc, &para).line_spacing,
            EffectiveLineSpacing::Multiple(1.0)
        );
    }

    /// 段落边框按边级联、底纹 theme 解引、keep 系列旗标(widowControl 缺省开)。
    #[test]
    fn para_borders_shading_and_keep_flags_resolve() {
        let border = |val: &str| Border {
            val: val.to_string(),
            sz_eighth_pt: 8,
            space_pt: 1,
            color: None,
        };
        let mut style_ppr = ParaProps::default();
        style_ppr.borders.top = Some(border("single"));
        style_ppr.borders.bottom = Some(border("double"));
        let mut doc = doc_with_styles(vec![(
            "S1",
            para_style(None, RunProps::default(), style_ppr),
        )]);
        doc.theme
            .colors
            .set(ThemeColor::Accent2, Color::new([0xED, 0x7D, 0x31]));

        let mut para = styled_para(Some("S1"));
        para.ppr.borders.bottom = Some(border("none")); // 直接盖掉样式的 bottom
        para.ppr.shd_fill = Some(ColorRef::Theme(ThemeColor::Accent2));
        para.ppr.page_break_before = Some(true);
        para.ppr.keep_next = Some(true);
        let eff = resolve_para(&doc, &para);
        assert_eq!(
            eff.borders.top.as_ref().map(|b| b.val.as_str()),
            Some("single")
        );
        assert_eq!(
            eff.borders.bottom.as_ref().map(|b| b.val.as_str()),
            Some("none")
        );
        assert!(eff.borders.any(), "top 仍是可见边框");
        assert_eq!(eff.shading, Some(Color::new([0xED, 0x7D, 0x31])));
        assert!(eff.page_break_before);
        assert!(eff.keep_next);
        assert!(!eff.keep_lines);
        assert!(eff.widow_control, "widowControl 缺省开");

        // shd fill="auto" 显式无底纹。
        para.ppr.shd_fill = Some(ColorRef::Auto);
        assert_eq!(resolve_para(&doc, &para).shading, None);
    }

    /// jc 归一化:left/start/center/right/end/both/distribute;未知值容错为未设置。
    #[test]
    fn justification_normalizes_jc_values() {
        assert_eq!(Justification::from_attr("left"), Some(Justification::Left));
        assert_eq!(Justification::from_attr("start"), Some(Justification::Left));
        assert_eq!(
            Justification::from_attr("center"),
            Some(Justification::Center)
        );
        assert_eq!(
            Justification::from_attr("right"),
            Some(Justification::Right)
        );
        assert_eq!(Justification::from_attr("end"), Some(Justification::Right));
        assert_eq!(
            Justification::from_attr("both"),
            Some(Justification::Justify)
        );
        assert_eq!(
            Justification::from_attr("distribute"),
            Some(Justification::Distribute)
        );
        assert_eq!(Justification::from_attr("wat"), None);
    }
}
