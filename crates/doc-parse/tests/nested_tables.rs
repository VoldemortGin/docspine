//! 单元格内嵌套表的三种导出:文字一个都不能丢(纯文本压平成行、Markdown 退回 HTML 表、
//! HTML 真嵌套 `<table>`)。每个用例现场构造 `.docx` 部件再解析,不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_core::export::{to_html, to_markdown, to_text};
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

fn build(body: &str, rels: &str, extra: &[(&str, String)]) -> Document {
    let doc = format!(
        r#"<w:document xmlns:w="{W_NS}" xmlns:r="{REL_NS}"
  xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><w:body>{body}</w:body></w:document>"#
    );
    let rels = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{rels}</Relationships>"#
    );
    let mut all: Vec<(&str, &str)> = vec![
        ("word/document.xml", &doc),
        ("word/_rels/document.xml.rels", &rels),
    ];
    all.extend(extra.iter().map(|(n, x)| (*n, x.as_str())));
    parse_parts(&all)
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// 单元格:`props` 为 `w:tcPr` 内容,`inner` 为单元格块内容。
fn tc(props: &str, inner: &str) -> String {
    format!("<w:tc><w:tcPr>{props}</w:tcPr>{inner}</w:tc>")
}

fn tr(cells: &[String]) -> String {
    format!("<w:tr>{}</w:tr>", cells.concat())
}

fn tbl(rows: &[String]) -> String {
    format!("<w:tbl><w:tblPr/><w:tblGrid/>{}</w:tbl>", rows.concat())
}

#[test]
fn two_level_nested_table_in_all_exports() {
    let inner = tbl(&[
        tr(&[tc("", &p("a")), tc("", &p("b"))]),
        tr(&[tc("", &p("c")), tc("", &p("d"))]),
    ]);
    let outer = tbl(&[tr(&[tc("", &(p("O") + &inner)), tc("", &p("X"))])]);
    // 末尾补一个空段落(Word 要求单元格以段落结尾)。
    let doc = build(&(outer + &p("end")), "", &[]);

    // 纯文本:嵌套表的行压平成单元格内的行,与单元格内多段落同一风格(行内单元格 `\t`)。
    assert_eq!(to_text(&doc), "O\na\tb\nc\td\tX\nend");
    let md = to_markdown(&doc);
    for t in ["O", "a", "b", "c", "d", "X"] {
        assert!(md.contains(t), "{t} 丢了: {md}");
    }
    let html = to_html(&doc);
    assert_eq!(html.matches("<table").count(), 2, "{html}");
    assert!(
        html.contains("<td>a</td>") && html.contains("<td>d</td>"),
        "{html}"
    );
}

#[test]
fn three_level_nested_table_keeps_every_text() {
    let l3 = tbl(&[tr(&[tc("", &p("deep1")), tc("", &p("deep2"))])]);
    let l2 = tbl(&[tr(&[tc("", &(p("mid") + &l3))])]);
    let l1 = tbl(&[tr(&[tc("", &(p("top") + &l2))])]);
    let doc = build(&l1, "", &[]);
    assert_eq!(to_text(&doc), "top\nmid\ndeep1\tdeep2");
    let md = to_markdown(&doc);
    let html = to_html(&doc);
    for t in ["top", "mid", "deep1", "deep2"] {
        assert!(md.contains(t) && html.contains(t), "{t} 丢了");
    }
    assert_eq!(html.matches("<table").count(), 3);
}

#[test]
fn nested_table_inside_merged_cell() {
    let inner = tbl(&[tr(&[tc("", &p("in1")), tc("", &p("in2"))])]);
    let outer = tbl(&[
        tr(&[tc(r#"<w:gridSpan w:val="2"/>"#, &(p("span") + &inner))]),
        tr(&[tc("", &p("l")), tc("", &p("r"))]),
    ]);
    let doc = build(&outer, "", &[]);
    assert_eq!(to_text(&doc), "span\nin1\tin2\nl\tr");
    let html = to_html(&doc);
    assert!(html.contains("colspan=\"2\""), "{html}");
    assert_eq!(html.matches("<table").count(), 2, "{html}");
    assert!(to_markdown(&doc).contains("in2"));
}

fn numbering() -> String {
    format!(
        r#"<w:numbering xmlns:w="{W_NS}"><w:abstractNum w:abstractNumId="0">
 <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
</w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    )
}

#[test]
fn nested_table_with_list_footnote_and_picture() {
    let li = |t: &str| {
        format!(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>{t}</w:t></w:r></w:p>"#
        )
    };
    let note = r#"<w:p><w:r><w:t>ref</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>"#;
    let pic = r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="P" descr="cat"/>
        <a:graphic><a:graphicData><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:blipFill><a:blip r:embed="rImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic>
        </wp:inline></w:drawing></w:r></w:p>"#;
    let inner = tbl(&[tr(&[tc("", &(li("one") + &li("two") + note + pic))])]);
    let outer = tbl(&[tr(&[tc("", &(p("O") + &inner))])]);
    let notes = format!(
        r#"<w:footnotes xmlns:w="{W_NS}"><w:footnote w:id="1">{}</w:footnote></w:footnotes>"#,
        p("note body")
    );
    let rels = format!(
        r#"<Relationship Id="rN" Type="{REL_NS}/footnotes" Target="footnotes.xml"/>
<Relationship Id="rImg" Type="{REL_NS}/image" Target="media/image1.png"/>"#
    );
    let doc = build(
        &outer,
        &rels,
        &[
            ("word/numbering.xml", numbering()),
            ("word/footnotes.xml", notes),
            ("word/media/image1.png", "x".to_string()),
        ],
    );
    let txt = to_text(&doc);
    for t in ["1. one", "2. two", "ref[1]", "[图片: cat]", "[1] note body"] {
        assert!(txt.contains(t), "{t} 丢了: {txt}");
    }
    let md = to_markdown(&doc);
    assert!(
        md.contains("[^1]") && md.contains("[^1]: note body"),
        "{md}"
    );
    assert!(md.contains("image1.png") && md.contains("1. one"), "{md}");
    let html = to_html(&doc);
    assert!(
        html.contains("fnref-1") && html.contains("alt=\"cat\""),
        "{html}"
    );
}

#[test]
fn text_box_inside_nested_table_cell() {
    let tb = format!(
        r#"<w:p><w:r><w:t>anchor</w:t><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml"><v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#,
        p("boxed")
    );
    let inner = tbl(&[tr(&[tc("", &tb)])]);
    let outer = tbl(&[tr(&[tc("", &(p("O") + &inner))])]);
    let doc = build(&outer, "", &[]);
    assert_eq!(to_text(&doc), "O\nanchor\nboxed");
    assert!(to_markdown(&doc).contains("boxed"));
    assert!(to_html(&doc).contains("boxed"));
}

/// 嵌套表出现在单元格内文本框里(GFM 单元格放不下,必须退回 HTML 表而不是丢字)。
#[test]
fn table_inside_text_box_inside_cell_is_kept() {
    let inner = tbl(&[tr(&[tc("", &p("tbx"))])]);
    let tb = format!(
        r#"<w:p><w:r><w:t>anchor</w:t><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml"><v:textbox><w:txbxContent>{inner}<w:p/></w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#
    );
    let outer = tbl(&[tr(&[tc("", &tb), tc("", &p("y"))])]);
    let doc = build(&outer, "", &[]);
    assert!(to_text(&doc).contains("tbx"));
    let md = to_markdown(&doc);
    assert!(md.contains("tbx"), "{md}");
    assert!(to_html(&doc).contains("tbx"));
}

#[test]
fn deep_nesting_is_bounded_and_does_not_overflow() {
    let mut x = tbl(&[tr(&[tc("", &p("core"))])]);
    for i in 0..200 {
        x = tbl(&[tr(&[tc("", &(p(&format!("L{i}")) + &x))])]);
    }
    let doc = build(&x, "", &[]);
    // 解析层深度守卫截断后,三种导出递归都有界。
    let txt = to_text(&doc);
    assert!(txt.contains("L199"));
    let _ = to_markdown(&doc);
    let _ = to_html(&doc);
}
