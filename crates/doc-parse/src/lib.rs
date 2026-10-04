#![forbid(unsafe_code)]
//! `doc-parse` —— docspine 的 OOXML 读取层(本轮核心)。
//!
//! 把一个 `.docx`(zip + XML)解析成 [`ParsedDoc`]:一个 [`Document`] 结构化模型,外加一份
//! `media` 字节表(`裸文件名 -> 原始图片字节`)。解析全程容错,失败收敛成 [`DocError`]。
//!
//! 旧二进制 `.doc`(OLE/CFB)在 [`legacy`] 模块里做**能力探测 + 类型化降级**(默认 docx 优先,
//! 完整正文重建后续),见 [`legacy::probe_doc`]。

mod xml;
mod zip_pkg;

pub mod legacy;

use std::collections::BTreeMap;
use std::path::Path;

use doc_core::model::{Block, Document, Section};
use doc_core::{DocError, Result};

pub use doc_core::LimitKind;
use zip_pkg::Package;
pub use zip_pkg::ZipLimits;

/// 解析输出:结构化文档 + media 字节(键为裸文件名,如 `image1.png`)。
#[derive(Debug, Clone)]
pub struct ParsedDoc {
    pub document: Document,
    pub media: BTreeMap<String, Vec<u8>>,
}

/// 从磁盘路径解析一个 `.docx`(缺省 [`ZipLimits`])。
pub fn parse_path(path: &Path) -> Result<ParsedDoc> {
    parse_path_with_limits(path, &ZipLimits::default())
}

/// 以给定 [`ZipLimits`] 从磁盘路径解析一个 `.docx`。
pub fn parse_path_with_limits(path: &Path, limits: &ZipLimits) -> Result<ParsedDoc> {
    let bytes = std::fs::read(path)?;
    parse_bytes_with_limits(&bytes, limits)
}

/// 从内存字节解析一个 `.docx`。
///
/// 若字节看起来是旧二进制 `.doc`(OLE/CFB 复合文档,魔数 `D0 CF 11 E0`),返回一个带提示的
/// [`DocError::Unsupported`](docx 优先,旧二进制 `.doc` 走 [`legacy`] 探测,正文重建后续)。
/// 使用缺省 [`ZipLimits`];zip 炸弹 / 超多条目等触达限额时返回 [`DocError::LimitExceeded`]。
pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedDoc> {
    parse_bytes_with_limits(bytes, &ZipLimits::default())
}

/// 以给定 [`ZipLimits`] 从内存字节解析一个 `.docx`(语义同 [`parse_bytes`])。
pub fn parse_bytes_with_limits(bytes: &[u8], limits: &ZipLimits) -> Result<ParsedDoc> {
    // 旧二进制 .doc 的早判:CFB 魔数。给出清晰的类型化降级,而不是含糊的 zip 错误。
    if bytes.len() >= 8 && bytes[..8] == legacy::CFB_MAGIC {
        return Err(DocError::Unsupported(
            "input is a legacy binary .doc (OLE/CFB compound document); docspine targets .docx \
             (OOXML) first — full binary .doc body reconstruction is deferred. Use \
             doc_parse::legacy::probe_doc for basic detection."
                .into(),
        ));
    }

    let pkg = Package::open_bytes_with_limits(bytes, limits)?;

    // 1) media:一次性收集字节 + 建立长度索引(供 Picture.image_bytes_len 回填)。
    let media = pkg.collect_media();
    let media_index: BTreeMap<String, usize> =
        media.iter().map(|(k, v)| (k.clone(), v.len())).collect();

    // 2) word/document.xml(必有) + 其 rels(把图片 r:id 映射到 media 名)。
    let doc_xml = pkg.document_xml()?;
    let rels_xml = pkg.document_rels_str();

    // 3) 走 w:body -> 块序列(段落 + 表格,表格是重点)+ 节序列(sectPr 页面几何)。
    let (body, mut sections) = xml::document::parse(&doc_xml, rels_xml.as_deref(), &media_index);

    // 3b) 页眉页脚:节里只有 r:id 引用,经主文档 rels 定位 `word/header*.xml` /
    //     `word/footer*.xml`;内容复用块级解析。指向同一部件的多个 r:id 归一成第一个,
    //     关系 / 部件缺失的引用丢弃。脚注尾注走固定部件名 `word/footnotes.xml` / `endnotes.xml`。
    let header_footers =
        load_header_footers(&pkg, rels_xml.as_deref(), &media_index, &mut sections);
    let footnotes = pkg
        .part_str("word/footnotes.xml")
        .map(|s| {
            xml::document::parse_notes(
                &s,
                pkg_rels(&pkg, "word/footnotes.xml").as_deref(),
                &media_index,
                b"footnote",
            )
        })
        .unwrap_or_default();
    let endnotes = pkg
        .part_str("word/endnotes.xml")
        .map(|s| {
            xml::document::parse_notes(
                &s,
                pkg_rels(&pkg, "word/endnotes.xml").as_deref(),
                &media_index,
                b"endnote",
            )
        })
        .unwrap_or_default();
    // 批注:固定部件名 `word/comments.xml`(与脚注尾注一致),带自己的 rels。
    let comments = pkg
        .part_str("word/comments.xml")
        .map(|s| {
            xml::document::parse_comments(
                &s,
                pkg_rels(&pkg, "word/comments.xml").as_deref(),
                &media_index,
            )
        })
        .unwrap_or_default();

    // 4) 跨部件表(C-5/C-6):styles.xml -> 样式表、numbering.xml -> 编号表、
    //    theme1.xml -> 主题;部件缺失时为空缺省(有效样式解析器落到 Word 内置兜底,
    //    列表段按普通段渲染)。级联/计数在 doc-core,这里只机械搬运。
    let styles = pkg
        .styles_xml_str()
        .map(|s| xml::styles::parse(&s))
        .unwrap_or_default();
    let numbering = pkg
        .numbering_xml_str()
        .map(|s| xml::numbering::parse(&s))
        .unwrap_or_default();
    let theme = pkg
        .theme_xml_str()
        .map(|s| xml::theme::parse(&s))
        .unwrap_or_default();
    let settings_xml = pkg.settings_xml_str();
    let default_tab_stop = settings_xml.as_deref().and_then(xml::settings::parse);
    let even_and_odd_headers = settings_xml
        .as_deref()
        .is_some_and(xml::settings::even_and_odd_headers);

    Ok(ParsedDoc {
        document: Document {
            body,
            sections,
            styles,
            theme,
            numbering,
            default_tab_stop,
            header_footers,
            even_and_odd_headers,
            footnotes,
            endnotes,
            comments,
        },
        media,
    })
}

/// 某部件自己的 `.rels` 文本(可缺)。
fn pkg_rels(pkg: &Package, part: &str) -> Option<String> {
    pkg.part_str(&xml::part_rels_path(part))
}

/// 解析各节引用的页眉 / 页脚部件,返回 `归一后的 r:id -> 块序列`,并就地改写 / 过滤各节的引用
/// (见 [`parse_bytes_with_limits`] 步骤 3b)。
fn load_header_footers(
    pkg: &Package,
    rels_xml: Option<&str>,
    media_index: &BTreeMap<String, usize>,
    sections: &mut [Section],
) -> BTreeMap<String, Vec<Block>> {
    let mut parts = BTreeMap::new();
    let Some(rels_xml) = rels_xml else {
        for s in sections.iter_mut() {
            s.headers.clear();
            s.footers.clear();
        }
        return parts;
    };
    let rels = xml::parse_rels(rels_xml);
    // 部件路径 -> 归一后的 r:id(先到先得)。
    let mut canon: BTreeMap<String, String> = BTreeMap::new();
    for sect in sections.iter_mut() {
        for refs in [&mut sect.headers, &mut sect.footers] {
            refs.retain_mut(|r| {
                let Some(rel) = rels.get(&r.rel_id) else {
                    return false;
                };
                let path = xml::part_path_from_target(&rel.target);
                let Some(part_xml) = pkg.part_str(&path) else {
                    return false;
                };
                let id = canon
                    .entry(path.clone())
                    .or_insert_with(|| r.rel_id.clone());
                r.rel_id = id.clone();
                parts.entry(id.clone()).or_insert_with(|| {
                    xml::document::parse_hdr_ftr(
                        &part_xml,
                        pkg_rels(pkg, &path).as_deref(),
                        media_index,
                    )
                });
                true
            });
        }
    }
    parts
}
