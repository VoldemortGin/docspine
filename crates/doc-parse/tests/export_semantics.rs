//! 导出语义回归:标题识别走样式表(styleId 任意)、编号标签 / 超链接 / 图片进文本导出、
//! `gridBefore` / `gridAfter` 与孤立 `vMerge`。每个用例现场构造 `.docx` 的各部件再解析,
//! 不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_core::export::{to_html, to_markdown, to_text};
use doc_core::Document;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// 把 `w:body` 内容包成 `document.xml`。
fn body_doc(body: &str) -> String {
    format!(
        r#"<w:document xmlns:w="{W_NS}"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
  xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <w:body>{body}</w:body></w:document>"#
    )
}

/// 按 `(部件名, 内容)` 打成 zip 再解析。
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

/// 一个 `w:body` + 一份 `styles.xml`(`<w:style>` 列表原样塞入)。
fn with_styles(body: &str, styles: &str) -> Document {
    let doc = body_doc(body);
    let styles = format!(r#"<w:styles xmlns:w="{W_NS}">{styles}</w:styles>"#);
    parse_parts(&[("word/document.xml", &doc), ("word/styles.xml", &styles)])
}

fn style(id: &str, name: &str, based_on: Option<&str>, ppr: &str) -> String {
    let based = based_on
        .map(|b| format!(r#"<w:basedOn w:val="{b}"/>"#))
        .unwrap_or_default();
    format!(
        r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/>{based}<w:pPr>{ppr}</w:pPr></w:style>"#
    )
}

fn p_styled(style_id: &str, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:pStyle w:val="{style_id}"/></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
    )
}

// ============================================================ 任务 B:标题走样式表

#[test]
fn numeric_style_id_with_heading_name_is_heading() {
    let styles = style("1", "heading 1", None, "") + &style("2", "Heading 2", None, "");
    let doc = with_styles(
        &(p_styled("1", "章") + &p_styled("2", "节") + &p_styled("9", "正文")),
        &styles,
    );
    let md = to_markdown(&doc);
    assert_eq!(md, "# 章\n\n## 节\n\n正文");
    let html = to_html(&doc);
    assert!(
        html.contains("<h1>章</h1>") && html.contains("<h2>节</h2>"),
        "{html}"
    );
}

#[test]
fn localized_and_spaced_names_match_builtin_headings() {
    let styles = style("a", "标题 3", None, "")
        + &style("b", "HEADING   4", None, "")
        + &style("c", "Title", None, "")
        + &style("d", "Subtitle", None, "")
        + &style("e", "Normal Text", None, "");
    let body = p_styled("a", "A")
        + &p_styled("b", "B")
        + &p_styled("c", "C")
        + &p_styled("d", "D")
        + &p_styled("e", "E");
    let md = to_markdown(&with_styles(&body, &styles));
    assert_eq!(md, "### A\n\n#### B\n\n# C\n\n## D\n\nE");
}

#[test]
fn custom_style_based_on_heading_is_heading() {
    let styles = style("H1", "heading 1", None, "")
        + &style("Chapter", "章标题", Some("H1"), "")
        + &style("SubChapter", "小章标题", Some("Chapter"), "");
    let md = to_markdown(&with_styles(
        &(p_styled("Chapter", "一") + &p_styled("SubChapter", "二")),
        &styles,
    ));
    assert_eq!(md, "# 一\n\n# 二");
}

#[test]
fn outline_level_in_style_is_heading_even_without_heading_name() {
    let styles = style("Zh", "我的标题", None, r#"<w:outlineLvl w:val="2"/>"#)
        + &style("Child", "派生", Some("Zh"), "");
    let md = to_markdown(&with_styles(
        &(p_styled("Zh", "x") + &p_styled("Child", "y")),
        &styles,
    ));
    assert_eq!(md, "### x\n\n### y");
}

#[test]
fn direct_outline_level_beats_style() {
    let styles = style("Zh", "我的标题", None, r#"<w:outlineLvl w:val="2"/>"#);
    let body = r#"<w:p><w:pPr><w:pStyle w:val="Zh"/><w:outlineLvl w:val="0"/></w:pPr><w:r><w:t>top</w:t></w:r></w:p>
        <w:p><w:pPr><w:outlineLvl w:val="1"/></w:pPr><w:r><w:t>plain style, direct lvl</w:t></w:r></w:p>"#;
    let md = to_markdown(&with_styles(body, &styles));
    assert_eq!(md, "# top\n\n## plain style, direct lvl");
}

#[test]
fn outline_level_9_is_body_text() {
    // 直接 9 压过标题样式名;样式上的 9 压过样式名与 styleId 字面匹配。
    let styles = style(
        "Heading1",
        "heading 1",
        None,
        r#"<w:outlineLvl w:val="9"/>"#,
    ) + &style("Heading2", "heading 2", None, "");
    let body = r#"<w:p><w:pPr><w:pStyle w:val="Heading2"/><w:outlineLvl w:val="9"/></w:pPr><w:r><w:t>a</w:t></w:r></w:p>"#
        .to_string()
        + &p_styled("Heading1", "b");
    let md = to_markdown(&with_styles(&body, &styles));
    assert_eq!(md, "a\n\nb");
}

#[test]
fn levels_beyond_six_are_clamped_for_markdown_and_html() {
    let styles = style("L7", "heading 7", None, "")
        + &style("L9", "x", None, r#"<w:outlineLvl w:val="8"/>"#);
    let doc = with_styles(
        &(p_styled("L7", "seven") + &p_styled("L9", "nine")),
        &styles,
    );
    assert_eq!(to_markdown(&doc), "###### seven\n\n###### nine");
    let html = to_html(&doc);
    assert!(
        html.contains("<h6>seven</h6>") && html.contains("<h6>nine</h6>"),
        "{html}"
    );
}

#[test]
fn literal_style_id_still_works_without_styles_part() {
    let doc = parse_parts(&[(
        "word/document.xml",
        &body_doc(&(p_styled("Heading2", "h") + &p_styled("标题1", "t") + &p_styled("Title", "T"))),
    )]);
    assert_eq!(to_markdown(&doc), "## h\n\n# t\n\n# T");
    // styleId 字面匹配也在有样式表、但样式名不是标题时生效(不回归)。
    let styles = style("Heading3", "Custom Name", None, "");
    assert_eq!(
        to_markdown(&with_styles(&p_styled("Heading3", "z"), &styles)),
        "### z"
    );
}

#[test]
fn based_on_cycle_terminates() {
    let styles = style("A", "a", Some("B"), "") + &style("B", "b", Some("A"), "");
    let doc = with_styles(&(p_styled("A", "x")), &styles);
    assert_eq!(to_markdown(&doc), "x");
    // 环上带标题名的样式仍可识别。
    let styles = style("A", "heading 2", Some("B"), "") + &style("B", "b", Some("A"), "");
    assert_eq!(
        to_markdown(&with_styles(&p_styled("A", "x"), &styles)),
        "## x"
    );
}

#[test]
fn heading_recognition_applies_in_table_cells_html() {
    // 单元格里不输出标题标签(仍是单元格文本),但不得 panic、文字保留。
    let styles = style("1", "heading 1", None, "");
    let body = format!(
        "<w:tbl><w:tr><w:tc>{}</w:tc></w:tr></w:tbl>",
        p_styled("1", "cell")
    );
    let doc = with_styles(&body, &styles);
    assert!(to_html(&doc).contains("cell"));
    assert_eq!(to_text(&doc), "cell");
}
