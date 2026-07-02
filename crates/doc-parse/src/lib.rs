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

use doc_core::model::Document;
use doc_core::{DocError, Result};

use zip_pkg::Package;

/// 解析输出:结构化文档 + media 字节(键为裸文件名,如 `image1.png`)。
#[derive(Debug, Clone)]
pub struct ParsedDoc {
    pub document: Document,
    pub media: BTreeMap<String, Vec<u8>>,
}

/// 从磁盘路径解析一个 `.docx`。
pub fn parse_path(path: &Path) -> Result<ParsedDoc> {
    let bytes = std::fs::read(path)?;
    parse_bytes(&bytes)
}

/// 从内存字节解析一个 `.docx`。
///
/// 若字节看起来是旧二进制 `.doc`(OLE/CFB 复合文档,魔数 `D0 CF 11 E0`),返回一个带提示的
/// [`DocError::Unsupported`](docx 优先,旧二进制 `.doc` 走 [`legacy`] 探测,正文重建后续)。
pub fn parse_bytes(bytes: &[u8]) -> Result<ParsedDoc> {
    // 旧二进制 .doc 的早判:CFB 魔数。给出清晰的类型化降级,而不是含糊的 zip 错误。
    if bytes.len() >= 8 && bytes[..8] == legacy::CFB_MAGIC {
        return Err(DocError::Unsupported(
            "input is a legacy binary .doc (OLE/CFB compound document); docspine targets .docx \
             (OOXML) first — full binary .doc body reconstruction is deferred. Use \
             doc_parse::legacy::probe_doc for basic detection."
                .into(),
        ));
    }

    let pkg = Package::open_bytes(bytes)?;

    // 1) media:一次性收集字节 + 建立长度索引(供 Picture.image_bytes_len 回填)。
    let media = pkg.collect_media();
    let media_index: BTreeMap<String, usize> =
        media.iter().map(|(k, v)| (k.clone(), v.len())).collect();

    // 2) word/document.xml(必有) + 其 rels(把图片 r:id 映射到 media 名)。
    let doc_xml = pkg.document_xml()?;
    let rels_xml = pkg.document_rels_str();

    // 3) 走 w:body -> 块序列(段落 + 表格,表格是重点)+ 节序列(sectPr 页面几何)。
    let (body, sections) = xml::document::parse(&doc_xml, rels_xml.as_deref(), &media_index);

    // 4) 跨部件表(C-5):styles.xml -> 样式表、theme1.xml -> 主题;部件缺失时为空缺省
    //    (有效样式解析器落到 Word 内置兜底)。级联合并在 doc-core::style,这里只机械搬运。
    let styles = pkg
        .styles_xml_str()
        .map(|s| xml::styles::parse(&s))
        .unwrap_or_default();
    let theme = pkg
        .theme_xml_str()
        .map(|s| xml::theme::parse(&s))
        .unwrap_or_default();

    Ok(ParsedDoc {
        document: Document {
            body,
            sections,
            styles,
            theme,
        },
        media,
    })
}
