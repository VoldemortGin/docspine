//! 「小文件、大展开」规模回归:几 KB 的合成 `.docx` 曾能让导出 / 解析卡十秒级或吃数百 MB
//! (basedOn 长链 × 段落、海量脚注引用、字段指令 × 结果 run、`mc:Choice` 字段栈快照、公式 / Markdown 放大)。
//! 这里不靠挂钟超时(挡不住二次方回归),而是断言**有界性 / 线性**:截断计数、共享指针个数、
//! 输出长度与输入规模成线性。链遍历步数 / 注查找探测 / 快照字节 / 页眉量高次数等**内部计数**的
//! 断言在各自模块的单测里(`style.rs` / `export.rs` / `xml/document.rs` / `header.rs` / `model.rs`)。
//! 每个用例现场合成输入,1 秒量级内跑完。

use std::collections::BTreeSet;
use std::io::{Cursor, Write};

use doc_core::export::{to_html, to_markdown, to_text};
use doc_core::model::{Block, DiagnosticKind, Document, MAX_FIELD_INSTR, MAX_NOTES};
use doc_core::style::MAX_STYLE_CHAIN;
use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";
const MC_NS: &str = "http://schemas.openxmlformats.org/markup-compatibility/2006";
const M_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/math";
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

fn parse_parts(parts: &[(&str, String)]) -> Document {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in parts {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    parse_bytes(&zip.finish().unwrap().into_inner())
        .expect("parse")
        .document
}

fn body_doc(body: &str) -> String {
    format!(
        r#"<w:document xmlns:w="{W_NS}" xmlns:mc="{MC_NS}" xmlns:m="{M_NS}" xmlns:r="{REL_NS}"><w:body>{body}</w:body></w:document>"#
    )
}

fn simple(body: &str) -> Document {
    parse_parts(&[("word/document.xml", body_doc(body))])
}

fn count_of(doc: &Document, kind: DiagnosticKind, part: &str) -> Option<usize> {
    doc.diagnostics
        .iter()
        .find(|d| d.kind == kind && d.part == part)
        .map(|d| d.count)
}

fn first_para(doc: &Document) -> &doc_core::Paragraph {
    match &doc.body[0] {
        Block::Paragraph(p) => p,
        other => panic!("expected paragraph, got {other:?}"),
    }
}

/// basedOn 长链:链上超出上限的最基样式被截断(诊断计数 = 超限样式数),根上的 `numPr` 不再生效;
/// 三个文本导出与样式体检都在有限步内返回,输出里没有列表标签。
#[test]
fn long_based_on_chain_is_capped_and_reported() {
    let len = 2_000usize;
    let mut styles = format!(r#"<w:styles xmlns:w="{W_NS}">"#);
    for i in 0..len {
        let based = if i > 0 {
            format!(r#"<w:basedOn w:val="s{}"/>"#, i - 1)
        } else {
            String::new()
        };
        let num = if i == 0 {
            r#"<w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr>"#
        } else {
            ""
        };
        styles.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="s{i}"><w:name w:val="n{i}"/>{based}{num}</w:style>"#
        ));
    }
    styles.push_str("</w:styles>");
    let numbering = format!(
        r#"<w:numbering xmlns:w="{W_NS}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum><w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    );
    let para = format!(
        r#"<w:p><w:pPr><w:pStyle w:val="s{}"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#,
        len - 1
    );
    let doc = parse_parts(&[
        ("word/document.xml", body_doc(&para.repeat(100))),
        ("word/styles.xml", styles),
        ("word/numbering.xml", numbering),
    ]);
    assert_eq!(
        count_of(&doc, DiagnosticKind::StyleChainTruncated, "word/styles.xml"),
        Some(len - MAX_STYLE_CHAIN)
    );
    assert!(doc.styles.validate().is_empty());
    assert_eq!(to_text(&doc), vec!["x"; 100].join("\n"));
    let _ = (to_markdown(&doc), to_html(&doc));
}

/// 几千个**不同**的脚注引用:编号按首次引用顺序、一个不错;文末每条恰好一次。
#[test]
fn thousands_of_distinct_footnote_refs_number_in_order() {
    let f = 3_000usize;
    let refs: String = (1..=f)
        .map(|i| format!(r#"<w:r><w:footnoteReference w:id="{i}"/></w:r>"#))
        .collect();
    let notes: String = (1..=f)
        .map(|i| {
            format!(r#"<w:footnote w:id="{i}"><w:p><w:r><w:t>n{i}</w:t></w:r></w:p></w:footnote>"#)
        })
        .collect();
    let doc = parse_parts(&[
        ("word/document.xml", body_doc(&format!("<w:p>{refs}</w:p>"))),
        (
            "word/_rels/document.xml.rels",
            format!(
                r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rF" Type="{REL_NS}/footnotes" Target="footnotes.xml"/></Relationships>"#
            ),
        ),
        (
            "word/footnotes.xml",
            format!(r#"<w:footnotes xmlns:w="{W_NS}">{notes}</w:footnotes>"#),
        ),
    ]);
    let text = to_text(&doc);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 1 + f, "一行正文 + 每条注一行");
    assert!(lines[0].starts_with("[1][2][3]") && lines[0].ends_with(&format!("[{f}]")));
    assert_eq!(lines[f], format!("[{f}] n{f}"));
    // 注条目上限以内全部保留。
    assert!(f < MAX_NOTES && doc.footnotes.len() == f);
}

/// 100 KB 指令 × 2000 个结果 run:指令被截到上限,且所有 run 共享同一份(不同指针个数 = 1)。
#[test]
fn huge_field_instruction_is_capped_and_shared_across_result_runs() {
    let instr = format!("PAGE {}", "x".repeat(100_000));
    let results = r#"<w:r><w:t>a</w:t></w:r>"#.repeat(2_000);
    let doc = simple(&format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>{instr}</w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r>{results}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#
    ));
    let fields: Vec<&str> = first_para(&doc)
        .runs
        .iter()
        .filter_map(|r| r.field.as_deref())
        .collect();
    assert_eq!(fields.len(), 2_000);
    let distinct: BTreeSet<*const u8> = fields.iter().map(|f| f.as_ptr()).collect();
    assert_eq!(distinct.len(), 1, "结果 run 应共享同一份指令");
    assert!(fields[0].len() <= MAX_FIELD_INSTR);
    assert_eq!(
        count_of(
            &doc,
            DiagnosticKind::FieldInstrTruncated,
            "word/document.xml"
        ),
        Some(1)
    );
}

/// 深字段栈(每帧指令接近上限)× 大量 `mc:Choice`:快照只拷贝指针(字节数的精确上界在
/// `xml/document.rs` 单测);这里断言结果仍正确——落选 Choice 回滚后字段指令不被污染、不超上限。
#[test]
fn many_mc_choices_over_a_deep_field_stack_keep_field_state_intact() {
    let instr = "y".repeat(MAX_FIELD_INSTR - 50);
    let begin = r#"<w:r><w:fldChar w:fldCharType="begin"/></w:r>"#;
    let separate = r#"<w:r><w:fldChar w:fldCharType="separate"/></w:r>"#;
    let instr_run = |t: &str| format!("<w:r><w:instrText>{t}</w:instrText></w:r>");
    // 39 层外层字段都已进入结果区,最内层仍在指令区(会被试解析追加指令)。
    let mut body = String::new();
    for _ in 0..39 {
        body.push_str(&format!("{begin}{}{separate}", instr_run(&instr)));
    }
    body.push_str(&format!("{begin}{}", instr_run(&instr)));
    body.push_str(
        &format!(
            r#"<mc:AlternateContent><mc:Choice Requires="w14">{}</mc:Choice></mc:AlternateContent>"#,
            instr_run("zzzz")
        )
        .repeat(300),
    );
    body.push_str(&format!("{separate}<w:r><w:t>r</w:t></w:r>"));
    let doc = simple(&format!("<w:p>{body}</w:p>"));
    let field = first_para(&doc)
        .runs
        .iter()
        .find_map(|r| r.field.as_deref())
        .expect("结果 run 带字段指令");
    assert!(field.len() <= MAX_FIELD_INSTR);
    assert!(!field.contains('z'), "落选 Choice 追加的指令必须回滚");
}

/// 公式:大量并列 `m:d` 输出与输入成线性(每个恰好 `(a)`),不因线性化反复复制而放大。
#[test]
fn math_linearization_output_is_linear_in_input() {
    let n = 3_000usize;
    let one = r#"<m:d><m:e><m:r><m:t>a</m:t></m:r></m:e></m:d>"#;
    let doc = simple(&format!("<w:p><m:oMath>{}</m:oMath></w:p>", one.repeat(n)));
    assert_eq!(to_text(&doc), "(a)".repeat(n));
}

/// Markdown 转义至多把每个字符翻倍(线性放大),不论内容多“特殊”。
#[test]
fn markdown_escape_expansion_is_at_most_double() {
    let evil = "[](<>*_`&~\\#-".repeat(5_000);
    let doc = simple(&format!(
        "<w:p><w:r><w:t>{}</w:t></w:r></w:p>",
        evil.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
    ));
    let md = to_markdown(&doc);
    assert!(
        md.len() <= 2 * evil.len() + 2,
        "{} vs {}",
        md.len(),
        evil.len()
    );
}
