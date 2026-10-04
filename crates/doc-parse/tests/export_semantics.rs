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

// ============================================================ 任务 C:编号标签 / 超链接 / 图片

const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// 带关系表与额外部件的合成 docx:`rels` = `(rId, 类型后缀, Target, 是否外部)`。
fn build(body: &str, rels: &[(&str, &str, &str, bool)], parts: &[(&str, String)]) -> Document {
    let rels_xml = format!(
        r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#,
        rels.iter()
            .map(|(id, ty, target, ext)| {
                let mode = if *ext {
                    r#" TargetMode="External""#
                } else {
                    ""
                };
                let target = target
                    .replace('&', "&amp;")
                    .replace('"', "&quot;")
                    .replace('<', "&lt;");
                format!(r#"<Relationship Id="{id}" Type="{REL_NS}/{ty}" Target="{target}"{mode}/>"#)
            })
            .collect::<String>()
    );
    let doc = body_doc(body);
    let mut all: Vec<(&str, &str)> = vec![
        ("word/document.xml", &doc),
        ("word/_rels/document.xml.rels", &rels_xml),
    ];
    all.extend(parts.iter().map(|(n, x)| (*n, x.as_str())));
    parse_parts(&all)
}

/// 编号部件:abstract 0 = 十进制多级(`%1.` / `%1.%2` / `(%3)` 小写字母);
/// abstract 1 = 项目符号两级;abstract 2 = `%1.` + `%2.` 两级十进制(嵌套有序列表);
/// num 1 / 2 → abstract 0(不同 numId 各自从头计),num 3 → 1,num 4 → 2。
fn numbering_xml() -> String {
    format!(
        r#"<w:numbering xmlns:w="{W_NS}">
<w:abstractNum w:abstractNumId="0">
 <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
 <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1.%2"/></w:lvl>
 <w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="(%3)"/></w:lvl>
</w:abstractNum>
<w:abstractNum w:abstractNumId="1">
 <w:lvl w:ilvl="0"><w:numFmt w:val="bullet"/><w:lvlText w:val="&#x2022;"/></w:lvl>
 <w:lvl w:ilvl="1"><w:numFmt w:val="bullet"/><w:lvlText w:val="o"/></w:lvl>
</w:abstractNum>
<w:abstractNum w:abstractNumId="2">
 <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl>
 <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%2."/></w:lvl>
</w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="3"><w:abstractNumId w:val="1"/></w:num>
<w:num w:numId="4"><w:abstractNumId w:val="2"/></w:num>
</w:numbering>"#
    )
}

fn li(num_id: u32, ilvl: u32, text: &str) -> String {
    format!(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num_id}"/></w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"#
    )
}

fn plain(text: &str) -> String {
    format!("<w:p><w:r><w:t>{text}</w:t></w:r></w:p>")
}

fn with_numbering(body: &str) -> Document {
    build(body, &[], &[("word/numbering.xml", numbering_xml())])
}

#[test]
fn multilevel_numbered_list_labels_in_all_exports() {
    let body = li(1, 0, "A")
        + &li(1, 1, "B")
        + &li(1, 1, "C")
        + &li(1, 2, "D")
        + &li(1, 0, "E")
        + &li(1, 1, "F");
    let doc = with_numbering(&body);
    assert_eq!(to_text(&doc), "1. A\n1.1 B\n1.2 C\n(a) D\n2. E\n2.1 F");
    // Markdown:`1.` 是合法有序列表语法,直接用;`1.1` / `(a)` 不是,作文字前缀成独立段。
    assert_eq!(
        to_markdown(&doc),
        // `(a)` 标签前缀是文字,经统一 Markdown 转义后括号带反斜杠(原先不转义括号)。
        "1. A\n\n1.1 B\n\n1.2 C\n\n\\(a\\) D\n\n2. E\n\n2.1 F"
    );
    assert_eq!(
        to_html(&doc),
        "<p>1. A</p>\n<p>1.1 B</p>\n<p>1.2 C</p>\n<p>(a) D</p>\n<p>2. E</p>\n<p>2.1 F</p>"
    );
}

#[test]
fn bullet_lists_use_dash_in_markdown_and_indent_by_level() {
    let body = li(3, 0, "x") + &li(3, 0, "y") + &li(3, 1, "z") + &li(3, 0, "w");
    let doc = with_numbering(&body);
    assert_eq!(to_markdown(&doc), "- x\n- y\n    - z\n- w");
    assert_eq!(to_text(&doc), "\u{2022} x\n\u{2022} y\no z\n\u{2022} w");
    assert!(to_html(&doc).starts_with("<p>\u{2022} x</p>\n<p>\u{2022} y</p>\n<p>o z</p>"));
}

#[test]
fn nested_ordered_list_markdown_is_indented() {
    let body = li(4, 0, "A") + &li(4, 1, "B") + &li(4, 1, "C") + &li(4, 0, "D");
    let doc = with_numbering(&body);
    assert_eq!(to_markdown(&doc), "1. A\n    1. B\n    2. C\n2. D");
    assert_eq!(to_text(&doc), "1. A\n1. B\n2. C\n2. D");
}

#[test]
fn list_level_jump_does_not_over_indent() {
    // 从 0 级直接跳到 2 级:缩进不得超过父级内容 +1 层(否则 Markdown 当成缩进代码块)。
    let body = li(3, 0, "a") + &li(3, 1, "b");
    let md = to_markdown(&with_numbering(&li(1, 2, "deep")));
    // 标签前缀走统一 Markdown 转义:括号带反斜杠。
    assert_eq!(md, "\\(a\\) deep");
    assert_eq!(to_markdown(&with_numbering(&body)), "- a\n    - b");
}

#[test]
fn different_num_id_restarts_numbering() {
    let body = li(1, 0, "a") + &li(1, 0, "b") + &li(2, 0, "c") + &li(1, 0, "d");
    let doc = with_numbering(&body);
    assert_eq!(to_text(&doc), "1. a\n2. b\n1. c\n3. d");
    assert_eq!(to_markdown(&doc), "1. a\n2. b\n1. c\n3. d");
}

#[test]
fn list_counters_continue_across_table_cells() {
    let body = li(1, 0, "a")
        + &format!(
            "<w:tbl><w:tr><w:tc>{}</w:tc><w:tc>{}</w:tc></w:tr></w:tbl>",
            li(1, 0, "b"),
            plain("plain")
        )
        + &li(1, 0, "c");
    let doc = with_numbering(&body);
    assert_eq!(to_text(&doc), "1. a\n2. b\tplain\n3. c");
    assert_eq!(
        to_markdown(&doc),
        // 单元格内的标签前缀 `2. ` 位于单元格(行)首,按统一转义写成 `2\.`。
        "1. a\n\n| 2\\. b | plain |\n| --- | --- |\n\n3. c"
    );
    let html = to_html(&doc);
    assert!(
        html.contains("<td>2. b</td>") && html.ends_with("<p>3. c</p>"),
        "{html}"
    );
}

#[test]
fn list_counters_are_scoped_per_header_footnote_and_body() {
    let hdr = format!(r#"<w:hdr xmlns:w="{W_NS}">{}</w:hdr>"#, li(1, 0, "head"));
    let notes = format!(
        r#"<w:footnotes xmlns:w="{W_NS}"><w:footnote w:id="1">{}</w:footnote></w:footnotes>"#,
        li(1, 0, "note")
    );
    let body = li(1, 0, "a")
        + &li(1, 0, "b")
        + r#"<w:p><w:r><w:footnoteReference w:id="1"/></w:r></w:p>"#
        + r#"<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr>"#;
    let doc = build(
        &body,
        &[
            ("rIdH", "header", "header1.xml", false),
            ("rIdN", "footnotes", "footnotes.xml", false),
        ],
        &[
            ("word/numbering.xml", numbering_xml()),
            ("word/header1.xml", hdr),
            ("word/footnotes.xml", notes),
        ],
    );
    let txt = to_text(&doc);
    // 页眉、脚注各自从 1 起,不吃正文的计数,正文也不被它们推进。
    assert!(
        txt.contains("[Header: default]\n1. head\n1. a\n2. b"),
        "{txt}"
    );
    assert!(txt.contains("[1] 1. note"), "{txt}");
}

#[test]
fn list_numbering_in_text_box_does_not_disturb_body() {
    let tb = format!(
        r#"<w:p><w:r><w:t>anchor</w:t><w:pict><v:shape xmlns:v="urn:schemas-microsoft-com:vml"><v:textbox><w:txbxContent>{}</w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"#,
        li(1, 0, "boxed")
    );
    let doc = with_numbering(&(li(1, 0, "a") + &tb + &li(1, 0, "b")));
    assert_eq!(to_text(&doc), "1. a\nanchor\n1. boxed\n2. b");
}

#[test]
fn empty_list_paragraph_advances_counter_without_label_line() {
    let body = li(1, 0, "a") + &li(1, 0, "") + &li(1, 0, "c");
    let doc = with_numbering(&body);
    assert_eq!(to_text(&doc), "1. a\n\n3. c");
    // Markdown 跳过空段,不打断列表链。
    assert_eq!(to_markdown(&doc), "1. a\n3. c");
}

#[test]
fn numbered_heading_keeps_label_in_markdown_and_html() {
    let styles = style("H1", "heading 1", None, "");
    let body = r#"<w:p><w:pPr><w:pStyle w:val="H1"/><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>Intro</w:t></w:r></w:p>"#;
    let doc = build(
        body,
        &[],
        &[
            ("word/numbering.xml", numbering_xml()),
            (
                "word/styles.xml",
                format!(r#"<w:styles xmlns:w="{W_NS}">{styles}</w:styles>"#),
            ),
        ],
    );
    assert_eq!(to_markdown(&doc), "# 1.1 Intro");
    assert!(to_html(&doc).contains("<h1>1.1 Intro</h1>"));
}

fn link(rel: &str, text: &str) -> String {
    format!(r#"<w:hyperlink r:id="{rel}"><w:r><w:t>{text}</w:t></w:r></w:hyperlink>"#)
}

fn link_doc(targets: &[(&str, &str)], body: &str) -> Document {
    let rels: Vec<(&str, &str, &str, bool)> = targets
        .iter()
        .map(|(id, t)| (*id, "hyperlink", *t, true))
        .collect();
    build(body, &rels, &[])
}

#[test]
fn hyperlinks_become_markdown_and_html_links() {
    let body = format!(
        "<w:p><w:r><w:t>see </w:t></w:r>{}<w:r><w:t> now</w:t></w:r></w:p>",
        link("r1", "the site")
    );
    let doc = link_doc(&[("r1", "https://example.com/a?b=1&c=2")], &body);
    assert_eq!(to_text(&doc), "see the site now");
    assert_eq!(
        to_markdown(&doc),
        "see [the site](https://example.com/a?b=1&c=2) now"
    );
    assert_eq!(
        to_html(&doc),
        "<p>see <a href=\"https://example.com/a?b=1&amp;c=2\">the site</a> now</p>"
    );
}

#[test]
fn link_split_over_runs_is_one_link_and_escapes_applied() {
    let body = r#"<w:p><w:hyperlink r:id="r1"><w:r><w:t>a]b</w:t></w:r><w:r><w:t>[c</w:t></w:r></w:hyperlink></w:p>"#;
    let doc = link_doc(&[("r1", "https://e.com/x y/(z)")], body);
    assert_eq!(
        to_markdown(&doc),
        "[a\\]b\\[c](https://e.com/x%20y/%28z%29)"
    );
    assert_eq!(
        to_html(&doc),
        "<p><a href=\"https://e.com/x y/(z)\">a]b[c</a></p>"
    );
}

#[test]
fn only_http_https_mailto_become_links() {
    for (target, linked) in [
        ("http://a.com", true),
        ("HTTPS://a.com", true),
        ("mailto:me@a.com", true),
        ("javascript:alert(1)", false),
        ("  JaVaScRiPt:alert(1)", false),
        ("java\tscript:alert(1)", false),
        ("file:///etc/passwd", false),
        ("data:text/html,x", false),
        ("ftp://a.com", false),
        ("../relative.docx", false),
    ] {
        let doc = link_doc(
            &[("r1", target)],
            &format!("<w:p>{}</w:p>", link("r1", "t")),
        );
        let (md, html) = (to_markdown(&doc), to_html(&doc));
        if linked {
            assert!(md.starts_with("[t]("), "{target}: {md}");
            assert!(html.contains("<a href="), "{target}: {html}");
        } else {
            assert_eq!(md, "t", "{target}");
            assert_eq!(html, "<p>t</p>", "{target}");
        }
        assert_eq!(to_text(&doc), "t");
    }
}

#[test]
fn anchor_links_are_plain_text() {
    let body =
        r#"<w:p><w:hyperlink w:anchor="_Toc1"><w:r><w:t>jump</w:t></w:r></w:hyperlink></w:p>"#;
    let doc = build(body, &[], &[]);
    assert_eq!(to_markdown(&doc), "jump");
    assert_eq!(to_html(&doc), "<p>jump</p>");
}

#[test]
fn html_href_attribute_is_escaped() {
    let doc = link_doc(
        &[("r1", "https://a.com/\"onmouseover=\"x")],
        &format!("<w:p>{}</w:p>", link("r1", "t")),
    );
    let html = to_html(&doc);
    assert!(
        html.contains("href=\"https://a.com/&quot;onmouseover=&quot;x\""),
        "{html}"
    );
}

fn pic(alt_attrs: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><wp:docPr id="1" name="Picture 1" {alt_attrs}/>
        <a:graphic><a:graphicData><pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture"><pic:blipFill><a:blip r:embed="rImg"/></pic:blipFill></pic:pic></a:graphicData></a:graphic>
        </wp:inline></w:drawing></w:r>"#
    )
}

fn pic_doc(alt_attrs: &str, before: &str) -> Document {
    build(
        &format!("<w:p>{before}{}</w:p>", pic(alt_attrs)),
        &[("rImg", "image", "media/image1.png", false)],
        &[("word/media/image1.png", "x".to_string())],
    )
}

#[test]
fn pictures_with_alt_in_all_exports() {
    let doc = pic_doc(
        r#"descr="A red cat" title="ignored""#,
        "<w:r><w:t>fig: </w:t></w:r>",
    );
    assert_eq!(to_text(&doc), "fig: [图片: A red cat]");
    assert_eq!(to_markdown(&doc), "fig: ![A red cat](image1.png)");
    assert_eq!(
        to_html(&doc),
        "<p>fig: <img alt=\"A red cat\" src=\"image1.png\"></p>"
    );
}

#[test]
fn picture_alt_falls_back_to_title_then_empty() {
    let doc = pic_doc(r#"title="Only title""#, "");
    assert_eq!(to_markdown(&doc), "![Only title](image1.png)");
    let doc = pic_doc("", "");
    // 无 alt:纯文本不输出(段落为空被跳过),Markdown / HTML 仍带图。
    assert_eq!(to_text(&doc), "");
    assert_eq!(to_markdown(&doc), "![](image1.png)");
    assert_eq!(to_html(&doc), "<p><img alt=\"\" src=\"image1.png\"></p>");
}

#[test]
fn picture_alt_is_escaped_and_whitespace_collapsed() {
    let doc = pic_doc(r#"descr="a ] &quot;b&quot;&#10;c &lt;d&gt;""#, "");
    // 统一 Markdown 转义后 `<` `>` 也要转义(原先只转义 `[` `]`)。
    assert_eq!(to_markdown(&doc), "![a \\] \"b\" c \\<d\\>](image1.png)");
    assert_eq!(
        to_html(&doc),
        "<p><img alt=\"a ] &quot;b&quot; c &lt;d&gt;\" src=\"image1.png\"></p>"
    );
    assert_eq!(to_text(&doc), "[图片: a ] \"b\" c <d>]");
}

#[test]
fn picture_in_table_cell_and_link_nesting() {
    let body = format!(
        r#"<w:tbl><w:tr><w:tc><w:p>{}</w:p></w:tc></w:tr></w:tbl>"#,
        pic(r#"descr="logo""#)
    );
    let doc = build(
        &body,
        &[("rImg", "image", "media/image1.png", false)],
        &[("word/media/image1.png", "x".to_string())],
    );
    assert_eq!(to_text(&doc), "[图片: logo]");
    assert!(
        to_markdown(&doc).contains("| ![logo](image1.png) |"),
        "{}",
        to_markdown(&doc)
    );
    assert!(to_html(&doc).contains("<td><img alt=\"logo\" src=\"image1.png\"></td>"));
}

// ============================================================ 任务 E:gridBefore / gridAfter / 孤立 vMerge

fn tc(text: &str, tcpr: &str) -> String {
    format!(r#"<w:tc><w:tcPr>{tcpr}</w:tcPr><w:p><w:r><w:t>{text}</w:t></w:r></w:p></w:tc>"#)
}

fn tr(trpr: &str, cells: &[String]) -> String {
    format!("<w:tr><w:trPr>{trpr}</w:trPr>{}</w:tr>", cells.concat())
}

fn tbl(cols: usize, rows: &[String]) -> String {
    let grid: String = (0..cols).map(|_| r#"<w:gridCol w:w="2000"/>"#).collect();
    format!(
        "<w:tbl><w:tblGrid>{grid}</w:tblGrid>{}</w:tbl>",
        rows.concat()
    )
}

const RESTART: &str = r#"<w:vMerge w:val="restart"/>"#;
const CONT: &str = "<w:vMerge/>";

fn table_doc(cols: usize, rows: &[String]) -> Document {
    parse_parts(&[("word/document.xml", &body_doc(&tbl(cols, rows)))])
}

#[test]
fn grid_before_and_after_are_parsed_and_clamped() {
    let doc = table_doc(
        3,
        &[
            tr(
                r#"<w:gridBefore w:val="1"/><w:gridAfter w:val="2"/>"#,
                &[tc("x", "")],
            ),
            tr(
                r#"<w:gridBefore w:val="99999999999"/><w:gridAfter w:val="-3"/>"#,
                &[tc("y", "")],
            ),
            tr(
                r#"<w:trPrChange w:id="1"><w:trPr><w:gridBefore w:val="2"/></w:trPr></w:trPrChange>"#,
                &[tc("z", "")],
            ),
        ],
    );
    let doc_core::Block::Table(t) = &doc.body[0] else {
        panic!("table")
    };
    assert_eq!((t.rows[0].grid_before, t.rows[0].grid_after), (1, 2));
    assert_eq!(
        (t.rows[1].grid_before, t.rows[1].grid_after),
        (doc_core::model::MAX_TABLE_COLS as u32, 0),
        "超大值钳到 MAX_TABLE_COLS,非法负数按缺失"
    );
    assert_eq!(
        (t.rows[2].grid_before, t.rows[2].grid_after),
        (0, 0),
        "修订前旧属性不生效"
    );
}

#[test]
fn vmerge_below_grid_before_row_aligns_by_grid_column() {
    // 行 1 在网格第 0 列留空:其第一个单元格实际在第 1 列,正好接在行 0 的 vMerge restart 之下。
    let doc = table_doc(
        3,
        &[
            tr("", &[tc("a", ""), tc("b", RESTART), tc("c", "")]),
            tr(r#"<w:gridBefore w:val="1"/>"#, &[tc("", CONT), tc("d", "")]),
        ],
    );
    assert_eq!(
        to_html(&doc),
        "<table>\n<tr>\n<td>a</td>\n<td rowspan=\"2\">b</td>\n<td>c</td>\n</tr>\n\
         <tr>\n<td></td>\n<td>d</td>\n</tr>\n</table>"
    );
}

#[test]
fn grid_before_wider_than_one_uses_colspan_filler() {
    let doc = table_doc(
        4,
        &[
            tr("", &[tc("a", ""), tc("b", ""), tc("c", ""), tc("d", "")]),
            tr(
                r#"<w:gridBefore w:val="2"/><w:gridAfter w:val="1"/>"#,
                &[tc("x", "")],
            ),
        ],
    );
    assert_eq!(
        to_html(&doc),
        "<table>\n<tr>\n<td>a</td>\n<td>b</td>\n<td>c</td>\n<td>d</td>\n</tr>\n\
         <tr>\n<td colspan=\"2\"></td>\n<td>x</td>\n</tr>\n</table>"
    );
}

#[test]
fn grid_before_and_after_in_markdown_pipe_table_and_plain_text() {
    let doc = table_doc(
        3,
        &[
            tr("", &[tc("a", ""), tc("b", ""), tc("c", "")]),
            tr(r#"<w:gridBefore w:val="1"/>"#, &[tc("x", ""), tc("y", "")]),
            tr(r#"<w:gridAfter w:val="1"/>"#, &[tc("p", ""), tc("q", "")]),
        ],
    );
    assert_eq!(
        to_markdown(&doc),
        "| a | b | c |\n| --- | --- | --- |\n|  | x | y |\n| p | q |  |"
    );
    assert_eq!(to_text(&doc), "a\tb\tc\n\tx\ty\np\tq");
}

#[test]
fn orphan_vmerge_continue_keeps_its_content() {
    // 上方不是 restart(普通格 / 首行 / 另一个孤立 continue):按普通单元格输出,内容不丢。
    let doc = table_doc(
        2,
        &[
            tr("", &[tc("a", ""), tc("lonely-first-row", CONT)]),
            tr("", &[tc("b", ""), tc("orphan", CONT)]),
            tr("", &[tc("c", ""), tc("orphan-chain", CONT)]),
        ],
    );
    let html = to_html(&doc);
    for kept in ["lonely-first-row", "orphan", "orphan-chain"] {
        assert!(html.contains(&format!("<td>{kept}</td>")), "{kept}: {html}");
    }
    assert!(!html.contains("rowspan"), "{html}");
    assert!(to_markdown(&doc).contains("<td>orphan</td>"));
    assert!(to_text(&doc).contains("orphan-chain"));
}

#[test]
fn proper_vmerge_chain_still_spans_and_swallows_continuations() {
    let doc = table_doc(
        2,
        &[
            tr("", &[tc("m", RESTART), tc("1", "")]),
            tr("", &[tc("", CONT), tc("2", "")]),
            tr("", &[tc("", CONT), tc("3", "")]),
            tr("", &[tc("n", ""), tc("4", "")]),
        ],
    );
    assert_eq!(
        to_html(&doc),
        "<table>\n<tr>\n<td rowspan=\"3\">m</td>\n<td>1</td>\n</tr>\n<tr>\n<td>2</td>\n</tr>\n\
         <tr>\n<td>3</td>\n</tr>\n<tr>\n<td>n</td>\n<td>4</td>\n</tr>\n</table>"
    );
}
