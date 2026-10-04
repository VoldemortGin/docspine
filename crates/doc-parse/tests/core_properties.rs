//! `docProps/core.xml` 文档属性:经包根 `_rels/.rels` 的 core-properties 关系定位(回退
//! `docProps/core.xml`);缺失字段 `None`;部件缺失 / 畸形不 panic、返回全空;属性不进诊断。

use std::io::{Cursor, Write};

use doc_core::model::CoreProperties;
use doc_core::Document;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const PKG_REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const CORE_REL: &str =
    "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";

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

fn doc_xml() -> String {
    format!(
        r#"<w:document xmlns:w="{W_NS}"><w:body><w:p><w:r><w:t>x</w:t></w:r></w:p></w:body></w:document>"#
    )
}

const CORE: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
  xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/"
  xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance">
  <dc:title>Quarterly &amp; Review</dc:title>
  <dc:subject>Finance</dc:subject>
  <dc:creator>Ada Lovelace</dc:creator>
  <cp:keywords>a, b</cp:keywords>
  <dc:description></dc:description>
  <cp:lastModifiedBy>Charles Babbage</cp:lastModifiedBy>
  <cp:revision>3</cp:revision>
  <dcterms:created xsi:type="dcterms:W3CDTF">2026-09-01T08:00:00Z</dcterms:created>
  <dcterms:modified xsi:type="dcterms:W3CDTF">2026-09-30T17:30:00Z</dcterms:modified>
</cp:coreProperties>"#;

#[test]
fn core_properties_parsed_from_default_path() {
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml()),
        ("docProps/core.xml", CORE),
    ]);
    let p = &doc.core_properties;
    assert_eq!(p.title.as_deref(), Some("Quarterly & Review"));
    assert_eq!(p.subject.as_deref(), Some("Finance"));
    assert_eq!(p.creator.as_deref(), Some("Ada Lovelace"));
    assert_eq!(p.keywords.as_deref(), Some("a, b"));
    assert_eq!(p.last_modified_by.as_deref(), Some("Charles Babbage"));
    assert_eq!(p.revision.as_deref(), Some("3"));
    assert_eq!(p.created.as_deref(), Some("2026-09-01T08:00:00Z"));
    assert_eq!(p.modified.as_deref(), Some("2026-09-30T17:30:00Z"));
    // 空元素与缺失元素都是 None。
    assert_eq!(p.description, None);
    assert_eq!(p.category, None);
    assert_eq!(p.language, None);
}

#[test]
fn core_properties_located_via_package_rels() {
    let rels = format!(
        r#"<Relationships xmlns="{PKG_REL_NS}"><Relationship Id="rId9" Type="{CORE_REL}" Target="meta/custom-core.xml"/></Relationships>"#
    );
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml()),
        ("_rels/.rels", &rels),
        ("meta/custom-core.xml", CORE),
        // 惯例路径放另一份:rels 指向的优先。
        (
            "docProps/core.xml",
            "<cp:coreProperties xmlns:cp=\"x\" xmlns:dc=\"y\"><dc:title>wrong</dc:title></cp:coreProperties>",
        ),
    ]);
    assert_eq!(doc.core_properties.creator.as_deref(), Some("Ada Lovelace"));
    assert_eq!(
        doc.core_properties.title.as_deref(),
        Some("Quarterly & Review")
    );
}

#[test]
fn missing_part_gives_all_empty() {
    let doc = parse_parts(&[("word/document.xml", &doc_xml())]);
    assert_eq!(doc.core_properties, CoreProperties::default());
}

#[test]
fn malformed_part_does_not_panic_and_keeps_what_was_read() {
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml()),
        (
            "docProps/core.xml",
            "<cp:coreProperties xmlns:cp=\"x\" xmlns:dc=\"y\"><dc:title>T</dc:title><dc:creator>Cut",
        ),
    ]);
    assert_eq!(doc.core_properties.title.as_deref(), Some("T"));
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml()),
        ("docProps/core.xml", "\u{0}\u{1}not xml <<<"),
    ]);
    assert_eq!(doc.core_properties, CoreProperties::default());
}

/// 隐私:属性值不进任何诊断(诊断只含种类 / 部件路径 / 计数);core.xml 损坏也不例外。
#[test]
fn properties_never_leak_into_diagnostics() {
    let doc = parse_parts(&[
        ("word/document.xml", &doc_xml()),
        (
            "docProps/core.xml",
            "<cp:coreProperties xmlns:cp=\"x\" xmlns:dc=\"y\"><dc:creator>SECRETNAME</dc:creator><dc:title>Cut",
        ),
    ]);
    assert_eq!(doc.core_properties.creator.as_deref(), Some("SECRETNAME"));
    assert!(!format!("{:?}", doc.diagnostics).contains("SECRETNAME"));
}
