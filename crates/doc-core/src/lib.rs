#![forbid(unsafe_code)]
//! `doc-core` —— docspine 的领域核:结构化 Word 文档模型 + twip/EMU 几何 + 类型化错误。
//!
//! 这里**没有任何 IO / zip / XML 逻辑**,只有纯数据类型,供 `doc-parse` 填充、供
//! `py-bindings` 暴露。保持 domain-neutral、稳定、可测。

pub mod error;
pub mod geom;
pub mod model;

pub use error::{DocError, Result};
pub use geom::{
    emu_to_points, twips_to_points, Emu, Twips, EMU_PER_INCH, EMU_PER_POINT, TWIPS_PER_INCH,
    TWIPS_PER_POINT,
};
pub use model::{Block, Cell, Color, Document, Paragraph, Picture, Row, Table, TextRun, VMerge};
