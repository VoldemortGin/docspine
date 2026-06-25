//! `doc-ocr` 图像表格几何重建的单测 —— 用**合成的 OCR 词框**(不跑模型,纯几何)断言行列
//! 聚类、网格线、槽位分配与跨格判定都正确。
//!
//! 这一层是确定性的纯几何内核 [`reconstruct_from_words`],不依赖 ONNX 模型,所以离线必跑。

use doc_ocr::{reconstruct_from_words, ImageTableOptions};
use ocrspine::{BBox, OcrWord};

/// 造一个位于 `(x0,y0)-(x1,y1)` 的词框。
fn word(text: &str, x0: f64, y0: f64, x1: f64, y1: f64) -> OcrWord {
    OcrWord {
        text: text.to_string(),
        bbox: BBox::new(x0, y0, x1, y1),
        confidence: 95.0,
        quad: [
            (x0 as f32, y0 as f32),
            (x1 as f32, y0 as f32),
            (x1 as f32, y1 as f32),
            (x0 as f32, y1 as f32),
        ],
    }
}

/// 一张清晰的 2 行 × 3 列表格:行间、列间都有明显空隙。
fn grid_words() -> Vec<OcrWord> {
    vec![
        // 第 0 行,y ~ [10, 30]
        word("R0C0", 10.0, 10.0, 60.0, 30.0),
        word("R0C1", 110.0, 10.0, 160.0, 30.0),
        word("R0C2", 210.0, 10.0, 260.0, 30.0),
        // 第 1 行,y ~ [70, 90]
        word("R1C0", 10.0, 70.0, 60.0, 90.0),
        word("R1C1", 110.0, 70.0, 160.0, 90.0),
        word("R1C2", 210.0, 70.0, 260.0, 90.0),
    ]
}

#[test]
fn reconstructs_2x3_grid() {
    let words = grid_words();
    let res = reconstruct_from_words(&words, &ImageTableOptions::default());
    assert_eq!(res.tables.len(), 1);
    let t = &res.tables[0];
    assert_eq!(t.row_count, 2);
    assert_eq!(t.col_count, 3);
    // 网格线数 = 槽数 + 1。
    assert_eq!(t.rows.len(), 3);
    assert_eq!(t.cols.len(), 4);
    // 6 个有词的格。
    assert_eq!(t.cells.len(), 6);
}

#[test]
fn assigns_text_to_correct_slots() {
    let words = grid_words();
    let res = reconstruct_from_words(&words, &ImageTableOptions::default());
    let t = &res.tables[0];
    for cell in &t.cells {
        let expected = format!("R{}C{}", cell.row, cell.col);
        assert_eq!(cell.text, expected, "cell @({},{})", cell.row, cell.col);
        assert_eq!(cell.row_span, 1);
        assert_eq!(cell.col_span, 1);
        assert!((cell.confidence - 95.0).abs() < 1e-3);
    }
}

#[test]
fn too_few_words_yields_no_table() {
    let words = vec![word("solo", 0.0, 0.0, 10.0, 10.0)];
    let res = reconstruct_from_words(&words, &ImageTableOptions::default());
    assert!(res.tables.is_empty());
}

#[test]
fn min_confidence_filters_words() {
    let mut words = grid_words();
    // 把一格的置信度压到很低。
    words[2].confidence = 10.0;
    let opts = ImageTableOptions {
        min_confidence: 50.0,
        ..ImageTableOptions::default()
    };
    let res = reconstruct_from_words(&words, &opts);
    let t = &res.tables[0];
    // 被过滤掉那一格不应出现。
    assert!(t.cells.iter().all(|c| c.text != "R0C2"));
}
