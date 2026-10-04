//! 页眉页脚 / 脚注尾注:现场合成 docx(纯 zip + 手写 XML,不落二进制 fixture),
//! 断言模型(节引用 + 部件表 + 注表 + 正文引用位置)与 `to_text` / `to_markdown` 导出。

use std::io::{Cursor, Write};

use doc_core::export::{to_markdown, to_text};
use doc_core::model::{Block, HeaderFooterKind, NoteKind, RunSegment};
use doc_parse::{parse_bytes, ParsedDoc};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// 合成 docx:`body_xml` 是 `w:body` 内容;`rels` 是 `(rId, 类型后缀, Target)`;
/// `parts` 是额外部件 `(路径, XML 全文)`。
fn build(body_xml: &str, rels: &[(&str, &str, &str)], parts: &[(&str, String)]) -> ParsedDoc {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document {W_NS}><w:body>{body_xml}</w:body></w:document>"#
    );
    let rels_xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#,
        rels.iter()
            .map(|(id, ty, target)| format!(
                r#"<Relationship Id="{id}" Type="{REL_NS}/{ty}" Target="{target}"/>"#
            ))
            .collect::<String>()
    );
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        let mut all: Vec<(String, String)> = vec![
            ("word/document.xml".into(), doc),
            ("word/_rels/document.xml.rels".into(), rels_xml),
        ];
        all.extend(parts.iter().map(|(n, x)| (n.to_string(), x.clone())));
        for (name, body) in all {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner()).expect("parse synthetic docx")
}

fn hdr(inner: &str) -> String {
    format!(r#"<w:hdr {W_NS}>{inner}</w:hdr>"#)
}
fn ftr(inner: &str) -> String {
    format!(r#"<w:ftr {W_NS}>{inner}</w:ftr>"#)
}
fn p(t: &str) -> String {
    format!("<w:p><w:r><w:t>{t}</w:t></w:r></w:p>")
}

/// `w:footnotes` / `w:endnotes` 部件:含两条 separator 与给定的正常注 `(id, 文字)`。
fn notes_part(root: &str, tag: &str, notes: &[(i64, &str)]) -> String {
    let body: String = notes
        .iter()
        .map(|(id, t)| format!(r#"<w:{tag} w:id="{id}">{}</w:{tag}>"#, p(t)))
        .collect();
    format!(
        r#"<w:{root} {W_NS}>
        <w:{tag} w:type="separator" w:id="-1"><w:p><w:r><w:separator/></w:r></w:p></w:{tag}>
        <w:{tag} w:type="continuationSeparator" w:id="0"><w:p><w:r><w:t>SEPARATOR-JUNK</w:t></w:r></w:p></w:{tag}>
        {body}</w:{root}>"#
    )
}

fn fn_ref(id: i64) -> String {
    format!(r#"<w:r><w:footnoteReference w:id="{id}"/></w:r>"#)
}
fn en_ref(id: i64) -> String {
    format!(r#"<w:r><w:endnoteReference w:id="{id}"/></w:r>"#)
}

fn sect(inner: &str) -> String {
    format!("<w:sectPr>{inner}</w:sectPr>")
}

// ------------------------------------------------------------------ 页眉页脚

#[test]
fn header_footer_default_first_even_are_parsed_with_kinds() {
    let body = format!(
        "{}{}",
        p("Body text"),
        sect(
            r#"<w:headerReference w:type="default" r:id="rIdH1"/>
               <w:headerReference w:type="first" r:id="rIdH2"/>
               <w:headerReference w:type="even" r:id="rIdH3"/>
               <w:footerReference w:type="default" r:id="rIdF1"/>
               <w:footerReference w:type="first" r:id="rIdF2"/>
               <w:footerReference w:type="even" r:id="rIdF3"/>"#
        )
    );
    let d = build(
        &body,
        &[
            ("rIdH1", "header", "header1.xml"),
            ("rIdH2", "header", "header2.xml"),
            ("rIdH3", "header", "header3.xml"),
            ("rIdF1", "footer", "footer1.xml"),
            ("rIdF2", "footer", "footer2.xml"),
            ("rIdF3", "footer", "footer3.xml"),
        ],
        &[
            ("word/header1.xml", hdr(&p("Head default"))),
            ("word/header2.xml", hdr(&p("Head first"))),
            ("word/header3.xml", hdr(&p("Head even"))),
            ("word/footer1.xml", ftr(&p("Foot default"))),
            ("word/footer2.xml", ftr(&p("Foot first"))),
            ("word/footer3.xml", ftr(&p("Foot even"))),
        ],
    );
    let doc = &d.document;
    let s = &doc.sections[0];
    let kinds =
        |refs: &[doc_core::model::HeaderFooterRef]| refs.iter().map(|r| r.kind).collect::<Vec<_>>();
    let want = [
        HeaderFooterKind::Default,
        HeaderFooterKind::First,
        HeaderFooterKind::Even,
    ];
    assert_eq!(kinds(&s.headers), want);
    assert_eq!(kinds(&s.footers), want);
    assert_eq!(doc.header_footers.len(), 6);
    let first_header = &doc.header_footers[&s.headers[1].rel_id];
    let Block::Paragraph(para) = &first_header[0] else {
        panic!("paragraph")
    };
    assert_eq!(para.text(), "Head first");

    let txt = to_text(doc);
    assert_eq!(
        txt,
        "[Header: default]\nHead default\n[Header: first]\nHead first\n\
         [Header: even]\nHead even\nBody text\n\
         [Footer: default]\nFoot default\n[Footer: first]\nFoot first\n\
         [Footer: even]\nFoot even"
    );
    let md = to_markdown(doc);
    assert!(
        md.starts_with("**Header (default)**\n\nHead default\n\n"),
        "{md}"
    );
    assert!(md.ends_with("**Footer (even)**\n\nFoot even"), "{md}");
    assert!(md.contains("\n\nBody text\n\n"), "{md}");
}

#[test]
fn header_with_table_is_extracted() {
    let table = r#"<w:tbl><w:tblGrid><w:gridCol w:w="100"/><w:gridCol w:w="100"/></w:tblGrid>
        <w:tr><w:tc><w:p><w:r><w:t>L</w:t></w:r></w:p></w:tc><w:tc><w:p><w:r><w:t>R</w:t></w:r></w:p></w:tc></w:tr></w:tbl>"#;
    let d = build(
        &sect(r#"<w:headerReference w:type="default" r:id="rIdH"/>"#),
        &[("rIdH", "header", "header1.xml")],
        &[("word/header1.xml", hdr(&format!("{table}<w:p/>")))],
    );
    let parts: Vec<_> = d.document.header_footers.values().collect();
    assert!(matches!(parts[0][0], Block::Table(_)));
    assert_eq!(to_text(&d.document), "[Header: default]\nL\tR");
    assert!(to_markdown(&d.document).contains("| L | R |"));
}

#[test]
fn header_shared_by_two_sections_is_exported_once() {
    let r = r#"<w:headerReference w:type="default" r:id="rIdH"/>"#;
    let body = format!(
        "{}<w:p><w:pPr>{}</w:pPr></w:p>{}{}",
        p("one"),
        sect(r),
        p("two"),
        sect(r)
    );
    let d = build(
        &body,
        &[("rIdH", "header", "header1.xml")],
        &[("word/header1.xml", hdr(&p("Shared Header")))],
    );
    let doc = &d.document;
    assert_eq!(doc.sections.len(), 2);
    assert_eq!(doc.sections[0].headers, doc.sections[1].headers);
    assert_eq!(doc.header_footers.len(), 1);
    assert_eq!(to_text(doc).matches("Shared Header").count(), 1);
    assert_eq!(to_markdown(doc).matches("Shared Header").count(), 1);
}

#[test]
fn two_rel_ids_to_one_part_are_deduped() {
    let body = format!(
        "{}{}{}",
        p("a"),
        format_args!(
            "<w:p><w:pPr>{}</w:pPr></w:p>",
            sect(r#"<w:headerReference w:type="default" r:id="rIdA"/>"#)
        ),
        sect(r#"<w:headerReference w:type="default" r:id="rIdB"/>"#)
    );
    let d = build(
        &body,
        &[
            ("rIdA", "header", "header1.xml"),
            ("rIdB", "header", "/word/header1.xml"),
        ],
        &[("word/header1.xml", hdr(&p("Once")))],
    );
    assert_eq!(d.document.header_footers.len(), 1);
    assert_eq!(to_text(&d.document).matches("Once").count(), 1);
}

#[test]
fn empty_header_part_adds_no_noise_to_exports() {
    let d = build(
        &format!(
            "{}{}",
            p("Body"),
            sect(r#"<w:headerReference w:type="default" r:id="rIdH"/>"#)
        ),
        &[("rIdH", "header", "header1.xml")],
        &[("word/header1.xml", hdr("<w:p/>"))],
    );
    assert_eq!(to_text(&d.document), "Body");
    assert_eq!(to_markdown(&d.document), "Body");
}

#[test]
fn header_reference_to_missing_part_or_rel_does_not_panic() {
    let d = build(
        &format!(
            "{}{}",
            p("Body"),
            sect(
                r#"<w:headerReference w:type="default" r:id="rIdGone"/>
                   <w:footerReference w:type="default" r:id="rIdNoRel"/>"#
            )
        ),
        // rIdGone 的 Target 部件不存在;rIdNoRel 在 rels 里根本没有。
        &[("rIdGone", "header", "header9.xml")],
        &[],
    );
    assert!(d.document.header_footers.is_empty());
    assert!(d.document.sections[0].headers.is_empty());
    assert!(d.document.sections[0].footers.is_empty());
    assert_eq!(to_text(&d.document), "Body");
}

#[test]
fn malformed_header_part_degrades_without_panic() {
    let d = build(
        &format!(
            "{}{}",
            p("Body"),
            sect(r#"<w:headerReference w:type="default" r:id="rIdH"/>"#)
        ),
        &[("rIdH", "header", "header1.xml")],
        // 截断 / 标签错配的畸形 XML:按现有容错风格降级,不报错不 panic。
        &[(
            "word/header1.xml",
            format!(r#"<w:hdr {W_NS}><w:p><w:r><w:t>Half</w:t></w:r></w:p><w:p><w:r><w:t>Cut"#),
        )],
    );
    let txt = to_text(&d.document);
    assert!(txt.contains("Half"), "{txt:?}");
    assert!(txt.contains("Body"), "{txt:?}");
}

#[test]
fn header_inside_malformed_garbage_part_is_ignored() {
    let d = build(
        &format!(
            "{}{}",
            p("Body"),
            sect(r#"<w:headerReference w:type="default" r:id="rIdH"/>"#)
        ),
        &[("rIdH", "header", "header1.xml")],
        &[("word/header1.xml", "\u{0}<<<not xml".to_string())],
    );
    assert_eq!(to_text(&d.document), "Body");
}

// ------------------------------------------------------------------ 脚注尾注

#[test]
fn footnote_and_endnote_refs_keep_position_and_export() {
    let body = format!(
        "<w:p><w:r><w:t>A</w:t></w:r>{}<w:r><w:t>B</w:t></w:r>{}<w:r><w:t>C</w:t></w:r></w:p>",
        fn_ref(1),
        en_ref(1)
    );
    let d = build(
        &body,
        &[
            ("rIdFn", "footnotes", "footnotes.xml"),
            ("rIdEn", "endnotes", "endnotes.xml"),
        ],
        &[
            (
                "word/footnotes.xml",
                notes_part("footnotes", "footnote", &[(1, "Foot one")]),
            ),
            (
                "word/endnotes.xml",
                notes_part("endnotes", "endnote", &[(1, "End one")]),
            ),
        ],
    );
    let doc = &d.document;
    // 引用在 run 序列里带 id,且位于 A 与 B 之间 / B 与 C 之间。
    let Block::Paragraph(para) = &doc.body[0] else {
        panic!("paragraph")
    };
    let segs: Vec<&RunSegment> = para.runs.iter().flat_map(|r| &r.segments).collect();
    assert_eq!(segs.len(), 5);
    assert_eq!(
        segs[1],
        &RunSegment::NoteRef {
            kind: NoteKind::Footnote,
            id: 1
        }
    );
    assert_eq!(
        segs[3],
        &RunSegment::NoteRef {
            kind: NoteKind::Endnote,
            id: 1
        }
    );
    // 段落 text() 仍不含标记(历史语义不变)。
    assert_eq!(para.text(), "ABC");
    // 注表只含正常注,separator 类被跳过。
    assert_eq!(doc.footnotes.keys().copied().collect::<Vec<_>>(), [1]);
    assert_eq!(doc.endnotes.keys().copied().collect::<Vec<_>>(), [1]);

    assert_eq!(to_text(doc), "A[1]B[e1]C\n[1] Foot one\n[e1] End one");
    assert_eq!(
        to_markdown(doc),
        "A[^1]B[^e1]C\n\n[^1]: Foot one\n\n[^e1]: End one"
    );
    assert!(!to_text(doc).contains("SEPARATOR-JUNK"));
}

#[test]
fn notes_are_numbered_by_appearance_and_reused() {
    let body = format!(
        "<w:p><w:r><w:t>x</w:t></w:r>{}{}{}</w:p>",
        fn_ref(7),
        fn_ref(3),
        fn_ref(7)
    );
    let d = build(
        &body,
        &[("rIdFn", "footnotes", "footnotes.xml")],
        &[(
            "word/footnotes.xml",
            notes_part("footnotes", "footnote", &[(3, "three"), (7, "seven")]),
        )],
    );
    assert_eq!(to_text(&d.document), "x[1][2][1]\n[1] seven\n[2] three");
}

#[test]
fn footnote_ref_inside_table_cell_is_marked() {
    let body = format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="100"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r>{}</w:p></w:tc></w:tr></w:tbl>"#,
        fn_ref(1)
    );
    let d = build(
        &body,
        &[("rIdFn", "footnotes", "footnotes.xml")],
        &[(
            "word/footnotes.xml",
            notes_part("footnotes", "footnote", &[(1, "in cell")]),
        )],
    );
    assert_eq!(to_text(&d.document), "cell[1]\n[1] in cell");
    assert!(to_markdown(&d.document).contains("| cell[^1] |"));
}

#[test]
fn dangling_note_ref_does_not_panic_and_leaves_no_marker() {
    let body = format!(
        "<w:p><w:r><w:t>A</w:t></w:r>{}{}<w:r><w:t>B</w:t></w:r></w:p>",
        fn_ref(99),
        en_ref(5)
    );
    // 一个有部件但没有该 id;一个根本没有注部件。
    let d = build(
        &body,
        &[("rIdFn", "footnotes", "footnotes.xml")],
        &[(
            "word/footnotes.xml",
            notes_part("footnotes", "footnote", &[(1, "other")]),
        )],
    );
    assert_eq!(to_text(&d.document), "AB");
    assert_eq!(to_markdown(&d.document), "AB");
}

#[test]
fn malformed_notes_part_degrades_without_panic() {
    let d = build(
        &format!("<w:p><w:r><w:t>A</w:t></w:r>{}</w:p>", fn_ref(1)),
        &[("rIdFn", "footnotes", "footnotes.xml")],
        &[(
            "word/footnotes.xml",
            format!(r#"<w:footnotes {W_NS}><w:footnote w:id="1"><w:p><w:r><w:t>Cut off"#),
        )],
    );
    let txt = to_text(&d.document);
    assert!(txt.starts_with("A[1]"), "{txt:?}");
}

#[test]
fn notes_without_id_or_with_bad_id_are_ignored() {
    let d = build(
        &format!("<w:p><w:r><w:t>A</w:t></w:r>{}</w:p>", fn_ref(1)),
        &[],
        &[(
            "word/footnotes.xml",
            format!(
                r#"<w:footnotes {W_NS}><w:footnote>{}</w:footnote><w:footnote w:id="x">{}</w:footnote><w:footnote w:id="1">{}</w:footnote></w:footnotes>"#,
                p("noid"),
                p("badid"),
                p("good")
            ),
        )],
    );
    assert_eq!(to_text(&d.document), "A[1]\n[1] good");
}

#[test]
fn deeply_nested_header_content_no_stack_overflow() {
    let n = 10_000;
    let inner = format!(
        "{}{}{}",
        "<w:sdt><w:sdtContent>".repeat(n),
        p("deep header"),
        "</w:sdtContent></w:sdt>".repeat(n)
    );
    let d = build(
        &format!(
            "{}{}",
            p("Body"),
            sect(r#"<w:headerReference w:type="default" r:id="rIdH"/>"#)
        ),
        &[("rIdH", "header", "header1.xml")],
        &[("word/header1.xml", hdr(&inner))],
    );
    assert!(to_text(&d.document).contains("Body"));
}
