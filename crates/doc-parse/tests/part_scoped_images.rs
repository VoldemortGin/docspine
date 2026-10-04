//! 页眉 / 脚注部件里的图片:每个部件有自己的 `.rels`,其 `rId1` 可能与 `document.xml.rels`
//! 的 `rId1` 指向不同媒体。解析阶段必须用**所在部件自己的** rels 解 `r:embed`,
//! 得到不依赖部件作用域的 `media_name`(`word/media/` 裸文件名)。
//! 现场合成 docx(纯 zip + 手写 XML + 极小合成 PNG 字节)。

use std::io::{Cursor, Write};

use doc_core::model::{Block, Picture};
use doc_parse::{parse_bytes, ParsedDoc};
use zip::write::SimpleFileOptions;
use zip::ZipWriter;

const NS: &str = r#"xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture""#;
const REL_NS: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";

/// 极小合成 PNG 字节:签名 + 带标记的 IHDR/IEND 外壳(解析层不解码图片,只搬字节)。
fn png(marker: u8) -> Vec<u8> {
    let mut v = b"\x89PNG\r\n\x1a\n".to_vec();
    v.extend_from_slice(b"IHDR");
    v.extend_from_slice(&[0, 0, 0, 1, 0, 0, 0, 1, 8, 2, 0, 0, 0, marker]);
    v.extend_from_slice(b"IEND");
    v
}

fn rels(entries: &[(&str, &str, &str)]) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">{}</Relationships>"#,
        entries
            .iter()
            .map(|(id, ty, target)| format!(
                r#"<Relationship Id="{id}" Type="{REL_NS}/{ty}" Target="{target}"/>"#
            ))
            .collect::<String>()
    )
}

/// 引用 `rel_id` 的行内图片 run。
fn pic_run(rel_id: &str) -> String {
    format!(
        r#"<w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><a:graphic><a:graphicData><pic:pic><pic:blipFill><a:blip r:embed="{rel_id}"/></pic:blipFill></pic:pic></a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"#
    )
}

fn first_picture(blocks: &[Block]) -> &Picture {
    let Block::Paragraph(p) = &blocks[0] else {
        panic!("paragraph")
    };
    &p.runs[0].pictures[0]
}

/// 正文、页眉、脚注都用 `rId1`,但分别指向 body.png / head.png / note.png。
fn build() -> ParsedDoc {
    let body = format!(
        r#"<w:p>{}</w:p><w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr>"#,
        pic_run("rId1")
    );
    let files: Vec<(&str, Vec<u8>)> = vec![
        (
            "word/document.xml",
            format!(
                r#"<?xml version="1.0"?><w:document {NS}><w:body>{body}</w:body></w:document>"#
            )
            .into_bytes(),
        ),
        (
            "word/_rels/document.xml.rels",
            rels(&[("rId1", "image", "media/body.png"), ("rIdH", "header", "header1.xml")])
                .into_bytes(),
        ),
        (
            "word/header1.xml",
            format!(r#"<w:hdr {NS}><w:p>{}</w:p></w:hdr>"#, pic_run("rId1")).into_bytes(),
        ),
        (
            "word/_rels/header1.xml.rels",
            rels(&[("rId1", "image", "media/head.png")]).into_bytes(),
        ),
        (
            "word/footnotes.xml",
            format!(
                r#"<w:footnotes {NS}><w:footnote w:id="1"><w:p>{}</w:p></w:footnote></w:footnotes>"#,
                pic_run("rId1")
            )
            .into_bytes(),
        ),
        (
            "word/_rels/footnotes.xml.rels",
            rels(&[("rId1", "image", "media/note.png")]).into_bytes(),
        ),
        ("word/media/body.png", png(1)),
        ("word/media/head.png", png(2)),
        ("word/media/note.png", png(3)),
    ];
    let mut buf = Cursor::new(Vec::new());
    {
        let mut zip = ZipWriter::new(&mut buf);
        let opts = SimpleFileOptions::default();
        for (name, bytes) in files {
            zip.start_file(name, opts).expect("start_file");
            zip.write_all(&bytes).expect("write");
        }
        zip.finish().expect("finish zip");
    }
    parse_bytes(&buf.into_inner()).expect("parse synthetic docx")
}

#[test]
fn header_footnote_and_body_images_resolve_with_their_own_part_rels() {
    let d = build();
    let doc = &d.document;
    let body = first_picture(&doc.body);
    let head = first_picture(&doc.header_footers["rIdH"]);
    let note = first_picture(&doc.footnotes[&1]);

    // 三处 rel_id 撞号,但各自解到自己部件 rels 指向的媒体。
    assert_eq!(
        (
            body.rel_id.as_str(),
            head.rel_id.as_str(),
            note.rel_id.as_str()
        ),
        ("rId1", "rId1", "rId1")
    );
    assert_eq!(body.media_name.as_deref(), Some("body.png"));
    assert_eq!(head.media_name.as_deref(), Some("head.png"));
    assert_eq!(note.media_name.as_deref(), Some("note.png"));

    // media 名是不依赖部件作用域的标识:各自取到正确字节,长度索引也对。
    assert_eq!(d.media["body.png"], png(1));
    assert_eq!(d.media["head.png"], png(2));
    assert_eq!(d.media["note.png"], png(3));
    assert_eq!(head.image_bytes_len, png(2).len());
}
