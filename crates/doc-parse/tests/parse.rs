//! `doc-parse` 验收测试:用 `zip` 写出一个最小但合法的 `.docx`,断言 `parse_bytes`
//! 还原出段落(带样式 run)、表格(单元格 + **横向 gridSpan 合并** + **纵向 vMerge 合并** +
//! **嵌套表** + 单元格填充)。
//!
//! 不落二进制 fixture —— docx 在测试里现合成,确定性、自包含。

use std::io::{Cursor, Write};

use doc_core::model::{Block, VMerge};
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

/// 把上面的部件压成一个内存里的 `.docx` zip 字节串。
fn build_minimal_docx() -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        for (name, body) in [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", DOCUMENT),
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
    parse_bytes(&build_minimal_docx()).expect("parse minimal docx")
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
    assert_eq!(run.text, "Hello docspine");
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
