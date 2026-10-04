//! 样式级编号(`w:style > w:pPr > w:numPr`):Word「多级列表链接到标题样式」的标准做法。
//! 级别由 `numbering.xml` 里 `w:lvl > w:pStyle` 反向关联决定。每个用例现场构造 `.docx` 再解析。

use std::io::{Cursor, Write};

use doc_core::export::{to_html, to_markdown, to_text};
use doc_core::Document;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

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

/// `styles` = `<w:style>` 列表;`numbering` = `w:numbering` 子元素;`body` = `w:body` 内容。
fn build(styles: &str, numbering: &str, body: &str) -> Document {
    let doc = format!(r#"<w:document xmlns:w="{W_NS}"><w:body>{body}</w:body></w:document>"#);
    let styles = format!(r#"<w:styles xmlns:w="{W_NS}">{styles}</w:styles>"#);
    let numbering = format!(r#"<w:numbering xmlns:w="{W_NS}">{numbering}</w:numbering>"#);
    parse_parts(&[
        ("word/document.xml", &doc),
        ("word/styles.xml", &styles),
        ("word/numbering.xml", &numbering),
    ])
}

/// 段落样式:`name` 为显示名,`based` 为 basedOn,`num_pr` 为 `w:numPr` 内容(空串 = 无)。
fn style(id: &str, name: &str, based: Option<&str>, num_pr: &str) -> String {
    let based = based
        .map(|b| format!(r#"<w:basedOn w:val="{b}"/>"#))
        .unwrap_or_default();
    let ppr = if num_pr.is_empty() {
        String::new()
    } else {
        format!("<w:pPr><w:numPr>{num_pr}</w:numPr></w:pPr>")
    };
    format!(
        r#"<w:style w:type="paragraph" w:styleId="{id}"><w:name w:val="{name}"/>{based}{ppr}</w:style>"#
    )
}

fn num_id(n: u32) -> String {
    format!(r#"<w:numId w:val="{n}"/>"#)
}

fn p(style_id: &str, own_num_pr: &str, text: &str) -> String {
    let st = if style_id.is_empty() {
        String::new()
    } else {
        format!(r#"<w:pStyle w:val="{style_id}"/>"#)
    };
    let np = if own_num_pr.is_empty() {
        String::new()
    } else {
        format!("<w:numPr>{own_num_pr}</w:numPr>")
    };
    format!("<w:p><w:pPr>{st}{np}</w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>")
}

/// abstract 0:三级章节号 `%1` / `%1.%2` / `%1.%2.%3`,各级 `pStyle` 链接到 Heading1..3;num 1 → 它。
/// abstract 1:两级 `A.` / `A.a`(无 pStyle 链接);num 2 → 它。
fn numbering() -> String {
    r#"<w:abstractNum w:abstractNumId="0">
 <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:pStyle w:val="Heading1"/><w:lvlText w:val="%1"/></w:lvl>
 <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:pStyle w:val="Heading2"/><w:lvlText w:val="%1.%2"/></w:lvl>
 <w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:pStyle w:val="Heading3"/><w:lvlText w:val="%1.%2.%3"/></w:lvl>
</w:abstractNum>
<w:abstractNum w:abstractNumId="1">
 <w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="upperLetter"/><w:lvlText w:val="%1."/></w:lvl>
 <w:lvl w:ilvl="1"><w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%1.%2"/></w:lvl>
 <w:lvl w:ilvl="2"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="[%3]"/></w:lvl>
</w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
<w:num w:numId="2"><w:abstractNumId w:val="1"/></w:num>"#
        .to_string()
}

/// 标题样式只写 `numId`(不写 ilvl),级别全靠 `lvl@pStyle`。
fn heading_styles() -> String {
    style("Heading1", "heading 1", None, &num_id(1))
        + &style("Heading2", "heading 2", Some("Heading1"), &num_id(1))
        + &style("Heading3", "heading 3", Some("Heading2"), &num_id(1))
}

#[test]
fn heading_styles_linked_to_multilevel_list_number_and_restart() {
    let body = p("Heading1", "", "A")
        + &p("Heading2", "", "a")
        + &p("Heading3", "", "x")
        + &p("Heading2", "", "b")
        + &p("Heading1", "", "B")
        + &p("Heading2", "", "c")
        + &p("", "", "body");
    let doc = build(&heading_styles(), &numbering(), &body);
    assert_eq!(
        to_text(&doc),
        "1 A\n1.1 a\n1.1.1 x\n1.2 b\n2 B\n2.1 c\nbody"
    );
    // Markdown 标题带章节号,与直接 numPr 的 `# 1.1 Intro` 同一呈现。
    assert_eq!(
        to_markdown(&doc),
        "# 1 A\n\n## 1.1 a\n\n### 1.1.1 x\n\n## 1.2 b\n\n# 2 B\n\n## 2.1 c\n\nbody"
    );
    let html = to_html(&doc);
    assert!(
        html.contains("<h1>1 A</h1>") && html.contains("<h2>2.1 c</h2>"),
        "{html}"
    );
}

#[test]
fn explicit_num_id_zero_cancels_style_numbering() {
    let body =
        p("Heading1", "", "A") + &p("Heading1", &num_id(0), "unnumbered") + &p("Heading1", "", "B");
    let doc = build(&heading_styles(), &numbering(), &body);
    assert_eq!(to_text(&doc), "1 A\nunnumbered\n2 B");
}

#[test]
fn derived_style_inherits_numbering_through_based_on() {
    // MyH 没有自己的 numPr / pStyle 链接:沿 basedOn 找到 Heading2 的链接 → 二级。
    let styles = heading_styles() + &style("MyH", "My Heading", Some("Heading2"), "");
    let body = p("Heading1", "", "A") + &p("MyH", "", "mine");
    let doc = build(&styles, &numbering(), &body);
    assert_eq!(to_text(&doc), "1 A\n1.1 mine");
}

#[test]
fn derived_style_with_num_id_zero_cancels_inherited_numbering() {
    let styles = heading_styles() + &style("Plain", "Plain", Some("Heading1"), &num_id(0));
    let body = p("Heading1", "", "A") + &p("Plain", "", "off");
    let doc = build(&styles, &numbering(), &body);
    assert_eq!(to_text(&doc), "1 A\noff");
}

/// 样式 numPr 的 `ilvl` 与 `lvl@pStyle` 不一致:按 ECMA-376 §17.9.23(`pStyle`):样式含编号定义时,
/// numPr 里的级别被忽略,级别由 `lvl@pStyle` 决定 —— 链接胜出。
#[test]
fn lvl_p_style_link_beats_style_ilvl() {
    let styles = style(
        "Heading2",
        "heading 2",
        None,
        &(r#"<w:ilvl w:val="0"/>"#.to_string() + &num_id(1)),
    );
    let doc = build(&styles, &numbering(), &p("Heading2", "", "x"));
    // Heading2 被链到 ilvl 1,首项:父级计数 0 起 → 取 start=1 记 `1.1`。
    assert_eq!(to_text(&doc), "1.1 x");
}

/// 没有 `lvl@pStyle` 链接时才用样式 numPr 自带的 `ilvl`(缺省 0)。
#[test]
fn style_ilvl_used_when_no_p_style_link() {
    let styles = style(
        "Deep",
        "Deep",
        None,
        &(r#"<w:ilvl w:val="2"/>"#.to_string() + &num_id(2)),
    ) + &style("Top", "Top", None, &num_id(2));
    let body = p("Top", "", "t") + &p("Deep", "", "d");
    let doc = build(&styles, &numbering(), &body);
    assert_eq!(to_text(&doc), "A. t\n[1] d");
}

#[test]
fn own_num_pr_beats_style_and_own_ilvl_overrides_level() {
    let styles = heading_styles();
    let body = p("Heading1", &num_id(2), "own list")
        + &p(
            "Heading1",
            r#"<w:ilvl w:val="1"/>"#,
            "own ilvl keeps style list",
        );
    let doc = build(&styles, &numbering(), &body);
    // 段落自带 numId=2 → 用它(abstract 1,ilvl 0 = `A.`);只写 ilvl=1 → numId 仍来自样式(num 1)。
    assert_eq!(to_text(&doc), "A. own list\n1.1 own ilvl keeps style list");
}

#[test]
fn based_on_cycle_terminates() {
    let styles = style("A", "A", Some("B"), &num_id(2)) + &style("B", "B", Some("A"), "");
    let doc = build(&styles, &numbering(), &(p("A", "", "x") + &p("B", "", "y")));
    assert_eq!(to_text(&doc), "A. x\nB. y");
}

#[test]
fn default_paragraph_style_numbering_applies_without_p_style() {
    let styles = format!(
        r#"<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/><w:pPr><w:numPr>{}</w:numPr></w:pPr></w:style>"#,
        num_id(2)
    );
    let doc = build(&styles, &numbering(), &p("", "", "x"));
    assert_eq!(to_text(&doc), "A. x");
}
