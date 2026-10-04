//! 页眉页脚生效开关(`w:titlePg` / `w:evenAndOddHeaders`)与字段标记(`w:fldSimple` /
//! 复杂字段 `w:fldChar` + `w:instrText`)的解析:现场合成 docx(纯 zip + 手写 XML)。

use std::io::{Cursor, Write};

use doc_core::export::to_text;
use doc_core::model::{Block, Paragraph};
use doc_parse::{parse_bytes, ParsedDoc};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

/// 合成 docx:`body_xml` 是 `w:body` 内容;`parts` 是额外部件 `(路径, XML 全文)`。
fn build(body_xml: &str, parts: &[(&str, String)]) -> ParsedDoc {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W_NS}><w:body>{body_xml}</w:body></w:document>"#
    );
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        let mut all: Vec<(String, String)> = vec![("word/document.xml".into(), doc)];
        all.extend(parts.iter().map(|(n, x)| (n.to_string(), x.clone())));
        for (name, body) in all {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner()).expect("parse synthetic docx")
}

fn settings(inner: &str) -> (&'static str, String) {
    (
        "word/settings.xml",
        format!(r#"<w:settings {W_NS}><w:zoom w:percent="100"/>{inner}</w:settings>"#),
    )
}

fn first_para(d: &ParsedDoc) -> &Paragraph {
    match &d.document.body[0] {
        Block::Paragraph(p) => p,
        other => panic!("expected paragraph, got {other:?}"),
    }
}

/// `(run 文字, field)` 序列,便于整体断言。
fn runs(p: &Paragraph) -> Vec<(String, Option<String>)> {
    p.runs.iter().map(|r| (r.text(), r.field.clone())).collect()
}

fn some(s: &str) -> Option<String> {
    Some(s.to_string())
}

// ------------------------------------------------------------------ 生效开关

#[test]
fn title_pg_is_parsed_per_section_with_on_off_semantics() {
    let body = r#"<w:p><w:pPr><w:sectPr><w:titlePg/></w:sectPr></w:pPr></w:p>
        <w:p><w:pPr><w:sectPr><w:titlePg w:val="0"/></w:sectPr></w:pPr></w:p>
        <w:p/><w:sectPr><w:pgSz w:w="12240" w:h="15840"/></w:sectPr>"#;
    let d = build(body, &[]);
    let flags: Vec<bool> = d.document.sections.iter().map(|s| s.title_pg).collect();
    assert_eq!(flags, [true, false, false]);
}

#[test]
fn even_and_odd_headers_is_read_from_settings() {
    let body = "<w:p/><w:sectPr/>";
    let on = build(body, &[settings(r#"<w:evenAndOddHeaders/>"#)]);
    assert!(on.document.even_and_odd_headers);
    let on_after_tab = build(
        body,
        &[settings(
            r#"<w:defaultTabStop w:val="720"/><w:evenAndOddHeaders w:val="true"/>"#,
        )],
    );
    assert!(on_after_tab.document.even_and_odd_headers);
    assert_eq!(on_after_tab.document.default_tab_stop, Some(720));
    let off = build(body, &[settings(r#"<w:evenAndOddHeaders w:val="false"/>"#)]);
    assert!(!off.document.even_and_odd_headers);
    let missing = build(body, &[]);
    assert!(!missing.document.even_and_odd_headers);
}

// ------------------------------------------------------------------ 字段

#[test]
fn fld_simple_marks_its_result_runs() {
    let body = r#"<w:p><w:r><w:t xml:space="preserve">Page </w:t></w:r><w:fldSimple w:instr=" PAGE  \* MERGEFORMAT "><w:r><w:t>7</w:t></w:r></w:fldSimple><w:r><w:t xml:space="preserve"> end</w:t></w:r></w:p>"#;
    let d = build(body, &[]);
    assert_eq!(
        runs(first_para(&d)),
        [
            ("Page ".into(), None),
            ("7".into(), some(r"PAGE  \* MERGEFORMAT")),
            (" end".into(), None),
        ]
    );
    // 缓存结果照常进正文 / 导出。
    assert_eq!(to_text(&d.document), "Page 7 end");
}

#[test]
fn complex_field_marks_result_between_separate_and_end() {
    let body = r#"<w:p>
        <w:r><w:t xml:space="preserve">of </w:t></w:r>
        <w:r><w:fldChar w:fldCharType="begin"/></w:r>
        <w:r><w:instrText xml:space="preserve"> NUM</w:instrText></w:r>
        <w:r><w:instrText xml:space="preserve">PAGES </w:instrText></w:r>
        <w:r><w:fldChar w:fldCharType="separate"/></w:r>
        <w:r><w:t>12</w:t></w:r>
        <w:r><w:fldChar w:fldCharType="end"/></w:r>
        <w:r><w:t>!</w:t></w:r>
        </w:p>"#;
    let d = build(body, &[]);
    assert_eq!(
        runs(first_para(&d)),
        [
            ("of ".into(), None),
            ("12".into(), some("NUMPAGES")),
            ("!".into(), None),
        ]
    );
    assert_eq!(to_text(&d.document), "of 12!");
}

/// 无缓存结果的字段(复杂字段无 `separate`、空 `fldSimple`):留一个空文字、带标记的 run。
#[test]
fn field_without_cached_result_leaves_an_empty_marked_run() {
    let body = r#"<w:p>
        <w:r><w:fldChar w:fldCharType="begin"/></w:r>
        <w:r><w:instrText>PAGE</w:instrText></w:r>
        <w:r><w:fldChar w:fldCharType="end"/></w:r>
        <w:fldSimple w:instr="NUMPAGES"/>
        </w:p>"#;
    let d = build(body, &[]);
    assert_eq!(
        runs(first_para(&d)),
        [("".into(), some("PAGE")), ("".into(), some("NUMPAGES"))]
    );
    assert_eq!(to_text(&d.document), "");
}

/// 嵌套在外层字段**指令区**里的字段结果不是可见文字的一部分:不标记(文字照旧保留)。
/// 外层结果里的 run 标记外层指令;字段跨段落延续。
#[test]
fn nested_field_in_instruction_is_not_marked_and_fields_span_paragraphs() {
    let body = r#"<w:p>
        <w:r><w:fldChar w:fldCharType="begin"/></w:r>
        <w:r><w:instrText xml:space="preserve">IF </w:instrText></w:r>
        <w:r><w:fldChar w:fldCharType="begin"/></w:r>
        <w:r><w:instrText>PAGE</w:instrText></w:r>
        <w:r><w:fldChar w:fldCharType="separate"/></w:r>
        <w:r><w:t>1</w:t></w:r>
        <w:r><w:fldChar w:fldCharType="end"/></w:r>
        <w:r><w:instrText xml:space="preserve"> = 1 "a" "b"</w:instrText></w:r>
        <w:r><w:fldChar w:fldCharType="separate"/></w:r>
        <w:r><w:t>a</w:t></w:r>
        </w:p>
        <w:p><w:r><w:t>still</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r><w:r><w:t>after</w:t></w:r></w:p>"#;
    let d = build(body, &[]);
    let outer = r#"IF  = 1 "a" "b""#;
    assert_eq!(
        runs(first_para(&d)),
        [("1".into(), None), ("a".into(), some(outer))]
    );
    let Block::Paragraph(second) = &d.document.body[1] else {
        panic!("paragraph")
    };
    assert_eq!(
        runs(second),
        [("still".into(), some(outer)), ("after".into(), None)]
    );
}

/// 畸形:孤立的 `end` / `separate` 不 panic、不误标;未闭合的 `begin` 只影响其后的 run。
#[test]
fn unbalanced_field_chars_degrade_without_panic() {
    let body = r#"<w:p>
        <w:r><w:fldChar w:fldCharType="end"/></w:r>
        <w:r><w:fldChar w:fldCharType="separate"/></w:r>
        <w:r><w:t>plain</w:t></w:r>
        <w:r><w:fldChar w:fldCharType="begin"/></w:r>
        <w:r><w:instrText>PAGE</w:instrText></w:r>
        <w:r><w:t>tail</w:t></w:r>
        </w:p>"#;
    let d = build(body, &[]);
    assert_eq!(
        runs(first_para(&d)),
        [("plain".into(), None), ("tail".into(), None)]
    );
}
