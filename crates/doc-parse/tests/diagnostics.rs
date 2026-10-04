//! 解析诊断通道(`Document::diagnostics`):内容被静默截断 / 跳过 / 钳制时调用方能知道。
//! 诊断只含种类 / 部件路径 / 计数,绝不含正文。每个用例现场构造 `.docx`,不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_core::model::{Block, Diagnostic, DiagnosticKind};
use doc_core::Document;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn parse_parts(parts: &[(&str, &str)]) -> Document {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        for (name, body) in parts {
            zip.start_file(*name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner()).expect("parse").document
}

fn doc_xml(body: &str) -> String {
    format!(
        r#"<w:document xmlns:w="{W_NS}" xmlns:r="{REL_NS}"><w:body>{body}</w:body></w:document>"#
    )
}

fn rels(entries: &[(&str, &str, &str)]) -> String {
    let inner: String = entries
        .iter()
        .map(|(id, ty, target)| {
            format!(r#"<Relationship Id="{id}" Type="{REL_NS}/{ty}" Target="{target}"/>"#)
        })
        .collect();
    format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{inner}</Relationships>"#
    )
}

fn simple(body: &str) -> Document {
    parse_parts(&[("word/document.xml", &doc_xml(body))])
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn count_of(doc: &Document, kind: DiagnosticKind, part: &str) -> Option<usize> {
    doc.diagnostics
        .iter()
        .find(|d| d.kind == kind && d.part == part)
        .map(|d| d.count)
}

#[test]
fn clean_document_has_no_diagnostics() {
    let hdr = format!(r#"<w:hdr xmlns:w="{W_NS}">{}</w:hdr>"#, p("head"));
    let body =
        p("hello") + r#"<w:sectPr><w:headerReference w:type="default" r:id="rH"/></w:sectPr>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&body)),
        (
            "word/_rels/document.xml.rels",
            &rels(&[("rH", "header", "header1.xml")]),
        ),
        ("word/header1.xml", &hdr),
    ]);
    assert_eq!(doc.diagnostics, Vec::<Diagnostic>::new());
}

#[test]
fn truncated_document_xml_is_reported_and_partial_content_kept() {
    // 在第二段文字中途截断:没有任何结束标签。
    let xml = format!(
        r#"<w:document xmlns:w="{W_NS}"><w:body>{}<w:p><w:r><w:t>cut off he"#,
        p("kept")
    );
    let doc = parse_parts(&[("word/document.xml", &xml)]);
    let Some(Block::Paragraph(first)) = doc.body.first() else {
        panic!("已解析的部分应保留");
    };
    assert_eq!(first.text(), "kept");
    assert_eq!(
        count_of(&doc, DiagnosticKind::XmlTruncated, "word/document.xml"),
        Some(1)
    );
}

#[test]
fn mid_tag_corruption_is_reported() {
    let xml = format!(
        r#"<w:document xmlns:w="{W_NS}"><w:body>{}<w:p></w:q><w:r><w:t>x</w:t></w:r></w:body></w:document>"#,
        p("kept")
    );
    let doc = parse_parts(&[("word/document.xml", &xml)]);
    assert_eq!(
        count_of(&doc, DiagnosticKind::XmlTruncated, "word/document.xml"),
        Some(1)
    );
    assert!(!doc.body.is_empty());
}

#[test]
fn truncated_header_part_is_reported_with_its_own_path() {
    let hdr = format!(
        r#"<w:hdr xmlns:w="{W_NS}">{}<w:p><w:r><w:t>oops"#,
        p("head")
    );
    let body = p("b") + r#"<w:sectPr><w:headerReference w:type="default" r:id="rH"/></w:sectPr>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&body)),
        (
            "word/_rels/document.xml.rels",
            &rels(&[("rH", "header", "header1.xml")]),
        ),
        ("word/header1.xml", &hdr),
    ]);
    assert_eq!(
        count_of(&doc, DiagnosticKind::XmlTruncated, "word/header1.xml"),
        Some(1)
    );
    assert_eq!(
        count_of(&doc, DiagnosticKind::XmlTruncated, "word/document.xml"),
        None
    );
}

#[test]
fn truncated_styles_numbering_and_notes_are_reported() {
    let styles = format!(r#"<w:styles xmlns:w="{W_NS}"><w:style w:styleId="A"><w:name w:val="A"/"#);
    let notes = format!(r#"<w:footnotes xmlns:w="{W_NS}"><w:footnote w:id="1"><w:p>"#);
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&p("x"))),
        ("word/styles.xml", &styles),
        ("word/footnotes.xml", &notes),
    ]);
    assert_eq!(
        count_of(&doc, DiagnosticKind::XmlTruncated, "word/styles.xml"),
        Some(1)
    );
    assert_eq!(
        count_of(&doc, DiagnosticKind::XmlTruncated, "word/footnotes.xml"),
        Some(1)
    );
}

#[test]
fn nesting_beyond_limit_is_reported_with_skipped_count() {
    let mut x = format!("<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>", p("core"));
    for _ in 0..100 {
        x = format!("<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>", x) + &p("pad");
    }
    let doc = simple(&x);
    let n = count_of(
        &doc,
        DiagnosticKind::NestingDepthExceeded,
        "word/document.xml",
    )
    .expect("应报告嵌套超限");
    assert!(n >= 1);
}

#[test]
fn grid_span_and_grid_columns_clamped_are_reported() {
    let cols: String = (0..70).map(|_| r#"<w:gridCol w:w="100"/>"#).collect();
    let tbl = format!(
        r#"<w:tbl><w:tblGrid>{cols}</w:tblGrid><w:tr><w:trPr><w:gridBefore w:val="500"/></w:trPr><w:tc><w:tcPr><w:gridSpan w:val="99"/></w:tcPr>{}</w:tc></w:tr></w:tbl>"#,
        p("c")
    );
    let doc = simple(&tbl);
    assert_eq!(
        count_of(&doc, DiagnosticKind::GridSpanClamped, "word/document.xml"),
        Some(1)
    );
    // 7 个被丢弃的 gridCol + 1 次 gridBefore 钳制。
    assert_eq!(
        count_of(
            &doc,
            DiagnosticKind::TableColumnsClamped,
            "word/document.xml"
        ),
        Some(8)
    );
}

#[test]
fn in_range_grid_values_are_not_reported() {
    let tbl = format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr>{}</w:tc></w:tr></w:tbl>"#,
        p("c")
    );
    assert!(simple(&tbl).diagnostics.is_empty());
}

#[test]
fn dangling_header_relationship_or_part_is_reported() {
    // rH1:关系存在但部件缺失;rH2:关系本身缺失。
    let body = p("b")
        + r#"<w:sectPr><w:headerReference w:type="default" r:id="rH1"/><w:headerReference w:type="first" r:id="rH2"/></w:sectPr>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&body)),
        (
            "word/_rels/document.xml.rels",
            &rels(&[("rH1", "header", "header1.xml")]),
        ),
    ]);
    assert_eq!(
        count_of(&doc, DiagnosticKind::MissingPart, "word/document.xml"),
        Some(2)
    );
}

#[test]
fn missing_rels_part_with_header_reference_is_reported() {
    let body = p("b") + r#"<w:sectPr><w:headerReference w:type="default" r:id="rH"/></w:sectPr>"#;
    let doc = simple(&body);
    assert_eq!(
        count_of(&doc, DiagnosticKind::MissingPart, "word/document.xml"),
        Some(1)
    );
}

#[test]
fn picture_pointing_to_missing_media_is_reported() {
    let pic = r#"<w:p><w:r><w:drawing><wp:inline xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="P"/>
        <a:graphic xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:graphicData><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:blipFill><a:blip r:embed="rImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic>
        </wp:inline></w:drawing></w:r></w:p>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(pic)),
        (
            "word/_rels/document.xml.rels",
            &rels(&[("rImg", "image", "media/gone.png")]),
        ),
    ]);
    assert_eq!(
        count_of(&doc, DiagnosticKind::MissingPart, "word/document.xml"),
        Some(1)
    );
}

#[test]
fn alt_chunk_is_reported_and_legacy_counter_kept() {
    let body = p("a") + r#"<w:altChunk r:id="rA"/><w:altChunk r:id="rB"/>"#;
    let doc = simple(&body);
    assert_eq!(doc.alt_chunk_count, 2);
    assert_eq!(
        count_of(
            &doc,
            DiagnosticKind::AltChunkNotImported,
            "word/document.xml"
        ),
        Some(2)
    );
}

#[test]
fn numbering_start_beyond_word_limit_is_reported() {
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W_NS}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="99999"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&p("x"))),
        ("word/numbering.xml", &numbering),
    ]);
    assert_eq!(
        count_of(
            &doc,
            DiagnosticKind::NumberingValueClamped,
            "word/numbering.xml"
        ),
        Some(1)
    );
}

#[test]
fn diagnostics_never_contain_document_text() {
    let secret = "TOPSECRETBODYTEXT";
    let xml = format!(
        r#"<w:document xmlns:w="{W_NS}"><w:body>{}<w:altChunk r:id="x"/><w:p><w:r><w:t>{secret}-cut"#,
        p(secret)
    );
    let doc = parse_parts(&[("word/document.xml", &xml)]);
    assert!(!doc.diagnostics.is_empty());
    let dump = format!("{:?}", doc.diagnostics);
    assert!(!dump.contains(secret), "{dump}");
    for d in &doc.diagnostics {
        assert!(!d.kind.code().contains(secret) && !d.part.contains(secret));
    }
}

#[test]
fn kind_codes_are_kebab_case_and_stable() {
    assert_eq!(DiagnosticKind::XmlTruncated.code(), "xml-truncated");
    assert_eq!(
        DiagnosticKind::NestingDepthExceeded.code(),
        "nesting-depth-exceeded"
    );
    assert_eq!(
        DiagnosticKind::TableColumnsClamped.code(),
        "table-columns-clamped"
    );
    assert_eq!(DiagnosticKind::GridSpanClamped.code(), "grid-span-clamped");
    assert_eq!(
        DiagnosticKind::NumberingValueClamped.code(),
        "numbering-value-clamped"
    );
    assert_eq!(DiagnosticKind::MissingPart.code(), "missing-part");
    assert_eq!(
        DiagnosticKind::AltChunkNotImported.code(),
        "alt-chunk-not-imported"
    );
}
