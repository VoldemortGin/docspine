//! fuzz target 与种子生成器共用的最小 `.docx` 打包帮助函数(现场构造,不落二进制 fixture)。

use std::io::{Cursor, Write};

use zip::write::SimpleFileOptions;
use zip::ZipWriter;

pub const CONTENT_TYPES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"#;

pub const ROOT_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"#;

/// 把任意字节当作 `word/document.xml`,配上最小必需部件打成 `.docx`。
pub fn pack_document_xml(document_xml: &[u8]) -> Vec<u8> {
    pack(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("word/document.xml", document_xml),
    ])
}

/// 按给定 `(部件名, 字节)` 打 zip(deflate)。
pub fn pack(parts: &[(&str, &[u8])]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        for (name, body) in parts {
            zip.start_file(*name, opts).expect("start_file");
            zip.write_all(body).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    buf.into_inner()
}

const W: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships""#;

/// 最小合法 `word/document.xml`:标题段、编号段(numId 1)、脚注 / 批注引用、一张表、
/// 引用页眉 / 页脚的节,让各附属部件(styles / numbering / header / footnotes / comments)被真正用到。
fn minimal_document() -> String {
    format!(
        r#"<w:document {W}><w:body>
<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>T</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr></w:pPr><w:r><w:t>item</w:t></w:r>
<w:r><w:footnoteReference w:id="1"/></w:r><w:r><w:commentReference w:id="0"/></w:r></w:p>
<w:tbl><w:tblGrid><w:gridCol w:w="2000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>c</w:t></w:r></w:p></w:tc></w:tr></w:tbl>
<w:sectPr><w:headerReference w:type="default" r:id="rIdHdr"/><w:footerReference w:type="default" r:id="rIdFtr"/><w:pgSz w:w="11906" w:h="16838"/></w:sectPr>
</w:body></w:document>"#
    )
}

const DOC_RELS: &str = r#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
<Relationship Id="rIdHdr" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/header" Target="header1.xml"/>
<Relationship Id="rIdFtr" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/footer" Target="footer1.xml"/>
</Relationships>"#;

fn minimal_styles() -> String {
    format!(
        r#"<w:styles {W}><w:docDefaults><w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>
<w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
<w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/><w:rPr><w:b/></w:rPr></w:style></w:styles>"#
    )
}

fn minimal_numbering() -> String {
    format!(
        r#"<w:numbering {W}><w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/></w:lvl></w:abstractNum>
<w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num></w:numbering>"#
    )
}

fn minimal_hdr_ftr(root: &str) -> String {
    format!(r#"<w:{root} {W}><w:p><w:r><w:t>x</w:t></w:r></w:p></w:{root}>"#)
}

fn minimal_notes() -> String {
    format!(
        r#"<w:footnotes {W}><w:footnote w:id="1"><w:p><w:r><w:t>note</w:t></w:r></w:p></w:footnote></w:footnotes>"#
    )
}

fn minimal_comments() -> String {
    format!(
        r#"<w:comments {W}><w:comment w:id="0" w:author="a"><w:p><w:r><w:t>c</w:t></w:r></w:p></w:comment></w:comments>"#
    )
}

fn minimal_settings() -> String {
    format!(r#"<w:settings {W}><w:defaultTabStop w:val="720"/></w:settings>"#)
}

/// 多部件 target 的部件种类数(`pack_part` 的 `selector` 对它取模)。
pub const PART_KINDS: u8 = 7;

/// 按 `selector % PART_KINDS` 选一个部件(0 document / 1 styles / 2 numbering / 3 header /
/// 4 footnotes / 5 comments / 6 settings),用 `xml` 替换它;其余部件取最小合法内容,打成 `.docx`。
pub fn pack_part(selector: u8, xml: &[u8]) -> Vec<u8> {
    let kind = selector % PART_KINDS;
    let pick = |k: u8, default: String| -> Vec<u8> {
        if kind == k {
            xml.to_vec()
        } else {
            default.into_bytes()
        }
    };
    let document = pick(0, minimal_document());
    let styles = pick(1, minimal_styles());
    let numbering = pick(2, minimal_numbering());
    let header = pick(3, minimal_hdr_ftr("hdr"));
    let footnotes = pick(4, minimal_notes());
    let comments = pick(5, minimal_comments());
    let settings = pick(6, minimal_settings());
    let footer = minimal_hdr_ftr("ftr");
    pack(&[
        ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
        ("_rels/.rels", ROOT_RELS.as_bytes()),
        ("word/document.xml", &document),
        ("word/_rels/document.xml.rels", DOC_RELS.as_bytes()),
        ("word/styles.xml", &styles),
        ("word/numbering.xml", &numbering),
        ("word/header1.xml", &header),
        ("word/footer1.xml", footer.as_bytes()),
        ("word/footnotes.xml", &footnotes),
        ("word/comments.xml", &comments),
        ("word/settings.xml", &settings),
    ])
}

/// 三个文本导出器各跑一遍,再把编号表里每个 numId / 层级的计数引擎连推两次(文本导出不展开
/// 编号标签,PDF 映射才会;这里补上以便无需渲染也能撞到 `w:start` 极端值),供各解析 target
/// 末尾调用。只要不 panic / 不 OOM 即可。
pub fn exercise_exports(doc: &doc_core::Document) {
    let _ = doc_core::export::to_text(doc);
    let _ = doc_core::export::to_markdown(doc);
    let _ = doc_core::export::to_html(doc);
    let mut counters = doc_core::ListCounters::new();
    for &num_id in doc.numbering.nums.keys() {
        for ilvl in 0..9 {
            for _ in 0..2 {
                let _ = counters.advance(&doc.numbering, num_id, ilvl);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 七种部件各自被替换成畸形内容时,打出的包仍能解析(`Err` 也可),且用最小内容时
    /// 各附属部件真被解析到(脚注 / 批注 / 页眉进模型)。
    #[test]
    fn pack_part_covers_every_part_kind() {
        for selector in 0..PART_KINDS {
            let _ = doc_parse::parse_bytes(&pack_part(selector, b"<not-closed"));
        }
        // selector 取模:任何字节都落在合法种类。
        let _ = doc_parse::parse_bytes(&pack_part(255, b""));
        let parsed = doc_parse::parse_bytes(&pack_part(0, minimal_document().as_bytes()))
            .expect("minimal package parses");
        let doc = &parsed.document;
        assert!(!doc.footnotes.is_empty() && !doc.comments.is_empty());
        assert!(!doc.header_footers.is_empty());
        assert!(!doc.numbering.is_empty() && !doc.styles.styles.is_empty());
        exercise_exports(doc);
    }
}
