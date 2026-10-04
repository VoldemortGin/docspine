//! 生成种子语料(现场构造,不落二进制 fixture;`fuzz/corpus/` 已 .gitignore)。
//!
//! 用法(从仓库根):`cargo run --manifest-path fuzz/Cargo.toml --bin make_seeds`
//! 写入 `fuzz/corpus/{parse_docx,render_pdf,parse_document_xml,parse_parts}/`:
//! - `parse_document_xml`:裸 `document.xml`;
//! - `parse_docx`:完整 docx(含 styles / numbering / rels);
//! - `render_pdf`:同 `parse_docx` 的 docx 种子 + 裸 `document.xml`(见 target 说明);
//! - `parse_parts`:首字节部件种类(0 document / 1 styles / 2 numbering)+ 该部件 XML。

use std::fs;
use std::path::Path;

use docspine_fuzz::{pack, CONTENT_TYPES, ROOT_RELS};

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
  xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
  xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
  xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
  xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
  xmlns:wps="http://schemas.microsoft.com/office/word/2010/wordprocessingShape"
  xmlns:v="urn:schemas-microsoft-com:vml""#;

const DOC_RELS: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rIdImg" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
  <Relationship Id="rIdSt" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/styles" Target="styles.xml"/>
  <Relationship Id="rIdNum" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/numbering" Target="numbering.xml"/>
</Relationships>"#;

const STYLES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults><w:rPrDefault><w:rPr><w:rFonts w:ascii="Calibri"/><w:sz w:val="22"/></w:rPr></w:rPrDefault></w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading1"><w:name w:val="heading 1"/><w:basedOn w:val="Normal"/>
    <w:pPr><w:keepNext/><w:spacing w:before="240" w:after="60"/></w:pPr><w:rPr><w:b/><w:sz w:val="32"/></w:rPr></w:style>
  <w:style w:type="table" w:styleId="Grid"><w:name w:val="Table Grid"/>
    <w:tblPr><w:tblBorders><w:top w:val="single" w:sz="4"/><w:bottom w:val="single" w:sz="4"/></w:tblBorders></w:tblPr></w:style>
</w:styles>"#;

const NUMBERING: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0"><w:lvl w:ilvl="0"><w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/>
    <w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr></w:lvl></w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
</w:numbering>"#;

/// basedOn 链(带根上的 numPr)+ 一个环:覆盖样式链截断 / 防环路径。
const STYLES_CHAIN: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:style w:type="paragraph" w:styleId="A"><w:pPr><w:numPr><w:numId w:val="1"/></w:numPr></w:pPr></w:style>
  <w:style w:type="paragraph" w:styleId="B"><w:basedOn w:val="A"/></w:style>
  <w:style w:type="paragraph" w:styleId="C"><w:basedOn w:val="B"/></w:style>
  <w:style w:type="paragraph" w:styleId="X"><w:basedOn w:val="Y"/></w:style>
  <w:style w:type="paragraph" w:styleId="Y"><w:basedOn w:val="X"/></w:style>
</w:styles>"#;

/// 脚注部件(与 `field_choices_math` 里的引用配套)。
const NOTES: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:footnotes xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:footnote w:id="1"><w:p><w:r><w:t>note</w:t></w:r></w:p></w:footnote>
  <w:footnote w:id="2"/>
</w:footnotes>"#;

/// 一个 1x1 PNG 的最小头部字节(够 media 路径用;不是合法图也无妨)。
const PNG_STUB: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR\0\0\0\x01\0\0\0\x01\x08\x06\0\0\0";

/// `w:body` 内容片段,每个对应一类解析路径。
const BODIES: &[(&str, &str)] = &[
    (
        "plain",
        r#"<w:p><w:r><w:t>Hello, docspine</w:t></w:r></w:p><w:sectPr><w:pgSz w:w="11906" w:h="16838"/><w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"/></w:sectPr>"#,
    ),
    (
        "styled_list",
        r#"<w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Title</w:t></w:r></w:p>
<w:p><w:pPr><w:numPr><w:ilvl w:val="0"/><w:numId w:val="1"/></w:numPr><w:jc w:val="both"/></w:pPr>
  <w:r><w:rPr><w:b/><w:i/><w:color w:val="FF0000"/><w:sz w:val="28"/><w:vertAlign w:val="superscript"/></w:rPr><w:t xml:space="preserve">item </w:t></w:r><w:r><w:tab/><w:t>two</w:t><w:br/></w:r></w:p>"#,
    ),
    (
        "table",
        r#"<w:tbl><w:tblPr><w:tblStyle w:val="Grid"/><w:tblW w:w="5000" w:type="pct"/></w:tblPr>
<w:tblGrid><w:gridCol w:w="3000"/><w:gridCol w:w="3000"/></w:tblGrid>
<w:tr><w:tc><w:tcPr><w:gridSpan w:val="2"/><w:shd w:val="clear" w:fill="DDDDDD"/></w:tcPr><w:p><w:r><w:t>merged</w:t></w:r></w:p></w:tc></w:tr>
<w:tr><w:tc><w:tcPr><w:vMerge w:val="restart"/></w:tcPr><w:p><w:r><w:t>a</w:t></w:r></w:p></w:tc>
<w:tc><w:tbl><w:tblGrid><w:gridCol w:w="1000"/></w:tblGrid><w:tr><w:tc><w:p><w:r><w:t>nested</w:t></w:r></w:p></w:tc></w:tr></w:tbl><w:p/></w:tc></w:tr>
<w:tr><w:tc><w:tcPr><w:vMerge/></w:tcPr><w:p/></w:tc><w:tc><w:p/></w:tc></w:tr></w:tbl>"#,
    ),
    (
        "wrappers_fields",
        r#"<w:p><w:ins w:id="1" w:author="x"><w:r><w:t>ins</w:t></w:r></w:ins><w:moveTo w:id="2" w:author="x"><w:r><w:t>mv</w:t></w:r></w:moveTo>
<w:smartTag w:uri="u" w:element="e"><w:r><w:t>smart</w:t></w:r></w:smartTag>
<w:hyperlink r:id="rIdImg"><w:r><w:t>link</w:t></w:r></w:hyperlink>
<w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE </w:instrText></w:r><w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>1</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>
<w:r><w:sym w:font="Wingdings" w:char="F0FC"/></w:r><w:r><w:noBreakHyphen/></w:r></w:p>"#,
    ),
    (
        "drawing_textbox",
        r#"<w:p><w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><a:graphic><a:graphicData><a:blip r:embed="rIdImg"/></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>
<w:r><mc:AlternateContent><mc:Choice Requires="wps"><w:drawing><wp:anchor behindDoc="1"><wp:positionH relativeFrom="page"><wp:posOffset>100</wp:posOffset></wp:positionH><wp:extent cx="500000" cy="500000"/>
<a:graphic><a:graphicData><wps:wsp><wps:txbx><w:txbxContent><w:p><w:r><w:t>boxed</w:t></w:r></w:p></w:txbxContent></wps:txbx></wps:wsp></a:graphicData></a:graphic></wp:anchor></w:drawing></mc:Choice>
<mc:Fallback><w:pict><v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>fb</w:t></w:r></w:p></w:txbxContent></v:textbox></v:shape></w:pict></mc:Fallback></mc:AlternateContent></w:r></w:p>"#,
    ),
    (
        "field_choices_math",
        r#"<w:p><w:r><w:fldChar w:fldCharType="begin"/></w:r><w:r><w:instrText> PAGE \* roman </w:instrText></w:r>
<mc:AlternateContent><mc:Choice Requires="w14"><w:r><w:instrText>zz</w:instrText></w:r></mc:Choice></mc:AlternateContent>
<w:r><w:fldChar w:fldCharType="separate"/></w:r><w:r><w:t>i</w:t></w:r><w:r><w:t>ii</w:t></w:r><w:r><w:fldChar w:fldCharType="end"/></w:r>
<w:r><w:footnoteReference w:id="1"/></w:r></w:p>
<w:p><m:oMath xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math"><m:nary><m:naryPr><m:chr m:val="∑"/></m:naryPr><m:sub><m:r><m:t>i=1</m:t></m:r></m:sub><m:sup><m:r><m:t>n</m:t></m:r></m:sup>
<m:e><m:d><m:dPr><m:begChr m:val="["/><m:endChr m:val=""/></m:dPr><m:e><m:m><m:mr><m:e><m:r><m:t>a</m:t></m:r></m:e></m:mr></m:m></m:e></m:d></m:e></m:nary></m:oMath>
<w:r><w:t>[x](javascript:1) &lt;b&gt; # - 1. |</w:t></w:r></w:p>"#,
    ),
    (
        "sections_cols",
        r#"<w:p><w:pPr><w:sectPr><w:cols w:num="2" w:space="720"/><w:pgSz w:w="12240" w:h="15840" w:orient="landscape"/></w:sectPr></w:pPr><w:r><w:t>s1</w:t></w:r></w:p>
<w:p><w:r><w:t>s2</w:t></w:r></w:p><w:sectPr><w:type w:val="nextPage"/></w:sectPr>"#,
    ),
];

fn document_xml(body: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\" standalone=\"yes\"?>\n<w:document {NS}>\n  <w:body>{body}</w:body>\n</w:document>"
    )
}

fn write(dir: &Path, name: &str, bytes: &[u8]) {
    fs::create_dir_all(dir).expect("mkdir corpus dir");
    fs::write(dir.join(name), bytes).expect("write seed");
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus");
    let mut count = 0;
    for (name, body) in BODIES {
        let xml = document_xml(body);
        let docx = pack(&[
            ("[Content_Types].xml", CONTENT_TYPES.as_bytes()),
            ("_rels/.rels", ROOT_RELS.as_bytes()),
            ("word/document.xml", xml.as_bytes()),
            ("word/_rels/document.xml.rels", DOC_RELS.as_bytes()),
            ("word/styles.xml", STYLES.as_bytes()),
            ("word/numbering.xml", NUMBERING.as_bytes()),
            ("word/media/image1.png", PNG_STUB),
        ]);
        write(
            &root.join("parse_document_xml"),
            &format!("{name}.xml"),
            xml.as_bytes(),
        );
        write(&root.join("parse_docx"), &format!("{name}.docx"), &docx);
        write(&root.join("render_pdf"), &format!("{name}.docx"), &docx);
        write(
            &root.join("render_pdf"),
            &format!("{name}.xml"),
            xml.as_bytes(),
        );
        write(
            &root.join("parse_parts"),
            &format!("{name}.bin"),
            &[&[0u8][..], xml.as_bytes()].concat(),
        );
        count += 1;
    }
    for (kind, name, xml) in [
        (1u8, "styles", STYLES),
        (2, "numbering", NUMBERING),
        (1, "styles_chain_cycle", STYLES_CHAIN),
        (4, "footnotes", NOTES),
    ] {
        write(
            &root.join("parse_parts"),
            &format!("{name}.bin"),
            &[&[kind][..], xml.as_bytes()].concat(),
        );
    }
    println!("wrote seeds for {count} bodies under {}", root.display());
}
