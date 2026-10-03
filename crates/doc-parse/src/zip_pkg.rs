//! docx zip 容器读取。
//!
//! `.docx` = OOXML = 一个 zip 包。这里把整个包**一次性读进内存**(文档通常不大),
//! 然后按名取用各 XML 部件与 media 字节。zip 层失败收敛成 [`DocError::Zip`];触达
//! [`ZipLimits`] 的资源限额(zip 炸弹 / 超多条目 / 超长名)收敛成 [`DocError::LimitExceeded`]。

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use doc_core::{DocError, LimitKind, Result};
use zip::ZipArchive;

const MIB: u64 = 1024 * 1024;

/// 压缩比检查只对声明未压缩大小超过该阈值的条目生效(小条目高压缩比很正常)。
const RATIO_MIN_BYTES: u64 = MIB;

/// 读取 zip 包时的资源限额(防 zip 炸弹 / 恶意包拖垮进程)。
///
/// 用 [`ZipLimits::default`] 取缺省值,按需覆盖单个字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZipLimits {
    /// 条目总数上限(含目录条目)。缺省 10 000。
    pub max_entries: usize,
    /// 单条目解压字节上限(声明值与实际读出值都检查)。缺省 256 MiB。
    pub max_entry_bytes: u64,
    /// 全包累计实际解压字节上限。缺省 1 GiB。
    pub max_total_bytes: u64,
    /// 单条目压缩比(未压缩 / 压缩)上限,声明值与实际读出值各查一次;仅当未压缩量 > 1 MiB
    /// 时判定。缺省 10 000:deflate 的理论上限约 1032:1,永远触发不了它(大块零的纯色位图
    /// 等合法 media 不误伤);内存风险已由单条目 / 总量上限兜住,这里只拦 bzip2 / zstd /
    /// lzma 等其他压缩方法的极端比值炸弹。
    pub max_compression_ratio: u32,
    /// 条目名字节长度上限。缺省 1024。
    pub max_name_len: usize,
}

impl Default for ZipLimits {
    fn default() -> Self {
        ZipLimits {
            max_entries: 10_000,
            max_entry_bytes: 256 * MIB,
            max_total_bytes: 1024 * MIB,
            max_compression_ratio: 10_000,
            max_name_len: 1024,
        }
    }
}

fn limit_err(kind: LimitKind, limit: u64, actual: u64) -> DocError {
    DocError::LimitExceeded {
        kind,
        limit,
        actual,
    }
}

/// 压缩比检查:未压缩量 > 1 MiB 且 `未压缩 / 压缩 > max_compression_ratio` 即超限;
/// 压缩大小为 0 时比值视为无穷(`actual = u64::MAX`)。
fn check_ratio(uncompressed: u64, compressed: u64, limits: &ZipLimits) -> Result<()> {
    let max_ratio = u64::from(limits.max_compression_ratio);
    if uncompressed > RATIO_MIN_BYTES && uncompressed > compressed.saturating_mul(max_ratio) {
        // 向上取整,保证 actual > limit。
        let actual = match compressed {
            0 => u64::MAX,
            c => uncompressed.div_ceil(c),
        };
        return Err(limit_err(LimitKind::CompressionRatio, max_ratio, actual));
    }
    Ok(())
}

/// 条目名是否安全:拒绝绝对路径 / 盘符前缀 / 任何 `..` 组件(`/` 与 `\` 都按分隔符看)。
fn is_safe_name(name: &str) -> bool {
    !name.starts_with(['/', '\\'])
        && name.as_bytes().get(1) != Some(&b':')
        && !name.split(['/', '\\']).any(|c| c == "..")
}

/// 解包后的 docx 原始部件集合(尚未解析 XML)。
pub struct Package {
    /// 部件路径 -> 原始字节(如 `word/document.xml`)。包含 XML 与 media。
    parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    /// 以 [`ZipLimits`] 限额从内存字节打开一个 docx 包,读出全部条目。
    pub fn open_bytes_with_limits(bytes: &[u8], limits: &ZipLimits) -> Result<Package> {
        let reader = Cursor::new(bytes);
        let mut archive =
            ZipArchive::new(reader).map_err(|e| DocError::Zip(format!("open archive: {e}")))?;
        if archive.len() > limits.max_entries {
            return Err(limit_err(
                LimitKind::Entries,
                limits.max_entries as u64,
                archive.len() as u64,
            ));
        }
        let mut parts = BTreeMap::new();
        let mut total: u64 = 0;
        for i in 0..archive.len() {
            let file = archive
                .by_index(i)
                .map_err(|e| DocError::Zip(format!("entry {i}: {e}")))?;
            // 用 zip 规范化的名字(始终是 `/` 分隔)。
            let name = file.name().to_string();
            if name.len() > limits.max_name_len {
                return Err(limit_err(
                    LimitKind::NameLength,
                    limits.max_name_len as u64,
                    name.len() as u64,
                ));
            }
            if file.enclosed_name().is_none() || !is_safe_name(&name) {
                return Err(DocError::Zip(format!("unsafe entry path: {name:?}")));
            }
            // 跳过目录条目。
            if file.is_dir() {
                continue;
            }
            let declared = file.size();
            if declared > limits.max_entry_bytes {
                return Err(limit_err(
                    LimitKind::EntryBytes,
                    limits.max_entry_bytes,
                    declared,
                ));
            }
            let compressed = file.compressed_size();
            check_ratio(declared, compressed, limits)?;
            // 不信任声明大小做预分配;最多读到「本条目上限」与「剩余总额」中较小者 + 1 字节,
            // 多读出的那 1 字节即证明超限(防声明造假)。
            let remaining = limits.max_total_bytes.saturating_sub(total);
            let cap = limits.max_entry_bytes.min(remaining);
            let mut buf = Vec::new();
            file.take(cap.saturating_add(1))
                .read_to_end(&mut buf)
                .map_err(|e| DocError::Zip(format!("read {name}: {e}")))?;
            let got = buf.len() as u64;
            if got > limits.max_entry_bytes {
                return Err(limit_err(
                    LimitKind::EntryBytes,
                    limits.max_entry_bytes,
                    got,
                ));
            }
            // 实际读出量再查一次压缩比(声明可以造假)。
            check_ratio(got, compressed, limits)?;
            total += got;
            if total > limits.max_total_bytes {
                return Err(limit_err(
                    LimitKind::TotalBytes,
                    limits.max_total_bytes,
                    total,
                ));
            }
            parts.insert(name, buf);
        }
        Ok(Package { parts })
    }

    /// 取一个部件并解码为 UTF-8 字符串(XML 部件用)。
    pub fn part_str(&self, name: &str) -> Option<String> {
        self.parts
            .get(name)
            .map(|v| String::from_utf8_lossy(v).into_owned())
    }

    /// 主文档部件 `word/document.xml` 的文本(必有,缺失即非法 docx)。
    pub fn document_xml(&self) -> Result<String> {
        self.part_str("word/document.xml")
            .ok_or_else(|| DocError::Zip("missing word/document.xml".into()))
    }

    /// 主文档关系文件 `word/_rels/document.xml.rels` 的文本(把 `r:embed/r:id` 映射到 media)。
    pub fn document_rels_str(&self) -> Option<String> {
        self.part_str("word/_rels/document.xml.rels")
    }

    /// 样式部件 `word/styles.xml` 的文本(可缺;缺失即空样式表)。
    pub fn styles_xml_str(&self) -> Option<String> {
        self.part_str("word/styles.xml")
    }

    /// 编号部件 `word/numbering.xml` 的文本(可缺;缺失即空编号表,列表段按普通段渲染)。
    pub fn numbering_xml_str(&self) -> Option<String> {
        self.part_str("word/numbering.xml")
    }

    /// 设置部件 `word/settings.xml` 的文本(可缺;缺失即全 Word 缺省)。
    pub fn settings_xml_str(&self) -> Option<String> {
        self.part_str("word/settings.xml")
    }

    /// 主题部件文本:标准名 `word/theme/theme1.xml`;容错取 `word/theme/` 下第一个
    /// `.xml`(BTreeMap 序,确定性)。可缺;缺失即空主题。
    pub fn theme_xml_str(&self) -> Option<String> {
        self.part_str("word/theme/theme1.xml").or_else(|| {
            self.parts
                .iter()
                .find(|(k, _)| k.starts_with("word/theme/") && k.ends_with(".xml"))
                .map(|(_, v)| String::from_utf8_lossy(v).into_owned())
        })
    }

    /// 收集全部 `word/media/*` 字节,键为**裸文件名**(如 `image1.png`)。
    pub fn collect_media(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        for (k, v) in &self.parts {
            if let Some(rest) = k.strip_prefix("word/media/") {
                if !rest.is_empty() && !rest.contains('/') {
                    out.insert(rest.to_string(), v.clone());
                }
            }
        }
        out
    }
}
