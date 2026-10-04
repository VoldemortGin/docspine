//! 静默丢正文回归:修订移动 / smartTag / customXml / mc:AlternateContent / 文本框 /
//! 符号与特殊连字符 / 复杂字段。每类元素现场构造 `document.xml`,断言 `to_text()`
//! 含被包裹的文字且顺序正确。不落二进制 fixture。

use std::io::{Cursor, Write};

use doc_core::export::{to_html, to_markdown, to_text};
use doc_core::model::{Block, RunSegment};
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
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"#;

/// 把一段 `w:body` 内容包进带常用命名空间声明的 `w:document`,压成最小 `.docx` 并解析。
fn parse_body(body_xml: &str) -> ParsedDoc {
    let doc = format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
  xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
  xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
  xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
  xmlns:w14="http://schemas.microsoft.com/office/word/2010/wordml"
  xmlns:w16se="http://schemas.microsoft.com/office/word/2015/wordml/symex"
  xmlns:v="urn:schemas-microsoft-com:vml"
  xmlns:o="urn:schemas-microsoft-com:office:office"
  mc:Ignorable="w14 w16se wps">
  <w:body>{body_xml}</w:body>
</w:document>"#
    );
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        for (name, body) in [
            ("[Content_Types].xml", CONTENT_TYPES),
            ("_rels/.rels", ROOT_RELS),
            ("word/document.xml", doc.as_str()),
            ("word/_rels/document.xml.rels", DOC_RELS),
        ] {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(body.as_bytes()).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner()).expect("parse synthetic docx")
}

fn text_of(body_xml: &str) -> String {
    to_text(&parse_body(body_xml).document)
}

/// 断言 `needles` 依次出现在 `hay` 中(顺序正确)。
fn assert_in_order(hay: &str, needles: &[&str]) {
    let mut from = 0;
    for n in needles {
        match hay[from..].find(n) {
            Some(i) => from += i + n.len(),
            None => panic!("{n:?} missing (or out of order) in {hay:?}"),
        }
    }
}

// ------------------------------------------------------------------ 1. moveTo / moveFrom

#[test]
fn move_to_kept_move_from_dropped() {
    let txt = text_of(
        r#"<w:p>
             <w:r><w:t>A </w:t></w:r>
             <w:moveFromRangeStart w:id="1" w:name="move1"/>
             <w:moveFrom w:id="2"><w:r><w:delText>OLD</w:delText></w:r></w:moveFrom>
             <w:moveFromRangeEnd w:id="1"/>
             <w:moveToRangeStart w:id="3" w:name="move1"/>
             <w:moveTo w:id="4"><w:r><w:t>moved</w:t></w:r></w:moveTo>
             <w:moveToRangeEnd w:id="3"/>
             <w:r><w:t> Z</w:t></w:r>
           </w:p>"#,
    );
    assert_eq!(txt, "A moved Z");
}

// ------------------------------------------------------------------ 2/3. smartTag / customXml

#[test]
fn smart_tag_and_inline_custom_xml_are_transparent_and_nest() {
    let txt = text_of(
        r#"<w:p>
             <w:r><w:t>one </w:t></w:r>
             <w:smartTag w:uri="urn:x" w:element="place">
               <w:smartTagPr><w:attr w:name="a" w:val="b"/></w:smartTagPr>
               <w:customXml w:element="city">
                 <w:customXmlPr><w:attr w:name="k" w:val="v"/></w:customXmlPr>
                 <w:r><w:t>two</w:t></w:r>
               </w:customXml>
               <w:r><w:t> three</w:t></w:r>
             </w:smartTag>
             <w:hyperlink w:anchor="bm"><w:smartTag w:element="x"><w:r><w:t> four</w:t></w:r></w:smartTag></w:hyperlink>
           </w:p>"#,
    );
    assert_eq!(txt, "one two three four");
}

#[test]
fn block_level_custom_xml_is_transparent_in_body_and_cell() {
    let parsed = parse_body(
        r#"<w:p><w:r><w:t>before</w:t></w:r></w:p>
           <w:customXml w:element="root">
             <w:customXmlPr/>
             <w:p><w:r><w:t>cx para</w:t></w:r></w:p>
             <w:customXml w:element="inner">
               <w:tbl><w:tblGrid><w:gridCol w:w="1200"/></w:tblGrid>
                 <w:tr><w:tc>
                   <w:customXml w:element="cell"><w:p><w:r><w:t>cell cx</w:t></w:r></w:p></w:customXml>
                 </w:tc></w:tr>
               </w:tbl>
             </w:customXml>
           </w:customXml>
           <w:p><w:r><w:t>after</w:t></w:r></w:p>"#,
    );
    let body = &parsed.document.body;
    assert_eq!(body.len(), 4);
    assert!(matches!(body[2], Block::Table(_)));
    assert_eq!(to_text(&parsed.document), "before\ncx para\ncell cx\nafter");
}

// ------------------------------------------------------------------ 4. mc:AlternateContent

#[test]
fn alternate_content_run_level_falls_back_when_choice_unknown() {
    // Word 写 emoji 的真实形态:Choice 是本解析器不认识的 w16se:symEx -> 取 Fallback。
    let txt = text_of(
        r#"<w:p>
             <w:r><w:t>smile </w:t></w:r>
             <w:r>
               <mc:AlternateContent>
                 <mc:Choice Requires="w16se"><w16se:symEx w16se:font="Segoe UI Emoji" w16se:char="1F600"/></mc:Choice>
                 <mc:Fallback><w:t>😀</w:t></mc:Fallback>
               </mc:AlternateContent>
             </w:r>
             <w:r><w:t> end</w:t></w:r>
           </w:p>"#,
    );
    assert_eq!(txt, "smile 😀 end");
}

#[test]
fn alternate_content_paragraph_level_prefers_supported_choice() {
    // 段落内:Choice 产出 run -> 用 Choice,Fallback 不重复输出。
    let txt = text_of(
        r#"<w:p>
             <w:r><w:t>x </w:t></w:r>
             <mc:AlternateContent>
               <mc:Choice Requires="w14"><w:r><w:t>choice</w:t></w:r></mc:Choice>
               <mc:Fallback><w:r><w:t>fallback</w:t></w:r></mc:Fallback>
             </mc:AlternateContent>
             <w:r><w:t> y</w:t></w:r>
           </w:p>"#,
    );
    assert_eq!(txt, "x choice y");
}

#[test]
fn alternate_content_block_level_choice_and_fallback_paths() {
    let txt = text_of(
        r#"<mc:AlternateContent>
             <mc:Choice Requires="w14"><w:p><w:r><w:t>block choice</w:t></w:r></w:p></mc:Choice>
             <mc:Fallback><w:p><w:r><w:t>block fallback</w:t></w:r></w:p></mc:Fallback>
           </mc:AlternateContent>
           <mc:AlternateContent>
             <mc:Choice Requires="w99"><w99:thing xmlns:w99="urn:x"/></mc:Choice>
             <mc:Fallback><w:p><w:r><w:t>second fallback</w:t></w:r></w:p></mc:Fallback>
           </mc:AlternateContent>"#,
    );
    assert_eq!(txt, "block choice\nsecond fallback");
}

// ------------------------------------------------------------------ 5. 文本框

const WPS_TEXTBOX_RUN: &str = r#"<w:r>
  <mc:AlternateContent>
    <mc:Choice Requires="wps">
      <w:drawing>
        <wp:anchor behindDoc="0" distT="0" distB="0" distL="0" distR="0" simplePos="0" relativeHeight="1" locked="0" layoutInCell="1" allowOverlap="1">
          <wp:simplePos x="0" y="0"/>
          <wp:positionH relativeFrom="column"><wp:posOffset>100</wp:posOffset></wp:positionH>
          <wp:positionV relativeFrom="paragraph"><wp:posOffset>200</wp:posOffset></wp:positionV>
          <wp:extent cx="914400" cy="457200"/>
          <wp:docPr id="1" name="Text Box 1"/>
          <a:graphic><a:graphicData uri="http://schemas.microsoft.com/office/word/2010/wordprocessingShape">
            <wps:wsp>
              <wps:spPr/>
              <wps:txbx>
                <w:txbxContent>
                  <w:p><w:r><w:t>box line 1</w:t></w:r></w:p>
                  <w:tbl><w:tblGrid><w:gridCol w:w="1000"/></w:tblGrid>
                    <w:tr><w:tc><w:p><w:r><w:t>box cell</w:t></w:r></w:p></w:tc></w:tr>
                  </w:tbl>
                </w:txbxContent>
              </wps:txbx>
              <wps:bodyPr/>
            </wps:wsp>
          </a:graphicData></a:graphic>
        </wp:anchor>
      </w:drawing>
    </mc:Choice>
    <mc:Fallback>
      <w:pict>
        <v:shape style="width:72pt;height:36pt">
          <v:textbox><w:txbxContent><w:p><w:r><w:t>box line 1</w:t></w:r></w:p></w:txbxContent></v:textbox>
        </v:shape>
      </w:pict>
    </mc:Fallback>
  </mc:AlternateContent>
</w:r>"#;

#[test]
fn drawingml_text_box_extracted_after_its_paragraph_once() {
    let parsed = parse_body(&format!(
        r#"<w:p><w:r><w:t>anchor para</w:t></w:r>{WPS_TEXTBOX_RUN}<w:r><w:t> tail</w:t></w:r></w:p>
           <w:p><w:r><w:t>next para</w:t></w:r></w:p>"#
    ));
    let doc = &parsed.document;
    let Block::Paragraph(p) = &doc.body[0] else {
        panic!("expected paragraph");
    };
    // 段落正文本身不含文本框文字;文本框挂在 run 上。
    assert_eq!(p.text(), "anchor para tail");
    assert_eq!(p.text_boxes().count(), 1, "Choice chosen, Fallback dropped");
    assert_eq!(
        to_text(doc),
        "anchor para tail\nbox line 1\nbox cell\nnext para"
    );
    let md = to_markdown(doc);
    assert_in_order(
        &md,
        &["anchor para tail", "box line 1", "box cell", "next para"],
    );
    let html = to_html(doc);
    assert_in_order(
        &html,
        &[
            "<p>anchor para tail</p>",
            "<p>box line 1</p>",
            "<td>box cell</td>",
            "<p>next para</p>",
        ],
    );
}

#[test]
fn vml_text_box_extracted_and_following_content_kept() {
    // 旧式 VML:v:shape 内嵌 v:textbox。同时钉住 w:pict 子树被完整消费——之后的
    // run 与段落不被提前截断。
    let txt = text_of(
        r#"<w:p>
             <w:r><w:t>head </w:t></w:r>
             <w:r><w:pict>
               <v:shape style="width:72pt;height:36pt">
                 <v:imagedata r:id="rIdImg" o:title=""/>
                 <v:textbox><w:txbxContent><w:p><w:r><w:t>vml box</w:t></w:r></w:p></w:txbxContent></v:textbox>
               </v:shape>
             </w:pict></w:r>
             <w:r><w:t>same para tail</w:t></w:r>
           </w:p>
           <w:p><w:r><w:t>next para</w:t></w:r></w:p>"#,
    );
    assert_eq!(txt, "head same para tail\nvml box\nnext para");
}

#[test]
fn vml_picture_with_nested_shape_does_not_truncate_paragraph() {
    let parsed = parse_body(
        r#"<w:p>
             <w:r><w:pict><v:shape style="width:36pt;height:24pt"><v:imagedata r:id="rIdImg"/></v:shape></w:pict></w:r>
             <w:r><w:t>after pict</w:t></w:r>
           </w:p>
           <w:p><w:r><w:t>second</w:t></w:r></w:p>"#,
    );
    let doc = &parsed.document;
    assert_eq!(to_text(doc), "after pict\nsecond");
    let Block::Paragraph(p) = &doc.body[0] else {
        panic!("expected paragraph");
    };
    let pic = &p.runs[0].pictures[0];
    assert_eq!(pic.rel_id, "rIdImg");
    assert_eq!(pic.extent, Some((457_200, 304_800)));
}

#[test]
fn text_box_inside_table_cell_reaches_cell_text() {
    let parsed = parse_body(&format!(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid>
             <w:tr><w:tc><w:p><w:r><w:t>cell</w:t></w:r>{WPS_TEXTBOX_RUN}</w:p></w:tc></w:tr>
           </w:tbl>"#
    ));
    let Block::Table(t) = &parsed.document.body[0] else {
        panic!("expected table");
    };
    // 单元格便利文本:段落 + 其文本框直接段落(文本框内的表按惯例忽略)。
    assert_eq!(t.rows[0].cells[0].text(), "cell\nbox line 1");
    assert!(to_html(&parsed.document).contains("cell<br>box line 1"));
}

// ------------------------------------------------------------------ 6/7. 符号与特殊字符

#[test]
fn sym_soft_and_no_break_hyphen_ptab() {
    let parsed = parse_body(
        r#"<w:p><w:r>
             <w:t>a</w:t><w:sym w:font="Wingdings" w:char="F0E0"/>
             <w:sym w:font="Symbol" w:char="03B1"/>
             <w:t>co</w:t><w:softHyphen/><w:t>op</w:t>
             <w:t>e</w:t><w:noBreakHyphen/><w:t>mail</w:t>
             <w:ptab w:relativeTo="margin" w:alignment="right" w:leader="none"/><w:t>R</w:t>
             <w:tab/><w:t>T</w:t><w:br/><w:t>B</w:t><w:cr/><w:t>C</w:t>
             <w:sym w:font="Symbol" w:char="zz"/>
           </w:r></w:p>"#,
    );
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("expected paragraph");
    };
    assert_eq!(
        p.text(),
        "a\u{F0E0}\u{03B1}co\u{00AD}ope\u{2011}mail\tR\tT\nB\nC"
    );
    assert!(
        p.runs[0]
            .segments
            .iter()
            .filter(|s| matches!(s, RunSegment::Tab))
            .count()
            == 2
    );
}

// ------------------------------------------------------------------ 8. 复杂字段

#[test]
fn complex_field_keeps_result_and_hides_instruction() {
    let txt = text_of(
        r#"<w:p>
             <w:r><w:t>Page </w:t></w:r>
             <w:r><w:fldChar w:fldCharType="begin"/></w:r>
             <w:r><w:instrText xml:space="preserve"> PAGE \* MERGEFORMAT </w:instrText></w:r>
             <w:r><w:fldChar w:fldCharType="separate"/></w:r>
             <w:r><w:t>3</w:t></w:r>
             <w:r><w:fldChar w:fldCharType="end"/></w:r>
             <w:r><w:t> of 5</w:t></w:r>
           </w:p>"#,
    );
    assert_eq!(txt, "Page 3 of 5");
    assert!(!txt.contains("MERGEFORMAT"));
}

// ------------------------------------------------------------------ 深度守卫

#[test]
fn nested_new_containers_10k_levels_no_stack_overflow() {
    let levels = 3_000;
    let body = format!(
        "<w:p>{}<w:r><w:t>core</w:t></w:r>{}</w:p>\
         {}<w:p><w:r><w:t>core</w:t></w:r></w:p>{}\
         <w:p><w:r><w:t>after</w:t></w:r></w:p>",
        "<w:smartTag><w:customXml><w:moveTo>".repeat(levels),
        "</w:moveTo></w:customXml></w:smartTag>".repeat(levels),
        "<w:customXml><mc:AlternateContent><mc:Choice>".repeat(levels),
        "</mc:Choice></mc:AlternateContent></w:customXml>".repeat(levels),
    );
    let txt = text_of(&body);
    assert!(!txt.contains("core") && txt.ends_with("after"), "{txt:?}");
}

#[test]
fn nested_text_boxes_beyond_depth_skipped_not_panic() {
    let levels = 200;
    let open = "<w:p><w:r><w:pict><v:shape><v:textbox><w:txbxContent>";
    let close = "</w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>";
    let body = format!(
        "{}<w:p><w:r><w:t>core</w:t></w:r></w:p>{}<w:p><w:r><w:t>after</w:t></w:r></w:p>",
        open.repeat(levels),
        close.repeat(levels)
    );
    let txt = text_of(&body);
    assert!(!txt.contains("core") && txt.ends_with("after"), "{txt:?}");
}

// ------------------------------------------------------------------ 行级 / 单元格级 sdt·customXml

fn only_table(parsed: &ParsedDoc) -> &doc_core::model::Table {
    parsed
        .document
        .body
        .iter()
        .find_map(|b| match b {
            Block::Table(t) => Some(t),
            _ => None,
        })
        .expect("a table")
}

fn cell_text(c: &doc_core::model::Cell) -> String {
    c.blocks
        .iter()
        .filter_map(|b| match b {
            Block::Paragraph(p) => Some(p.text()),
            _ => None,
        })
        .collect()
}

#[test]
fn row_level_sdt_and_custom_xml_rows_are_transparent_and_nest() {
    let parsed = parse_body(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/></w:tblGrid>
             <w:tr><w:tc><w:p><w:r><w:t>r1</w:t></w:r></w:p></w:tc></w:tr>
             <w:sdt><w:sdtPr><w:alias w:val="x"/></w:sdtPr><w:sdtContent>
               <w:tr><w:tc><w:p><w:r><w:t>r2</w:t></w:r></w:p></w:tc></w:tr>
               <w:customXml w:element="e"><w:customXmlPr/>
                 <w:sdt><w:sdtContent>
                   <w:tr><w:trPr><w:tblHeader/></w:trPr>
                     <w:tc><w:p><w:r><w:t>r3</w:t></w:r></w:p></w:tc></w:tr>
                 </w:sdtContent></w:sdt>
               </w:customXml>
             </w:sdtContent></w:sdt>
             <w:tr><w:tc><w:p><w:r><w:t>r4</w:t></w:r></w:p></w:tc></w:tr>
           </w:tbl>"#,
    );
    let t = only_table(&parsed);
    let rows: Vec<String> = t.rows.iter().map(|r| cell_text(&r.cells[0])).collect();
    assert_eq!(rows, ["r1", "r2", "r3", "r4"]);
    assert!(t.rows[2].is_header);
    assert_eq!(to_text(&parsed.document).lines().count(), 4);
}

#[test]
fn cell_level_sdt_and_custom_xml_cells_keep_merges_and_order() {
    let parsed = parse_body(
        r#"<w:tbl><w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/></w:tblGrid>
             <w:tr>
               <w:tc><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>
               <w:sdt><w:sdtPr/><w:sdtContent>
                 <w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>b</w:t></w:r></w:p></w:tc>
               </w:sdtContent></w:sdt>
             </w:tr>
             <w:tr>
               <w:customXml w:element="c"><w:sdt><w:sdtContent>
                 <w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc>
               </w:sdtContent></w:sdt></w:customXml>
               <w:tc><w:tcPr><w:gridSpan w:val="2"/></w:tcPr><w:p><w:r><w:t>d</w:t></w:r></w:p></w:tc>
             </w:tr>
             <w:tr>
               <w:sdt><w:sdtContent>
                 <w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc>
               </w:sdtContent></w:sdt>
               <w:tc><w:p><w:r><w:t>e</w:t></w:r></w:p></w:tc>
               <w:tc><w:p><w:r><w:t>f</w:t></w:r></w:p></w:tc>
             </w:tr>
           </w:tbl>"#,
    );
    let t = only_table(&parsed);
    assert_eq!(t.rows[0].cells.len(), 2);
    assert_eq!(cell_text(&t.rows[0].cells[1]), "b");
    assert_eq!(t.rows[0].cells[1].grid_span, 2);
    assert_eq!(t.rows[1].cells.len(), 2);
    assert_eq!(t.rows[1].cells[0].v_merge, doc_core::model::VMerge::Restart);
    assert_eq!(t.rows[1].cells[1].grid_span, 2);
    assert_eq!(t.rows[2].cells.len(), 3);
    assert_eq!(
        t.rows[2].cells[0].v_merge,
        doc_core::model::VMerge::Continue
    );
    assert_eq!(cell_text(&t.rows[2].cells[2]), "f");
}

#[test]
fn nested_row_cell_sdt_beyond_depth_skipped_not_panic() {
    let levels = 3_000;
    let rows = format!(
        "{}<w:tr><w:tc><w:p><w:r><w:t>deep row</w:t></w:r></w:p></w:tc></w:tr>{}",
        "<w:sdt><w:sdtContent><w:customXml>".repeat(levels),
        "</w:customXml></w:sdtContent></w:sdt>".repeat(levels)
    );
    let cells = format!(
        "{}<w:tc><w:p><w:r><w:t>deep cell</w:t></w:r></w:p></w:tc>{}",
        "<w:sdt><w:sdtContent><w:customXml>".repeat(levels),
        "</w:customXml></w:sdtContent></w:sdt>".repeat(levels)
    );
    let body = format!(
        "<w:tbl><w:tr><w:tc><w:p><w:r><w:t>keep</w:t></w:r></w:p></w:tc></w:tr>{rows}\
         <w:tr>{cells}<w:tc><w:p><w:r><w:t>tail</w:t></w:r></w:p></w:tc></w:tr></w:tbl>\
         <w:p><w:r><w:t>after</w:t></w:r></w:p>"
    );
    let txt = text_of(&body);
    assert!(!txt.contains("deep"), "{txt:?}");
    assert!(
        txt.contains("keep") && txt.contains("tail") && txt.ends_with("after"),
        "{txt:?}"
    );
}

// ------------------------------------------------------------------ m:oMath / m:oMathPara

const M_NS: &str = r#"xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math""#;

#[test]
fn inline_omath_text_extracted_in_document_order() {
    let parsed = parse_body(&format!(
        r#"<w:p {M_NS}><w:r><w:t>Let </w:t></w:r>
             <m:oMath>
               <m:r><m:t>x</m:t></m:r>
               <m:f><m:num><m:r><m:t>1</m:t></m:r></m:num><m:den><m:r><m:t>2</m:t></m:r></m:den></m:f>
               <m:sSup><m:e><m:r><m:t>y</m:t></m:r></m:e><m:sup><m:r><m:t>3</m:t></m:r></m:sup></m:sSup>
             </m:oMath>
             <w:r><w:t> end</w:t></w:r></w:p>"#
    ));
    let txt = to_text(&parsed.document);
    // 复审:原期望 `x1/2y^3` 本身是错的(读成 x1 除以 2y³)。x 与分式、分式与 y^3 之间是乘法:
    // 分式整体加括号、结构与相邻项之间一个空格。
    assert_eq!(txt, "Let x (1/2) y^3 end");
    // Markdown 不转义普通括号与 `^`(最小转义集)。
    assert!(to_markdown(&parsed.document).contains("x (1/2) y^3"));
}

#[test]
fn omath_para_and_math_inside_containers_are_kept() {
    let txt = text_of(&format!(
        r#"<w:p {M_NS}><w:r><w:t>a</w:t></w:r>
             <m:oMathPara><m:oMath><m:r><m:t>E=mc</m:t></m:r></m:oMath>
               <m:oMath><m:r><m:t>F=ma</m:t></m:r></m:oMath></m:oMathPara>
           </w:p>
           <w:p {M_NS}><w:hyperlink w:anchor="k"><m:oMath><m:r><m:t>h</m:t></m:r></m:oMath></w:hyperlink>
             <w:sdt><w:sdtContent><m:oMath><m:r><m:t>s</m:t></m:r></m:oMath></w:sdtContent></w:sdt></w:p>
           <w:p {M_NS}><m:oMath><w:del w:id="1"><m:r><m:t>gone</m:t></m:r></w:del>
             <m:r><m:t>kept</m:t></m:r></m:oMath></w:p>"#
    ));
    assert_in_order(&txt, &["aE=mc F=ma", "hs", "kept"]);
    assert!(!txt.contains("gone"), "{txt:?}");
}

#[test]
fn omath_runs_are_flagged_as_math() {
    let parsed = parse_body(&format!(
        r#"<w:p {M_NS}><w:r><w:t>t</w:t></w:r><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></w:p>"#
    ));
    let Block::Paragraph(p) = &parsed.document.body[0] else {
        panic!("paragraph")
    };
    assert_eq!(
        p.runs.iter().map(|r| r.is_math).collect::<Vec<_>>(),
        [false, true]
    );
}

#[test]
fn deeply_nested_omath_no_stack_overflow() {
    let levels = 5_000;
    let body = format!(
        "<w:p {M_NS}><m:oMath>{}<m:r><m:t>deepmath</m:t></m:r>{}</m:oMath></w:p>\
         <w:p><w:r><w:t>after</w:t></w:r></w:p>",
        "<m:e>".repeat(levels),
        "</m:e>".repeat(levels)
    );
    let txt = text_of(&body);
    assert!(txt.ends_with("after"), "{txt:?}");
}

/// 单个 `m:r` 文本片段。
fn mr(t: &str) -> String {
    format!("<m:r><m:t>{t}</m:t></m:r>")
}

/// 把一段公式内部 XML 包成 `m:oMath` 段落,返回抽出的公式文本。
fn math_text(inner: &str) -> String {
    text_of(&format!("<w:p {M_NS}><m:oMath>{inner}</m:oMath></w:p>"))
}

#[test]
fn omath_fraction_is_linearized_not_glued() {
    let f = |num: &str, den: &str| format!("<m:f><m:num>{num}</m:num><m:den>{den}</m:den></m:f>");
    // 1/2 不能拼成 12。
    assert_eq!(math_text(&f(&mr("1"), &mr("2"))), "1/2");
    // 分子 / 分母含多个 m:r 片段时加括号。
    assert_eq!(math_text(&f(&(mr("a") + &mr("+b")), &mr("c"))), "(a+b)/c");
    assert_eq!(math_text(&f(&mr("a"), &(mr("c") + &mr("+d")))), "a/(c+d)");
    // 分式嵌套分式:内层是复合项,外层包括号。
    let inner = f(&mr("1"), &mr("2"));
    assert_eq!(math_text(&f(&inner, &mr("3"))), "(1/2)/3");
}

#[test]
fn omath_scripts_and_radicals_are_disambiguated() {
    let e = |t: &str| format!("<m:e>{}</m:e>", mr(t));
    // x² -> x^2(不是 x2)。
    assert_eq!(
        math_text(&format!(
            "<m:sSup>{}<m:sup>{}</m:sup></m:sSup>",
            e("x"),
            mr("2")
        )),
        "x^2"
    );
    // x_i。
    assert_eq!(
        math_text(&format!(
            "<m:sSub>{}<m:sub>{}</m:sub></m:sSub>",
            e("x"),
            mr("i")
        )),
        "x_i"
    );
    // 多片段底数加括号:(a+b)^2。
    assert_eq!(
        math_text(&format!(
            "<m:sSup><m:e>{}{}</m:e><m:sup>{}</m:sup></m:sSup>",
            mr("a"),
            mr("+b"),
            mr("2")
        )),
        "(a+b)^2"
    );
    // 根号:sqrt(x);空 m:deg(平方根)不影响。
    assert_eq!(
        math_text(&format!(
            "<m:rad><m:radPr><m:degHide m:val=\"1\"/></m:radPr><m:deg/>{}</m:rad>",
            e("x")
        )),
        "sqrt(x)"
    );
    // 带次数的根号不丢次数文字。
    assert_eq!(
        math_text(&format!(
            "<m:rad><m:deg>{}</m:deg>{}</m:rad>",
            mr("3"),
            e("x")
        )),
        "root(3,x)"
    );
}

#[test]
fn omath_unrecognized_structures_keep_plain_concatenation() {
    // 不认识的结构(这里是 `m:xyz` 外壳)仍按文档顺序纯拼接;结构外的文字与顺序不变。
    // (原先这条把 `m:d` / `m:sSubSup` 也当“其它结构”拼成 `abcde`,那正是被审查指出的错误输出:
    // 定界符括号与上下标记号丢失,见 `omath_delimiters_*` / `omath_subsup_*`。)
    let txt = math_text(&format!(
        "{}<m:xyz><m:e>{}</m:e>{}</m:xyz>",
        mr("a"),
        mr("b"),
        mr("c"),
    ));
    assert_eq!(txt, "abc");
}

#[test]
fn omath_structures_missing_slots_do_not_panic() {
    assert_eq!(math_text("<m:f><m:num/></m:f>"), "");
    assert_eq!(
        math_text(&format!("<m:f><m:num>{}</m:num></m:f>", mr("1"))),
        "1/"
    );
    assert_eq!(
        math_text(&format!("<m:sSup><m:sup>{}</m:sup></m:sSup>", mr("2"))),
        "^2"
    );
}

/// 把一段公式内部 XML 包成 `m:oMathPara > m:oMath`(Word 独占一行公式的写法)。
fn para_math_text(inner: &str) -> String {
    text_of(&format!(
        "<w:p {M_NS}><m:oMathPara><m:oMath>{inner}</m:oMath></m:oMathPara></w:p>"
    ))
}

#[test]
fn omathpara_wrapped_structures_are_linearized() {
    let e = |t: &str| format!("<m:e>{}</m:e>", mr(t));
    assert_eq!(
        para_math_text(&format!(
            "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
            mr("1"),
            mr("2")
        )),
        "1/2"
    );
    assert_eq!(
        para_math_text(&format!(
            "<m:sSup>{}<m:sup>{}</m:sup></m:sSup>",
            e("x"),
            mr("2")
        )),
        "x^2"
    );
    assert_eq!(
        para_math_text(&format!(
            "<m:sSub>{}<m:sub>{}</m:sub></m:sSub>",
            e("x"),
            mr("i")
        )),
        "x_i"
    );
    assert_eq!(
        para_math_text(&format!("<m:rad><m:deg/>{}</m:rad>", e("x"))),
        "sqrt(x)"
    );
}

#[test]
fn omathpara_with_two_omath_separated_by_space() {
    let f = |a: &str, b: &str| {
        format!(
            "<m:oMath><m:f><m:num>{}</m:num><m:den>{}</m:den></m:f></m:oMath>",
            mr(a),
            mr(b)
        )
    };
    let txt = text_of(&format!(
        "<w:p {M_NS}><m:oMathPara>{}{}</m:oMathPara></w:p>",
        f("1", "2"),
        f("3", "4")
    ));
    assert_eq!(txt, "1/2 3/4");
}

#[test]
fn omath_structures_inside_transparent_containers_are_linearized() {
    let frac = format!(
        "<m:f><m:num>{}</m:num><m:den>{}</m:den></m:f>",
        mr("1"),
        mr("2")
    );
    // 定界符 m:d > m:e > m:f:括号来自 `m:d` 的缺省 `(` `)`(原先输出 `1/2`,括号被丢)。
    assert_eq!(math_text(&format!("<m:d><m:e>{frac}</m:e></m:d>")), "(1/2)");
    // 大运算符 m:nary > m:e > m:f:缺省运算符 ∫,下限 `_i`,被积式前一个空格
    // (原先输出 `i1/2`,运算符被丢、下限与被积式粘连)。被积式 `1/2` 不是原子,加括号
    // (第二轮复审:被作用式非原子一律加括号,免得后续内容的归属不明)。
    assert_eq!(
        math_text(&format!(
            "<m:nary><m:naryPr/><m:sub>{}</m:sub><m:e>{frac}</m:e></m:nary>",
            mr("i")
        )),
        "∫_i (1/2)"
    );
    // 函数 m:func > m:fName + m:e > m:f:自变量非原子,加括号(原先输出 `sin1/2`,读作 sin1 除以 2)。
    assert_eq!(
        math_text(&format!(
            "<m:func><m:fName>{}</m:fName><m:e>{frac}</m:e></m:func>",
            mr("sin")
        )),
        "sin(1/2)"
    );
    // 结构槽位里再套定界符再套上标:sSup > e > d > e > sSup。
    let inner = format!(
        "<m:sSup><m:e>{}</m:e><m:sup>{}</m:sup></m:sSup>",
        mr("y"),
        mr("3")
    );
    assert_eq!(
        math_text(&format!(
            "<m:sSup><m:e><m:d><m:e>{inner}</m:e></m:d></m:e><m:sup>{}</m:sup></m:sSup>",
            mr("2")
        )),
        "(y^3)^2"
    );
    // 同一 m:d 内多个结构并列:用缺省分隔符 `|` 连接并加括号(原先输出 `1/21/2`,两个分式粘连)。
    assert_eq!(
        math_text(&format!("<m:d><m:e>{frac}</m:e><m:e>{frac}</m:e></m:d>")),
        "(1/2|1/2)"
    );
}

// ---- 公式线性化:括号看槽位的线性化结果(不是 m:t 片段数)、m:d / m:nary 等结构不丢记号

fn sup(base: &str, exp: &str) -> String {
    format!("<m:sSup><m:e>{base}</m:e><m:sup>{exp}</m:sup></m:sSup>")
}

fn frac(num: &str, den: &str) -> String {
    format!("<m:f><m:num>{num}</m:num><m:den>{den}</m:den></m:f>")
}

/// 审查错误 1:分子是**单个** `m:t` 写着 `a+b`,原先输出 `a+b/c`。
#[test]
fn omath_single_run_compound_numerator_is_parenthesized() {
    assert_eq!(math_text(&frac(&mr("a+b"), &mr("c"))), "(a+b)/c");
    assert_eq!(math_text(&frac(&mr("a"), &mr("c+d"))), "a/(c+d)");
}

/// 审查错误 2:上标是单个 `m:t` 的 `n+1`,原先输出 `x^n+1`。
#[test]
fn omath_single_run_compound_exponent_is_parenthesized() {
    assert_eq!(math_text(&sup(&mr("x"), &mr("n+1"))), "x^(n+1)");
    let sub = format!(
        "<m:sSub><m:e>{}</m:e><m:sub>{}</m:sub></m:sSub>",
        mr("a"),
        mr("i+1")
    );
    assert_eq!(math_text(&sub), "a_(i+1)");
}

/// 审查错误 3:`m:d` 的括号在 `m:dPr` 属性里,原先被丢:`2(x+1)` 输出 `2x+1`。
#[test]
fn omath_delimiters_use_dpr_attributes_with_defaults() {
    let d = |pr: &str, es: &[&str]| {
        let es: String = es.iter().map(|e| format!("<m:e>{}</m:e>", mr(e))).collect();
        format!("<m:d>{pr}{es}</m:d>")
    };
    assert_eq!(math_text(&(mr("2") + &d("", &["x+1"]))), "2(x+1)");
    assert_eq!(
        math_text(&d(
            r#"<m:dPr><m:begChr m:val="["/><m:endChr m:val="]"/></m:dPr>"#,
            &["a"]
        )),
        "[a]"
    );
    assert_eq!(
        math_text(&d(
            r#"<m:dPr><m:begChr m:val="{"/><m:endChr m:val="}"/></m:dPr>"#,
            &["a", "b"]
        )),
        "{a|b}"
    );
    // 自定义分隔符。
    assert_eq!(
        math_text(&d(
            r#"<m:dPr><m:sepChr m:val=","/></m:dPr>"#,
            &["a", "b", "c"]
        )),
        "(a,b,c)"
    );
    // 显式空串 = 该侧无括号;起止标签写法同样识别。
    assert_eq!(
        math_text(&d(
            r#"<m:dPr><m:begChr m:val=""/><m:endChr m:val=""></m:endChr></m:dPr>"#,
            &["a"]
        )),
        "a"
    );
    assert_eq!(
        math_text(&d(
            r#"<m:dPr><m:begChr m:val="|"/><m:endChr m:val="|"/></m:dPr>"#,
            &["x"]
        )),
        "|x|"
    );
}

/// 审查错误 4:`m:nary` 运算符在 `m:naryPr > m:chr`(缺省 ∫),原先被丢,求和输出 `i=1nx_i`。
#[test]
fn omath_nary_keeps_operator_and_bounds() {
    let xi = format!(
        "<m:sSub><m:e>{}</m:e><m:sub>{}</m:sub></m:sSub>",
        mr("x"),
        mr("i")
    );
    let nary = |pr: &str, sub: &str, sup: &str, e: &str| {
        format!("<m:nary>{pr}<m:sub>{sub}</m:sub><m:sup>{sup}</m:sup><m:e>{e}</m:e></m:nary>")
    };
    let sum_pr = r#"<m:naryPr><m:chr m:val="∑"/></m:naryPr>"#;
    assert_eq!(
        math_text(&nary(sum_pr, &mr("i=1"), &mr("n"), &xi)),
        "∑_(i=1)^n x_i"
    );
    // 缺省 ∫;上下限为空则省略对应部分;被积式为空不留尾空格。
    assert_eq!(
        math_text(&nary("", &mr("0"), &mr("1"), &mr("f"))),
        "∫_0^1 f"
    );
    assert_eq!(math_text(&nary("", "", "", &mr("f"))), "∫ f");
    assert_eq!(math_text(&nary(sum_pr, &mr("k"), "", "")), "∑_k");
    // 起止标签写法的 m:chr 同样识别。
    assert_eq!(
        math_text(&nary(
            r#"<m:naryPr><m:chr m:val="∏"></m:chr></m:naryPr>"#,
            "",
            &mr("n"),
            &mr("a")
        )),
        "∏^n a"
    );
}

/// 审查错误 4(续):`m:sSubSup` 原先拼成 `xi2`。
#[test]
fn omath_subsup_keeps_script_markers() {
    let ss = |e: &str, sub: &str, sup: &str| {
        format!("<m:sSubSup><m:e>{e}</m:e><m:sub>{sub}</m:sub><m:sup>{sup}</m:sup></m:sSubSup>")
    };
    assert_eq!(math_text(&ss(&mr("x"), &mr("i"), &mr("2"))), "x_i^2");
    assert_eq!(math_text(&ss(&mr("x"), &mr("i+1"), &mr("2"))), "x_(i+1)^2");
}

#[test]
fn omath_prescripts_limits_functions_matrix() {
    // m:sPre:前置上下标。
    let pre = format!(
        "<m:sPre><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:sPre>",
        mr("6"),
        mr("14"),
        mr("C")
    );
    // 前置上下标整体包在括号里紧贴底数(原 `_6^14 C` 的空格会让上下标看起来属于前项)。
    assert_eq!(math_text(&pre), "(_6^14)C");
    // m:limLow / m:limUpp。
    let lim = format!(
        "<m:limLow><m:e>{}</m:e><m:lim>{}</m:lim></m:limLow>",
        mr("lim"),
        mr("x→0")
    );
    assert_eq!(math_text(&lim), "lim_(x→0)");
    let upp = format!(
        "<m:limUpp><m:e>{}</m:e><m:lim>{}</m:lim></m:limUpp>",
        mr("x"),
        mr("n")
    );
    assert_eq!(math_text(&upp), "x^n");
    // m:func:自变量原子用空格,非原子加括号,函数名不丢。
    let func = |arg: &str| {
        format!(
            "<m:func><m:fName>{}</m:fName><m:e>{arg}</m:e></m:func>",
            mr("sin")
        )
    };
    assert_eq!(math_text(&func(&mr("x"))), "sin x");
    assert_eq!(math_text(&func(&mr("x+1"))), "sin(x+1)");
    // m:acc / m:bar。
    let acc = |pr: &str| format!("<m:acc>{pr}<m:e>{}</m:e></m:acc>", mr("x"));
    assert_eq!(
        math_text(&acc(r#"<m:accPr><m:chr m:val="¯"/></m:accPr>"#)),
        "x¯"
    );
    assert_eq!(math_text(&acc("")), "x\u{302}");
    let bar = |pos: &str| {
        format!(
            r#"<m:bar><m:barPr><m:pos m:val="{pos}"/></m:barPr><m:e>{}</m:e></m:bar>"#,
            mr("x")
        )
    };
    assert_eq!(math_text(&bar("top")), "overline(x)");
    assert_eq!(math_text(&bar("bot")), "underline(x)");
    // m:m 矩阵:行用 `;`、列用 `,`,整体方括号。
    let mx = |rows: &[&[&str]]| {
        let rows: String = rows
            .iter()
            .map(|r| {
                let cells: String = r.iter().map(|c| format!("<m:e>{}</m:e>", mr(c))).collect();
                format!("<m:mr>{cells}</m:mr>")
            })
            .collect();
        format!("<m:m>{rows}</m:m>")
    };
    assert_eq!(math_text(&mx(&[&["a", "b"], &["c", "d"]])), "[a,b;c,d]");
    assert_eq!(math_text(&mx(&[&["1"]])), "[1]");
}

/// 不认识的结构(`m:eqArr` 等多槽)保持按文档顺序拼接,但子结构之间加一个空格,免得相邻数字粘连。
#[test]
fn omath_unrecognized_multislot_structures_separate_children_with_space() {
    let eq = format!(
        "<m:eqArr><m:e>{}</m:e><m:e>{}</m:e></m:eqArr>",
        mr("1"),
        mr("2")
    );
    assert_eq!(math_text(&eq), "1 2");
    let boxed = format!("<m:box><m:e>{}</m:e></m:box>", mr("ab"));
    assert_eq!(math_text(&boxed), "ab");
}

/// 嵌套:分式里的上标、根号里的分式、上标里的分式。
#[test]
fn omath_nested_structures_follow_atom_rule() {
    // (x^2+1)/y:分子是复合文字(含上标结构)——分子结果 `x^2+1` 非原子,加括号。
    let num = sup(&mr("x"), &mr("2")) + &mr("+1");
    assert_eq!(math_text(&frac(&num, &mr("y"))), "(x^2+1)/y");
    // 分式里的上标 `x^2` 单独作分子:`x^2` 非原子 -> `(x^2)/y`。
    assert_eq!(
        math_text(&frac(&sup(&mr("x"), &mr("2")), &mr("y"))),
        "(x^2)/y"
    );
    // 根号里的分式:sqrt(1/2)。
    let rad = format!(
        "<m:rad><m:deg/><m:e>{}</m:e></m:rad>",
        frac(&mr("1"), &mr("2"))
    );
    assert_eq!(math_text(&rad), "sqrt(1/2)");
    // 上标里的分式:x^(1/2)。
    assert_eq!(
        math_text(&sup(&mr("x"), &frac(&mr("1"), &mr("2")))),
        "x^(1/2)"
    );
}

/// 原子判定边界:单字母 / 数字串(`3.14`)/ 被匹配括号完整包住(`(a)`)是原子(不加括号);
/// `2x` / `x1` / `α2` / `汉字`(多字符)、`-1` / `a b` / `(a)(b)` 不是;空槽位不加括号。
/// (第二轮复审:原先把纯字母数字串都当原子,`x^2x` 读不出是 x^(2x) 还是 x²·x。)
#[test]
fn omath_atom_boundaries() {
    let s = |t: &str| math_text(&sup(&mr("x"), &mr(t)));
    assert_eq!(s("2x"), "x^(2x)");
    assert_eq!(s("x1"), "x^(x1)");
    assert_eq!(s("3.14"), "x^3.14");
    assert_eq!(s("(a)"), "x^(a)");
    assert_eq!(s("-1"), "x^(-1)");
    assert_eq!(s("(a)(b)"), "x^((a)(b))");
    assert_eq!(s("a b"), "x^(a b)");
    assert_eq!(s("α2"), "x^(α2)");
    assert_eq!(s("汉字"), "x^(汉字)");
    assert_eq!(s("α"), "x^α");
    // 空槽位:不加括号(沿用 `x^` 的缺省输出)。
    assert_eq!(
        math_text("<m:sSup><m:e><m:r><m:t>x</m:t></m:r></m:e><m:sup/></m:sSup>"),
        "x^"
    );
}

/// 已被括号完整包住才算原子:`(a)+(b)` 首尾括号不配对,仍要加括号。
#[test]
fn omath_atom_requires_matching_outer_brackets() {
    assert_eq!(math_text(&sup(&mr("x"), &mr("(a)+(b)"))), "x^((a)+(b))");
    assert_eq!(math_text(&sup(&mr("x"), &mr("[a]"))), "x^[a]");
}

/// 深嵌套新结构仍是迭代实现,不栈溢出(受 `MAX_NEST_DEPTH` 约束)。
#[test]
fn deeply_nested_new_math_structures_no_stack_overflow() {
    let levels = 5_000;
    let body = format!(
        "<w:p {M_NS}><m:oMath>{}{}{}</m:oMath></w:p><w:p><w:r><w:t>after</w:t></w:r></w:p>",
        "<m:d><m:e><m:nary><m:e>".repeat(levels),
        mr("deep"),
        "</m:e></m:nary></m:e></m:d>".repeat(levels)
    );
    let txt = text_of(&body);
    assert!(
        txt.contains("deep") && txt.ends_with("after"),
        "{}",
        txt.len()
    );
}

#[test]
fn deeply_nested_math_structures_no_stack_overflow() {
    let levels = 5_000;
    let body = format!(
        "<w:p {M_NS}><m:oMath>{}{}{}</m:oMath></w:p><w:p><w:r><w:t>after</w:t></w:r></w:p>",
        "<m:f><m:num>".repeat(levels),
        mr("deep"),
        "</m:num></m:f>".repeat(levels)
    );
    let txt = text_of(&body);
    assert!(
        txt.contains("deep") && txt.ends_with("after"),
        "{}",
        txt.len()
    );
}

// ---- 公式线性化(复审第二轮):结构与相邻项的衔接、原子判定收窄。
// 读法约定(见 `xml/math.rs` 模块文档):原子 = 单字母 / 数字串 / 被匹配括号完整包住的串;
// `^` `_` `/` 只结合紧挨着的原子;空格分隔的相邻项是乘法;`+ - =` 等运算符优先级最低。

fn sub(base: &str, s: &str) -> String {
    format!("<m:sSub><m:e>{base}</m:e><m:sub>{s}</m:sub></m:sSub>")
}

fn func_of(name: &str, arg: &str) -> String {
    format!(
        "<m:func><m:fName>{}</m:fName><m:e>{arg}</m:e></m:func>",
        mr(name)
    )
}

/// 带分数 / 系数:`3` 后接分式原先输出 `31/2`(会读成 15.5),`x` 后接分式输出 `x1/2`。
#[test]
fn omath_number_or_letter_before_fraction_is_separated() {
    let half = frac(&mr("1"), &mr("2"));
    // 分式与乘法项相邻:分式整体加括号并用空格隔开,读作 3·(1/2),不会被读成 31/2。
    assert_eq!(math_text(&(mr("3") + &half)), "3 (1/2)");
    assert_eq!(math_text(&(mr("x") + &half)), "x (1/2)");
    // 分式在前、乘法项在后:`(1/2) x` 不会被读成 1/(2x)。
    assert_eq!(math_text(&(half.clone() + &mr("x"))), "(1/2) x");
    // 与低优先级运算符相邻时不必加括号:3+1/2 按优先级本就是 3+(1/2)。
    assert_eq!(math_text(&(mr("3+") + &half)), "3+1/2");
    assert_eq!(math_text(&(half + &mr("=y"))), "1/2=y");
}

/// `1/(2x)` 与 `(1/2)·x` 原先都输出 `1/2x`:两个不同的式子必须输出不同文本。
#[test]
fn omath_fraction_with_compound_denominator_differs_from_fraction_times_x() {
    let a = math_text(&frac(&mr("1"), &mr("2x")));
    let b = math_text(&(frac(&mr("1"), &mr("2")) + &mr("x")));
    // `2x` 不是原子,作分母加括号;后者分式整体加括号再接 x。
    assert_eq!(a, "1/(2x)");
    assert_eq!(b, "(1/2) x");
    assert_ne!(a, b);
}

/// 多字符底数:`(2x)²` 原先与 `2·x²` 都输出 `2x^2`;`(ab)²` 输出 `ab^2`。
#[test]
fn omath_multichar_base_is_parenthesized_and_coefficient_is_separated() {
    assert_eq!(math_text(&sup(&mr("2x"), &mr("2"))), "(2x)^2");
    // 系数 2 与结构 x^2 之间一个空格:读作 2·x²。
    assert_eq!(math_text(&(mr("2") + &sup(&mr("x"), &mr("2")))), "2 x^2");
    assert_eq!(math_text(&sup(&mr("ab"), &mr("2"))), "(ab)^2");
    // 重音同理:`ab` 不是原子,加括号后再挂重音符;单字母带重音仍是原子。
    let acc = |b: &str| {
        format!(
            r#"<m:acc><m:accPr><m:chr m:val="̇"/></m:accPr><m:e>{}</m:e></m:acc>"#,
            mr(b)
        )
    };
    assert_eq!(math_text(&acc("ab")), "(ab)\u{307}");
    assert_eq!(math_text(&acc("x")), "x\u{307}");
    assert_eq!(
        math_text(&sup(&acc("x"), &mr("2"))),
        "x\u{307}^2",
        "带组合重音的单字母是原子"
    );
}

/// 上下标后接内容:`x^23` / `x_12` / `e^xy` / `x^2y^2` 都会把后项读进上下标。
#[test]
fn omath_scripts_are_separated_from_following_terms() {
    // 上标只结合紧挨着的原子 `2`;空格之后的 3 是乘法项。
    assert_eq!(math_text(&(sup(&mr("x"), &mr("2")) + &mr("3"))), "x^2 3");
    assert_eq!(math_text(&(sub(&mr("x"), &mr("1")) + &mr("2"))), "x_1 2");
    assert_eq!(math_text(&(sup(&mr("e"), &mr("x")) + &mr("y"))), "e^x y");
    assert_eq!(
        math_text(&(sup(&mr("x"), &mr("2")) + &sup(&mr("y"), &mr("2")))),
        "x^2 y^2"
    );
    // 运算符本身就是分隔:不加空格。
    assert_eq!(math_text(&(sup(&mr("x"), &mr("2")) + &mr("+1"))), "x^2+1");
}

/// 相邻函数:`sin x` 后接 `cos x` 原先输出 `sin xcos x`。
#[test]
fn omath_adjacent_functions_are_separated() {
    let s = func_of("sin", &mr("x"));
    let c = func_of("cos", &mr("x"));
    // 前一个函数右侧紧邻乘法项:自变量改写成括号形式,杜绝读成 sin(x cos x)。
    assert_eq!(math_text(&(s.clone() + &c)), "sin(x) cos x");
    // 真正的 sin(x cos x) 输出不同。
    assert_eq!(math_text(&func_of("sin", &(mr("x") + &c))), "sin(x cos x)");
    assert_eq!(math_text(&(s + &mr("y"))), "sin(x) y");
}

/// 前置上下标:`x` 后接 `₆¹⁴C` 原先输出 `x_6^14 C`(上下标挂到了 x 上)。
#[test]
fn omath_prescript_after_term_binds_to_its_own_base() {
    let pre = format!(
        "<m:sPre><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{}</m:e></m:sPre>",
        mr("6"),
        mr("14"),
        mr("C")
    );
    // 前置上下标整体包在括号里紧贴底数 C,与前项 x 之间一个空格。
    assert_eq!(math_text(&(mr("x") + &pre)), "x (_6^14)C");
}

/// 求和的被加式:`x_i+1` 原先输出 `∑_(i=1)^n x_i+1`,看不出 +1 在不在求和内。
#[test]
fn omath_nary_operand_scope_is_explicit() {
    let sum = |e: &str| {
        format!(
            r#"<m:nary><m:naryPr><m:chr m:val="∑"/></m:naryPr><m:sub>{}</m:sub><m:sup>{}</m:sup><m:e>{e}</m:e></m:nary>"#,
            mr("i=1"),
            mr("n")
        )
    };
    let xi = sub(&mr("x"), &mr("i"));
    // +1 在求和内:被加式非原子,加括号。
    assert_eq!(
        math_text(&sum(&(xi.clone() + &mr("+1")))),
        "∑_(i=1)^n (x_i+1)"
    );
    // +1 在求和外:被加式是带下标的原子 `x_i`,不加括号;`+` 优先级最低,读作 (∑ x_i)+1。
    assert_eq!(math_text(&(sum(&xi) + &mr("+1"))), "∑_(i=1)^n x_i+1");
    // 求和后紧跟乘法项:整个求和加括号,y 不在求和内。
    assert_eq!(math_text(&(sum(&xi) + &mr("y"))), "(∑_(i=1)^n x_i) y");
}

/// `m:d` 的 `sepChr=""`:原先两个 `m:e` 粘成一项 `(ab)`。
#[test]
fn omath_delimiter_with_empty_separator_keeps_elements_apart() {
    let d = format!(
        r#"<m:d><m:dPr><m:sepChr m:val=""/></m:dPr><m:e>{}</m:e><m:e>{}</m:e></m:d>"#,
        mr("a"),
        mr("b")
    );
    assert_eq!(math_text(&d), "(a b)");
}
