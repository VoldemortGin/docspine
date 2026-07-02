#![forbid(unsafe_code)]
//! `doc-core` —— docspine 的领域核:结构化 Word 文档模型 + twip/EMU 几何 + 类型化错误。
//!
//! 这里**没有任何 IO / zip / XML 逻辑**,只有纯数据类型,供 `doc-parse` 填充、供
//! `py-bindings` 暴露。保持 domain-neutral、稳定、可测。

pub mod error;
pub mod export;
pub mod geom;
pub mod model;
pub mod style;

pub use error::{DocError, Result};
pub use geom::{
    emu_to_points, twips_to_points, Emu, Twips, EMU_PER_INCH, EMU_PER_POINT, TWIPS_PER_INCH,
    TWIPS_PER_POINT,
};
pub use model::{
    Block, BreakKind, Cell, Color, Document, Orientation, PageMargins, Paragraph, Picture, Row,
    RunSegment, Section, Table, TextRun, VMerge,
};
pub use style::{
    resolve_para, resolve_para_in_table, resolve_run, resolve_run_in_table, EffectiveLineSpacing,
    EffectiveParaProps, EffectiveRunProps, StyleTable, StyleWarning, Theme, UnderlineKind,
    VertAlign,
};
