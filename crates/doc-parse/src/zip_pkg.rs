//! docx zip 容器读取。
//!
//! `.docx` = OOXML = 一个 zip 包。这里把整个包**一次性读进内存**(文档通常不大),
//! 然后按名取用各 XML 部件与 media 字节。所有失败收敛成 [`DocError::Zip`]。

use std::collections::BTreeMap;
use std::io::{Cursor, Read};

use doc_core::{DocError, Result};
use zip::ZipArchive;

/// 解包后的 docx 原始部件集合(尚未解析 XML)。
pub struct Package {
    /// 部件路径 -> 原始字节(如 `word/document.xml`)。包含 XML 与 media。
    parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    /// 从内存字节打开一个 docx 包,读出全部条目。
    pub fn open_bytes(bytes: &[u8]) -> Result<Package> {
        let reader = Cursor::new(bytes);
        let mut archive =
            ZipArchive::new(reader).map_err(|e| DocError::Zip(format!("open archive: {e}")))?;
        let mut parts = BTreeMap::new();
        for i in 0..archive.len() {
            let mut file = archive
                .by_index(i)
                .map_err(|e| DocError::Zip(format!("entry {i}: {e}")))?;
            // 跳过目录条目。
            if file.is_dir() {
                continue;
            }
            // 用 zip 规范化的名字(始终是 `/` 分隔)。
            let name = file.name().to_string();
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf)
                .map_err(|e| DocError::Zip(format!("read {name}: {e}")))?;
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
