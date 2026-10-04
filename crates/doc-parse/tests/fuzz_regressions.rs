//! cargo-fuzz 发现的 panic 的回归测试(`fuzz/` 目录;任何输入只许 `Ok`/`Err`,不许 panic)。
//! 每个用例现场构造 `document.xml` 再打成最小 zip,不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_parse::parse_bytes;
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

fn parse_document_xml(xml: &str) -> doc_core::Result<doc_parse::ParsedDoc> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        zip.start_file("word/document.xml", opts)
            .expect("start_file");
        zip.write_all(xml.as_bytes()).expect("write");
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner())
}

/// `w:shd@w:fill` 里 6 字节的非 ASCII 串:`Color::from_hex` 曾在非字符边界切片 panic。
#[test]
fn non_ascii_hex_fill_in_tcpr_does_not_panic() {
    let xml =
        "<w:document xmlns:w=\"http://schemas.openxmlformats.org/wordprocessingml/2006/main\">\
        <w:body><w:tbl><w:tr><w:tc><w:tcPr><w:shd w:val=\"clear\" w:fill=\"DD\u{FFFD}D\"/></w:tcPr>\
        <w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr></w:tbl></w:body></w:document>";
    let parsed = parse_document_xml(xml).expect("lenient parse");
    assert!(doc_core::export::to_text(&parsed.document).contains('x'));
}

// ---- 以下为「修订 / 限额」类问题的最小触发输入(见 fuzz/ 多部件 target):
// 任何输入只许 Ok / Err,三个文本导出器也不许 panic / 卡死 / 分配爆炸。

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

/// 按 `(部件名, 内容)` 打成 zip 再解析。
fn parse_parts(parts: &[(&str, String)]) -> doc_core::Result<doc_parse::ParsedDoc> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in parts {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body.as_bytes()).unwrap();
    }
    parse_bytes(&zip.finish().unwrap().into_inner())
}

fn body_doc(body: &str) -> String {
    format!(r#"<w:document xmlns:w="{W_NS}"><w:body>{body}</w:body></w:document>"#)
}

fn export_all(doc: &doc_core::Document) {
    let _ = (
        doc_core::export::to_text(doc),
        doc_core::export::to_markdown(doc),
        doc_core::export::to_html(doc),
    );
}

/// `w:tblGridChange` 嵌套的结束标签曾让表格与 body 提前结束,其后正文全丢。
#[test]
fn tbl_grid_change_does_not_truncate_body() {
    let xml = body_doc(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="1"/><w:tblGridChange w:id="0"><w:tblGrid><w:gridCol w:w="2"/></w:tblGrid></w:tblGridChange></w:tblGrid>
        <w:tr><w:tc><w:p/></w:tc></w:tr></w:tbl><w:p><w:r><w:t>tail</w:t></w:r></w:p>"#,
    );
    let doc = parse_parts(&[("word/document.xml", xml)]).unwrap().document;
    assert!(doc_core::export::to_text(&doc).contains("tail"));
}

/// `w:pPrChange` 里的旧样式 / 编号不得覆盖现值。
#[test]
fn ppr_change_does_not_override_current_properties() {
    let xml = body_doc(
        r#"<w:p><w:pPr><w:pStyle w:val="A"/><w:pPrChange w:id="1"><w:pPr><w:pStyle w:val="B"/></w:pPr></w:pPrChange></w:pPr></w:p>"#,
    );
    let doc = parse_parts(&[("word/document.xml", xml)]).unwrap().document;
    let Some(doc_core::Block::Paragraph(p)) = doc.body.first() else {
        panic!("paragraph")
    };
    assert_eq!(p.style.as_deref(), Some("A"));
}

/// 无 `tblGrid`、首格 `gridSpan` 40 亿:钳到 63 列,导出立即返回。
#[test]
fn huge_grid_span_is_clamped_and_exports_return() {
    let xml = body_doc(
        r#"<w:tbl><w:tr><w:tc><w:tcPr><w:gridSpan w:val="4000000000"/></w:tcPr><w:p/></w:tc></w:tr></w:tbl>"#,
    );
    let doc = parse_parts(&[("word/document.xml", xml)]).unwrap().document;
    let doc_core::Block::Table(t) = &doc.body[0] else {
        panic!("table")
    };
    assert_eq!(t.col_count(), doc_core::model::MAX_TABLE_COLS);
    export_all(&doc);
}

/// `w:start` = 9e18 配字母 / 罗马格式:曾 capacity overflow / 近似死循环。编号标签在 PDF 映射
/// 里经 `ListCounters::advance` 展开(文本导出不展开),这里直接驱动计数引擎(连推三次,覆盖自增溢出)。
#[test]
fn extreme_numbering_start_labels_return() {
    for fmt in ["lowerLetter", "upperLetter", "lowerRoman", "upperRoman"] {
        let numbering = format!(
            r#"<w:numbering xmlns:w="{W_NS}"><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0">
            <w:start w:val="9000000000000000000"/><w:numFmt w:val="{fmt}"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum>
            <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
        );
        let xml = body_doc(
            r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>a</w:t></w:r></w:p>
            <w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>b</w:t></w:r></w:p>"#,
        );
        let doc = parse_parts(&[
            ("word/document.xml", xml),
            ("word/numbering.xml", numbering),
        ])
        .unwrap()
        .document;
        export_all(&doc);
        let mut counters = doc_core::ListCounters::new();
        for _ in 0..3 {
            let _ = counters.advance(&doc.numbering, 1, 0);
        }
    }
}

// ---- 「小文件、大展开」类(规模回归的最小输入;计数 / 线性断言见 `scale_regressions.rs`):
// 这里只钉“解析 + 三个导出器在有限时间内返回、不 panic”。

/// basedOn 长链(含成环)× 多段落:样式级编号 / 标题识别每段落沿链走曾是二次方。
#[test]
fn long_and_cyclic_based_on_chains_return() {
    let mut styles = format!(r#"<w:styles xmlns:w="{W_NS}">"#);
    for i in 0..300 {
        styles.push_str(&format!(
            r#"<w:style w:type="paragraph" w:styleId="s{i}"><w:basedOn w:val="s{}"/></w:style>"#,
            (i + 1) % 300 // 环
        ));
    }
    styles.push_str("</w:styles>");
    let body =
        r#"<w:p><w:pPr><w:pStyle w:val="s0"/></w:pPr><w:r><w:t>x</w:t></w:r></w:p>"#.repeat(50);
    let doc = parse_parts(&[
        ("word/document.xml", body_doc(&body)),
        ("word/styles.xml", styles),
    ])
    .unwrap()
    .document;
    export_all(&doc);
}

/// 一个字段指令 10 KB、结果 run 数百,外加大量 `mc:Choice`:曾按 run 克隆指令 / 按 Choice 深拷贝字段栈。
#[test]
fn big_field_instruction_with_many_results_and_choices_returns() {
    let instr = "x".repeat(10_000);
    let xml = body_doc(&format!(
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText>{instr}</w:instrText></w:r>{}<w:r><w:fldChar w:fldCharType="separate"/></w:r>{}<w:r><w:fldChar w:fldCharType="end"/></w:r></w:p>"#,
        r#"<mc:AlternateContent xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"><mc:Choice Requires="w14"/></mc:AlternateContent>"#.repeat(200),
        r#"<w:r><w:t>a</w:t></w:r>"#.repeat(500)
    ));
    let doc = parse_parts(&[("word/document.xml", xml)]).unwrap().document;
    export_all(&doc);
}

/// 数千个不同脚注引用:编号登记曾是线性扫描(二次方)。
#[test]
fn many_distinct_footnote_references_return() {
    let n = 2_000;
    let refs: String = (1..=n)
        .map(|i| format!(r#"<w:r><w:footnoteReference w:id="{i}"/></w:r>"#))
        .collect();
    let notes: String = (1..=n)
        .map(|i| format!(r#"<w:footnote w:id="{i}"><w:p/></w:footnote>"#))
        .collect();
    let doc = parse_parts(&[
        ("word/document.xml", body_doc(&format!("<w:p>{refs}</w:p>"))),
        (
            "word/footnotes.xml",
            format!(r#"<w:footnotes xmlns:w="{W_NS}">{notes}</w:footnotes>"#),
        ),
    ])
    .unwrap()
    .document;
    export_all(&doc);
}

/// 多节继承同一页眉部件 + 公式结构 + Markdown 特殊字符:一次性覆盖新增的线性化 / 转义路径。
#[test]
fn math_structures_and_markdown_specials_return() {
    let m = r#"<m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:nary><m:naryPr><m:chr m:val="∑"/></m:naryPr><m:sub><m:r><m:t>i=1</m:t></m:r></m:sub><m:sup><m:r><m:t>n</m:t></m:r></m:sup><m:e><m:d><m:dPr><m:begChr m:val="["/><m:endChr m:val=""/></m:dPr><m:e><m:m><m:mr><m:e><m:r><m:t>a</m:t></m:r></m:e></m:mr></m:m></m:e></m:d></m:e></m:nary></m:oMath>"#;
    let sections = r#"<w:p><w:pPr><w:sectPr/></w:pPr></w:p>"#.repeat(50);
    let xml = body_doc(&format!(
        r#"<w:p>{m}<w:r><w:t>[x](javascript:1) &lt;b&gt; # - 1. |</w:t></w:r></w:p>{sections}"#
    ));
    let doc = parse_parts(&[("word/document.xml", xml)]).unwrap().document;
    export_all(&doc);
}
