//! C-5 验收测试:styles.xml + theme1.xml 解析 + 有效样式端到端。
//!
//! 现合成一个「标题 1(Heading1,basedOn Normal)+ 正文(无 pStyle,落缺省段落样式)+
//! 引用(Quote,basedOn Normal)」的 `.docx`(正文**零直接格式化**),断言:
//! - 样式表 / 主题被机械搬运进 `Document.styles` / `Document.theme`;
//! - `doc_core::style::resolve_run/resolve_para` 的级联(docDefaults → basedOn 链 →
//!   直接格式化)、theme 字体/颜色间接引用、toggle XOR 全部端到端正确;
//! - 改 docDefaults 字号会**传导**到无样式正文(级联是活的,不是只解析不合并);
//! - `basedOn` 成环的部件:解析终止、resolve 不悬挂、validate 告警。
//!
//! 不落二进制 fixture —— docx 在测试里现合成,确定性、自包含。

use std::io::{Cursor, Write};

use doc_core::model::Block;
use doc_core::style::{
    resolve_para, resolve_run, FontRef, Justification, StyleKind, StyleWarning, ThemeFont,
};
use doc_parse::{parse_bytes, ParsedDoc};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>
  <Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>
</Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

const DOC_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#;

/// 正文:三个段落,**零直接格式化**——一切视觉都得从样式级联来。
const DOCUMENT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Chapter</w:t></w:r></w:p>
    <w:p><w:r><w:t>Body text</w:t></w:r></w:p>
    <w:p><w:pPr><w:pStyle w:val="Quote"/></w:pPr><w:r><w:t>Quoted</w:t></w:r></w:p>
  </w:body>
</w:document>"#;

/// styles.xml:docDefaults(theme 字体间接引用 + 字号 `{sz}` 半磅)+ Normal(缺省段落
/// 样式)+ Heading1(basedOn Normal:majorHAnsi / b / 32 半磅 / accent1 主题色 / 居中)+
/// Quote(basedOn Normal:i / 显式 RGB)。
fn styles_xml(doc_default_sz_half_points: u32) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults>
    <w:rPrDefault>
      <w:rPr>
        <w:rFonts w:asciiTheme="minorHAnsi" w:hAnsiTheme="minorHAnsi"
                  w:eastAsiaTheme="minorEastAsia" w:cstheme="minorBidi"/>
        <w:sz w:val="{doc_default_sz_half_points}"/>
      </w:rPr>
    </w:rPrDefault>
    <w:pPrDefault><w:pPr><w:jc w:val="left"/></w:pPr></w:pPrDefault>
  </w:docDefaults>
  <w:latentStyles w:defLockedState="0"><w:lsdException w:name="Normal"/></w:latentStyles>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal">
    <w:name w:val="Normal"/>
  </w:style>
  <w:style w:type="character" w:default="1" w:styleId="DefaultParagraphFont">
    <w:name w:val="Default Paragraph Font"/>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:pPr><w:jc w:val="center"/></w:pPr>
    <w:rPr>
      <w:rFonts w:asciiTheme="majorHAnsi"/>
      <w:b/>
      <w:sz w:val="32"/>
      <w:color w:themeColor="accent1"/>
    </w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="Quote">
    <w:name w:val="Quote"/>
    <w:basedOn w:val="Normal"/>
    <w:rPr><w:i/><w:color w:val="404040"/></w:rPr>
  </w:style>
</w:styles>"#
    )
}

/// theme1.xml:Office 风格的 clrScheme(节选)+ fontScheme(major "Calibri Light" /
/// minor "Calibri" + 东亚 "DengXian";cs 空串按缺失)。fmtScheme 与 extraClrSchemeLst
/// 存在以验证「不误收」。
const THEME: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Office">
  <a:themeElements>
    <a:clrScheme name="Office">
      <a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1>
      <a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1>
      <a:dk2><a:srgbClr val="44546A"/></a:dk2>
      <a:lt2><a:srgbClr val="E7E6E6"/></a:lt2>
      <a:accent1><a:srgbClr val="4472C4"/></a:accent1>
      <a:accent2><a:srgbClr val="ED7D31"/></a:accent2>
      <a:accent3><a:srgbClr val="A5A5A5"/></a:accent3>
      <a:accent4><a:srgbClr val="FFC000"/></a:accent4>
      <a:accent5><a:srgbClr val="5B9BD5"/></a:accent5>
      <a:accent6><a:srgbClr val="70AD47"/></a:accent6>
      <a:hlink><a:srgbClr val="0563C1"/></a:hlink>
      <a:folHlink><a:srgbClr val="954F72"/></a:folHlink>
    </a:clrScheme>
    <a:fontScheme name="Office">
      <a:majorFont>
        <a:latin typeface="Calibri Light"/><a:ea typeface=""/><a:cs typeface=""/>
        <a:font script="Hans" typeface="等线 Light"/>
      </a:majorFont>
      <a:minorFont>
        <a:latin typeface="Calibri"/><a:ea typeface="DengXian"/><a:cs typeface=""/>
      </a:minorFont>
    </a:fontScheme>
    <a:fmtScheme name="Office"/>
  </a:themeElements>
  <a:extraClrSchemeLst>
    <a:extraClrScheme><a:clrScheme name="X"><a:dk1><a:srgbClr val="123456"/></a:dk1></a:clrScheme></a:extraClrScheme>
  </a:extraClrSchemeLst>
</a:theme>"#;

/// 把部件集合压成内存 `.docx`。`styles` / `theme` 可缺(测部件缺失的兜底路径)。
fn build_docx(document_xml: &str, styles: Option<&str>, theme: Option<&str>) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        let mut parts: Vec<(&str, &str)> = vec![
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", document_xml),
            ("word/_rels/document.xml.rels", DOC_RELS),
        ];
        if let Some(s) = styles {
            parts.push(("word/styles.xml", s));
        }
        if let Some(t) = theme {
            parts.push(("word/theme/theme1.xml", t));
        }
        for (name, body) in parts {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    buf.into_inner()
}

fn parse_styled(doc_default_sz_half_points: u32) -> ParsedDoc {
    parse_bytes(&build_docx(
        DOCUMENT,
        Some(&styles_xml(doc_default_sz_half_points)),
        Some(THEME),
    ))
    .expect("parse styled docx")
}

/// 取第 `i` 个正文段落。
fn para(parsed: &ParsedDoc, i: usize) -> &doc_core::model::Paragraph {
    let Block::Paragraph(p) = &parsed.document.body[i] else {
        panic!("block {i} should be a paragraph");
    };
    p
}

// ============================================================ 机械搬运:styles / theme

#[test]
fn styles_table_is_mechanically_parsed() {
    let parsed = parse_styled(22);
    let st = &parsed.document.styles;

    // docDefaults:字号 22 半磅 = 11pt;四槽全是 theme 间接引用(尚未解引)。
    assert_eq!(st.doc_default_rpr.sz, Some(11.0));
    assert_eq!(
        st.doc_default_rpr.fonts.ascii,
        Some(FontRef::Theme(ThemeFont::MinorHAnsi))
    );
    assert_eq!(
        st.doc_default_rpr.fonts.east_asia,
        Some(FontRef::Theme(ThemeFont::MinorEastAsia))
    );
    assert_eq!(
        st.doc_default_rpr.fonts.cs,
        Some(FontRef::Theme(ThemeFont::MinorBidi))
    );
    assert_eq!(st.doc_default_ppr.jc, Some(Justification::Left));

    // 缺省样式登记:paragraph -> Normal,character -> DefaultParagraphFont。
    assert_eq!(st.default_para_style.as_deref(), Some("Normal"));
    assert_eq!(
        st.default_char_style.as_deref(),
        Some("DefaultParagraphFont")
    );

    // Heading1:种类/basedOn/名字 + rPr 片段(b 是 Some(true) 而非折叠 bool)。
    let h1 = &st.styles["Heading1"];
    assert_eq!(h1.kind, StyleKind::Paragraph);
    assert_eq!(h1.based_on.as_deref(), Some("Normal"));
    assert_eq!(h1.name.as_deref(), Some("heading 1"));
    assert_eq!(h1.rpr.b, Some(true));
    assert_eq!(h1.rpr.sz, Some(16.0));
    assert_eq!(
        h1.rpr.fonts.ascii,
        Some(FontRef::Theme(ThemeFont::MajorHAnsi))
    );
    assert_eq!(h1.ppr.jc, Some(Justification::Center));
    assert!(!h1.default);
    assert!(st.styles["Normal"].default);
}

#[test]
fn theme_font_and_color_schemes_are_parsed() {
    let parsed = parse_styled(22);
    let theme = &parsed.document.theme;

    assert_eq!(theme.fonts.major.latin.as_deref(), Some("Calibri Light"));
    assert_eq!(theme.fonts.minor.latin.as_deref(), Some("Calibri"));
    assert_eq!(theme.fonts.minor.east_asia.as_deref(), Some("DengXian"));
    assert_eq!(theme.fonts.major.east_asia, None, "typeface=\"\" 按缺失");
    assert_eq!(theme.fonts.minor.cs, None);

    // clrScheme:srgbClr 与 sysClr@lastClr 两种形态;extraClrSchemeLst 不得误收
    // (dk1 仍是主 clrScheme 的 000000,不是 123456)。
    assert_eq!(theme.colors.dk1.map(|c| c.rgb), Some([0x00, 0x00, 0x00]));
    assert_eq!(theme.colors.lt1.map(|c| c.rgb), Some([0xFF, 0xFF, 0xFF]));
    assert_eq!(
        theme.colors.accent1.map(|c| c.rgb),
        Some([0x44, 0x72, 0xC4])
    );
    assert_eq!(theme.colors.hlink.map(|c| c.rgb), Some([0x05, 0x63, 0xC1]));
    assert_eq!(
        theme.colors.fol_hlink.map(|c| c.rgb),
        Some([0x95, 0x4F, 0x72])
    );
}

// ============================================================ 有效样式端到端(级联活的)

#[test]
fn effective_styles_end_to_end_heading_body_quote() {
    let parsed = parse_styled(22);
    let doc = &parsed.document;

    // 标题 1:basedOn Normal;b 链上一次(奇数)→ 粗;sz 32 半磅 → 16pt 覆盖 docDefaults;
    // asciiTheme=majorHAnsi → "Calibri Light";hAnsi 未覆盖 → docDefaults minorHAnsi →
    // "Calibri";eastAsia 继承 docDefaults minorEastAsia → "DengXian";accent1 → 4472C4;
    // pPr jc=center。
    let h1 = para(&parsed, 0);
    let eff = resolve_run(doc, h1, &h1.runs[0]);
    assert!(eff.bold);
    assert!(!eff.italic);
    assert_eq!(eff.size_pt, 16.0);
    assert_eq!(eff.font_ascii, "Calibri Light");
    assert_eq!(eff.font_h_ansi, "Calibri");
    assert_eq!(eff.font_east_asia.as_deref(), Some("DengXian"));
    assert_eq!(eff.color.map(|c| c.rgb), Some([0x44, 0x72, 0xC4]));
    assert_eq!(resolve_para(doc, h1).align, Justification::Center);

    // 正文:无 pStyle → 缺省段落样式 Normal(空属性)→ 全部落 docDefaults:
    // 11pt / minorHAnsi → "Calibri" / 不粗不斜 / 左对齐 / 无色(auto)。
    let body = para(&parsed, 1);
    let eff = resolve_run(doc, body, &body.runs[0]);
    assert!(!eff.bold && !eff.italic && !eff.underline);
    assert_eq!(eff.size_pt, 11.0);
    assert_eq!(eff.font_ascii, "Calibri");
    assert_eq!(eff.font_east_asia.as_deref(), Some("DengXian"));
    assert_eq!(eff.color, None);
    assert_eq!(resolve_para(doc, body).align, Justification::Left);

    // 引用:basedOn Normal;i 一次 → 斜;字号未覆盖 → docDefaults 11pt;显式 RGB 404040。
    let quote = para(&parsed, 2);
    let eff = resolve_run(doc, quote, &quote.runs[0]);
    assert!(eff.italic);
    assert!(!eff.bold);
    assert_eq!(eff.size_pt, 11.0);
    assert_eq!(eff.font_ascii, "Calibri");
    assert_eq!(eff.color.map(|c| c.rgb), Some([0x40, 0x40, 0x40]));
}

#[test]
fn mutating_doc_defaults_size_propagates_to_effective_values() {
    // C-5 验收条款:改 fixture 里 docDefaults 的字号,渲染值必须跟着变(证明级联是活的)。
    // 22 半磅 -> 正文 11pt;28 半磅 -> 正文 14pt;而 Heading1 显式 sz=32 半磅,不受影响。
    for (half_points, expected_body_pt) in [(22u32, 11.0f32), (28, 14.0)] {
        let parsed = parse_styled(half_points);
        let doc = &parsed.document;
        let body = para(&parsed, 1);
        let eff = resolve_run(doc, body, &body.runs[0]);
        assert_eq!(eff.size_pt, expected_body_pt);
        let h1 = para(&parsed, 0);
        assert_eq!(resolve_run(doc, h1, &h1.runs[0]).size_pt, 16.0);
    }
}

#[test]
fn direct_formatting_overrides_style_chain_and_toggles_absolutely() {
    // 在 Heading1 段上加直接格式化:<w:b w:val="0"/> 恒关(样式链上 b 奇数次本应开);
    // <w:sz w:val="20"/> 覆盖样式链的 16pt。
    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr>
      <w:r><w:rPr><w:b w:val="0"/><w:sz w:val="20"/></w:rPr><w:t>muted heading</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"#;
    let parsed = parse_bytes(&build_docx(document, Some(&styles_xml(22)), Some(THEME)))
        .expect("parse direct-format docx");
    let doc = &parsed.document;
    let p = para(&parsed, 0);
    let eff = resolve_run(doc, p, &p.runs[0]);
    assert!(!eff.bold, "direct b=0 恒关,无视样式链奇偶");
    assert_eq!(eff.size_pt, 10.0, "direct sz 覆盖样式链");
    // 便利字段契约不变:direct b=0 折叠成 false。
    assert!(!p.runs[0].bold);
}

// ============================================================ 环 / 部件缺失

#[test]
fn based_on_cycle_terminates_with_warning() {
    let cyclic_styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="A">
    <w:basedOn w:val="B"/><w:rPr><w:sz w:val="28"/></w:rPr>
  </w:style>
  <w:style w:type="paragraph" w:styleId="B">
    <w:basedOn w:val="A"/><w:rPr><w:b/></w:rPr>
  </w:style>
</w:styles>"#;
    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:body><w:p><w:pPr><w:pStyle w:val="A"/></w:pPr><w:r><w:t>cyclic</w:t></w:r></w:p></w:body>
</w:document>"#;
    let parsed = parse_bytes(&build_docx(document, Some(cyclic_styles), None))
        .expect("cyclic styles must not hang the parser");
    let doc = &parsed.document;

    // 解析终止且属性仍生效(链截断,不悬挂):A 的 sz 28 半磅 = 14pt;B 的 b 计入 XOR。
    let p = para(&parsed, 0);
    let eff = resolve_run(doc, p, &p.runs[0]);
    assert_eq!(eff.size_pt, 14.0);
    assert!(eff.bold);

    // validate:环上成员各报一次。
    let warnings = doc.styles.validate();
    assert!(warnings.contains(&StyleWarning::BasedOnCycle {
        style_id: "A".into()
    }));
    assert!(warnings.contains(&StyleWarning::BasedOnCycle {
        style_id: "B".into()
    }));
}

#[test]
fn missing_styles_and_theme_parts_fall_back_to_word_defaults() {
    // 无 styles.xml / theme1.xml 的最简 docx:空表 + 空主题,有效值落 Word 内置兜底
    // (Calibri 11pt,注释出处见 doc-core/src/style.rs 常量)。
    let parsed = parse_bytes(&build_docx(DOCUMENT, None, None)).expect("parse bare docx");
    let doc = &parsed.document;
    assert!(doc.styles.styles.is_empty());
    assert_eq!(doc.theme, doc_core::style::Theme::default());
    assert!(doc.styles.validate().is_empty());

    let body = para(&parsed, 1);
    let eff = resolve_run(doc, body, &body.runs[0]);
    assert_eq!(eff.font_ascii, "Calibri");
    assert_eq!(eff.size_pt, 11.0);
    assert!(!eff.bold);
}
