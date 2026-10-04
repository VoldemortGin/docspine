//! Markdown 导出的文本转义:**在文档内容不能注入 Markdown 结构的前提下,转义越少越好**(给 RAG 用)。
//! 最小必要集(规则与理由见 `doc_core::export` 的 `md_escape`):
//! `[` 一律;`]` 只在链接文字 / 图片 alt;`(` `:` 只紧跟导出器生成的 `]`;`<` 只在后跟字母 / `/` / `!` / `?`;
//! `>` 只在行首;`&` 只在构成实体引用时;`_` 只在词边界;`*` 两侧都是空白时不转;反引号一律;`~` 同块两个以上;
//! `\` 只在后跟 ASCII 标点 / 换行 / 块尾;块的行首结构(`#` 标题、`- + *` 列表、只由 `- = * _` 组成的行、
//! `1.` / `1)` 有序列表、`>` 引用、表格分隔行)只在段落 / 标题续行等块里处理,**表格单元格里不做行首规则**;
//! 块首行 4 列以上的前导空白去掉,续行保留;`|` 单元格里一律、段落里出现表格分隔行时转义。
//! 直接用模型构造文档(不经 XML),断言精确输出。端到端(markdown-it 解析)验证在仓外脚本。

use doc_core::export::to_markdown;
use doc_core::model::{
    Block, BreakKind, Cell, Document, NoteKind, Paragraph, Picture, Row, RunSegment, Table, TextRun,
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

fn note_ref(id: i64) -> TextRun {
    TextRun {
        segments: vec![RunSegment::NoteRef {
            kind: NoteKind::Footnote,
            id,
        }],
        ..TextRun::default()
    }
}

/// 一个 run:文字片段之间插入换行(`w:br`)。
fn run_lines(lines: &[&str]) -> TextRun {
    let mut r = TextRun::default();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            r.segments.push(RunSegment::Break(BreakKind::Line));
        }
        r.segments.push(RunSegment::Text((*l).into()));
    }
    r
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

fn heading_md(text: &str) -> String {
    let mut p = para(vec![run(text)]);
    p.ppr.outline_lvl = Some(0);
    to_markdown(&doc_of(vec![p]))
}

fn cell(text: &str) -> Cell {
    Cell {
        blocks: vec![Block::Paragraph(para(vec![run(text)]))],
        ..Cell::default()
    }
}

/// 一行两列表格,第二行第一格是 `text`;返回该格所在的 Markdown 行。
fn cell_md(text: &str) -> String {
    let t = Table {
        rows: vec![
            Row {
                cells: vec![cell("h1"), cell("h2")],
                ..Row::default()
            },
            Row {
                cells: vec![cell(text), cell("x")],
                ..Row::default()
            },
        ],
        ..Table::default()
    };
    let md = to_markdown(&Document {
        body: vec![Block::Table(t)],
        ..Document::default()
    });
    md.lines().nth(2).unwrap_or_default().to_string()
}

/// 带一条脚注(内容 `note`)的文档:正文 `see[^1]`。返回脚注定义行。
fn note_def_md(note: &str) -> String {
    let mut d = doc_of(vec![para(vec![run("see"), note_ref(1)])]);
    d.footnotes
        .insert(1, vec![Block::Paragraph(para(vec![run(note)]))]);
    let md = to_markdown(&d);
    md.lines()
        .find(|l| l.starts_with("[^1]:"))
        .unwrap_or_default()
        .to_string()
}

// ------------------------------------------------------------------ 安全:复审列出的每个问题

/// 脚注定义 `[^1]: 内容` 在不开脚注扩展的 CommonMark 里是**链接引用定义**:内容是单个“词”
/// (`javascript:alert(1)`、`data:` URL、普通单词)或「词 + 标题」时,正文 `[^1]` 会变成链接。
/// 做法:单个词之后补一个空格 + U+200B(不是 CommonMark 空白,也不能作标题,定义因此不成立;渲染不可见);
/// 第二个词以 `"` `'` `(` 开头时转义它(不再是标题);内容以 `<` 开头时转义(不成尖括号目的地)。
#[test]
fn footnote_definition_never_parses_as_link_reference_definition() {
    assert_eq!(
        note_def_md("javascript:alert(1)"),
        "[^1]: javascript:alert(1) \u{200B}"
    );
    assert_eq!(note_def_md("Hello"), "[^1]: Hello \u{200B}");
    assert_eq!(
        note_def_md("data:text/html,x"),
        "[^1]: data:text/html,x \u{200B}"
    );
    assert_eq!(
        note_def_md("javascript:alert(1) \"t\""),
        "[^1]: javascript:alert(1) \\\"t\""
    );
    assert_eq!(note_def_md("x 'y'"), "[^1]: x \\'y'");
    assert_eq!(note_def_md("x (y)"), "[^1]: x \\(y)");
    assert_eq!(note_def_md("< x > t"), "[^1]: \\< x > t");
    // 两个以上普通词:本就不是引用定义,原样。
    assert_eq!(note_def_md("see page 3"), "[^1]: see page 3");
    // 脚注内容里的换行折成空格(否则下一行可作标题 / 目的地)。
    let mut d = doc_of(vec![para(vec![run("see"), note_ref(1)])]);
    d.footnotes.insert(
        1,
        vec![Block::Paragraph(para(vec![run_lines(&[
            "http://x", "\"t\"",
        ])]))],
    );
    assert!(
        to_markdown(&d).ends_with("[^1]: http://x \\\"t\""),
        "{}",
        to_markdown(&d)
    );
}

/// 段落里的管道表注入:出现表格分隔行时,整块的 `|` 都转义,分隔行首字符也转义。
#[test]
fn pipe_table_injection_in_paragraph_is_neutralized() {
    let d = doc_of(vec![para(vec![run_lines(&["a | b", ":-|:-", "c | d"])])]);
    assert_eq!(to_markdown(&d), "a \\| b\n\\:-\\|:-\nc \\| d");
    let d = doc_of(vec![para(vec![run_lines(&["| a | b |", "|---|---|"])])]);
    assert_eq!(to_markdown(&d), "\\| a \\| b \\|\n\\|---\\|---\\|");
    // 没有分隔行的段落里 `|` 不动。
    assert_eq!(md_of_text("a | b | c"), "a | b | c");
}

/// 标题结尾的 ` #` / ` ##` 是 ATX 结束序列,会被吃掉:转义第一个 `#`。
#[test]
fn heading_trailing_hashes_are_not_swallowed() {
    assert_eq!(heading_md("Issue #"), "# Issue \\#");
    assert_eq!(heading_md("C ##"), "# C \\##");
    assert_eq!(heading_md("##"), "# \\##");
    // 词内的 `#` 不是结束序列。
    assert_eq!(heading_md("C#"), "# C#");
}

/// 链接文字里的两个连续换行会产生空行、把链接截断:链接文字里的换行折成空格。
#[test]
fn link_text_line_breaks_fold_to_spaces() {
    let mut r = run_lines(&["x", "", "y"]);
    r.link_target = Some("https://ok.example/".into());
    let md = to_markdown(&doc_of(vec![para(vec![r])]));
    assert_eq!(md, "[x  y](https://ok.example/)");
}

/// 链接文字里含脚注标记时外层链接在 CommonMark 下失效:标记移到链接之外(与 HTML 的做法一致:
/// 先闭合链接、输出标记、再开新链接)。
#[test]
fn footnote_mark_inside_link_text_is_moved_outside() {
    let url = "https://ok.example/";
    let mut nr = note_ref(1);
    nr.link_target = Some(url.into());
    let mut d = doc_of(vec![para(vec![linked("A", url), nr, linked("B", url)])]);
    d.footnotes
        .insert(1, vec![Block::Paragraph(para(vec![run("see page 3")]))]);
    let md = to_markdown(&d);
    assert!(
        md.starts_with("[A](https://ok.example/)[^1][B](https://ok.example/)\n\n"),
        "{md}"
    );
}

/// 链接目的地里的 `&copy;` 会被 CommonMark 当实体解码成 `©`:构成实体引用的 `&` 反斜杠转义。
#[test]
fn link_url_entities_are_not_decoded() {
    let md = to_markdown(&doc_of(vec![para(vec![linked(
        "t",
        "https://e.example/?q=&copy;&a=1",
    )])]));
    assert_eq!(md, "[t](https://e.example/?q=\\&copy;&a=1)");
}

/// 链接文字里的 `\](javascript:…)`:反斜杠后跟标点要转义,`]` 在链接文字里转义,唯一的链接是导出器的。
#[test]
fn backslash_in_link_text_cannot_break_out_of_the_link() {
    let d = doc_of(vec![para(vec![linked(
        "a\\](javascript:alert(1))",
        "http://ok.example/",
    )])]);
    assert_eq!(
        to_markdown(&d),
        "[a\\\\\\](javascript:alert(1))](http://ok.example/)"
    );
}

#[test]
fn link_url_cannot_be_terminated_early() {
    let md = to_markdown(&doc_of(vec![para(vec![linked(
        "t",
        "http://ok.example/a b)(c)<d>\\e",
    )])]));
    assert_eq!(md, "[t](http://ok.example/a%20b%29%28c%29%3Cd%3E%5Ce)");
}

#[test]
fn non_whitelisted_link_scheme_outputs_text_only() {
    let md = to_markdown(&doc_of(vec![para(vec![linked(
        "click",
        "javascript:alert(1)",
    )])]));
    assert_eq!(md, "click");
}

#[test]
fn image_alt_cannot_close_the_image() {
    let mut r = run("");
    r.segments.clear();
    r.pictures.push(Picture {
        media_name: Some("image1.png".into()),
        alt: Some("x](javascript:alert(1)) <b>".into()),
        ..Picture::default()
    });
    let md = to_markdown(&doc_of(vec![para(vec![r])]));
    assert_eq!(md, "![x\\](javascript:alert(1)) \\<b>](image1.png)");
}

/// 文档内容里的链接 / 图片 / HTML / 引用定义语法都被中和(各取最小转义)。
#[test]
fn paragraph_text_cannot_inject_links_images_or_html() {
    assert_eq!(
        md_of_text("[x](javascript:alert(1))"),
        "\\[x](javascript:alert(1))"
    );
    assert_eq!(
        md_of_text("![x](http://evil.example/a.png)"),
        "!\\[x](http://evil.example/a.png)"
    );
    assert_eq!(
        md_of_text("<script>alert(1)</script>"),
        "\\<script>alert(1)\\</script>"
    );
    assert_eq!(md_of_text("<http://e.example>"), "\\<http://e.example>");
    assert_eq!(md_of_text("<!-- c -->"), "\\<!-- c -->");
    assert_eq!(md_of_text("<?php ?>"), "\\<?php ?>");
    assert_eq!(md_of_text("[ref]: http://evil/"), "\\[ref]: http://evil/");
    assert_eq!(
        md_of_text("&lt;b&gt; &#60;i&#62;"),
        "\\&lt;b\\&gt; \\&#60;i\\&#62;"
    );
    assert_eq!(md_of_text("&#x3C;b>"), "\\&#x3C;b>");
}

// ------------------------------------------------------------------ 跨 run 的上下文

/// 一个 run 以 `&` 结尾、下一个 run 以 `#x3C;` 开头:整块拼完再转义,照样识别实体引用。
#[test]
fn entity_split_across_runs_is_escaped() {
    let d = doc_of(vec![para(vec![run("&"), run("#x3C;b>")])]);
    assert_eq!(to_markdown(&d), "\\&#x3C;b>");
    let d = doc_of(vec![para(vec![run("q=&"), run("copy;")])]);
    assert_eq!(to_markdown(&d), "q=\\&copy;");
}

/// 导出器生成的 `]`(脚注标记)后紧跟下一个 run 的 `(` / `:`:转义,免得拼成内联链接 / 引用定义。
#[test]
fn generated_bracket_followed_by_paren_or_colon_is_escaped() {
    let mut d = doc_of(vec![para(vec![
        run("see"),
        note_ref(1),
        run("(javascript:alert(1))"),
    ])]);
    d.footnotes
        .insert(1, vec![Block::Paragraph(para(vec![run("see page 3")]))]);
    assert!(to_markdown(&d).starts_with("see[^1]\\(javascript:alert(1))\n\n"));
    let mut d = doc_of(vec![para(vec![note_ref(1), run(": javascript:alert(1)")])]);
    d.footnotes
        .insert(1, vec![Block::Paragraph(para(vec![run("see page 3")]))]);
    assert!(to_markdown(&d).starts_with("[^1]\\: javascript:alert(1)\n\n"));
    // 文档内容自己的 `](` 不是导出器生成的:`(` 不转义(`[` 已转义,构不成链接)。
    assert_eq!(md_of_text("a](b)"), "a](b)");
    // 文档内容的反斜杠紧挨导出器生成的 `[`:转义,免得吃掉脚注标记。
    let mut d = doc_of(vec![para(vec![run("a\\"), note_ref(1)])]);
    d.footnotes
        .insert(1, vec![Block::Paragraph(para(vec![run("see page 3")]))]);
    assert!(to_markdown(&d).starts_with("a\\\\[^1]\n\n"));
}

/// 行首判定跨 run:前一个 run 只有空白时,后一个 run 的 `#` 仍在行首。
#[test]
fn line_start_is_tracked_across_runs_and_breaks() {
    let d = doc_of(vec![para(vec![run("  "), run("# h")])]);
    assert_eq!(to_markdown(&d), "  \\# h");
    let d = doc_of(vec![para(vec![run_lines(&["x", "# y"])])]);
    assert_eq!(to_markdown(&d), "x\n\\# y");
    // 前一个 run 有正文时,后一个 run 开头的 `#` 不在行首,不转义。
    let d = doc_of(vec![para(vec![run("a"), run("# b")])]);
    assert_eq!(to_markdown(&d), "a# b");
}

// ------------------------------------------------------------------ 最小转义集:正例与反例

#[test]
fn brackets_parens_and_angle_brackets() {
    assert_eq!(md_of_text("[1]"), "\\[1]");
    assert_eq!(md_of_text("a]b"), "a]b");
    assert_eq!(md_of_text("f(x) = g(a, b)"), "f(x) = g(a, b)");
    assert_eq!(md_of_text("<b>"), "\\<b>");
    assert_eq!(md_of_text("</a>"), "\\</a>");
    assert_eq!(md_of_text("a < b"), "a < b");
    assert_eq!(md_of_text("a<3"), "a<3");
    assert_eq!(md_of_text("3 > 2"), "3 > 2");
    assert_eq!(md_of_text("a < b && c > d"), "a < b && c > d");
}

#[test]
fn ampersand_only_before_entity_references() {
    assert_eq!(md_of_text("AT&T and R&D"), "AT&T and R&D");
    assert_eq!(md_of_text("&copy;"), "\\&copy;");
    assert_eq!(md_of_text("&#169;"), "\\&#169;");
    assert_eq!(md_of_text("&#xA9;"), "\\&#xA9;");
    assert_eq!(md_of_text("&amp"), "&amp");
    assert_eq!(md_of_text("& ;"), "& ;");
}

#[test]
fn emphasis_characters() {
    assert_eq!(md_of_text("snake_case_name"), "snake_case_name");
    assert_eq!(md_of_text("a_b"), "a_b");
    assert_eq!(md_of_text("_em_"), "\\_em\\_");
    assert_eq!(md_of_text("__init__"), "\\_\\_init\\_\\_");
    assert_eq!(md_of_text("x*y"), "x\\*y");
    assert_eq!(md_of_text("a * b"), "a * b");
    assert_eq!(md_of_text("*em*"), "\\*em\\*");
    assert_eq!(md_of_text("`code`"), "\\`code\\`");
    assert_eq!(md_of_text("~10 km"), "~10 km");
    assert_eq!(md_of_text("~~del~~"), "\\~\\~del\\~\\~");
    assert_eq!(md_of_text("~a~"), "\\~a\\~");
}

#[test]
fn backslash_only_before_punctuation_or_at_block_end() {
    assert_eq!(md_of_text("path\\dir"), "path\\dir");
    assert_eq!(
        md_of_text("C:\\Program Files\\app"),
        "C:\\Program Files\\app"
    );
    assert_eq!(md_of_text("a\\*"), "a\\\\\\*");
    assert_eq!(md_of_text("end\\"), "end\\\\");
    let d = doc_of(vec![para(vec![run_lines(&["a\\", "b"])])]);
    assert_eq!(to_markdown(&d), "a\\\\\nb");
}

#[test]
fn line_start_block_markers() {
    assert_eq!(md_of_text("# not a heading"), "\\# not a heading");
    assert_eq!(md_of_text("###### six"), "\\###### six");
    assert_eq!(md_of_text("#hashtag"), "#hashtag");
    assert_eq!(md_of_text("####### seven"), "####### seven");
    assert_eq!(md_of_text("> not a quote"), "\\> not a quote");
    assert_eq!(md_of_text("- not a list"), "\\- not a list");
    assert_eq!(md_of_text("-5 degrees"), "-5 degrees");
    assert_eq!(md_of_text("+ not a list"), "\\+ not a list");
    assert_eq!(md_of_text("+3%"), "+3%");
    assert_eq!(md_of_text("* not a list"), "\\* not a list");
    assert_eq!(md_of_text("1. not a list"), "1\\. not a list");
    assert_eq!(md_of_text("2) not a list"), "2\\) not a list");
    assert_eq!(md_of_text("1.5 litres"), "1.5 litres");
    assert_eq!(md_of_text("1.2.3"), "1.2.3");
    assert_eq!(md_of_text("==="), "\\===");
    assert_eq!(md_of_text("---"), "\\---");
    assert_eq!(md_of_text("- - -"), "\\- - -");
    assert_eq!(md_of_text("___"), "\\_\\_\\_");
    assert_eq!(md_of_text("~~~"), "\\~\\~\\~");
    assert_eq!(md_of_text("=A1"), "=A1");
    // 非行首的 `#` / `-` / 数字加点不动。
    assert_eq!(md_of_text("a # b - c 1. d"), "a # b - c 1. d");
    // 段落续行也是行首(列表 / 标题 / setext 能打断或接续段落)。
    let d = doc_of(vec![para(vec![run_lines(&["Title", "====="])])]);
    assert_eq!(to_markdown(&d), "Title\n\\=====");
    let d = doc_of(vec![para(vec![run_lines(&["x", "- y", "1. z"])])]);
    assert_eq!(to_markdown(&d), "x\n\\- y\n1\\. z");
}

/// 4 列以上前导空白:块的首行会成缩进代码块,去掉;段落续行不会成代码块,保留缩进;
/// 空行之后的行是新块的首行,同样去掉。
#[test]
fn indentation_only_stripped_on_block_first_lines() {
    assert_eq!(md_of_text("    code"), "code");
    assert_eq!(md_of_text("\tcode"), "code");
    assert_eq!(md_of_text("   three"), "   three");
    let d = doc_of(vec![para(vec![run_lines(&[
        "def f(x):",
        "    return x",
        "\tpass",
    ])])]);
    assert_eq!(to_markdown(&d), "def f(x):\n    return x\n\tpass");
    let d = doc_of(vec![para(vec![run_lines(&["x", "", "    code"])])]);
    assert_eq!(to_markdown(&d), "x\n\ncode");
}

/// 表格单元格:只做行内转义 + `|`,不做任何行首规则(财务表格的 `-5` / `+3%` / `=A1` / `1.` 原样)。
#[test]
fn table_cells_have_no_line_start_rules() {
    assert_eq!(cell_md("-5"), "| -5 | x |");
    assert_eq!(cell_md("+3%"), "| +3% | x |");
    assert_eq!(cell_md("=A1"), "| =A1 | x |");
    assert_eq!(cell_md("1."), "| 1. | x |");
    assert_eq!(cell_md("- 2"), "| - 2 | x |");
    assert_eq!(cell_md("# x"), "| # x | x |");
    assert_eq!(cell_md("    x"), "|     x | x |");
    assert_eq!(cell_md("a|b"), "| a\\|b | x |");
    assert_eq!(cell_md("C:\\dir\\"), "| C:\\dir\\\\ | x |");
    assert_eq!(cell_md("[x](javascript:1)"), "| \\[x](javascript:1) | x |");
    assert_eq!(cell_md("<b>"), "| \\<b> | x |");
    // 单元格里的换行写成 `<br>`(不是空白):`*<br>*` 会成强调,`*` 要转义。
    let t = Table {
        rows: vec![Row {
            cells: vec![Cell {
                blocks: vec![Block::Paragraph(para(vec![run_lines(&["*", "*"])]))],
                ..Cell::default()
            }],
            ..Row::default()
        }],
        ..Table::default()
    };
    let md = to_markdown(&Document {
        body: vec![Block::Table(t)],
        ..Document::default()
    });
    assert_eq!(md.lines().next(), Some("| \\*<br>\\* |"));
}

#[test]
fn table_cell_breaks_become_br_and_pipes_are_escaped() {
    let mut multiline = run_lines(&["a|b", "[x](javascript:1)"]);
    multiline.link_target = None;
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
    assert_eq!(lines[0], "| h1 | \\<b>h2\\</b> |");
    assert_eq!(lines[1], "| --- | --- |");
    // 换行规整为导出器生成的 `<br>`(不被转义);`a\|b` 的反斜杠后跟标点先转义,再转义竖线。
    assert_eq!(lines[2], "| a\\|b<br>\\[x](javascript:1) | a\\\\\\|b |");
}

#[test]
fn headings_and_list_text() {
    assert_eq!(heading_md("[x](javascript:1)"), "# \\[x](javascript:1)");
    // 标题行内不做行首规则(`# - x` 仍是标题)。
    assert_eq!(heading_md("- x"), "# - x");
    assert_eq!(heading_md("1. x"), "# 1. x");
}

#[test]
fn footnote_content_is_escaped_inline_only() {
    assert_eq!(
        note_def_md("[x](javascript:1) <i> see"),
        "[^1]: \\[x](javascript:1) \\<i> see"
    );
    // 脚注内容不在行首:`- x` / `# x` 不转义。
    assert_eq!(note_def_md("- x y"), "[^1]: - x y");
}
