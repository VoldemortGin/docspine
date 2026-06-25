//! 图像表格几何重建 —— 把一张**图片里的表格**(扫描件/截图,无文本层、无矢量框线)从 OCR
//! 文字框重建成结构化网格。
//!
//! 思路移植自 pdfspine 的 `crates/pdf-api/src/image_table.rs`(行列带状聚类 + 网格线中点 +
//! 槽位分配 + 跨格判定),但这里**直接作用在图片像素坐标**上:输入是已经 OCR 出来的
//! [`OcrWord`](像素 bbox + 置信度),不渲染 PDF 页、不做坐标换算。颜色采样这类需要原始像素
//! 缓冲的细节本轮从略——网格 + 每格文字/置信度/跨度已能满足“图片表格 → 行列结构”的核心诉求。
//!
//! # 流水线
//!
//! 1. **过滤**:丢掉空白 / 低置信度的词。
//! 2. **聚类**:按垂直中心间隙聚成行带,按水平中心间隙聚成列带(纯几何、确定性)。
//! 3. **网格**:由带的外缘 + 相邻带间隙中点推出 `row_count+1` / `col_count+1` 条网格线。
//! 4. **分配**:每个词按其中心落入的网格槽归位;同槽文字按阅读顺序拼接、置信度取均值。
//! 5. **跨格**:词的像素 bbox 明显跨越后续行/列槽过半时,加宽该格的 `row_span`/`col_span`。

use std::collections::HashMap;

use doc_core::Result;
use ocrspine::OcrWord;

/// 图像表格的一个单元格:网格位置 + OCR 复原的文字/置信度。坐标为**图片像素**。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageTableCell {
    /// 0 基网格行(顶行优先)。
    pub row: usize,
    /// 0 基网格列(左列优先)。
    pub col: usize,
    /// 纵向跨行数;`>= 1`(合并格 `> 1`)。
    pub row_span: usize,
    /// 横向跨列数;`>= 1`(合并格 `> 1`)。
    pub col_span: usize,
    /// 单元格外框(图片像素:`(x0, y0, x1, y1)`),由网格线 + 跨度推出。
    pub bbox: (f64, f64, f64, f64),
    /// 中心落在本格内的词的文字,按阅读顺序(先上下、再左右)以单空格拼接。
    pub text: String,
    /// 本格内词的平均 OCR 置信度(`0.0..=100.0`);无词时为 `0.0`。
    pub confidence: f32,
}

/// 从一张图片重建出的一张表格。字段命名对齐 pdfspine 的 `ImageTable`,便于跨包对照。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageTable {
    /// 所有出现的单元格外框的并集(图片像素 `(x0, y0, x1, y1)`)。
    pub bbox: (f64, f64, f64, f64),
    /// 网格行数。
    pub row_count: usize,
    /// 网格列数。
    pub col_count: usize,
    /// `col_count + 1` 条竖网格线的 x(图片像素),从左到右。
    pub cols: Vec<f64>,
    /// `row_count + 1` 条横网格线的 y(图片像素),从上到下。
    pub rows: Vec<f64>,
    /// 仅含**有词**的单元格,行主序;空槽跳过。
    pub cells: Vec<ImageTableCell>,
}

/// [`reconstruct_table_from_image`] 的结果:重建出的所有表格。
/// v1 用单表启发式,故 `tables` 含 0 或 1 项。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ImageTableResult {
    pub tables: Vec<ImageTable>,
}

/// 图像表格重建的调参旋钮。
#[derive(Clone, Debug, PartialEq)]
pub struct ImageTableOptions {
    /// 丢弃低于此置信度(`0.0..=100.0`)的词。默认 `0.0`(全留)。
    pub min_confidence: f32,
    /// 行聚类间隙比例:垂直中心间隙超过 `row_gap_ratio * 中位词高` 时另起一行。默认 `0.5`。
    pub row_gap_ratio: f64,
    /// 列聚类间隙比例:水平中心间隙超过 `col_gap_ratio * 中位词宽` 时另起一列。默认 `0.7`。
    pub col_gap_ratio: f64,
}

impl Default for ImageTableOptions {
    fn default() -> Self {
        ImageTableOptions {
            min_confidence: 0.0,
            row_gap_ratio: 0.5,
            col_gap_ratio: 0.7,
        }
    }
}

/// 高层入口:OCR 一张图片字节,把文字框重建成表格网格。
///
/// OCR 出的词少于两个、或网格无有效单元格时,返回空 `tables`(不报错)。OCR 本身失败
/// (非图片字节、模型缺失)折成 [`DocError::Ocr`]。
pub fn reconstruct_table_from_image(
    bytes: &[u8],
    opts: &ImageTableOptions,
) -> Result<ImageTableResult> {
    let words = crate::ocr_words(bytes)?;
    Ok(reconstruct_from_words(&words, opts))
}

/// 纯几何内核:从已 OCR 出的词重建表格(无 IO,便于单测)。
pub fn reconstruct_from_words(words: &[OcrWord], opts: &ImageTableOptions) -> ImageTableResult {
    // 只留非空、足够置信的词。
    let words: Vec<&OcrWord> = words
        .iter()
        .filter(|w| w.confidence >= opts.min_confidence && !w.text.trim().is_empty())
        .collect();
    if words.len() < 2 {
        return ImageTableResult::default();
    }

    let row_bands = cluster_rows(&words, opts.row_gap_ratio);
    let col_bands = cluster_cols(&words, opts.col_gap_ratio);
    if row_bands.is_empty() || col_bands.is_empty() {
        return ImageTableResult::default();
    }

    let row_lines = grid_lines(&row_bands);
    let col_lines = grid_lines(&col_bands);
    let row_count = row_bands.len();
    let col_count = col_bands.len();

    // 每个词按中心落入的槽归位。
    let mut slot_words: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (i, w) in words.iter().enumerate() {
        let (cx, cy) = center(w);
        let (Some(r), Some(c)) = (slot_index(&row_lines, cy), slot_index(&col_lines, cx)) else {
            continue;
        };
        slot_words.entry((r, c)).or_default().push(i);
    }

    // 行主序建有词的单元格。
    let mut cells: Vec<ImageTableCell> = Vec::new();
    for r in 0..row_count {
        for c in 0..col_count {
            let Some(idxs) = slot_words.get(&(r, c)) else {
                continue;
            };
            if idxs.is_empty() {
                continue;
            }

            // 阅读顺序:先上下、再左右。
            let mut ordered = idxs.clone();
            ordered.sort_by(|&a, &b| {
                let (_, ya) = center(words[a]);
                let (_, yb) = center(words[b]);
                ya.partial_cmp(&yb)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then(
                        bbox(words[a])
                            .0
                            .partial_cmp(&bbox(words[b]).0)
                            .unwrap_or(std::cmp::Ordering::Equal),
                    )
            });

            let text = ordered
                .iter()
                .map(|&i| words[i].text.trim())
                .filter(|t| !t.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            let confidence =
                ordered.iter().map(|&i| words[i].confidence).sum::<f32>() / ordered.len() as f32;

            let col_span = max_span(&col_lines, &ordered, &words, c, col_count, Axis::X);
            let row_span = max_span(&row_lines, &ordered, &words, r, row_count, Axis::Y);

            let x0 = col_lines[c];
            let x1 = col_lines[(c + col_span).min(col_count)];
            let y0 = row_lines[r];
            let y1 = row_lines[(r + row_span).min(row_count)];

            cells.push(ImageTableCell {
                row: r,
                col: c,
                row_span,
                col_span,
                bbox: (x0, y0, x1, y1),
                text,
                confidence,
            });
        }
    }

    if cells.is_empty() {
        return ImageTableResult::default();
    }

    let bbox = cells.iter().fold(
        (
            f64::INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::NEG_INFINITY,
        ),
        |acc, cell| {
            (
                acc.0.min(cell.bbox.0),
                acc.1.min(cell.bbox.1),
                acc.2.max(cell.bbox.2),
                acc.3.max(cell.bbox.3),
            )
        },
    );

    ImageTableResult {
        tables: vec![ImageTable {
            bbox,
            row_count,
            col_count,
            cols: col_lines,
            rows: row_lines,
            cells,
        }],
    }
}

// ===================================================================== banding

/// 一维带:像素空间的闭区间 `[lo, hi]`。
#[derive(Clone, Copy, Debug)]
struct Band {
    lo: f64,
    hi: f64,
}

/// 按垂直中心间隙把词聚成有序行带。
fn cluster_rows(words: &[&OcrWord], gap_ratio: f64) -> Vec<Band> {
    let median_h = median(words.iter().map(|w| bbox_height(w)));
    let threshold = (gap_ratio * median_h).max(1.0);

    let mut idx: Vec<usize> = (0..words.len()).collect();
    idx.sort_by(|&a, &b| {
        center(words[a])
            .1
            .partial_cmp(&center(words[b]).1)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut bands: Vec<Band> = Vec::new();
    let mut run_mean = 0.0_f64;
    let mut run_n = 0usize;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;

    for &i in &idx {
        let b = bbox(words[i]);
        let cy = (b.1 + b.3) / 2.0;
        if run_n > 0 && (cy - run_mean) > threshold {
            bands.push(Band { lo, hi });
            run_mean = 0.0;
            run_n = 0;
            lo = f64::INFINITY;
            hi = f64::NEG_INFINITY;
        }
        run_n += 1;
        run_mean += (cy - run_mean) / run_n as f64;
        lo = lo.min(b.1);
        hi = hi.max(b.3);
    }
    if run_n > 0 {
        bands.push(Band { lo, hi });
    }
    bands
}

/// 按水平中心间隙把词聚成有序列带。
fn cluster_cols(words: &[&OcrWord], gap_ratio: f64) -> Vec<Band> {
    let median_w = median(words.iter().map(|w| bbox_width(w)));
    let threshold = (gap_ratio * median_w).max(1.0);

    let mut idx: Vec<usize> = (0..words.len()).collect();
    idx.sort_by(|&a, &b| {
        center(words[a])
            .0
            .partial_cmp(&center(words[b]).0)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut bands: Vec<Band> = Vec::new();
    let mut prev_cx: Option<f64> = None;
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;

    for &i in &idx {
        let b = bbox(words[i]);
        let cx = (b.0 + b.2) / 2.0;
        if let Some(p) = prev_cx {
            if (cx - p) > threshold {
                bands.push(Band { lo, hi });
                lo = f64::INFINITY;
                hi = f64::NEG_INFINITY;
            }
        }
        lo = lo.min(b.0);
        hi = hi.max(b.2);
        prev_cx = Some(cx);
    }
    if prev_cx.is_some() {
        bands.push(Band { lo, hi });
    }
    bands
}

/// 由带推出 `bands.len() + 1` 条网格线:外缘界定首尾线,内部线取相邻带间隙的中点。
fn grid_lines(bands: &[Band]) -> Vec<f64> {
    let mut lines = Vec::with_capacity(bands.len() + 1);
    lines.push(bands[0].lo);
    for pair in bands.windows(2) {
        lines.push((pair[0].hi + pair[1].lo) / 2.0);
    }
    lines.push(bands[bands.len() - 1].hi);
    lines
}

/// `lines[i]..lines[i+1]` 区间含 `v` 的 0 基槽位;界外向首/末槽夹取,避免边界 epsilon 丢词。
fn slot_index(lines: &[f64], v: f64) -> Option<usize> {
    let n = lines.len().checked_sub(1)?;
    if n == 0 {
        return None;
    }
    if v <= lines[0] {
        return Some(0);
    }
    if v >= lines[n] {
        return Some(n - 1);
    }
    for i in 0..n {
        if v >= lines[i] && v < lines[i + 1] {
            return Some(i);
        }
    }
    Some(n - 1)
}

/// 聚类轴。
#[derive(Clone, Copy)]
enum Axis {
    X,
    Y,
}

/// 某格沿一个轴的跨度(槽数):某词的像素区间跨入后续槽过半时加宽。
fn max_span(
    lines: &[f64],
    word_idx: &[usize],
    words: &[&OcrWord],
    start: usize,
    count: usize,
    axis: Axis,
) -> usize {
    let mut span = 1usize;
    for &i in word_idx {
        let b = bbox(words[i]);
        let (lo_v, hi_v) = match axis {
            Axis::X => (b.0, b.2),
            Axis::Y => (b.1, b.3),
        };
        let mut last = start;
        for ns in (start + 1)..count {
            let lo = lines[ns];
            let hi = lines[ns + 1];
            let overlap = (hi_v.min(hi) - lo_v.max(lo)).max(0.0);
            if overlap > (hi - lo) / 2.0 {
                last = ns;
            } else {
                break;
            }
        }
        span = span.max(last - start + 1);
    }
    span
}

// ====================================================================== helpers

/// 词的(归一化)像素 bbox `(x0, y0, x1, y1)`,保证 `x0<=x1`、`y0<=y1`。
fn bbox(w: &OcrWord) -> (f64, f64, f64, f64) {
    let (x0, x1) = if w.bbox.x0 <= w.bbox.x1 {
        (w.bbox.x0, w.bbox.x1)
    } else {
        (w.bbox.x1, w.bbox.x0)
    };
    let (y0, y1) = if w.bbox.y0 <= w.bbox.y1 {
        (w.bbox.y0, w.bbox.y1)
    } else {
        (w.bbox.y1, w.bbox.y0)
    };
    (x0, y0, x1, y1)
}

fn bbox_width(w: &OcrWord) -> f64 {
    let b = bbox(w);
    (b.2 - b.0).max(0.0)
}

fn bbox_height(w: &OcrWord) -> f64 {
    let b = bbox(w);
    (b.3 - b.1).max(0.0)
}

/// 词的(归一化)中心 `(cx, cy)`。
fn center(w: &OcrWord) -> (f64, f64) {
    let b = bbox(w);
    ((b.0 + b.2) / 2.0, (b.1 + b.3) / 2.0)
}

/// 有限值序列的中位数,空则 `0.0`。
fn median(vals: impl IntoIterator<Item = f64>) -> f64 {
    let mut v: Vec<f64> = vals.into_iter().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return 0.0;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}
