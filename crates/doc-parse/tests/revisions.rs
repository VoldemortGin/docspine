//! `w:tblGridChange` 修订标记验收:修订前的旧网格整体跳过(“接受全部修订”),
//! 且嵌套的同名结束标签不得让解析器提前返回(表格的行与其后正文不许丢)。fixture 现场构造。

use std::io::{Cursor, Write};

use doc_core::model::{Block, Document, Paragraph, Table};
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

fn paragraphs(doc: &Document) -> Vec<&Paragraph> {
    doc.body
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(p) => Some(p),
            _ => None,
        })
        .collect()
}

fn row(text: &str) -> String {
    format!("<w:tr><w:tc><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc></w:tr>")
}

// ---------------------------------------------------------------- tblGridChange

/// 表后跟两个段落、两行;`grid` 是 `w:tblGrid` 的完整写法。
fn grid_doc(grid: &str) -> Document {
    parse_body(&format!(
        "<w:tbl>{grid}{}{}</w:tbl>\
         <w:p><w:r><w:t>after1</w:t></w:r></w:p><w:p><w:r><w:t>after2</w:t></w:r></w:p>",
        row("r1"),
        row("r2")
    ))
}

fn assert_grid_ok(doc: &Document) {
    let t = first_table(doc);
    assert_eq!(t.grid_cols, vec![1000, 2000], "只含修订后的列宽");
    assert_eq!(t.rows.len(), 2, "表的所有行都在");
    let after: Vec<String> = paragraphs(doc).iter().map(|p| p.text()).collect();
    assert_eq!(after, ["after1", "after2"], "表后正文完整");
}

#[test]
fn tbl_grid_change_is_skipped_and_body_survives() {
    let doc = grid_doc(
        r#"<w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="2000"/>
        <w:tblGridChange w:id="0"><w:tblGrid><w:gridCol w:w="111"/><w:gridCol w:w="222"/><w:gridCol w:w="333"/></w:tblGrid></w:tblGridChange>
        </w:tblGrid>"#,
    );
    assert_grid_ok(&doc);
}

#[test]
fn expanded_grid_col_parses_like_self_closed() {
    let expanded = grid_doc(
        r#"<w:tblGrid><w:gridCol w:w="1000"></w:gridCol><w:gridCol w:w="2000"></w:gridCol></w:tblGrid>"#,
    );
    assert_grid_ok(&expanded);
    let closed =
        grid_doc(r#"<w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="2000"/></w:tblGrid>"#);
    assert_eq!(
        first_table(&expanded).grid_cols,
        first_table(&closed).grid_cols
    );
    assert_eq!(expanded.body.len(), closed.body.len());
}

#[test]
fn tbl_grid_change_with_expanded_grid_cols() {
    let doc = grid_doc(
        r#"<w:tblGrid><w:gridCol w:w="1000"></w:gridCol><w:gridCol w:w="2000"></w:gridCol>
        <w:tblGridChange w:id="0"><w:tblGrid><w:gridCol w:w="9"></w:gridCol></w:tblGrid></w:tblGridChange></w:tblGrid>"#,
    );
    assert_grid_ok(&doc);
}
