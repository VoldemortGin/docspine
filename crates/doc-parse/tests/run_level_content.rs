//! 带正文的 run 级元素:`w:ruby`(基字不丢)、`w:dir` / `w:bdo`(双向文本容器)、
//! `w:altChunk`(外部内容块,不解析、只计数)。现场合成 `document.xml`,不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_core::export::{to_html, to_markdown, to_text};
use doc_core::Document;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn parse_body(body: &str) -> Document {
    let doc = format!(
        r#"<w:document xmlns:w="{W_NS}" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><w:body>{body}</w:body></w:document>"#
    );
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        zip.start_file("word/document.xml", SimpleFileOptions::default())
            .expect("start_file");
        zip.write_all(doc.as_bytes()).expect("write");
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner()).expect("parse").document
}

fn run(t: &str) -> String {
    format!("<w:r><w:t>{t}</w:t></w:r>")
}

/// 注音:`w:rt` 放注音,`w:rubyBase` 放基字。
fn ruby(rt: &str, base: &str) -> String {
    format!(
        "<w:ruby><w:rubyPr><w:rubyAlign w:val=\"center\"/></w:rubyPr><w:rt>{}</w:rt><w:rubyBase>{}</w:rubyBase></w:ruby>",
        run(rt),
        run(base)
    )
}

// ---------------------------------------------------------------- ruby

#[test]
fn ruby_base_text_is_kept_and_reading_is_not_duplicated() {
    let body = format!("<w:p><w:r>{}</w:r>{}</w:p>", ruby("かん", "漢"), run("字"));
    let doc = parse_body(&body);
    assert_eq!(to_text(&doc), "漢字");
    assert_eq!(to_markdown(&doc), "漢字");
    assert_eq!(to_html(&doc), "<p>漢字</p>");
}

#[test]
fn ruby_keeps_position_inside_a_run_with_text_around_it() {
    let body = format!(
        "<w:p><w:r><w:t>前</w:t>{}<w:t>后</w:t></w:r></w:p>",
        ruby("ほん", "本")
    );
    assert_eq!(to_text(&parse_body(&body)), "前本后");
}

#[test]
fn ruby_with_multiple_base_runs_and_in_containers() {
    let multi = "<w:ruby><w:rt><w:r><w:t>xx</w:t></w:r></w:rt><w:rubyBase><w:r><w:t>A</w:t></w:r><w:r><w:t>B</w:t></w:r></w:rubyBase></w:ruby>";
    let body = format!(
        "<w:p><w:r>{multi}</w:r></w:p>\
         <w:p><w:hyperlink w:anchor=\"x\"><w:r>{}</w:r></w:hyperlink></w:p>\
         <w:tbl><w:tr><w:tc><w:p><w:r>{}</w:r></w:p></w:tc></w:tr></w:tbl>",
        ruby("a", "C"),
        ruby("b", "D")
    );
    assert_eq!(to_text(&parse_body(&body)), "AB\nC\nD");
}

#[test]
fn ruby_malformed_and_deeply_nested_do_not_panic() {
    // 缺 rubyBase / 空 ruby / 未闭合。
    for xml in [
        "<w:p><w:r><w:ruby><w:rt><w:r><w:t>x</w:t></w:r></w:rt></w:ruby><w:t>ok</w:t></w:r></w:p>",
        "<w:p><w:r><w:ruby/><w:t>ok</w:t></w:r></w:p>",
        "<w:p><w:r><w:ruby><w:rubyBase><w:r><w:t>ok",
    ] {
        let txt = to_text(&parse_body(xml));
        assert!(txt.is_empty() || txt.contains("ok"), "{xml}: {txt:?}");
    }
    // 深嵌套:ruby > rubyBase > r > ruby > ...(超过 MAX_NEST_DEPTH 的子树静默跳过)。
    let n = 5_000;
    let body = format!(
        "<w:p><w:r>{}{}{}</w:r></w:p><w:p>{}</w:p>",
        "<w:ruby><w:rubyBase><w:r>".repeat(n),
        "deep",
        "</w:r></w:rubyBase></w:ruby>".repeat(n),
        run("after")
    );
    assert!(to_text(&parse_body(&body)).ends_with("after"));
}

// ---------------------------------------------------------------- dir / bdo

#[test]
fn dir_and_bdo_runs_are_kept_in_order() {
    let body = format!(
        "<w:p>{}<w:dir w:val=\"rtl\">{}<w:bdo w:val=\"ltr\">{}</w:bdo></w:dir>{}</w:p>",
        run("a"),
        run("שלום"),
        run("b"),
        run("c")
    );
    assert_eq!(to_text(&parse_body(&body)), "aשלוםbc");
}

#[test]
fn dir_bdo_inside_other_run_containers_and_cells() {
    let body = format!(
        "<w:p><w:hyperlink w:anchor=\"x\"><w:bdo w:val=\"rtl\">{}</w:bdo></w:hyperlink><w:ins w:id=\"1\"><w:dir w:val=\"rtl\">{}</w:dir></w:ins><w:smartTag><w:dir w:val=\"ltr\">{}</w:dir></w:smartTag></w:p>\
         <w:tbl><w:tr><w:tc><w:p><w:dir w:val=\"rtl\">{}</w:dir></w:p></w:tc></w:tr></w:tbl>",
        run("1"),
        run("2"),
        run("3"),
        run("4")
    );
    assert_eq!(to_text(&parse_body(&body)), "123\n4");
}

#[test]
fn dir_bdo_deep_nesting_and_malformed_do_not_panic() {
    let n = 5_000;
    let body = format!(
        "<w:p>{}{}{}</w:p><w:p>{}</w:p>",
        "<w:dir w:val=\"rtl\"><w:bdo w:val=\"rtl\">".repeat(n),
        run("deep"),
        "</w:bdo></w:dir>".repeat(n),
        run("after")
    );
    assert!(to_text(&parse_body(&body)).ends_with("after"));
    let _ = to_text(&parse_body("<w:p><w:dir><w:r><w:t>unclosed"));
}

// ---------------------------------------------------------------- altChunk

#[test]
fn alt_chunk_is_counted_not_imported_and_does_not_eat_neighbours() {
    let body = format!(
        "<w:p>{}</w:p><w:altChunk r:id=\"rId9\"/><w:p>{}</w:p><w:altChunk r:id=\"rId10\"></w:altChunk>\
         <w:tbl><w:tr><w:tc><w:altChunk r:id=\"rId11\"/><w:p>{}</w:p></w:tc></w:tr></w:tbl>\
         <w:sdt><w:sdtContent><w:altChunk r:id=\"rId12\"/></w:sdtContent></w:sdt>",
        run("one"),
        run("two"),
        run("cell")
    );
    let doc = parse_body(&body);
    assert_eq!(doc.alt_chunk_count, 4);
    assert_eq!(to_text(&doc), "one\ntwo\ncell");
}

#[test]
fn documents_without_alt_chunk_report_zero() {
    assert_eq!(
        parse_body(&format!("<w:p>{}</w:p>", run("x"))).alt_chunk_count,
        0
    );
}
