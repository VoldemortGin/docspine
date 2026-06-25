//! quick-xml walker —— WordprocessingML 解析。
//!
//! - [`document`]:解析 `word/document.xml`(`w:body` -> `Vec<Block>`,表格是重点)。
//!
//! 本模块根放**关系(`.rels`)解析**与一批被多处复用的小工具(本地名、属性读取等)。
//! 所有 walker 都遵循家族约定:未知元素跳过、缺失属性 → `None`、**绝不 panic**。

pub mod document;

use std::collections::BTreeMap;

use quick_xml::events::attributes::Attribute;
use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;

/// 一个 OOXML 关系条目(`<Relationship Id="rIdN" Type="..." Target="..."/>`)。
///
/// docspine 当前只按 `r:id` 取 `target`(图片定位),`id`/`rel_type` 保留以完整刻画关系
/// 形状、供后续按类型过滤(如 header/footer/footnotes 关系)使用。
#[derive(Debug, Clone)]
pub struct Relationship {
    #[allow(dead_code)]
    pub id: String,
    #[allow(dead_code)]
    pub rel_type: String,
    pub target: String,
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
