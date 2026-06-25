#![forbid(unsafe_code)]
//! `doc-ocr` —— 图片 OCR 桥 + 图像表格几何重建。
//!
//! 把姊妹 crate [`ocrspine`] 的 PP-OCRv5(`tract-onnx`,本地、离线、确定性)套到 docx 内嵌
//! 图片上。这是缝的元模式:`OcrEngine` 协议来自 ocrspine,`PaddleOcr` 是确定性默认实现;
//! 本 crate 只把字节喂进去、把结果 [`OcrWord`] 映射成本地的 [`OcrItem`],并把
//! [`ocrspine::OcrError`] 折成 [`DocError::Ocr`]。
//!
//! **图像里的表格**:docx 内嵌的图片若本身是一张表格(扫描件/截图),先 OCR 出文字框,再用
//! 纯几何的方式把框聚类成行/列、推断网格、回填单元格——思路移植自 pdfspine 的
//! `image_table`(行列带状聚类 + 网格线中点 + 槽位分配),但这里直接作用在**图片像素坐标**上
//! (无 PDF 页、无渲染),输出 [`ImageTable`]。颜色采样这类需要原始像素缓冲的细节本轮从略。

mod table;

pub use table::{
    reconstruct_from_words, reconstruct_table_from_image, ImageTable, ImageTableCell,
    ImageTableOptions, ImageTableResult,
};

use doc_core::{DocError, Result};
use ocrspine::{OcrEngine, OcrError, OcrImage, OcrWord, PaddleOcr};

/// 一条 OCR 结果:文字 + 轴对齐外框 + 置信度。坐标原点在图片左上角,y 向下。
#[derive(Debug, Clone, PartialEq)]
pub struct OcrItem {
    pub text: String,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
    /// 置信度(`[0.0, 100.0]` 标度,沿用 ocrspine)。
    pub confidence: f32,
}

impl From<OcrWord> for OcrItem {
    fn from(w: OcrWord) -> Self {
        OcrItem {
            text: w.text,
            x0: w.bbox.x0,
            y0: w.bbox.y0,
            x1: w.bbox.x1,
            y1: w.bbox.y1,
            confidence: w.confidence,
        }
    }
}

/// 把 ocrspine 的错误折成本地 [`DocError::Ocr`]。
fn map_ocr_err(e: OcrError) -> DocError {
    DocError::Ocr(e.to_string())
}

/// 一次性 OCR:解码图片字节 -> 新建引擎 -> 识别 -> 映射。
///
/// 注意:每次调用都新建一个 [`PaddleOcr`]。这对**单张**图片是最简路径;批量图片请用
/// [`DocOcr`] 缓存引擎,避免重复构造。
pub fn ocr_image_bytes(bytes: &[u8]) -> Result<Vec<OcrItem>> {
    let image = OcrImage::from_encoded(bytes).map_err(map_ocr_err)?;
    let engine = PaddleOcr::new().map_err(map_ocr_err)?;
    let words = engine.recognize(&image).map_err(map_ocr_err)?;
    Ok(words.into_iter().map(OcrItem::from).collect())
}

/// 跨多次调用缓存 [`PaddleOcr`] 引擎的 OCR 器(批量图片时复用)。
pub struct DocOcr {
    engine: PaddleOcr,
}

impl DocOcr {
    /// 新建一个缓存引擎的 OCR 器。
    pub fn new() -> Result<Self> {
        let engine = PaddleOcr::new().map_err(map_ocr_err)?;
        Ok(DocOcr { engine })
    }

    /// 对一张图片字节做 OCR,复用已缓存的引擎。
    pub fn ocr(&self, bytes: &[u8]) -> Result<Vec<OcrItem>> {
        let image = OcrImage::from_encoded(bytes).map_err(map_ocr_err)?;
        let words = self.engine.recognize(&image).map_err(map_ocr_err)?;
        Ok(words.into_iter().map(OcrItem::from).collect())
    }
}

/// 内部:OCR 一张图片字节,直接拿到 ocrspine 的 [`OcrWord`](保留像素 bbox + 置信度),
/// 供表格几何重建用。
pub(crate) fn ocr_words(bytes: &[u8]) -> Result<Vec<OcrWord>> {
    let image = OcrImage::from_encoded(bytes).map_err(map_ocr_err)?;
    let engine = PaddleOcr::new().map_err(map_ocr_err)?;
    engine.recognize(&image).map_err(map_ocr_err)
}
