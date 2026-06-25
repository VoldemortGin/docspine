//! 旧二进制 `.doc`(OLE/CFB 复合文档)支持 —— **能力探测 + 类型化降级**。
//!
//! ## 结论(评估)
//!
//! 现代 `.docx` 是 OOXML(zip + XML),解析清晰、信息无损,是 docspine 的**主目标**。
//! 旧的 `.doc` 是 Microsoft 的二进制 Word 格式(\[MS-DOC\]):它把内容存进一个 **OLE/CFB
//! 复合文档**(Compound File Binary,魔数 `D0 CF 11 E0 A1 B1 1A E1`)的若干“流”里——
//! 主要是 `WordDocument` 流(FIB 文件信息块 + 文本 piece)、`0Table`/`1Table` 流(格式/
//! 样式/piece table)。把这堆二进制重建成段落/表格,需要解析 FIB、CLX/piece table、
//! 字符与段落 PRM/PAPX/CHPX、复杂的 fc/cp 映射——**工程量大且繁琐**,与 docx 路径几乎无
//! 共用面。
//!
//! 因此本轮**不做** `.doc` 的完整正文重建,只做两件稳的事:
//! 1. **早判降级**:`parse_bytes` 见到 CFB 魔数即返回清晰的 [`DocError::Unsupported`],
//!    而不是含糊的 zip 错误。
//! 2. **基础探测**([`probe_doc`]):在开启 `legacy-doc` 特性时,用纯 Rust 的 `cfb` crate
//!    打开复合文档,报告它是否含 `WordDocument` 流及流名清单——为后续真正实现 `.doc`
//!    解析留好缝,但不阻塞 docx 主线。

use doc_core::{DocError, Result};

/// OLE/CFB 复合文档魔数(`.doc` / 旧 `.xls` / 旧 `.ppt` 共用此容器头)。
pub const CFB_MAGIC: [u8; 8] = [0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1];

/// 一次旧 `.doc` 探测的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocProbe {
    /// 字节头是否为 CFB 魔数(即“看起来是一个 OLE 复合文档”)。
    pub is_cfb: bool,
    /// 是否含 `WordDocument` 流(即“看起来确实是一个 Word `.doc`”)。
    pub has_word_stream: bool,
    /// 复合文档内的流/存储路径清单(best-effort;特性关闭时为空)。
    pub streams: Vec<String>,
}

/// 仅凭字节头判断是否为 CFB 复合文档(零依赖、永远可用)。
#[must_use]
pub fn looks_like_cfb(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes[..8] == CFB_MAGIC
}

/// 探测一段字节是否是旧二进制 `.doc`,并(在 `legacy-doc` 特性开启时)列出其复合文档流。
///
/// - 不是 CFB:返回 `is_cfb = false` 的 [`DocProbe`](不报错——这只是探测)。
/// - 是 CFB 但**未开启** `legacy-doc` 特性:返回 [`DocError::Unsupported`],提示开启特性。
/// - 是 CFB 且开启特性:用 `cfb` crate 打开,报告流名 + 是否含 `WordDocument` 流。
pub fn probe_doc(bytes: &[u8]) -> Result<DocProbe> {
    if !looks_like_cfb(bytes) {
        return Ok(DocProbe {
            is_cfb: false,
            has_word_stream: false,
            streams: Vec::new(),
        });
    }
    probe_cfb(bytes)
}

#[cfg(feature = "legacy-doc")]
fn probe_cfb(bytes: &[u8]) -> Result<DocProbe> {
    use std::io::Cursor;

    let comp = cfb::CompoundFile::open(Cursor::new(bytes))
        .map_err(|e| DocError::Unsupported(format!("open CFB compound document: {e}")))?;

    let mut streams: Vec<String> = comp
        .walk()
        .map(|entry| entry.path().to_string_lossy().into_owned())
        .collect();
    streams.sort();

    // Word 的正文流路径就叫 "WordDocument"(根存储下)。匹配末段不带前导斜杠。
    let has_word_stream = streams.iter().any(|p| {
        p.trim_start_matches('/')
            .eq_ignore_ascii_case("WordDocument")
    });

    Ok(DocProbe {
        is_cfb: true,
        has_word_stream,
        streams,
    })
}

#[cfg(not(feature = "legacy-doc"))]
fn probe_cfb(_bytes: &[u8]) -> Result<DocProbe> {
    Err(DocError::Unsupported(
        "input is a legacy binary .doc (OLE/CFB); enable the `legacy-doc` cargo feature for \
         basic compound-document probing. Full .doc body reconstruction is deferred (docx first)."
            .into(),
    ))
}
