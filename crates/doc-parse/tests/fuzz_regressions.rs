//! cargo-fuzz 发现的 panic 的回归测试(`fuzz/` 目录;任何输入只许 `Ok`/`Err`,不许 panic)。
//! 每个用例现场构造 `document.xml` 再打成最小 zip,不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn parse_document_xml(xml: &str) -> doc_core::Result<doc_parse::ParsedDoc> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        zip.start_file("word/document.xml", opts)
            .expect("start_file");
        zip.write_all(xml.as_bytes()).expect("write");
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner())
}

/// `w:shd@w:fill` 里 6 字节的非 ASCII 串:`Color::from_hex` 曾在非字符边界切片 panic。
#[test]
fn non_ascii_hex_fill_in_tcpr_does_not_panic() {
    let xml =
        "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
        <w:body><w:tbl><w:tr><w:tc><w:tcPr><w:shd w:val=\"clear\" w:fill=\"DD\u{FFFD}D\"/></w:tcPr>\
        <w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>";
    let parsed = parse_document_xml(xml).expect("lenient parse");
    assert!(doc_core::export::to_text(&parsed.document).contains('x'));
}
