//! Markdown 导出的文本转义:文档内容绝不能在 Markdown 里形成链接 / 图片 / 行内 HTML / 强调 /
//! 代码段 / 块级标记(否则 `http`/`https`/`mailto` 的 scheme 白名单形同虚设)。
//! 直接用模型构造文档(不经 XML),再对输出做最小的“反向检查”。

use doc_core::export::to_markdown;
use doc_core::model::{
    Block, Cell, Document, NoteKind, Paragraph, Picture, Row, RunSegment, Table, TextRun,
};

fn run(text: &str) -> TextRun {
    TextRun::from_text(text)
}

fn linked(text: &str, url: &str) -> TextRun {
    TextRun {
        link_target: Some(url.to_string()),
        ..run(text)
    }
}

fn para(runs: Vec<TextRun>) -> Paragraph {
    Paragraph {
        runs,
        ..Paragraph::default()
    }
}

fn doc_of(paras: Vec<Paragraph>) -> Document {
    Document {
        body: paras.into_iter().map(Block::Paragraph).collect(),
        ..Document::default()
    }
}

fn md_of_text(text: &str) -> String {
    to_markdown(&doc_of(vec![para(vec![run(text)])]))
}

/// 某字符在输出里**未被转义**(前面反斜杠个数为偶数)的位置数。
fn unescaped(md: &str, ch: char) -> usize {
    let chars: Vec<char> = md.chars().collect();
    (0..chars.len())
        .filter(|&i| {
            chars[i] == ch && chars[..i].iter().rev().take_while(|&&c| c == '\\').count() % 2 == 0
        })
        .count()
}

/// 反向检查:去掉我们自己生成的 `<br>` 后,输出里不得有未转义的 `<`;
/// 链接 / 图片语法 `](` 的未转义个数恰为 `links`。
fn assert_inert(md: &str, links: usize) {
    let md = md.replace("<br>", "");
    assert_eq!(unescaped(&md, '<'), 0, "未转义的 `<`:{md}");
    assert_eq!(
        md.match_indices("](")
            .filter(|(i, _)| md[..*i].chars().rev().take_while(|&c| c == '\\').count() % 2 == 0)
            .count(),
        links,
        "未转义的 `](` 个数不符:{md}"
    );
}

/// 审查的绕过用例:https 链接文字里的 `\](javascript:…)` 经 CommonMark 会解析出指向 `javascript:` 的链接。
#[test]
fn backslash_in_link_text_cannot_break_out_of_the_link() {
    let d = doc_of(vec![para(vec![linked(
        "a\\](javascript:alert(1))",
        "http://ok.example/",
    )])]);
    let md = to_markdown(&d);
    // 反斜杠先转义:`\` -> `\\`,随后 `]` `(` `)` 各自转义;唯一的未转义 `](` 是我们自己的链接。
    assert_eq!(
        md,
        "[a\\\\\\]\\(javascript:alert\\(1\\)\\)](http://ok.example/)"
    );
    assert_inert(&md, 1);
}

#[test]
fn paragraph_text_cannot_inject_links_images_or_html() {
    for evil in [
        "[x](javascript:alert(1))",
        "![x](http://evil.example/a.png)",
        "<script>alert(1)</script>",
        "<img src=x onerror=alert(1)>",
        "[ref]: http://evil.example/",
        "`code` and *em* and _em_ and ~~del~~",
        "&lt;b&gt; &#60;i&#62;",
    ] {
        let md = md_of_text(evil);
        assert_inert(&md, 0);
        assert_eq!(unescaped(&md, '['), 0, "{md}");
        assert_eq!(unescaped(&md, '*'), 0, "{md}");
        assert_eq!(unescaped(&md, '_'), 0, "{md}");
        assert_eq!(unescaped(&md, '`'), 0, "{md}");
        assert_eq!(unescaped(&md, '&'), 0, "{md}");
        assert_eq!(unescaped(&md, '~'), 0, "{md}");
    }
}

#[test]
fn exact_escapes_for_common_specials() {
    assert_eq!(md_of_text("[x](y)"), "\\[x\\]\\(y\\)");
    assert_eq!(md_of_text("<b>"), "\\<b\\>");
    assert_eq!(md_of_text("a*b_c`d"), "a\\*b\\_c\\`d");
    assert_eq!(md_of_text("back\\slash"), "back\\\\slash");
    // 普通文字不被改动。
    assert_eq!(md_of_text("Hello, 世界 123"), "Hello, 世界 123");
}

#[test]
fn line_start_block_markers_are_escaped() {
    assert_eq!(md_of_text("# not a heading"), "\\# not a heading");
    assert_eq!(md_of_text("> not a quote"), "\\> not a quote");
    assert_eq!(md_of_text("- not a list"), "\\- not a list");
    assert_eq!(md_of_text("+ not a list"), "\\+ not a list");
    assert_eq!(md_of_text("1. not a list"), "1\\. not a list");
    assert_eq!(md_of_text("2) not a list"), "2\\) not a list");
    assert_eq!(md_of_text("==="), "\\===");
    assert_eq!(md_of_text("~~~"), "\\~\\~\\~");
    // 4 个以上前导空格会成缩进代码块:去掉前导空白。
    assert_eq!(md_of_text("    code"), "code");
    // 非行首的 `#` / `-` / 数字加点不动。
    assert_eq!(md_of_text("a # b - c 1. d"), "a # b - c 1. d");
}

/// run 之间的行首判定:前一个 run 只有空白时,后一个 run 的 `#` 仍在行首。
#[test]
fn line_start_is_tracked_across_runs_and_breaks() {
    let d = doc_of(vec![para(vec![run("  "), run("# h")])]);
    assert_eq!(to_markdown(&d), "  \\# h");
    let mut r = run("x");
    r.segments
        .push(RunSegment::Break(doc_core::model::BreakKind::Line));
    r.segments.push(RunSegment::Text("# y".into()));
    assert_eq!(to_markdown(&doc_of(vec![para(vec![r])])), "x\n\\# y");
    // 前一个 run 有正文时,后一个 run 开头的 `#` 不在行首,不转义。
    let d = doc_of(vec![para(vec![run("a"), run("# b")])]);
    assert_eq!(to_markdown(&d), "a# b");
}

#[test]
fn link_url_cannot_be_terminated_early() {
    let md = to_markdown(&doc_of(vec![para(vec![linked(
        "t",
        "http://ok.example/a b)(c)<d>\\e",
    )])]));
    assert_eq!(md, "[t](http://ok.example/a%20b%29%28c%29%3Cd%3E%5Ce)");
    assert_inert(&md, 1);
}

#[test]
fn non_whitelisted_link_scheme_outputs_escaped_text_only() {
    let md = to_markdown(&doc_of(vec![para(vec![linked(
        "click",
        "javascript:alert(1)",
    )])]));
    assert_eq!(md, "click");
}

#[test]
fn image_alt_is_escaped_and_cannot_close_the_image() {
    let mut r = run("");
    r.segments.clear();
    r.pictures.push(Picture {
        media_name: Some("image1.png".into()),
        alt: Some("x](javascript:alert(1)) <b>".into()),
        ..Picture::default()
    });
    let md = to_markdown(&doc_of(vec![para(vec![r])]));
    assert_eq!(
        md,
        "![x\\]\\(javascript:alert\\(1\\)\\) \\<b\\>](image1.png)"
    );
    assert_inert(&md, 1);
}

#[test]
fn headings_and_list_text_are_escaped() {
    let mut p = para(vec![run("[x](javascript:1)")]);
    p.ppr.outline_lvl = Some(0);
    let md = to_markdown(&doc_of(vec![p]));
    assert_eq!(md, "# \\[x\\]\\(javascript:1\\)");
    assert_inert(&md, 0);
}

fn cell(text: &str) -> Cell {
    Cell {
        blocks: vec![Block::Paragraph(para(vec![run(text)]))],
        ..Cell::default()
    }
}

#[test]
fn table_cell_pipes_newlines_and_markup_are_neutralized() {
    let mut multiline = run("a|b");
    multiline
        .segments
        .push(RunSegment::Break(doc_core::model::BreakKind::Line));
    multiline
        .segments
        .push(RunSegment::Text("[x](javascript:1)".into()));
    let t = Table {
        rows: vec![
            Row {
                cells: vec![cell("h1"), cell("<b>h2</b>")],
                ..Row::default()
            },
            Row {
                cells: vec![
                    Cell {
                        blocks: vec![Block::Paragraph(para(vec![multiline]))],
                        ..Cell::default()
                    },
                    cell("a\\|b"),
                ],
                ..Row::default()
            },
        ],
        ..Table::default()
    };
    let md = to_markdown(&Document {
        body: vec![Block::Table(t)],
        ..Document::default()
    });
    let lines: Vec<&str> = md.lines().collect();
    assert_eq!(lines[0], "| h1 | \\<b\\>h2\\</b\\> |");
    assert_eq!(lines[1], "| --- | --- |");
    // 单元格 `|` 转义、换行规整为 `<br>`,链接语法被中和;`a\|b` 里的反斜杠先转义再转义竖线。
    assert_eq!(
        lines[2],
        "| a\\|b<br>\\[x\\]\\(javascript:1\\) | a\\\\\\|b |"
    );
    assert_inert(&md, 0);
    // 表格行的单元格数不被内容里的 `|` 撑破:按未转义的 `|` 数。
    assert_eq!(unescaped(lines[2], '|'), 3);
}

#[test]
fn footnote_content_is_escaped() {
    let mut d = doc_of(vec![para(vec![TextRun {
        segments: vec![RunSegment::NoteRef {
            kind: NoteKind::Footnote,
            id: 1,
        }],
        ..TextRun::default()
    }])]);
    d.footnotes.insert(
        1,
        vec![Block::Paragraph(para(vec![run("[x](javascript:1) <i>")]))],
    );
    let md = to_markdown(&d);
    assert!(
        md.contains("[^1]: \\[x\\]\\(javascript:1\\) \\<i\\>"),
        "{md}"
    );
    assert_inert(&md, 0);
}
