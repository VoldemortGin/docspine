//! `mc:AlternateContent` 里的复杂字段(`w:fldChar`):字段栈只能被**选中的那一个分支**推进,
//! 落选分支(试解析过的 Choice)的字段状态必须回滚,否则后续正文会被误标成字段指令 / 结果。
//! 现场合成 docx(纯 zip + 手写 XML)。

use std::io::{Cursor, Write};

use doc_core::export::to_text;
use doc_core::model::{Block, Paragraph};
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

fn fc(kind: &str) -> String {
    format!(r#"<w:r><w:fldChar w:fldCharType="{kind}"/></w:r>"#)
}

fn instr(text: &str) -> String {
    format!(r#"<w:r><w:instrText xml:space="preserve"> {text} </w:instrText></w:r>"#)
}

fn t(text: &str) -> String {
    format!(r#"<w:r><w:t>{text}</w:t></w:r>"#)
}

/// 完整字段:begin / 指令 / separate / 缓存结果 / end;`result` 为空则无结果区文字。
fn field(name: &str, result: &str) -> String {
    let res = if result.is_empty() {
        String::new()
    } else {
        t(result)
    };
    format!(
        "{}{}{}{res}{}",
        fc("begin"),
        instr(name),
        fc("separate"),
        fc("end")
    )
}

/// `before` + `<mc:AlternateContent>(Choice, Fallback)` + `tail`(同一段落)。
fn para_with_alt(choice: &str, fallback: &str) -> String {
    format!(
        r#"<w:p>{}<mc:AlternateContent><mc:Choice Requires="w14">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent>{}</w:p>"#,
        t("before"),
        t("tail")
    )
}

fn first_para(d: &ParsedDoc) -> &Paragraph {
    match &d.document.body[0] {
        Block::Paragraph(p) => p,
        other => panic!("expected paragraph, got {other:?}"),
    }
}

fn runs(p: &Paragraph) -> Vec<(String, Option<String>)> {
    p.runs
        .iter()
        .filter(|r| !r.text().is_empty())
        .map(|r| (r.text(), r.field.clone()))
        .collect()
}

fn some(s: &str) -> Option<String> {
    Some(s.to_string())
}

/// 两个分支各有一个完整 PAGE 字段(带结果):只留选中分支那一份,正文完整,尾部不被标成字段。
#[test]
fn both_branches_with_complete_field_yield_one_field_and_clean_tail() {
    let branch = field("PAGE", "3");
    let d = build(&para_with_alt(&branch, &branch));
    assert_eq!(
        runs(first_para(&d)),
        [
            ("before".into(), None),
            ("3".into(), some("PAGE")),
            ("tail".into(), None)
        ]
    );
    assert_eq!(to_text(&d.document), "before3tail");
}

/// Choice 里的字段没有缓存结果(无正文内容 → 落选),Fallback 里的完整:同样只推进一次。
#[test]
fn rejected_choice_with_complete_empty_field_does_not_leak_state() {
    let d = build(&para_with_alt(&field("PAGE", ""), &field("PAGE", "3")));
    assert_eq!(
        runs(first_para(&d)),
        [
            ("before".into(), None),
            ("3".into(), some("PAGE")),
            ("tail".into(), None)
        ]
    );
}

/// 落选的 Choice 只有 begin + 指令 + separate(不配对、无正文):其字段帧不得泄漏到 Fallback
/// 之后——尾部正文不被标成字段结果。
#[test]
fn rejected_choice_with_unpaired_begin_does_not_leak_into_following_text() {
    let choice = format!("{}{}{}", fc("begin"), instr("PAGE"), fc("separate"));
    let d = build(&para_with_alt(&choice, &field("PAGE", "3")));
    assert_eq!(
        runs(first_para(&d)),
        [
            ("before".into(), None),
            ("3".into(), some("PAGE")),
            ("tail".into(), None)
        ]
    );
}

/// 落选的 Choice 只有 begin(指令区未结束),Fallback 是普通正文:尾部正文照常、不被吞成指令。
#[test]
fn rejected_choice_with_only_begin_keeps_following_text_visible() {
    let choice = format!("{}{}", fc("begin"), instr("PAGE"));
    let d = build(&para_with_alt(&choice, &t("fallback")));
    assert_eq!(
        runs(first_para(&d)),
        [
            ("before".into(), None),
            ("fallback".into(), None),
            ("tail".into(), None)
        ]
    );
}

/// 选中的分支自己不配对(只有 begin + 指令):不 panic,后续正文一个字都不丢。
#[test]
fn chosen_branch_with_only_begin_does_not_panic_or_lose_text() {
    let choice = format!("{}{}{}", fc("begin"), instr("PAGE"), t("kept"));
    let d = build(&para_with_alt(&choice, &t("fallback")));
    let text = to_text(&d.document);
    assert!(text.contains("before"), "{text}");
    assert!(text.contains("kept"), "{text}");
    assert!(text.contains("tail"), "{text}");
    assert!(!text.contains("fallback"), "{text}");
}

/// 块级 `mc:AlternateContent`(包着整个 `w:p`):两分支各一个完整字段,其后的段落不受影响。
#[test]
fn block_level_alt_content_advances_field_state_once() {
    let para = |inner: &str| format!("<w:p>{}</w:p>", inner);
    let choice = para(&format!("{}{}", fc("begin"), instr("PAGE")));
    let fallback = para(&field("PAGE", "3"));
    let body = format!(
        r#"<mc:AlternateContent><mc:Choice Requires="w14">{choice}</mc:Choice><mc:Fallback>{fallback}</mc:Fallback></mc:AlternateContent><w:p>{}</w:p>"#,
        t("after")
    );
    let d = build(&body);
    let Block::Paragraph(last) = d.document.body.last().expect("blocks") else {
        panic!("paragraph")
    };
    assert_eq!(runs(last), [("after".into(), None)]);
}

/// 落选 Choice 的 begin 不得让字段栈一直停在指令区:其后文档里完整的 PAGE 字段仍要被正常标记。
#[test]
fn rejected_choice_with_only_begin_does_not_break_later_fields() {
    let choice = format!("{}{}", fc("begin"), instr("PAGE"));
    let body = format!(
        "{}<w:p>{}</w:p>",
        para_with_alt(&choice, &t("fallback")),
        field("NUMPAGES", "9")
    );
    let d = build(&body);
    let Block::Paragraph(last) = d.document.body.last().expect("blocks") else {
        panic!("paragraph")
    };
    assert_eq!(runs(last), [("9".into(), some("NUMPAGES"))]);
}
