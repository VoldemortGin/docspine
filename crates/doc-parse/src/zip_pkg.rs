//! docx zip 容器读取。
//!
//! `.docx` = OOXML = 一个 zip 包。这里把整个包**一次性读进内存**(文档通常不大),
//! 然后按名取用各 XML 部件与 media 字节。zip 层失败收敛成 [`DocError::Zip`];触达
//! [`ZipLimits`] 的资源限额(zip 炸弹 / 超多条目 / 超长名)收敛成 [`DocError::LimitExceeded`]。

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use doc_core::{DocError, LimitKind, Result};
use zip::ZipArchive;

use crate::xml::{parse_rels, part_rels_path, resolve_part_path};

/// 主文档部件的缺省路径(包根 `_rels/.rels` 缺失 / 畸形 / 指向不存在的部件时的回退)。
const DEFAULT_MAIN_PART: &str = "word/document.xml";

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
    /// 主文档部件路径(经包根 `_rels/.rels` 的 `officeDocument` 关系定位,缺省 `word/document.xml`)。
    main: String,
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
        let main = locate_main_part(&parts);
        Ok(Package { parts, main })
    }

    /// 主文档部件的包内路径。
    pub fn main_part(&self) -> &str {
        &self.main
    }

    /// 主文档部件所在目录(包根为空串),相对 Target 的解析基准。
    pub fn main_dir(&self) -> &str {
        self.main.rsplit_once('/').map_or("", |(dir, _)| dir)
    }

    /// 附属部件的包内路径:优先经主部件 rels 按关系类型(`rel_suffix`,如 `styles`)定位 ——
    /// Target 相对主部件目录解析,逃出包根或指向不存在的部件都视为没找到 —— 找不到回退 `fallback`
    /// (固定路径,保持历史行为)。
    pub fn related_part(&self, rel_suffix: &str, fallback: &str) -> String {
        let suffix = format!("/{rel_suffix}");
        self.part_str(&part_rels_path(&self.main))
            .map(|xml| parse_rels(&xml))
            .and_then(|rels| {
                rels.values()
                    .filter(|r| r.rel_type.ends_with(&suffix))
                    .filter_map(|r| resolve_part_path(self.main_dir(), &r.target))
                    .find(|p| self.parts.contains_key(p))
            })
            .unwrap_or_else(|| fallback.to_string())
    }

    /// 取一个部件并解码为 UTF-8 字符串(XML 部件用)。
    pub fn part_str(&self, name: &str) -> Option<String> {
        self.parts
            .get(name)
            .map(|v| String::from_utf8_lossy(v).into_owned())
    }

    /// 主文档部件的文本(必有,缺失即非法 docx)。
    pub fn document_xml(&self) -> Result<String> {
        self.part_str(&self.main)
            .ok_or_else(|| DocError::Zip(format!("missing main document part {}", self.main)))
    }

    /// 主文档关系文件(如 `word/_rels/document.xml.rels`)的文本(把 `r:embed/r:id` 映射到 media)。
    pub fn document_rels_str(&self) -> Option<String> {
        self.part_str(&part_rels_path(&self.main))
    }

    /// 主题部件路径:经主部件 rels 定位,回退标准名 `word/theme/theme1.xml`;再不行容错取
    /// `word/theme/` 下第一个 `.xml`(BTreeMap 序,确定性)。可缺。
    pub fn theme_path(&self) -> Option<String> {
        let path = self.related_part("theme", "word/theme/theme1.xml");
        if self.parts.contains_key(&path) {
            return Some(path);
        }
        self.parts
            .keys()
            .find(|k| k.starts_with("word/theme/") && k.ends_with(".xml"))
            .cloned()
    }

    /// 收集 `word/media/*`(及主部件目录下的 `media/*`)字节,键为**裸文件名**(如 `image1.png`)。
    pub fn collect_media(&self) -> BTreeMap<String, Vec<u8>> {
        let main_media = match self.main_dir() {
            "" => "media/".to_string(),
            dir => format!("{dir}/media/"),
        };
        let mut out = BTreeMap::new();
        for (k, v) in &self.parts {
            let rest = k
                .strip_prefix("word/media/")
                .or_else(|| k.strip_prefix(main_media.as_str()));
            if let Some(rest) = rest {
                if !rest.is_empty() && !rest.contains('/') {
                    out.insert(rest.to_string(), v.clone());
                }
            }
        }
        out
    }
}

/// 主文档部件定位:包根 `_rels/.rels` 里类型以 `/officeDocument` 结尾、Target 解析后确实存在的
/// 第一条关系;`.rels` 缺失 / 畸形 / 无合法关系时回退 [`DEFAULT_MAIN_PART`]。
fn locate_main_part(parts: &BTreeMap<String, Vec<u8>>) -> String {
    parts
        .get("_rels/.rels")
        .map(|b| parse_rels(&String::from_utf8_lossy(b)))
        .and_then(|rels| {
            rels.values()
                .filter(|r| r.rel_type.ends_with("/officeDocument"))
                .filter_map(|r| resolve_part_path("", &r.target))
                .find(|p| parts.contains_key(p))
        })
        .unwrap_or_else(|| DEFAULT_MAIN_PART.to_string())
}
