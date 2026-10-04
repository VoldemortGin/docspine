//! 解析诊断通道(`Document::diagnostics`):内容被静默截断 / 跳过 / 钳制时调用方能知道。
//! 诊断只含种类 / 部件路径 / 计数,绝不含正文。每个用例现场构造 `.docx`,不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_core::model::{Block, Diagnostic, DiagnosticKind, MAX_NOTES, MAX_SECTIONS};
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
fn overlong_style_chain_is_reported() {
    // 100 层 basedOn 链:深度超过 MAX_STYLE_CHAIN(64)的样式 s64..s99 共 36 个。
    let mut styles = format!(r#"<w:styles xmlns:w="{W_NS}">"#);
    for i in 0..100 {
        let based = if i > 0 {
            format!(r#"<w:basedOn w:val="s{}"/>"#, i - 1)
        } else {
            String::new()
        };
        styles.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="s{i}">{based}</w:style>"#
        ));
    }
    styles.push_str("</w:styles>");
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&p("x"))),
        ("word/styles.xml", &styles),
    ]);
    assert_eq!(
        count_of(&doc, DiagnosticKind::StyleChainTruncated, "word/styles.xml"),
        Some(36)
    );
    assert_eq!(
        DiagnosticKind::StyleChainTruncated.code(),
        "style-chain-truncated"
    );
}

#[test]
fn notes_beyond_cap_are_dropped_and_reported() {
    // 脚注与批注部件各超出 MAX_NOTES 5 / 3 条:只收前 MAX_NOTES 条,多余的计入诊断。
    let n = MAX_NOTES + 5;
    let fns: String = (1..=n)
        .map(|i| format!(r#"<w:footnote w:id="{i}"/>"#))
        .collect();
    let cms: String = (1..=MAX_NOTES + 3)
        .map(|i| format!(r#"<w:comment w:id="{i}"/>"#))
        .collect();
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&p("x"))),
        (
            "word/footnotes.xml",
            &format!(r#"<w:footnotes xmlns:w="{W_NS}">{fns}</w:footnotes>"#),
        ),
        (
            "word/comments.xml",
            &format!(r#"<w:comments xmlns:w="{W_NS}">{cms}</w:comments>"#),
        ),
    ]);
    assert_eq!(doc.footnotes.len(), MAX_NOTES);
    assert_eq!(doc.comments.len(), MAX_NOTES);
    assert_eq!(
        count_of(&doc, DiagnosticKind::NotesTruncated, "word/footnotes.xml"),
        Some(5)
    );
    assert_eq!(
        count_of(&doc, DiagnosticKind::NotesTruncated, "word/comments.xml"),
        Some(3)
    );
    assert_eq!(DiagnosticKind::NotesTruncated.code(), "notes-truncated");
}

#[test]
fn sections_beyond_cap_merge_into_the_last_one_and_are_reported() {
    // MAX_SECTIONS + 5 个段落级 sectPr,再加带特殊页宽的文档级 sectPr(最后一节)。
    let n = MAX_SECTIONS + 5;
    let mut body = String::new();
    for _ in 0..n {
        body.push_str("<w:p><w:pPr><w:sectPr/></w:pPr></w:p>");
    }
    body.push_str(&p("tail"));
    body.push_str(r#"<w:sectPr><w:pgSz w:w="12345" w:h="15840"/></w:sectPr>"#);
    let doc = simple(&body);
    assert_eq!(doc.sections.len(), MAX_SECTIONS);
    // 被并入的是中间的节:最后一节仍是带特殊页宽的文档级 sectPr,正文不丢。
    assert_eq!(doc.sections.last().map(|s| s.page_width), Some(12345));
    assert_eq!(
        count_of(&doc, DiagnosticKind::SectionsTruncated, "word/document.xml"),
        Some(6)
    );
    assert!(doc
        .body
        .iter()
        .any(|b| matches!(b, Block::Paragraph(p) if p.runs.iter().any(|r| r.text() == "tail"))));
    assert_eq!(
        DiagnosticKind::SectionsTruncated.code(),
        "sections-truncated"
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

/// 超过节数上限时并入末节的中间节:它们引用的页眉页脚部件仍进导出(去重后照常输出),不丢字。
#[test]
fn sections_beyond_cap_keep_every_header_and_footer_in_exports() {
    let n = MAX_SECTIONS + 5;
    let merged = MAX_SECTIONS + 1; // 落在被并入的区间里
    let mut body = String::new();
    for i in 0..n {
        let refs = if i == 0 {
            r#"<w:headerReference w:type="default" r:id="rH1"/>"#.to_string()
        } else if i == merged {
            r#"<w:headerReference w:type="default" r:id="rH2"/><w:footerReference w:type="default" r:id="rF2"/><w:pgNumType w:start="500"/>"#
                .to_string()
        } else {
            String::new()
        };
        body.push_str(&format!(
            "<w:p><w:pPr><w:sectPr>{refs}</w:sectPr></w:pPr></w:p>"
        ));
    }
    body.push_str(&p("tail"));
    body.push_str("<w:sectPr/>");
    let hdr = |t: &str| format!(r#"<w:hdr xmlns:w="{W_NS}">{}</w:hdr>"#, p(t));
    let ftr = |t: &str| format!(r#"<w:ftr xmlns:w="{W_NS}">{}</w:ftr>"#, p(t));
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&body)),
        (
            "word/_rels/document.xml.rels",
            &rels(&[
                ("rH1", "header", "header1.xml"),
                ("rH2", "header", "header2.xml"),
                ("rF2", "footer", "footer2.xml"),
            ]),
        ),
        ("word/header1.xml", &hdr("HEADER-ONE")),
        ("word/header2.xml", &hdr("ONLY-IN-MERGED")),
        ("word/footer2.xml", &ftr("FOOT-MERGED")),
    ]);
    assert_eq!(doc.sections.len(), MAX_SECTIONS);
    assert!(count_of(&doc, DiagnosticKind::SectionsTruncated, "word/document.xml").is_some());
    for out in [
        doc_core::export::to_text(&doc),
        doc_core::export::to_markdown(&doc),
        doc_core::export::to_html(&doc),
    ] {
        for want in ["HEADER-ONE", "ONLY-IN-MERGED", "FOOT-MERGED", "tail"] {
            assert!(out.contains(want), "{want} 缺失");
        }
    }
}

/// 上限以内(1 万节以上,原上限会合并):每节一封信、页码各自从 1 重起,各节的页码起始值与页面几何
/// 全部保留,没有合并诊断。
#[test]
fn ten_thousand_letters_keep_their_page_number_restarts() {
    let n = 10_005usize;
    let mut body = String::new();
    for i in 0..n - 1 {
        let orient = if i % 2 == 0 {
            r#"<w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>"#
        } else {
            ""
        };
        body.push_str(&format!(
            r#"<w:p><w:pPr><w:sectPr>{orient}<w:pgNumType w:start="1"/></w:sectPr></w:pPr><w:r><w:t>L{i}</w:t></w:r></w:p>"#
        ));
    }
    body.push_str(&p("last"));
    body.push_str(r#"<w:sectPr><w:pgNumType w:start="1"/></w:sectPr>"#);
    let doc = simple(&body);
    assert_eq!(doc.sections.len(), n);
    assert!(doc.sections.iter().all(|s| s.page_number_start == Some(1)));
    assert_eq!(doc.sections[n - 3].page_width, 16838);
    assert_eq!(doc.sections[n - 2].page_width, 12240);
    assert!(count_of(&doc, DiagnosticKind::SectionsTruncated, "word/document.xml").is_none());
}

/// 注条目超过上限时优先保留**被正文引用**的注:引用到的注排在部件最后也不丢(原先丢的是位置靠后的
/// 条目,不论有没有被引用),多余的未引用注丢弃并计数;Start 形式 `<w:footnote>…</w:footnote>` 同样封顶。
#[test]
fn notes_cap_keeps_referenced_notes_first() {
    let unreferenced: String = (1000..1000 + MAX_NOTES + 5)
        .map(|i| format!(r#"<w:footnote w:id="{i}"><w:p/></w:footnote>"#))
        .collect();
    let referenced =
        r#"<w:footnote w:id="7"><w:p><w:r><w:t>KEPT-NOTE</w:t></w:r></w:p></w:footnote>"#;
    let body = r#"<w:p><w:r><w:t>see</w:t></w:r><w:r><w:footnoteReference w:id="7"/></w:r></w:p>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(body)),
        (
            "word/footnotes.xml",
            &format!(r#"<w:footnotes xmlns:w="{W_NS}">{unreferenced}{referenced}</w:footnotes>"#),
        ),
    ]);
    assert_eq!(doc.footnotes.len(), MAX_NOTES);
    assert!(doc.footnotes.contains_key(&7), "被引用的注必须保留");
    assert_eq!(
        count_of(&doc, DiagnosticKind::NotesTruncated, "word/footnotes.xml"),
        Some(6)
    );
    let text = doc_core::export::to_text(&doc);
    assert!(
        text.starts_with("see[1]") && text.contains("[1] KEPT-NOTE"),
        "{}",
        &text[..40.min(text.len())]
    );
}

/// 字段指令累积时先去前导空白、折叠连续空白再计入上限:约 4 KB 空白在前也不会把 `PAGE` 截掉。
#[test]
fn field_instruction_whitespace_does_not_count_against_the_cap() {
    let pad = " ".repeat(5_000);
    let body = format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText xml:space="preserve">{pad}</w:instrText></w:r><w:r><w:instrText xml:space="preserve">PAGE{pad}\* roman </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>7</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r></w:p><w:p><w:fldSimple w:instr="{pad}NUMPAGES{pad}"><w:r><w:t>9</w:t></w:r></w:fldSimple></w:p>"#
    );
    let doc = simple(&body);
    let fields: Vec<&str> = doc
        .body
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(p) => p.runs.iter().find_map(|r| r.field.as_deref()),
            _ => None,
        })
        .collect();
    assert_eq!(fields, ["PAGE \\* roman", "NUMPAGES"]);
    assert!(count_of(
        &doc,
        DiagnosticKind::FieldInstrTruncated,
        "word/document.xml"
    )
    .is_none());
}
