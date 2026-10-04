//! 页码设置 `w:pgNumType`(`w:start` / `w:fmt`)的解析:现场合成 docx(纯 zip + 手写 XML)。

use std::io::{Cursor, Write};

use doc_core::model::Section;
use doc_core::PageNumFormat;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main""#;

/// 每个 `pgNumType` 片段(`None` = 不写该元素)对应一节:前 n-1 节挂在段落 `w:pPr` 里,
/// 最后一节是 body 末尾的 `w:sectPr`。
fn sections_of(pg_num_types: &[Option<&str>]) -> Vec<Section> {
    let sect = |pg: &Option<&str>| {
        format!(
            "<w:sectPr><w:pgSz w:w=\"12240\" w:h=\"15840\"/>{}</w:sectPr>",
            pg.map(|a| format!("<w:pgNumType {a}/>"))
                .unwrap_or_default()
        )
    };
    let (last, init) = pg_num_types.split_last().expect("at least one section");
    let mut body = String::new();
    for pg in init {
        body.push_str(&format!("<w:p><w:pPr>{}</w:pPr></w:p>", sect(pg)));
    }
    body.push_str(&format!("<w:p/>{}", sect(last)));
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {W_NS}><w:body>{body}</w:body></w:document>"#
    );
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        zip.start_file("word/document.xml", SimpleFileOptions::default())
            .expect("start_file");
        zip.write_all(doc.as_bytes()).expect("write");
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner())
        .expect("parse synthetic docx")
        .document
        .sections
}

#[test]
fn start_and_fmt_are_read_per_section() {
    let s = sections_of(&[
        Some(r#"w:fmt="lowerRoman" w:start="3""#),
        Some(r#"w:fmt="upperLetter""#),
        Some(r#"w:start="1""#),
        None,
    ]);
    assert_eq!(s.len(), 4);
    assert_eq!(s[0].page_number_start, Some(3));
    assert_eq!(s[0].page_number_format, PageNumFormat::LowerRoman);
    assert_eq!(s[1].page_number_start, None);
    assert_eq!(s[1].page_number_format, PageNumFormat::UpperLetter);
    assert_eq!(s[2].page_number_start, Some(1));
    assert_eq!(s[2].page_number_format, PageNumFormat::Decimal);
    assert_eq!(s[3].page_number_start, None);
    assert_eq!(s[3].page_number_format, PageNumFormat::Decimal);
}

#[test]
fn every_supported_fmt_value_is_recognised() {
    for (value, want) in [
        ("decimal", PageNumFormat::Decimal),
        ("lowerRoman", PageNumFormat::LowerRoman),
        ("upperRoman", PageNumFormat::UpperRoman),
        ("lowerLetter", PageNumFormat::LowerLetter),
        ("upperLetter", PageNumFormat::UpperLetter),
    ] {
        let s = sections_of(&[Some(&format!(r#"w:fmt="{value}""#))]);
        assert_eq!(s[0].page_number_format, want, "{value}");
    }
}

#[test]
fn unknown_fmt_is_flagged_other() {
    let s = sections_of(&[Some(r#"w:fmt="chineseCounting""#)]);
    assert_eq!(s[0].page_number_format, PageNumFormat::Other);
}

#[test]
fn start_zero_is_valid() {
    let s = sections_of(&[Some(r#"w:start="0""#)]);
    assert_eq!(s[0].page_number_start, Some(0));
}

#[test]
fn negative_or_garbage_start_is_treated_as_missing() {
    for bad in ["-1", "abc", "", "1.5", "99999999999999999999"] {
        let s = sections_of(&[Some(&format!(r#"w:start="{bad}""#))]);
        assert_eq!(s[0].page_number_start, None, "{bad:?}");
    }
    // 起始值非法不影响同元素上的 fmt。
    let s = sections_of(&[Some(r#"w:start="-4" w:fmt="upperRoman""#)]);
    assert_eq!(s[0].page_number_start, None);
    assert_eq!(s[0].page_number_format, PageNumFormat::UpperRoman);
}
