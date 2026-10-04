//! 字段指令的内存放大:结果区里每个 run 共享**同一份**指令(不按 run 克隆),单个字段指令
//! 有长度上限(超出截断并记 `field-instr-truncated` 诊断)。现场合成 docx,不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_core::model::{Block, DiagnosticKind, Paragraph, MAX_FIELD_INSTR};
use doc_parse::{parse_bytes, ParsedDoc};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006""#;

fn build(body_xml: &str) -> ParsedDoc {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><w:document {NS}><w:body>{body_xml}</w:body></w:document>"#
    );
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        zip.start_file("word/document.xml", SimpleFileOptions::default())
            .expect("start_file");
        zip.write_all(doc.as_bytes()).expect("write");
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner()).expect("parse synthetic docx")
}

fn first_para(d: &ParsedDoc) -> &Paragraph {
    match &d.document.body[0] {
        Block::Paragraph(p) => p,
        other => panic!("expected paragraph, got {other:?}"),
    }
}

fn fc(kind: &str) -> String {
    format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#)
}

/// begin / 指令 / separate / `n` 个结果 run / end。
fn complex_field(instr: &str, n: usize) -> String {
    format!(
        r#"<w:p>{}<w:r><w:instrText>{instr}</w:instrText></w:r>{}{}{}</w:p>"#,
        fc("begin"),
        fc("separate"),
        r#"<w:r><w:t>a</w:t></w:r>"#.repeat(n),
        fc("end")
    )
}

fn diag_count(d: &ParsedDoc, kind: DiagnosticKind) -> Option<usize> {
    d.document
        .diagnostics
        .iter()
        .find(|x| x.kind == kind && x.part == "word/document.xml")
        .map(|x| x.count)
}

fn field_ptrs(p: &Paragraph) -> Vec<*const u8> {
    p.runs
        .iter()
        .filter_map(|r| r.field.as_deref().map(str::as_ptr))
        .collect()
}

/// 结果区里的 N 个 run 共享同一份指令(指针相等),而不是各克隆一份。
#[test]
fn complex_field_result_runs_share_one_instruction() {
    let d = build(&complex_field("PAGE \\* MERGEFORMAT", 50));
    let ptrs = field_ptrs(first_para(&d));
    assert_eq!(ptrs.len(), 50);
    assert!(
        ptrs.iter().all(|&p| p == ptrs[0]),
        "50 个 run 应共享同一份指令"
    );
}

/// `w:fldSimple` 的指令同理:每个结果 run 共享同一份。
#[test]
fn fld_simple_result_runs_share_one_instruction() {
    let runs = r#"<w:r><w:t>a</w:t></w:r>"#.repeat(30);
    let d = build(&format!(
        r#"<w:p><w:fldSimple w:instr=" PAGE ">{runs}</w:fldSimple></w:p>"#
    ));
    let ptrs = field_ptrs(first_para(&d));
    assert_eq!(ptrs.len(), 30);
    assert!(ptrs.iter().all(|&p| p == ptrs[0]));
}

/// 超长 `w:instrText`:截断到上限(仍保留首词,可被识别)并记诊断;所有结果 run 共享该份。
#[test]
fn overlong_instr_text_is_truncated_and_reported() {
    let instr = format!("PAGE {}", "x".repeat(10 * MAX_FIELD_INSTR));
    let d = build(&complex_field(&instr, 20));
    let p = first_para(&d);
    for r in &p.runs {
        let f = r.field.as_deref().expect("field");
        assert!(f.len() <= MAX_FIELD_INSTR, "len {}", f.len());
        assert!(f.starts_with("PAGE "));
    }
    assert_eq!(diag_count(&d, DiagnosticKind::FieldInstrTruncated), Some(1));
    assert_eq!(
        DiagnosticKind::FieldInstrTruncated.code(),
        "field-instr-truncated"
    );
}

/// 超长 `w:fldSimple@w:instr`:同样截断 + 诊断;多字节字符不在中间被切断(不 panic)。
#[test]
fn overlong_fld_simple_instr_is_truncated_on_char_boundary() {
    let instr = format!("PAGE {}", "汉".repeat(MAX_FIELD_INSTR));
    let d = build(&format!(
        r#"<w:p><w:fldSimple w:instr="{instr}"><w:r><w:t>a</w:t></w:r></w:fldSimple></w:p>"#
    ));
    let f = first_para(&d).runs[0].field.as_deref().expect("field");
    assert!(f.len() <= MAX_FIELD_INSTR && f.starts_with("PAGE 汉"));
    assert_eq!(diag_count(&d, DiagnosticKind::FieldInstrTruncated), Some(1));
}

/// 上限以内的指令原样保留,不记诊断。
#[test]
fn instr_within_cap_is_kept_verbatim() {
    let d = build(&complex_field("PAGE \\* roman", 2));
    assert_eq!(
        first_para(&d).runs[0].field.as_deref(),
        Some("PAGE \\* roman")
    );
    assert_eq!(diag_count(&d, DiagnosticKind::FieldInstrTruncated), None);
}
