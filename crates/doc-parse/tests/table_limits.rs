//! 表格网格尺寸限额验收:`gridSpan` 钳到 Word 的 63 列上限、`tblGrid` 列数截到 63,
//! `Table::col_count` 有上限且求和饱和;恶意的巨大 `gridSpan` / 超量 `gridCol` 解析与
//! 三种文本导出都不 panic、不分配爆炸、立即返回。fixture 现场构造。

use std::io::{Cursor, Write};
use std::time::{Duration, Instant};

use doc_core::export::{to_html, to_markdown, to_text};
use doc_core::model::{Block, Document, Table, MAX_TABLE_COLS};
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn parse_body(body: &str) -> Document {
    let xml = format!(r#"<w:document xmlns:w="{W_NS}"><w:body>{body}</w:body></w:document>"#);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file("word/document.xml", SimpleFileOptions::default())
        .unwrap();
    zip.write_all(xml.as_bytes()).unwrap();
    parse_bytes(&zip.finish().unwrap().into_inner())
        .expect("lenient parse")
        .document
}

fn first_table(doc: &Document) -> &Table {
    doc.body
        .iter()
        .find_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        })
        .expect("table")
}

fn cell(span: &str) -> String {
    format!(
        r#"<w:tc><w:tcPr><w:gridSpan w:val="{span}"/></w:tcPr><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc>"#
    )
}

/// 三种导出都得很快返回(防分配爆炸 / 近似死循环)。
fn assert_exports_fast(doc: &Document) {
    let t0 = Instant::now();
    let _ = (to_text(doc), to_markdown(doc), to_html(doc));
    assert!(t0.elapsed() < Duration::from_secs(5), "导出应立即返回");
}

#[test]
fn huge_grid_span_is_clamped_to_word_limit() {
    let doc = parse_body(&format!(
        "<w:tbl><w:tr>{}{}</w:tr></w:tbl>",
        cell("4000000000"),
        cell("99999999999999999999")
    ));
    let t = first_table(&doc);
    assert_eq!(t.rows[0].cells[0].grid_span as usize, MAX_TABLE_COLS);
    assert!(t.rows[0].cells[1].grid_span as usize <= MAX_TABLE_COLS);
    assert!(t.col_count() <= MAX_TABLE_COLS);
    assert_exports_fast(&doc);
    assert!(to_html(&doc).contains(&format!("colspan=\"{MAX_TABLE_COLS}\"")));
}

#[test]
fn many_grid_cols_are_capped() {
    let grid: String = (0..10_000).map(|_| r#"<w:gridCol w:w="100"/>"#).collect();
    let rows: String = (0..200)
        .map(|_| "<w:tr><w:tc><w:p/></w:tc></w:tr>")
        .collect();
    let doc = parse_body(&format!(
        "<w:tbl><w:tblGrid>{grid}</w:tblGrid>{rows}</w:tbl>"
    ));
    let t = first_table(&doc);
    assert_eq!(t.grid_cols.len(), MAX_TABLE_COLS);
    assert_eq!(t.col_count(), MAX_TABLE_COLS);
    assert_eq!(t.rows.len(), 200, "行不受列上限影响");
    assert_exports_fast(&doc);
}

#[test]
fn many_cells_in_first_row_without_grid_cap_col_count() {
    let row: String = (0..5_000).map(|_| cell("1")).collect();
    let doc = parse_body(&format!("<w:tbl><w:tr>{row}</w:tr></w:tbl>"));
    let t = first_table(&doc);
    assert_eq!(t.rows[0].cells.len(), 5_000, "单元格内容一个不丢");
    assert_eq!(t.col_count(), MAX_TABLE_COLS);
}

#[test]
fn col_count_sum_saturates() {
    let mut doc = parse_body(&format!(
        "<w:tbl><w:tr>{}{}</w:tr></w:tbl>",
        cell("1"),
        cell("1")
    ));
    let Block::Table(t) = &mut doc.body[0] else {
        panic!("table")
    };
    for c in &mut t.rows[0].cells {
        c.grid_span = u32::MAX; // 手工构造的 IR 也不许溢出。
    }
    assert_eq!(t.col_count(), MAX_TABLE_COLS);
    assert_exports_fast(&doc);
}

#[test]
fn normal_63_column_table_is_untouched() {
    let grid: String = (0..63).map(|_| r#"<w:gridCol w:w="100"/>"#).collect();
    let row: String = (0..62).map(|_| cell("1")).collect::<String>() + &cell("1");
    let doc = parse_body(&format!(
        "<w:tbl><w:tblGrid>{grid}</w:tblGrid><w:tr>{row}</w:tr><w:tr>{}</w:tr></w:tbl>",
        cell("63")
    ));
    let t = first_table(&doc);
    assert_eq!(t.grid_cols.len(), 63);
    assert_eq!(t.col_count(), 63);
    assert_eq!(t.rows[0].cells.len(), 63);
    assert_eq!(t.rows[1].cells[0].grid_span, 63);
}
