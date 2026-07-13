//! `doc-parse` 验收测试:用 `zip` 写出一个最小但合法的 `.docx`,断言 `parse_bytes`
//! 还原出段落(带样式 run)、表格(单元格 + **横向 gridSpan 合并** + **纵向 vMerge 合并** +
//! **嵌套表** + 单元格填充)。
//!
//! 不落二进制 fixture —— docx 在测试里现合成,确定性、自包含。

use std::io::{Cursor, Write};

use doc_core::model::{Block, BreakKind, Orientation, RunSegment, VMerge};
use doc_parse::{parse_bytes, ParsedDoc};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

const DOC_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"/>"#;

// 一份文档:一个带样式 run 的标题段 + 一张表格(横向合并 + 纵向合并 + 嵌套表 + 填充)。
const DOCUMENT: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/><w:jc w:val="center"/></w:pPr>
      <w:r>
        <w:rPr>
          <w:rFonts w:ascii="Calibri"/>
          <w:b/><w:i/><w:sz w:val="48"/>
          <w:color w:val="1F4E79"/>
        </w:rPr>
        <w:t>Hello docspine</w:t>
      </w:r>
    </w:p>
    <w:tbl>
      <w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr>
      <w:tblGrid>
        <w:gridCol w:w="2400"/>
        <w:gridCol w:w="2400"/>
        <w:gridCol w:w="2400"/>
      </w:tblGrid>
      <w:tr>
        <w:trPr><w:trHeight w:val="400"/><w:tblHeader/></w:trPr>
        <w:tc>
          <w:tcPr>
            <w:gridSpan w:val="2"/>
            <w:shd w:fill="FFCC00"/>
          </w:tcPr>
          <w:p><w:r><w:t>Merged Header</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:tcPr><w:vMerge w:val="restart"/></w:tcPr>
          <w:p><w:r><w:t>Spanning Down</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
      <w:tr>
        <w:tc>
          <w:tcPr><w:tcW w:w="2400" w:type="dxa"/></w:tcPr>
          <w:p><w:r><w:t>A2</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:p><w:r><w:t>B2</w:t></w:r></w:p>
          <w:tbl>
            <w:tblGrid><w:gridCol w:w="1200"/></w:tblGrid>
            <w:tr><w:tc><w:p><w:r><w:t>nested</w:t></w:r></w:p></w:tc></w:tr>
          </w:tbl>
        </w:tc>
        <w:tc>
          <w:tcPr><w:vMerge w:val="continue"/></w:tcPr>
          <w:p/>
        </w:tc>
      </w:tr>
    </w:tbl>
  </w:body>
</w:document>"#;

/// 把给定的 `word/document.xml` 压成一个内存里的最小合法 `.docx` zip 字节串。
fn build_docx(document_xml: &str) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        for (name, body) in [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", document_xml),
            ("word/_rels/document.xml.rels", DOC_RELS),
        ] {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    buf.into_inner()
}

fn parse() -> ParsedDoc {
    parse_bytes(&build_docx(DOCUMENT)).expect("parse minimal docx")
}

/// 便利:把一段 `w:body` 内容包进 `w:document` 根,解析成 [`ParsedDoc`]。
fn parse_body_xml(body_xml: &str) -> ParsedDoc {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>{body_xml}</w:body>
</w:document>"#
    );
    parse_bytes(&build_docx(&doc)).expect("parse synthetic docx")
}

#[test]
fn parses_two_top_level_blocks() {
    let parsed = parse();
    assert_eq!(parsed.document.body.len(), 2, "one paragraph + one table");
    assert!(matches!(parsed.document.body[0], Block::Paragraph(_)));
    assert!(matches!(parsed.document.body[1], Block::Table(_)));
}

#[test]
fn parses_paragraph_runs_and_styling() {
    let parsed = parse();
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected a paragraph");
    };
    assert_eq!(p.style.as_deref(), Some("Heading1"));
    assert_eq!(p.align.as_deref(), Some("center"));
    assert_eq!(p.text(), "Hello docspine");

    let run = &p.runs[0];
    assert_eq!(run.text(), "Hello docspine");
    assert!(run.bold);
    assert!(run.italic);
    assert_eq!(run.size_pt, Some(24.0)); // w:sz="48" half-points / 2
    assert_eq!(run.font.as_deref(), Some("Calibri"));
    assert_eq!(run.color.map(|c| c.rgb), Some([0x1F, 0x4E, 0x79]));
}

#[test]
fn parses_table_grid_and_header_row() {
    let parsed = parse();
    let Block::Table(t) = &parsed.document.body[1] else {
        panic!("expected a table");
    };
    assert_eq!(t.style.as_deref(), Some("TableGrid"));
    assert_eq!(t.grid_cols, vec![2400, 2400, 2400]);
    assert_eq!(t.col_count(), 3);
    assert_eq!(t.rows.len(), 2);

    let header = &t.rows[0];
    assert!(header.is_header);
    assert_eq!(header.height, Some(400));
}

#[test]
fn parses_horizontal_grid_span_and_fill() {
    let parsed = parse();
    let Block::Table(t) = &parsed.document.body[1] else {
        unreachable!()
    };
    // 首行首格:横向跨 2 列(gridSpan=2),黄色填充。
    let merged = &t.rows[0].cells[0];
    assert_eq!(merged.grid_span, 2);
    assert_eq!(merged.text(), "Merged Header");
    assert_eq!(merged.fill.map(|c| c.rgb), Some([0xFF, 0xCC, 0x00]));
}

#[test]
fn parses_vertical_v_merge_restart_and_continue() {
    let parsed = parse();
    let Block::Table(t) = &parsed.document.body[1] else {
        unreachable!()
    };
    // 首行末格:纵向合并起始(restart),承载内容。
    let restart = &t.rows[0].cells[1];
    assert_eq!(restart.v_merge, VMerge::Restart);
    assert_eq!(restart.text(), "Spanning Down");
    assert!(!restart.is_vmerge_continuation());

    // 次行末格:纵向合并延续(continue),内容空。
    let cont = &t.rows[1].cells[2];
    assert_eq!(cont.v_merge, VMerge::Continue);
    assert!(cont.is_vmerge_continuation());
}

#[test]
fn parses_cell_width_dxa() {
    let parsed = parse();
    let Block::Table(t) = &parsed.document.body[1] else {
        unreachable!()
    };
    // 次行首格带绝对宽度 tcW dxa=2400 twip。
    let a2 = &t.rows[1].cells[0];
    assert_eq!(a2.width, Some(2400));
    assert_eq!(a2.text(), "A2");
}

#[test]
fn parses_nested_table_inside_cell() {
    let parsed = parse();
    let Block::Table(t) = &parsed.document.body[1] else {
        unreachable!()
    };
    // 次行第二格里有一张嵌套表(以及它自己的段落 "B2")。
    let b2 = &t.rows[1].cells[1];
    assert_eq!(b2.text(), "B2"); // 直接段落文字(忽略嵌套表)
    let nested = b2
        .blocks
        .iter()
        .find_map(|blk| match blk {
            Block::Table(nt) => Some(nt),
            Block::Paragraph(_) => None,
        })
        .expect("a nested table inside cell B2");
    assert_eq!(nested.rows.len(), 1);
    let Block::Paragraph(np) = &nested.rows[0].cells[0].blocks[0] else {
        panic!("nested cell should hold a paragraph");
    };
    assert_eq!(np.text(), "nested");
}

// ============================================================ C-2:节(sectPr)页面几何

#[test]
fn no_sectpr_yields_single_default_section() {
    // 整篇没有任何 sectPr(既有最小 fixture)-> 恰好一节 Word 默认页面设置,覆盖全部块。
    let parsed = parse();
    assert_eq!(parsed.document.sections.len(), 1);
    let s = &parsed.document.sections[0];
    assert_eq!((s.page_width, s.page_height), (12_240, 15_840)); // Letter
    assert_eq!(s.orientation, Orientation::Portrait);
    assert_eq!(
        (
            s.margins.top,
            s.margins.right,
            s.margins.bottom,
            s.margins.left
        ),
        (1_440, 1_440, 1_440, 1_440)
    );
    assert_eq!(
        (s.margins.header, s.margins.footer, s.margins.gutter),
        (720, 720, 0)
    );
    assert_eq!(s.cols, 1);
    assert_eq!(s.end_block, parsed.document.body.len());
}

#[test]
fn body_final_sectpr_parses_geometry() {
    // body 末尾 sectPr:A4 横向 + 自定义边距 + 两栏。
    let parsed = parse_body_xml(
        r#"<w:p><w:r><w:t>only</w:t></w:r></w:p>
           <w:sectPr>
             <w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>
             <w:pgMar w:top="720" w:right="1080" w:bottom="360" w:left="1800"
                      w:header="500" w:footer="400" w:gutter="100"/>
             <w:cols w:num="2" w:space="708"/>
           </w:sectPr>"#,
    );
    assert_eq!(parsed.document.sections.len(), 1);
    let s = &parsed.document.sections[0];
    assert_eq!((s.page_width, s.page_height), (16_838, 11_906)); // A4 横向
    assert_eq!(s.orientation, Orientation::Landscape);
    assert_eq!(
        (
            s.margins.top,
            s.margins.right,
            s.margins.bottom,
            s.margins.left
        ),
        (720, 1_080, 360, 1_800)
    );
    assert_eq!(
        (s.margins.header, s.margins.footer, s.margins.gutter),
        (500, 400, 100)
    );
    assert_eq!(s.cols, 2);
    assert_eq!(s.end_block, 1);
}

#[test]
fn mid_body_sectpr_splits_sections_and_keeps_following_content() {
    // 段内 pPr>sectPr 结束包含它的节;**之后的正文不得截断**(修复:旧 walker 在段内
    // sectPr 处提前 break,导致其后整个 body 丢失)。
    let parsed = parse_body_xml(
        r#"<w:p><w:r><w:t>first section text</w:t></w:r></w:p>
           <w:p>
             <w:pPr>
               <w:sectPr>
                 <w:pgSz w:w="12240" w:h="15840"/>
                 <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/>
               </w:sectPr>
             </w:pPr>
           </w:p>
           <w:p><w:r><w:t>second section text</w:t></w:r></w:p>
           <w:sectPr>
             <w:pgSz w:w="15840" w:h="12240" w:orient="landscape"/>
           </w:sectPr>"#,
    );
    let doc = &parsed.document;
    // 内容丢失修复:段内 sectPr 之后的段落还在。
    assert_eq!(doc.body.len(), 3);
    let Block::Paragraph(last) = &doc.body[2] else {
        panic!("expected trailing paragraph");
    };
    assert_eq!(last.text(), "second section text");

    // 节归属:第一节含块 0..2(含承载 sectPr 的空段),第二节含块 2..3。
    assert_eq!(doc.sections.len(), 2);
    assert_eq!(doc.sections[0].end_block, 2);
    assert_eq!(doc.sections[0].orientation, Orientation::Portrait);
    assert_eq!(doc.sections[1].end_block, 3);
    assert_eq!(doc.sections[1].orientation, Orientation::Landscape);
    assert_eq!(
        (doc.sections[1].page_width, doc.sections[1].page_height),
        (15_840, 12_240)
    );
}

#[test]
fn ppr_props_after_nested_container_still_parse() {
    // pPr 深度计数修复:w:numPr(嵌套容器)之后的 w:jc 不再被提前 break 丢掉。
    let parsed = parse_body_xml(
        r#"<w:p>
             <w:pPr>
               <w:numPr><w:ilvl w:val="1"/><w:numId w:val="5"/></w:numPr>
               <w:jc w:val="right"/>
             </w:pPr>
             <w:r><w:t>numbered</w:t></w:r>
           </w:p>"#,
    );
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected paragraph");
    };
    assert_eq!(p.list_level, Some(1));
    assert_eq!(p.align.as_deref(), Some("right"));
}

// ============================================================ C-3:run 分段与内容丢失修复

#[test]
fn run_segments_capture_breaks_tabs_and_types() {
    // w:br@w:type 终于被读取:page/column 与缺省换行区分;w:cr 视作换行;w:tab 独立成段。
    let parsed = parse_body_xml(
        r#"<w:p><w:r>
             <w:t>a</w:t><w:tab/><w:t>b</w:t>
             <w:br w:type="page"/><w:t>c</w:t>
             <w:br w:type="column"/><w:br/><w:cr/><w:t>d</w:t>
           </w:r></w:p>"#,
    );
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected paragraph");
    };
    let run = &p.runs[0];
    assert_eq!(
        run.segments,
        vec![
            RunSegment::Text("a".into()),
            RunSegment::Tab,
            RunSegment::Text("b".into()),
            RunSegment::Break(BreakKind::Page),
            RunSegment::Text("c".into()),
            RunSegment::Break(BreakKind::Column),
            RunSegment::Break(BreakKind::Line),
            RunSegment::Break(BreakKind::Line),
            RunSegment::Text("d".into()),
        ]
    );
    // 折叠契约不变:Tab -> '\t',Break(任意种类)-> '\n'。
    assert_eq!(run.text(), "a\tb\nc\n\n\nd");
}

#[test]
fn sdt_content_is_transparent_at_block_and_inline_level() {
    // w:sdt(封面/目录等结构化文档标签)整体丢弃 -> 修复:sdtContent 透明展开。
    // 覆盖:块级 sdt(含嵌套 sdt)、行内 sdt、单元格内 sdt。
    let parsed = parse_body_xml(
        r#"<w:sdt>
             <w:sdtPr><w:alias w:val="Cover"/></w:sdtPr>
             <w:sdtContent>
               <w:p><w:r><w:t>cover title</w:t></w:r></w:p>
               <w:sdt><w:sdtContent>
                 <w:p><w:r><w:t>nested sdt para</w:t></w:r></w:p>
               </w:sdtContent></w:sdt>
             </w:sdtContent>
           </w:sdt>
           <w:p>
             <w:r><w:t>before </w:t></w:r>
             <w:sdt>
               <w:sdtPr><w:date/></w:sdtPr>
               <w:sdtContent><w:r><w:t>2026-07-02</w:t></w:r></w:sdtContent>
             </w:sdt>
             <w:r><w:t> after</w:t></w:r>
           </w:p>
           <w:tbl>
             <w:tblGrid><w:gridCol w:w="1200"/></w:tblGrid>
             <w:tr><w:tc>
               <w:sdt><w:sdtContent>
                 <w:p><w:r><w:t>cell sdt text</w:t></w:r></w:p>
               </w:sdtContent></w:sdt>
             </w:tc></w:tr>
           </w:tbl>"#,
    );
    let doc = &parsed.document;
    // 块级 sdt 展开成两个段落 + 行内段 + 表 = 4 个顶层块。
    assert_eq!(doc.body.len(), 4);
    let texts: Vec<String> = doc
        .body
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(p) => Some(p.text()),
            Block::Table(_) => None,
        })
        .collect();
    assert_eq!(
        texts,
        vec!["cover title", "nested sdt para", "before 2026-07-02 after"]
    );
    let Block::Table(t) = &doc.body[3] else {
        panic!("expected table");
    };
    assert_eq!(t.rows[0].cells[0].text(), "cell sdt text");
}

#[test]
fn fldsimple_cached_result_is_kept() {
    // w:fldSimple(字段)缓存结果整体丢弃 -> 修复:当作 run 容器透明展开。
    let parsed = parse_body_xml(
        r#"<w:p>
             <w:r><w:t>Page </w:t></w:r>
             <w:fldSimple w:instr=" PAGE \* MERGEFORMAT ">
               <w:r><w:rPr><w:b/></w:rPr><w:t>7</w:t></w:r>
             </w:fldSimple>
             <w:r><w:t> of 9</w:t></w:r>
           </w:p>"#,
    );
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected paragraph");
    };
    assert_eq!(p.text(), "Page 7 of 9");
    // 字段结果 run 的样式也保留。
    assert!(p.runs.iter().any(|r| r.bold && r.text() == "7"));
}

#[test]
fn malformed_bytes_yield_error_not_panic() {
    // 非 zip 字节 -> Err(DocError),绝不 panic。
    assert!(parse_bytes(b"not a docx zip at all").is_err());
}

#[test]
fn legacy_doc_bytes_yield_typed_unsupported() {
    // CFB 魔数 -> 清晰的 Unsupported 降级(docx 优先),绝不 panic、绝不当成坏 zip。
    let mut cfb = doc_parse::legacy::CFB_MAGIC.to_vec();
    cfb.extend_from_slice(&[0u8; 64]);
    let err = parse_bytes(&cfb).expect_err("legacy .doc should be Unsupported");
    assert_eq!(err.kind(), "unsupported");
}

/// 旧二进制 `.doc` 探测(需 `legacy-doc` 特性):用 `cfb` crate 现造一个含 `WordDocument`
/// 流的复合文档,断言 `probe_doc` 能识别它并列出流名。证明 `.doc` 基础探测真的可用。
#[cfg(feature = "legacy-doc")]
#[test]
fn probe_doc_detects_word_stream_in_cfb() {
    use std::io::{Cursor, Write};

    // 用 cfb 写一个最小复合文档,放一个 WordDocument 流。
    let mut comp = cfb::CompoundFile::create(Cursor::new(Vec::new())).expect("create CFB");
    {
        let mut stream = comp.create_stream("WordDocument").expect("create stream");
        stream
            .write_all(b"\xec\xa5fake FIB bytes")
            .expect("write stream");
    }
    let bytes = comp.into_inner().into_inner();

    let probe = doc_parse::legacy::probe_doc(&bytes).expect("probe should succeed");
    assert!(probe.is_cfb);
    assert!(
        probe.has_word_stream,
        "should detect the WordDocument stream"
    );
    assert!(probe.streams.iter().any(|s| s.contains("WordDocument")));
}

// ============================================================ 直接格式化补齐(C-4)

/// pPr 直接格式化:spacing / ind / shd / keep 系列 + pBdr 落进 `Paragraph.ppr`;
/// 便利字段(style/align)契约不变。
#[test]
fn direct_ppr_spacing_indent_borders_shading_and_flags() {
    use doc_core::style::{ColorRef, LineSpacingRule};

    let parsed = parse_body_xml(
        r#"<w:p>
             <w:pPr>
               <w:pStyle w:val="Body"/>
               <w:keepNext/>
               <w:keepLines w:val="0"/>
               <w:pageBreakBefore/>
               <w:widowControl w:val="0"/>
               <w:pBdr>
                 <w:top w:val="single" w:sz="8" w:space="2" w:color="FF0000"/>
                 <w:bottom w:val="double" w:sz="4" w:space="0" w:color="auto"/>
               </w:pBdr>
               <w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/>
               <w:spacing w:before="240" w:after="120" w:line="360" w:lineRule="auto"/>
               <w:ind w:left="720" w:right="360" w:hanging="360"/>
               <w:contextualSpacing/>
               <w:jc w:val="both"/>
             </w:pPr>
             <w:r><w:t>styled</w:t></w:r>
           </w:p>"#,
    );
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected a paragraph");
    };
    // 便利字段不变。
    assert_eq!(p.style.as_deref(), Some("Body"));
    assert_eq!(p.align.as_deref(), Some("both"));

    let ppr = &p.ppr;
    assert_eq!(ppr.space_before, Some(240));
    assert_eq!(ppr.space_after, Some(120));
    assert_eq!(ppr.line, Some(LineSpacingRule::Auto(360)));
    assert_eq!(ppr.ind_left, Some(720));
    assert_eq!(ppr.ind_right, Some(360));
    assert_eq!(ppr.ind_hanging, Some(360));
    assert_eq!(ppr.ind_first_line, None);
    assert_eq!(ppr.keep_next, Some(true));
    assert_eq!(ppr.keep_lines, Some(false));
    assert_eq!(ppr.page_break_before, Some(true));
    assert_eq!(ppr.widow_control, Some(false));
    assert_eq!(ppr.contextual_spacing, Some(true));
    assert_eq!(
        ppr.jc,
        Some(doc_core::style::Justification::Justify),
        "归一化 jc 与原样 align 并存"
    );

    let top = ppr.borders.top.as_ref().expect("top border");
    assert_eq!(top.val, "single");
    assert_eq!(top.sz_eighth_pt, 8);
    assert_eq!(top.space_pt, 2);
    assert_eq!(
        top.color,
        Some(ColorRef::Rgb(doc_core::model::Color::new([0xFF, 0, 0])))
    );
    let bottom = ppr.borders.bottom.as_ref().expect("bottom border");
    assert_eq!(bottom.val, "double");
    assert_eq!(bottom.color, Some(ColorRef::Auto));
    assert!(ppr.borders.left.is_none());
    assert_eq!(
        ppr.shd_fill,
        Some(ColorRef::Rgb(doc_core::model::Color::new([
            0xD9, 0xE2, 0xF3
        ])))
    );
}

/// pBdr 之后的 pPr 属性(jc)不被子树吞掉;pPr 内嵌 rPr(段落标记符属性,含同名异义的
/// w:spacing 字符间距)不污染段落属性。
#[test]
fn ppr_after_pbdr_and_paragraph_mark_rpr_do_not_bleed() {
    let parsed = parse_body_xml(
        r#"<w:p>
             <w:pPr>
               <w:pBdr><w:top w:val="single" w:sz="4"/></w:pBdr>
               <w:rPr><w:spacing w:val="20"/><w:sz w:val="96"/></w:rPr>
               <w:jc w:val="center"/>
             </w:pPr>
             <w:r><w:t>x</w:t></w:r>
           </w:p>"#,
    );
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected a paragraph");
    };
    assert_eq!(p.align.as_deref(), Some("center"), "pBdr 后属性不丢");
    assert!(p.ppr.borders.top.is_some());
    // 段落标记符 rPr 的 w:spacing(字符间距)不得写进段落 spacing。
    assert_eq!(p.ppr.space_before, None);
    assert_eq!(p.ppr.space_after, None);
    // run 自身的字号不受段落标记符 rPr 影响。
    assert_eq!(p.runs[0].size_pt, None);
}

/// rPr 直接格式化补齐:rStyle / szCs / 下划线种类 / highlight / vertAlign 落进 `run.rpr`;
/// underline 便利字段按种类折叠(none → false)。
#[test]
fn direct_rpr_rstyle_szcs_underline_kind_highlight_vert_align() {
    use doc_core::style::{Highlight, UnderlineKind, VertAlign};

    let parsed = parse_body_xml(
        r#"<w:p>
             <w:r>
               <w:rPr>
                 <w:rStyle w:val="Emphasis"/>
                 <w:sz w:val="20"/><w:szCs w:val="24"/>
                 <w:u w:val="double"/>
                 <w:highlight w:val="yellow"/>
                 <w:vertAlign w:val="superscript"/>
               </w:rPr>
               <w:t>rich</w:t>
             </w:r>
             <w:r>
               <w:rPr><w:u w:val="none"/><w:highlight w:val="none"/></w:rPr>
               <w:t>plain</w:t>
             </w:r>
           </w:p>"#,
    );
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected a paragraph");
    };
    let rich = &p.runs[0].rpr;
    assert_eq!(rich.r_style.as_deref(), Some("Emphasis"));
    assert_eq!(rich.sz, Some(10.0));
    assert_eq!(rich.sz_cs, Some(12.0));
    assert_eq!(rich.u, Some(UnderlineKind::Double));
    assert_eq!(
        rich.highlight,
        Some(Highlight::On(doc_core::model::Color::new([0xFF, 0xFF, 0])))
    );
    assert_eq!(rich.vert_align, Some(VertAlign::Superscript));
    assert!(p.runs[0].underline, "非 none 种类折叠为 true");

    let plain = &p.runs[1].rpr;
    assert_eq!(plain.u, Some(UnderlineKind::None), "显式关保真");
    assert_eq!(plain.highlight, Some(Highlight::Off));
    assert!(!p.runs[1].underline);
}

// ============================================================ C-6:numbering 部件接线

/// 把 document.xml 与若干额外部件压成最小合法 docx(numbering / styles 部件测试用)。
fn build_docx_with_parts(document_xml: &str, extra_parts: &[(&str, &str)]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        let base = [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", document_xml),
            ("word/_rels/document.xml.rels", DOC_RELS),
        ];
        for (name, body) in base.iter().copied().chain(extra_parts.iter().copied()) {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    buf.into_inner()
}

/// 便利:把 body 内容 + 额外部件解析成 [`ParsedDoc`]。
fn parse_body_with_parts(body_xml: &str, extra_parts: &[(&str, &str)]) -> ParsedDoc {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>{body_xml}</w:body>
</w:document>"#
    );
    parse_bytes(&build_docx_with_parts(&doc, extra_parts)).expect("parse synthetic docx")
}

/// numbering.xml 部件接线 + `w:numPr` 的 numId/ilvl 捕获(C-6)。
#[test]
fn numbering_part_and_num_pr_wire_into_document() {
    use doc_core::numbering::NumFmt;

    let numbering = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0">
    <w:lvl w:ilvl="0">
      <w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/>
      <w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr>
    </w:lvl>
    <w:lvl w:ilvl="1">
      <w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2."/>
      <w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr>
    </w:lvl>
  </w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
</w:numbering>"#;
    let parsed = parse_body_with_parts(
        r#"<w:p>
             <w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr><w:jc w:val="center"/></w:pPr>
             <w:r><w:t>item</w:t></w:r>
           </w:p>
           <w:p>
             <w:pPr><w:numPr><w:ilvl w:val="1"/><w:numId w:val="1"/></w:numPr></w:pPr>
             <w:r><w:t>sub</w:t></w:r>
           </w:p>"#,
        &[("word/numbering.xml", numbering)],
    );
    let doc = &parsed.document;
    assert!(!doc.numbering.is_empty(), "numbering 部件应接进 Document");
    let lvl0 = doc.numbering.level(1, 0).expect("numId 1 / ilvl 0");
    assert_eq!(lvl0.fmt, NumFmt::Decimal);
    assert_eq!(lvl0.lvl_text.as_deref(), Some("%1."));
    assert_eq!(lvl0.ppr.ind_left, Some(720));

    let Block::Paragraph(p0) = &doc.body[0] else {
        panic!("expected a paragraph");
    };
    assert_eq!(p0.num_id, Some(1), "numId 捕获(历史缺陷修复)");
    assert_eq!(p0.list_level, Some(0));
    assert_eq!(p0.align.as_deref(), Some("center"), "numPr 之后的 jc 不丢");
    let Block::Paragraph(p1) = &doc.body[1] else {
        panic!("expected a paragraph");
    };
    assert_eq!((p1.num_id, p1.list_level), (Some(1), Some(1)));
}

/// 无 numbering 部件:空编号表(列表段按普通段渲染),numPr 字段照常捕获。
#[test]
fn missing_numbering_part_yields_empty_table() {
    let parsed = parse_body_xml(
        r#"<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="9"/></w:numPr></w:pPr>
           <w:r><w:t>x</w:t></w:r></w:p>"#,
    );
    assert!(parsed.document.numbering.is_empty());
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected a paragraph");
    };
    assert_eq!(p.num_id, Some(9));
}

// ============================================================ C-7:表格保真解析

/// tblPr 的 C-7 属性:tblBorders(六槽)/ tblCellMar / tblInd / tblW(pct)/ jc。
#[test]
fn tblpr_borders_margins_indent_width_jc() {
    use doc_core::model::TableWidth;
    use doc_core::style::Justification;

    let parsed = parse_body_xml(
        r#"<w:tbl>
             <w:tblPr>
               <w:tblStyle w:val="TableGrid"/>
               <w:tblW w:w="2500" w:type="pct"/>
               <w:jc w:val="center"/>
               <w:tblInd w:w="360" w:type="dxa"/>
               <w:tblBorders>
                 <w:top w:val="single" w:sz="8" w:color="FF0000"/>
                 <w:bottom w:val="single" w:sz="8"/>
                 <w:start w:val="single" w:sz="4"/>
                 <w:end w:val="single" w:sz="4"/>
                 <w:insideH w:val="dashed" w:sz="2"/>
                 <w:insideV w:val="none"/>
               </w:tblBorders>
               <w:tblCellMar>
                 <w:top w:w="60" w:type="dxa"/>
                 <w:start w:w="200" w:type="dxa"/>
               </w:tblCellMar>
             </w:tblPr>
             <w:tblGrid><w:gridCol w:w="2400"/></w:tblGrid>
             <w:tr><w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc></w:tr>
           </w:tbl>"#,
    );
    let Block::Table(t) = &parsed.document.body[0] else {
        panic!("expected a table");
    };
    assert_eq!(t.style.as_deref(), Some("TableGrid"));
    assert_eq!(t.width, Some(TableWidth::Pct(50.0)), "2500/50 = 50%");
    assert_eq!(t.jc, Some(Justification::Center));
    assert_eq!(t.indent, Some(360));
    let b = &t.borders;
    let top = b.top.as_ref().expect("top");
    assert_eq!((top.val.as_str(), top.sz_eighth_pt), ("single", 8));
    assert!(top.color.is_some(), "显式色保真");
    assert!(
        b.left.is_some() && b.right.is_some(),
        "start/end 别名落 left/right"
    );
    assert_eq!(b.inside_h.as_ref().map(|x| x.val.as_str()), Some("dashed"));
    assert_eq!(b.inside_v.as_ref().map(|x| x.val.as_str()), Some("none"));
    assert_eq!(
        (t.cell_margins.top, t.cell_margins.left),
        (Some(60), Some(200))
    );
    assert_eq!(t.cell_margins.bottom, None, "未给的边不猜");
}

/// trPr 的 C-7 属性(hRule / cantSplit)与 tcPr 的 C-7 属性
/// (tcBorders / vAlign / tcMar / pct-tcW)。
#[test]
fn trpr_and_tcpr_c7_props() {
    use doc_core::model::{CellVAlign, HeightRule};

    let parsed = parse_body_xml(
        r#"<w:tbl>
             <w:tblGrid><w:gridCol w:w="2400"/><w:gridCol w:w="2400"/></w:tblGrid>
             <w:tr>
               <w:trPr><w:trHeight w:val="600" w:hRule="exact"/><w:cantSplit/></w:trPr>
               <w:tc>
                 <w:tcPr>
                   <w:tcW w:w="2500" w:type="pct"/>
                   <w:tcBorders>
                     <w:top w:val="single" w:sz="12"/>
                     <w:bottom w:val="none"/>
                   </w:tcBorders>
                   <w:vAlign w:val="center"/>
                   <w:tcMar><w:start w:w="288" w:type="dxa"/><w:top w:w="120" w:type="dxa"/></w:tcMar>
                 </w:tcPr>
                 <w:p><w:r><w:t>a</w:t></w:r></w:p>
               </w:tc>
               <w:tc>
                 <w:tcPr><w:vAlign w:val="bottom"/></w:tcPr>
                 <w:p><w:r><w:t>b</w:t></w:r></w:p>
               </w:tc>
             </w:tr>
             <w:tr>
               <w:trPr><w:trHeight w:val="400" w:hRule="auto"/></w:trPr>
               <w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc>
               <w:tc><w:p><w:r><w:t>d</w:t></w:r></w:p></w:tc>
             </w:tr>
           </w:tbl>"#,
    );
    let Block::Table(t) = &parsed.document.body[0] else {
        panic!("expected a table");
    };
    let r0 = &t.rows[0];
    assert_eq!(r0.height, Some(600));
    assert_eq!(r0.height_rule, HeightRule::Exact);
    assert!(r0.cant_split);
    assert_eq!(t.rows[1].height_rule, HeightRule::Auto);
    assert!(!t.rows[1].cant_split);

    let c0 = &r0.cells[0];
    assert_eq!(c0.width, None, "pct 不落 dxa 槽");
    assert_eq!(c0.width_pct, Some(50.0));
    assert_eq!(c0.borders.top.as_ref().map(|b| b.sz_eighth_pt), Some(12));
    assert_eq!(
        c0.borders.bottom.as_ref().map(|b| b.val.as_str()),
        Some("none"),
        "显式 none 保真(冲突消解在渲染侧)"
    );
    assert_eq!(c0.borders.left, None);
    assert_eq!(c0.v_align, Some(CellVAlign::Center));
    assert_eq!((c0.margins.left, c0.margins.top), (Some(288), Some(120)));
    assert_eq!(c0.margins.right, None);
    assert_eq!(r0.cells[1].v_align, Some(CellVAlign::Bottom));
}

/// 裸 `<w:vMerge/>`(省略 val)是**延续格**(ECMA-376 §17.4.85,Word 实写形)。
#[test]
fn bare_vmerge_is_continue() {
    let parsed = parse_body_xml(
        r#"<w:tbl>
             <w:tblGrid><w:gridCol w:w="2400"/></w:tblGrid>
             <w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr>
               <w:p><w:r><w:t>anchor</w:t></w:r></w:p></w:tc></w:tr>
             <w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc></w:tr>
           </w:tbl>"#,
    );
    let Block::Table(t) = &parsed.document.body[0] else {
        panic!("expected a table");
    };
    assert_eq!(t.rows[0].cells[0].v_merge, VMerge::Restart);
    assert_eq!(
        t.rows[1].cells[0].v_merge,
        VMerge::Continue,
        "省略 val = continue"
    );
}

/// styles.xml 的表样式 tblPr 片段(tblBorders + tblCellMar)接进 `Style.tblpr`(C-7)。
#[test]
fn style_tblpr_borders_and_margins_wire_into_style_table() {
    let styles = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="table" w:styleId="TableGrid">
    <w:name w:val="Table Grid"/>
    <w:tblPr>
      <w:tblBorders>
        <w:top w:val="single" w:sz="4"/><w:insideH w:val="single" w:sz="4"/>
      </w:tblBorders>
      <w:tblCellMar><w:start w:w="120" w:type="dxa"/></w:tblCellMar>
    </w:tblPr>
  </w:style>
</w:styles>"#;
    let parsed = parse_body_with_parts(
        r#"<w:p><w:r><w:t>x</w:t></w:r></w:p>"#,
        &[("word/styles.xml", styles)],
    );
    let style = parsed
        .document
        .styles
        .styles
        .get("TableGrid")
        .expect("TableGrid style");
    assert_eq!(
        style.tblpr.borders.top.as_ref().map(|b| b.sz_eighth_pt),
        Some(4)
    );
    assert!(style.tblpr.borders.inside_h.is_some());
    assert_eq!(style.tblpr.cell_margins.left, Some(120));
}

/// §3j:`w:hyperlink@r:id` 经 `word/_rels` 解出外链 URI、`@w:anchor` 存成 "#书签",
/// 都盖到容器内 run 的 `link_target`;超链接之外的 run 不受影响。
#[test]
fn hyperlink_targets_resolve_external_and_internal() {
    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <w:body>
    <w:p>
      <w:hyperlink r:id="rId20"><w:r><w:t>ext</w:t></w:r></w:hyperlink>
      <w:r><w:t>plain</w:t></w:r>
      <w:hyperlink w:anchor="bm1"><w:r><w:t>int</w:t></w:r></w:hyperlink>
    </w:p>
  </w:body>
</w:document>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId20" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink" Target="https://example.com/x" TargetMode="External"/>
</Relationships>"#;
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        for (name, body) in [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", document),
            ("word/_rels/document.xml.rels", rels),
        ] {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    let parsed = parse_bytes(&buf.into_inner()).expect("parse hyperlink docx");
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected a paragraph");
    };
    assert_eq!(p.runs.len(), 3);
    assert_eq!(p.runs[0].text(), "ext");
    assert_eq!(
        p.runs[0].link_target.as_deref(),
        Some("https://example.com/x")
    );
    assert_eq!(p.runs[1].text(), "plain");
    assert_eq!(p.runs[1].link_target, None, "非超链接 run 不带目标");
    assert_eq!(p.runs[2].text(), "int");
    assert_eq!(p.runs[2].link_target.as_deref(), Some("#bm1"));
}
