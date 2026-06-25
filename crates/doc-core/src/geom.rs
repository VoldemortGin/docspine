//! WordprocessingML 的原生长度单位。
//!
//! Word 文档里有两套单位:
//! - **twip**(twentieth of a point):正文版式的主单位 —— `1440 twip = 1 inch`,
//!   `20 twip = 1 point`。表格宽度(`w:tcW`)、行高、缩进等都用它。
//! - **EMU**(English Metric Units):DrawingML 图形/图片尺寸用 —— `914400 EMU = 1 inch`,
//!   `12700 EMU = 1 point`(`wp:extent@cx/cy`)。
//!
//! 这里保持 domain-neutral —— 只放单位换算,不掺任何业务语义。

/// 每英寸的 twip 数。
pub const TWIPS_PER_INCH: i64 = 1_440;

/// 每磅(point)的 twip 数。
pub const TWIPS_PER_POINT: f64 = 20.0;

/// 每英寸的 EMU 数。
pub const EMU_PER_INCH: i64 = 914_400;

/// 每磅(point)的 EMU 数。
pub const EMU_PER_POINT: f64 = 12_700.0;

/// twip —— 正文版式的原生长度单位(i64)。
pub type Twips = i64;

/// English Metric Units —— DrawingML 图形尺寸的原生单位(i64)。
pub type Emu = i64;

/// 把 twip 换算成磅(point,1/72 英寸)。
#[inline]
pub fn twips_to_points(twips: Twips) -> f64 {
    twips as f64 / TWIPS_PER_POINT
}

/// 把 EMU 换算成磅(point,1/72 英寸)。
#[inline]
pub fn emu_to_points(emu: Emu) -> f64 {
    emu as f64 / EMU_PER_POINT
}
