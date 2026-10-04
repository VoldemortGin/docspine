//! 格式修订(`w:pPrChange` / `w:rPrChange` / `w:tcPrChange` / `w:tblPrChange` / `w:trPrChange` /
//! `w:sectPrChange`)验收:里面装的是**修订前**旧属性,按“接受全部修订”只认当前值,
//! 旧值不得后写胜出。fixture 现场构造。

use std::io::{Cursor, Write};

use doc_core::model::{Block, Document, Paragraph, Table, VMerge};
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

#[test]
fn ppr_change_old_props_do_not_override_current() {
    let doc = parse_body(
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/><w:numPr><w:ilvl w:val="1"/><w:numId w:val="5"/></w:numPr><w:jc w:val="center"/>
        <w:pPrChange w:id="1"><w:pPr><w:pStyle w:val="Normal"/><w:numPr><w:ilvl w:val="0"/><w:numId w:val="9"/></w:numPr><w:jc w:val="left"/></w:pPr></w:pPrChange>
        </w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
    );
    let p = &paragraphs(&doc)[0];
    assert_eq!(p.style.as_deref(), Some("Heading1"));
    assert_eq!(p.num_id, Some(5));
    assert_eq!(p.list_level, Some(1));
    assert_eq!(p.align.as_deref(), Some("center"));
}

#[test]
fn rpr_change_old_props_do_not_override_current() {
    let doc = parse_body(
        r#"<w:p><w:r><w:rPr><w:b/><w:sz w:val="40"/>
        <w:rPrChange w:id="1"><w:rPr><w:b w:val="0"/><w:sz w:val="20"/></w:rPr></w:rPrChange></w:rPr><w:t>x</w:t></w:r></w:p>"#,
    );
    let r = &paragraphs(&doc)[0].runs[0];
    assert!(r.bold, "现值粗体不被旧值覆盖");
    assert_eq!(r.rpr.b, Some(true));
    assert_eq!(r.rpr.sz, Some(20.0));
}

#[test]
fn tcpr_change_old_props_do_not_override_current() {
    let doc = parse_body(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="1"/></w:tblGrid><w:tr><w:tc><w:tcPr><w:gridSpan w:val="3"/><w:vMerge w:val="restart"/>
        <w:tcPrChange w:id="1"><w:tcPr><w:gridSpan w:val="1"/><w:vMerge/><w:tcW w:w="77" w:type="dxa"/></w:tcPr></w:tcPrChange>
        </w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    let c = &first_table(&doc).rows[0].cells[0];
    assert_eq!(c.grid_span, 3);
    assert_eq!(c.v_merge, VMerge::Restart);
    assert_eq!(c.width, None);
}

#[test]
fn tblpr_change_old_props_do_not_override_current() {
    let doc = parse_body(
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/><w:jc w:val="center"/>
        <w:tblPrChange w:id="1"><w:tblPr><w:tblStyle w:val="Old"/><w:jc w:val="right"/></w:tblPr></w:tblPrChange></w:tblPr>
        <w:tblGrid><w:gridCol w:w="1"/></w:tblGrid><w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    let t = first_table(&doc);
    assert_eq!(t.style.as_deref(), Some("Grid"));
    assert_eq!(t.jc, Some(doc_core::style::Justification::Center));
}

#[test]
fn trpr_change_old_props_do_not_override_current() {
    let doc = parse_body(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="1"/></w:tblGrid><w:tr><w:trPr><w:cantSplit/><w:trHeight w:val="500"/>
        <w:trPrChange w:id="1"><w:trPr><w:cantSplit w:val="0"/><w:trHeight w:val="100"/></w:trPr></w:trPrChange></w:trPr>
        <w:tc><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    let r = &first_table(&doc).rows[0];
    assert!(r.cant_split);
    assert_eq!(r.height, Some(500));
}

#[test]
fn sectpr_change_old_props_do_not_override_current() {
    let doc = parse_body(
        r#"<w:p><w:r><w:t>x</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="12240" w:h="15840"/><w:titlePg/>
        <w:sectPrChange w:id="1"><w:sectPr><w:pgSz w:w="1000" w:h="2000" w:orient="landscape"/><w:titlePg w:val="0"/></w:sectPr></w:sectPrChange></w:sectPr>"#,
    );
    let s = &doc.sections[0];
    assert_eq!((s.page_width, s.page_height), (12240, 15840));
    assert!(s.title_pg);
}

/// styles.xml 与 numbering.xml 走同一套共享的 rPr / pPr / style-tblPr walker,同样不认旧值。
#[test]
fn styles_and_numbering_parts_ignore_prop_changes() {
    let styles = format!(
        r#"<w:styles xmlns:w="{W_NS}"><w:style w:type="paragraph" w:styleId="S1">
        <w:pPr><w:jc w:val="center"/><w:pPrChange w:id="1"><w:pPr><w:jc w:val="right"/></w:pPr></w:pPrChange></w:pPr>
        <w:rPr><w:b/><w:rPrChange w:id="2"><w:rPr><w:b w:val="0"/></w:rPr></w:rPrChange></w:rPr></w:style>
        <w:style w:type="table" w:styleId="T1"><w:tblPr><w:tblBorders><w:top w:val="single" w:sz="8"/></w:tblBorders>
        <w:tblPrChange w:id="3"><w:tblPr><w:tblBorders><w:top w:val="double" w:sz="2"/></w:tblBorders></w:tblPr></w:tblPrChange></w:tblPr></w:style></w:styles>"#
    );
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W_NS}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/>
        <w:pPr><w:ind w:left="720"/><w:pPrChange w:id="1"><w:pPr><w:ind w:left="1"/></w:pPr></w:pPrChange></w:pPr>
        <w:rPr><w:b/><w:rPrChange w:id="2"><w:rPr><w:b w:val="0"/></w:rPr></w:rPrChange></w:rPr></w:lvl></w:abstractNum>
        <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let doc = format!(r#"<w:document xmlns:w="{W_NS}"><w:body><w:p/></w:body></w:document>"#);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in [
        ("word/document.xml", &doc),
        ("word/styles.xml", &styles),
        ("word/numbering.xml", &numbering),
    ] {
        zip.start_file(name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    let d = parse_bytes(&zip.finish().unwrap().into_inner())
        .unwrap()
        .document;

    let s1 = &d.styles.styles["S1"];
    assert_eq!(s1.ppr.jc, Some(doc_core::style::Justification::Center));
    assert_eq!(s1.rpr.b, Some(true));
    let top = d.styles.styles["T1"].tblpr.borders.top.as_ref().unwrap();
    assert_eq!((top.val.as_str(), top.sz_eighth_pt), ("single", 8));
    let lvl = d.numbering.level(1, 0).expect("level");
    assert_eq!(lvl.ppr.ind_left, Some(720));
    assert_eq!(lvl.rpr.b, Some(true));
}
