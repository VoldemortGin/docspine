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
