//! 解析 `docProps/core.xml`(文档核心属性):逐个子元素按本地名取纯文本。
//!
//! 容错:畸形 / 截断 XML 保留已读到的字段,绝不 panic;空元素当缺失。

use doc_core::model::CoreProperties;
use quick_xml::events::Event;
use quick_xml::Reader;

use super::{local_name, skip_element};

/// 解析 `core.xml` 文本。根元素的直接子元素中认得的取其文本,其余跳过。
pub fn parse(xml: &str) -> CoreProperties {
    let mut props = CoreProperties::default();
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_root = false;
    loop {
        match reader.read_event_into(&mut buf) {
            // 根 `cp:coreProperties`:进入后逐个读子元素。
            Ok(Event::Start(_)) if !in_root => in_root = true,
            Ok(Event::Start(e)) => {
                let name = local_name(e.name().as_ref()).to_vec();
                let slot = match name.as_slice() {
                    b"title" => Some(&mut props.title),
                    b"subject" => Some(&mut props.subject),
                    b"creator" => Some(&mut props.creator),
                    b"keywords" => Some(&mut props.keywords),
                    b"description" => Some(&mut props.description),
                    b"category" => Some(&mut props.category),
                    b"lastModifiedBy" => Some(&mut props.last_modified_by),
                    b"revision" => Some(&mut props.revision),
                    b"created" => Some(&mut props.created),
                    b"modified" => Some(&mut props.modified),
                    b"language" => Some(&mut props.language),
                    _ => None,
                };
                match slot {
                    Some(slot) => {
                        let text = read_text(&mut reader);
                        if !text.is_empty() {
                            *slot = Some(text);
                        }
                    }
                    None => skip_element(&mut reader),
                }
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    props
}

/// 读当前元素的纯文本直到其结束标签(已消费起始标签)。读错误 / 截断时返回已读到的部分。
fn read_text(reader: &mut Reader<&[u8]>) -> String {
    let mut out = String::new();
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Text(t)) => {
                if let Ok(s) = t.unescape() {
                    out.push_str(&s);
                }
            }
            Ok(Event::CData(c)) => out.push_str(&String::from_utf8_lossy(&c)),
            Ok(Event::End(_)) | Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    out.trim().to_string()
}
