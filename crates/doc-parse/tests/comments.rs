//! 批注部件(`word/comments.xml`):现场合成 docx(纯 zip + 手写 XML,不落二进制 fixture),
//! 断言模型(`Document.comments` + 正文里的 `RunSegment::CommentRef`)与默认导出不含批注。

use std::io::{Cursor, Write};

use doc_core::export::{to_html, to_markdown, to_text};
use doc_core::model::{Block, RunSegment};
use doc_parse::{parse_bytes, ParsedDoc};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

/// 合成 docx:`body_xml` 是 `w:body` 内容;`parts` 是额外部件 `(路径, XML 全文)`。
/// 批注部件按固定名 `word/comments.xml` 定位(与脚注尾注一致),不需要在 rels 里登记。
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

fn p(t: &str) -> String {
    format!("<w:p><w:r><w:t>{t}</w:t></w:r></w:p>")
}

fn comments_part(inner: &str) -> (&'static str, String) {
    (
        "word/comments.xml",
        format!(r#"<w:comments {W_NS}>{inner}</w:comments>"#),
    )
}

/// 带批注锚点的正文段落:`commentRangeStart` + 文字 + `commentRangeEnd` + 引用 run。
fn anchored(id: i64, text: &str) -> String {
    format!(
        r#"<w:p><w:commentRangeStart w:id="{id}"/><w:r><w:t>{text}</w:t></w:r><w:commentRangeEnd w:id="{id}"/><w:r><w:rPr><w:rStyle w:val="CommentReference"/></w:rPr><w:commentReference w:id="{id}"/></w:r></w:p>"#
    )
}

fn para_text(b: &Block) -> String {
    match b {
        Block::Paragraph(p) => p.text(),
        Block::Table(_) => panic!("expected paragraph"),
    }
}

#[test]
fn basic_comment_has_author_date_initials_and_content() {
    let d = build(
        &anchored(0, "Reviewed text"),
        &[comments_part(&format!(
            r#"<w:comment w:id="0" w:author="Alice Reviewer" w:date="2026-10-03T09:30:00Z" w:initials="AR">{}</w:comment>"#,
            p("Please rephrase.")
        ))],
    );
    let doc = &d.document;
    assert_eq!(doc.comments.len(), 1);
    let c = &doc.comments[&0];
    assert_eq!(c.id, 0);
    assert_eq!(c.author.as_deref(), Some("Alice Reviewer"));
    assert_eq!(c.date.as_deref(), Some("2026-10-03T09:30:00Z"));
    assert_eq!(c.initials.as_deref(), Some("AR"));
    assert_eq!(para_text(&c.blocks[0]), "Please rephrase.");

    // 正文里留下带 id 的引用点,且不产生文字。
    let Block::Paragraph(para) = &doc.body[0] else {
        panic!("paragraph")
    };
    let refs: Vec<_> = para
        .runs
        .iter()
        .flat_map(|r| &r.segments)
        .filter(|s| matches!(s, RunSegment::CommentRef { .. }))
        .collect();
    assert_eq!(refs, [&RunSegment::CommentRef { id: 0 }]);
    assert_eq!(para.text(), "Reviewed text");
}

#[test]
fn missing_attributes_become_none_and_bad_ids_are_skipped() {
    let d = build(
        &p("Body"),
        &[comments_part(&format!(
            r#"<w:comment w:id="1">{}</w:comment>
               <w:comment w:author="NoId">{}</w:comment>
               <w:comment w:id="x" w:author="BadId">{}</w:comment>
               <w:comment w:id="1" w:author="Dup">{}</w:comment>"#,
            p("bare"),
            p("noid"),
            p("badid"),
            p("dup")
        ))],
    );
    let doc = &d.document;
    assert_eq!(doc.comments.keys().copied().collect::<Vec<_>>(), [1]);
    let c = &doc.comments[&1];
    assert_eq!((&c.author, &c.date, &c.initials), (&None, &None, &None));
    // 重复 id 以先出现者为准。
    assert_eq!(para_text(&c.blocks[0]), "bare");
}

#[test]
fn comment_content_may_contain_a_table() {
    let table = r#"<w:tbl><w:tblGrid><w:gridCol w:w="100"/><w:gridCol w:w="100"/></w:tblGrid>
        <w:tr><w:tc><w:p><w:r><w:t>L</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>R</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let d = build(
        &anchored(3, "x"),
        &[comments_part(&format!(
            r#"<w:comment w:id="3" w:author="B">{}{table}</w:comment>"#,
            p("intro")
        ))],
    );
    let c = &d.document.comments[&3];
    assert!(matches!(c.blocks[0], Block::Paragraph(_)));
    let Block::Table(t) = &c.blocks[1] else {
        panic!("table")
    };
    assert_eq!(t.rows[0].cells.len(), 2);
    assert_eq!(t.rows[0].cells[1].text(), "R");
}

#[test]
fn dangling_comment_reference_does_not_panic() {
    // 引用 id 99,而部件里只有 1;另有一个根本没有批注部件的情形见下一个测试。
    let d = build(
        &anchored(99, "text"),
        &[comments_part(&format!(
            r#"<w:comment w:id="1" w:author="A">{}</w:comment>"#,
            p("other")
        ))],
    );
    let doc = &d.document;
    assert!(!doc.comments.contains_key(&99));
    let Block::Paragraph(para) = &doc.body[0] else {
        panic!("paragraph")
    };
    assert!(para
        .runs
        .iter()
        .flat_map(|r| &r.segments)
        .any(|s| s == &RunSegment::CommentRef { id: 99 }));
    assert_eq!(to_text(doc), "text");
}

#[test]
fn missing_or_malformed_comments_part_degrades_without_panic() {
    // 部件缺失:模型为空,引用点仍在。
    let d = build(&anchored(0, "text"), &[]);
    assert!(d.document.comments.is_empty());
    assert_eq!(to_text(&d.document), "text");

    // 截断 / 垃圾:不报错不 panic。
    let d = build(
        &anchored(0, "text"),
        &[(
            "word/comments.xml",
            format!(
                r#"<w:comments {W_NS}><w:comment w:id="0" w:author="A"><w:p><w:r><w:t>Cut off"#
            ),
        )],
    );
    assert_eq!(to_text(&d.document), "text");
    let d = build(
        &anchored(0, "text"),
        &[("word/comments.xml", "\u{0}<<<not xml".to_string())],
    );
    assert!(d.document.comments.is_empty());
    // 空 / 自闭合批注元素。
    let d = build(
        &p("x"),
        &[comments_part(r#"<w:comment w:id="5" w:author="E"/>"#)],
    );
    assert!(d.document.comments[&5].blocks.is_empty());
}

#[test]
fn deeply_nested_comment_content_no_stack_overflow() {
    let n = 10_000;
    let inner = format!(
        "{}{}{}",
        "<w:sdt><w:sdtContent>".repeat(n),
        p("deep comment"),
        "</w:sdtContent></w:sdt>".repeat(n)
    );
    let d = build(
        &anchored(0, "Body"),
        &[comments_part(&format!(
            r#"<w:comment w:id="0">{inner}</w:comment>"#
        ))],
    );
    assert_eq!(to_text(&d.document), "Body");
}

#[test]
fn default_exports_never_contain_comment_content_or_metadata() {
    let d = build(
        &anchored(0, "Visible body"),
        &[comments_part(&format!(
            r#"<w:comment w:id="0" w:author="Secret Author" w:initials="SA">{}</w:comment>"#,
            p("SECRET-COMMENT-BODY")
        ))],
    );
    for out in [
        to_text(&d.document),
        to_markdown(&d.document),
        to_html(&d.document),
    ] {
        assert!(out.contains("Visible body"), "{out}");
        assert!(!out.contains("SECRET-COMMENT-BODY"), "{out}");
        assert!(!out.contains("Secret Author"), "{out}");
    }
}
