//! quick-xml walker —— WordprocessingML 解析。
//!
//! - [`document`]:解析 `word/document.xml`(`w:body` -> `Vec<Block>`,表格是重点)。
//! - [`styles`]:解析 `word/styles.xml`(docDefaults + 样式定义 -> `StyleTable`,C-5)。
//! - [`numbering`]:解析 `word/numbering.xml`(编号层级 + 实例 -> `NumberingTable`,C-6)。
//! - [`theme`]:解析 `word/theme/theme1.xml`(fontScheme + clrScheme -> `Theme`,C-5)。
//! - [`settings`]:解析 `word/settings.xml`(缺省制表位间隔,C-9)。
//! - [`props`]:document.xml 与 styles.xml 共用的 rPr / pPr / 表格属性片段解析器。
//!
//! 本模块根放**关系(`.rels`)解析**与一批被多处复用的小工具(本地名、属性读取、跳树等)。
//! 所有 walker 都遵循家族约定:未知元素跳过、缺失属性 → `None`、**绝不 panic**。

pub mod document;
pub mod numbering;
pub mod props;
pub mod settings;
pub mod styles;
pub mod theme;

use std::collections::BTreeMap;

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// 一个 OOXML 关系条目(`<Relationship Id="rIdN" Type="..." Target="..."/>`)。
///
/// docspine 当前只按 `r:id` 取 `target`(图片 / 页眉页脚部件定位),`id`/`rel_type` 保留以
/// 完整刻画关系形状、供后续按类型过滤使用。
#[derive(Debug, Clone)]
pub struct Relationship {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub rel_type: String,
    pub target: String,
}

/// 一个部件解析期收集的诊断计数(由 `document` 模块的 `Ctx` 累加,`lib.rs` 转成
/// `doc_core::Diagnostic`)。只计数,**不含任何正文**。
#[derive(Debug, Default, Clone, Copy)]
pub struct PartStats {
    /// 超过嵌套深度上限被整棵跳过的子树数。
    pub nest_skipped: usize,
    /// 被丢弃 / 钳制的表格列数(`gridCol` 多余列 + `gridBefore` / `gridAfter` 钳制次数)。
    pub cols_clamped: usize,
    /// `gridSpan` 被钳的单元格数。
    pub span_clamped: usize,
    /// 指向缺失 media 的图片数。
    pub missing_media: usize,
    /// `w:altChunk` 个数(含页眉页脚 / 注内的)。
    pub alt_chunks: usize,
}

/// 一份 XML 是否完整良构地读到了结尾。各 walker 遇读错误都是 `break`、返回已解析的部分;
/// 这里单独走一遍:读错误(畸形 / 标签错配)或在元素未闭合时遇到 EOF(中途截断)都算不完整。
/// 集中在一处判定,避免每个 walker 各写一遍。
pub fn is_complete_xml(xml: &str) -> bool {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    let mut depth = 0usize;
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => depth = depth.saturating_sub(1),
            Ok(Event::Eof) => return depth == 0,
            Err(_) => return false,
            _ => {}
        }
        buf.clear();
    }
}

/// 解析一份 `.rels` XML,得到 `rId -> Relationship` 映射。容错:解析出错则返回已得部分。
pub fn parse_rels(xml: &str) -> BTreeMap<String, Relationship> {
    let mut map = BTreeMap::new();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"Relationship" {
                    let mut id = String::new();
                    let mut rel_type = String::new();
                    let mut target = String::new();
                    for attr in e.attributes().flatten() {
                        match attr.key.as_ref() {
                            b"Id" => id = attr_string(&attr),
                            b"Type" => rel_type = attr_string(&attr),
                            b"Target" => target = attr_string(&attr),
                            _ => {}
                        }
                    }
                    if !id.is_empty() {
                        map.insert(
                            id.clone(),
                            Relationship {
                                id,
                                rel_type,
                                target,
                            },
                        );
                    }
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    map
}

/// 把主文档关系的 `Target` 规范化成 media map 的键(裸文件名)。
/// docx 里 `document.xml.rels` 的图片 Target 形如 `media/image1.png`,偶有 `../media/...`。
pub fn media_name_from_target(target: &str) -> String {
    let mut t = target;
    while let Some(rest) = t.strip_prefix("../") {
        t = rest;
    }
    t.rsplit('/').next().unwrap_or(t).to_string()
}

/// 把关系的 `Target` 解析成包内部件路径:`/` 开头视作包根绝对路径,否则相对 `base_dir`
/// (持有该关系的部件所在目录,如 `word`;包根为空串),`.` / `..` 组件折叠。`..` 越过包根
/// 视为非法返回 `None`(拒绝逃出包根)。如 (`word`, `header1.xml`) -> `word/header1.xml`。
pub fn resolve_part_path(base_dir: &str, target: &str) -> Option<String> {
    let joined = match target.strip_prefix('/') {
        Some(abs) => abs.to_string(),
        None if base_dir.is_empty() => target.to_string(),
        None => format!("{base_dir}/{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for seg in joined.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            s => parts.push(s),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// 一个部件自己的关系文件路径:`word/header1.xml` -> `word/_rels/header1.xml.rels`。
pub fn part_rels_path(part: &str) -> String {
    match part.rsplit_once('/') {
        Some((dir, name)) => format!("{dir}/_rels/{name}.rels"),
        None => format!("_rels/{part}.rels"),
    }
}

/// 取一个(可能带命名空间前缀的)元素名的本地名,如 `w:p` -> `p`。
pub fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().position(|&b| b == b':') {
        Some(i) => &qname[i + 1..],
        None => qname,
    }
}

/// 把一个属性的值解码成 `String`(容错:解码失败给空串)。
pub fn attr_string(attr: &Attribute) -> String {
    attr.unescape_value()
        .map(|c| c.into_owned())
        .unwrap_or_default()
}

/// 取元素的某个属性值(按本地名匹配,忽略命名空间前缀)。
pub fn attr_of(e: &BytesStart, key: &[u8]) -> Option<String> {
    for attr in e.attributes().flatten() {
        if local_name(attr.key.as_ref()) == key {
            return Some(attr_string(&attr));
        }
    }
    None
}

/// 读取一个 WordprocessingML 布尔型开关元素的 `w:val`。
///
/// 这类元素(`w:b` / `w:i` / `w:tblHeader` 等)的语义:元素**存在且无 `val`** 即为真;
/// `val="0"`/`"false"`/`"off"` 为假;`val="1"`/`"true"`/`"on"` 为真。
pub fn on_off_val(e: &BytesStart) -> bool {
    match attr_of(e, b"val") {
        None => true,
        Some(v) => !(v == "0" || v.eq_ignore_ascii_case("false") || v.eq_ignore_ascii_case("off")),
    }
}

/// 是否为格式修订容器(`w:pPrChange` / `w:rPrChange` / `w:tcPrChange` / `w:tblPrChange` /
/// `w:trPrChange` / `w:sectPrChange`)。其内装的是**修订前**的旧属性,按“接受全部修订”
/// 各属性 walker 遇到它必须 [`skip_element`] 整体跳过,否则旧值会后写胜出覆盖现值。
pub fn is_prop_change(name: &[u8]) -> bool {
    matches!(
        name,
        b"pPrChange"
            | b"rPrChange"
            | b"tcPrChange"
            | b"tblPrChange"
            | b"trPrChange"
            | b"sectPrChange"
    )
}

/// 跳过当前已打开元素的全部内容,直到其匹配的结束标签。已消费该元素的起始标签。
/// 通过深度计数处理同名嵌套。各部件 walker 共用。
pub fn skip_element<R: std::io::BufRead>(reader: &mut Reader<R>) {
    let mut depth = 1usize;
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(_)) => depth += 1,
            Ok(Event::End(_)) => {
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
}
