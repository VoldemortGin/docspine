//! 部件定位:主部件经包根 `_rels/.rels` 的 `officeDocument` 关系找,附属部件(styles / numbering /
//! settings / footnotes / endnotes / comments / theme)经主部件 rels 按关系类型找;找不到再回退
//! 固定路径。相对 Target 基于主部件所在目录解析并规范化,逃出包根的 `..` 一律拒绝。

use std::io::{Cursor, Write};

use doc_core::export::{to_markdown, to_text};
use doc_core::Document;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const PKG_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";

fn parse_parts(parts: &[(&str, &str)]) -> Result<Document, doc_core::DocError> {
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
    parse_bytes(&buf.into_inner()).map(|p| p.document)
}

fn doc_xml(body: &str) -> String {
    format!(
        r#"<w:document xmlns:w="{W_NS}" xmlns:r="{REL_NS}"><w:body>{body}</w:body></w:document>"#
    )
}

/// `(rId, 类型后缀, Target)` 列表 -> `.rels` XML。
fn rels(entries: &[(&str, &str, &str)]) -> String {
    let inner: String = entries
        .iter()
        .map(|(id, ty, target)| {
            format!(r#"<Relationship Id="{id}" Type="{REL_NS}/{ty}" Target="{target}"/>"#)
        })
        .collect();
    format!(r#"<Relationships xmlns="{PKG_REL_NS}">{inner}</Relationships>"#)
}

fn p(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn styled(style: &str, text: &str) -> String {
    format!(r#"<w:p><w:pPr><w:pStyle w:val="{style}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#)
}

fn styles(id: &str) -> String {
    format!(
        r#"<w:styles xmlns:w="{W_NS}"><w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="heading 1"/></w:style></w:styles>"#
    )
}

#[test]
fn main_part_document2_with_its_own_rels() {
    let hdr = format!(r#"<w:hdr xmlns:w="{W_NS}">{}</w:hdr>"#, p("HEAD"));
    let body =
        p("body2") + r#"<w:sectPr><w:headerReference w:type="default" r:id="rH"/></w:sectPr>"#;
    let doc = parse_parts(&[
        (
            "_rels/.rels",
            &rels(&[("rId1", "officeDocument", "word/document2.xml")]),
        ),
        ("word/document2.xml", &doc_xml(&body)),
        (
            "word/_rels/document2.xml.rels",
            &rels(&[("rH", "header", "header1.xml")]),
        ),
        ("word/header1.xml", &hdr),
    ])
    .expect("parse");
    assert_eq!(to_text(&doc), "[Header: default]\nHEAD\nbody2");
    assert!(doc.diagnostics.is_empty(), "{:?}", doc.diagnostics);
}

#[test]
fn main_part_outside_word_dir_resolves_relative_targets_from_its_dir() {
    // 主部件在 `doc/main.xml`:相对 Target 以 `doc/` 为基准,`../shared/` 回到包根下。
    let body = styled("Big", "Title");
    let hdr = format!(r#"<w:hdr xmlns:w="{W_NS}">{}</w:hdr>"#, p("HEAD"));
    let body = body + r#"<w:sectPr><w:headerReference w:type="default" r:id="rH"/></w:sectPr>"#;
    let doc = parse_parts(&[
        (
            "_rels/.rels",
            &rels(&[("rId1", "officeDocument", "/doc/main.xml")]),
        ),
        ("doc/main.xml", &doc_xml(&body)),
        (
            "doc/_rels/main.xml.rels",
            &rels(&[
                ("rS", "styles", "parts/st.xml"),
                ("rH", "header", "../shared/hdr.xml"),
            ]),
        ),
        ("doc/parts/st.xml", &styles("Big")),
        ("shared/hdr.xml", &hdr),
    ])
    .expect("parse");
    assert!(to_markdown(&doc).contains("# Title"));
    assert!(to_text(&doc).contains("HEAD"));
}

#[test]
fn ancillary_parts_with_unconventional_names_found_via_rels() {
    let notes = |tag: &str, text: &str| {
        format!(
            r#"<w:{tag}s xmlns:w="{W_NS}"><w:{tag} w:id="1">{}</w:{tag}></w:{tag}s>"#,
            p(text)
        )
    };
    let comments = format!(
        r#"<w:comments xmlns:w="{W_NS}"><w:comment w:id="1" w:author="A">{}</w:comment></w:comments>"#,
        p("c")
    );
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W_NS}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1)"/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let settings = format!(r#"<w:settings xmlns:w="{W_NS}"><w:evenAndOddHeaders/></w:settings>"#);
    let body = styled("Big", "T")
        + r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r><w:r><w:footnoteReference w:id="1"/></w:r></w:p>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&body)),
        (
            "word/_rels/document.xml.rels",
            &rels(&[
                ("r1", "styles", "custom/s.xml"),
                ("r2", "numbering", "custom/n.xml"),
                ("r3", "settings", "custom/set.xml"),
                ("r4", "footnotes", "custom/f.xml"),
                ("r5", "endnotes", "custom/e.xml"),
                ("r6", "comments", "custom/c.xml"),
            ]),
        ),
        ("word/custom/s.xml", &styles("Big")),
        ("word/custom/n.xml", &numbering),
        ("word/custom/set.xml", &settings),
        ("word/custom/f.xml", &notes("footnote", "FOOT")),
        ("word/custom/e.xml", &notes("endnote", "END")),
        ("word/custom/c.xml", &comments),
    ])
    .expect("parse");
    let txt = to_text(&doc);
    assert!(
        txt.contains("1) item[1]") && txt.contains("[1] FOOT"),
        "{txt}"
    );
    assert!(to_markdown(&doc).contains("# T"));
    assert!(doc.even_and_odd_headers);
    assert_eq!(doc.endnotes.len(), 1);
    assert_eq!(doc.comments.len(), 1);
}

#[test]
fn theme_found_via_rels_with_custom_name() {
    let theme = r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"><a:themeElements><a:fontScheme name="x"><a:majorFont><a:latin typeface="MajorX"/></a:majorFont><a:minorFont><a:latin typeface="MinorX"/></a:minorFont></a:fontScheme></a:themeElements></a:theme>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&p("x"))),
        (
            "word/_rels/document.xml.rels",
            &rels(&[("r1", "theme", "theme/custom-theme.xml")]),
        ),
        ("word/theme/custom-theme.xml", theme),
        // 惯例名下放另一份不同的:rels 指向的优先。
        ("word/theme/theme1.xml", "<a:theme xmlns:a=\"x\"/>"),
    ])
    .expect("parse");
    assert_ne!(doc.theme, doc_core::style::Theme::default());
}

#[test]
fn missing_or_malformed_package_rels_fall_back_to_default_paths() {
    let parts_no_rels = [("word/document.xml", doc_xml(&p("plain")))];
    let doc = parse_parts(&[(parts_no_rels[0].0, &parts_no_rels[0].1)]).expect("parse");
    assert_eq!(to_text(&doc), "plain");

    let doc = parse_parts(&[
        ("_rels/.rels", "<Relationships><Relationship Id=\"r\" Ty"),
        ("word/document.xml", &doc_xml(&p("plain"))),
    ])
    .expect("parse");
    assert_eq!(to_text(&doc), "plain");

    // officeDocument 指向不存在的部件:回退。
    let doc = parse_parts(&[
        (
            "_rels/.rels",
            &rels(&[("rId1", "officeDocument", "word/nope.xml")]),
        ),
        ("word/document.xml", &doc_xml(&p("plain"))),
    ])
    .expect("parse");
    assert_eq!(to_text(&doc), "plain");
}

#[test]
fn no_main_part_anywhere_is_a_typed_error() {
    let err = parse_parts(&[("word/other.xml", "<x/>")]).expect_err("无主部件");
    assert!(matches!(err, doc_core::DocError::Zip(_)), "{err:?}");
}

#[test]
fn targets_escaping_the_package_root_are_rejected_without_panic() {
    // 主部件 Target 逃出包根:拒绝,回退到默认路径。
    let doc = parse_parts(&[
        (
            "_rels/.rels",
            &rels(&[("rId1", "officeDocument", "../../word/evil.xml")]),
        ),
        ("word/document.xml", &doc_xml(&p("safe"))),
        ("word/evil.xml", &doc_xml(&p("evil"))),
    ])
    .expect("parse");
    assert_eq!(to_text(&doc), "safe");

    // 附属部件 / 页眉 Target 逃出包根:当作缺失(回退固定路径 / 丢弃引用)。
    let body = p("b") + r#"<w:sectPr><w:headerReference w:type="default" r:id="rH"/></w:sectPr>"#;
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml(&body)),
        (
            "word/_rels/document.xml.rels",
            &rels(&[
                ("r1", "styles", "../../../styles.xml"),
                ("rH", "header", "../../../hdr.xml"),
            ]),
        ),
        ("styles.xml", &styles("Big")),
        (
            "hdr.xml",
            &format!(r#"<w:hdr xmlns:w="{W_NS}">{}</w:hdr>"#, p("H")),
        ),
    ])
    .expect("parse");
    assert_eq!(to_text(&doc), "b");
    assert!(doc.styles.styles.is_empty());
}
