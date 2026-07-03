//! 解析 `word/settings.xml`(文档设置)—— 目前只取 `w:defaultTabStop@w:val`
//! (缺省制表位间隔,twip;C-9 的制表位推进依据)。
//!
//! 与 numbering / styles walker 同一套模式:定位根元素,机械搬运需要的属性;
//! 容错——部件畸形 / 属性缺失 → `None`,渲染侧落 Word 缺省 720 twip。

use doc_core::geom::Twips;
use quick_xml::events::Event;
use quick_xml::Reader;

use super::{attr_of, local_name};

/// 解析 `word/settings.xml` 文本,返回 `w:defaultTabStop@w:val`(twip)。
/// 非正值 / 缺失 / 畸形输入 → `None`。
pub fn parse(xml: &str) -> Option<Twips> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(false);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Empty(e)) | Ok(Event::Start(e)) => {
                if local_name(e.name().as_ref()) == b"defaultTabStop" {
                    return attr_of(&e, b"val")
                        .and_then(|s| s.parse::<Twips>().ok())
                        .filter(|v| *v > 0);
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    None
}

// ============================================================ 单测

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_default_tab_stop() {
        let xml = r#"<?xml version="1.0"?>
<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:zoom w:percent="100"/>
  <w:defaultTabStop w:val="1440"/>
</w:settings>"#;
        assert_eq!(parse(xml), Some(1440));
    }

    #[test]
    fn missing_or_malformed_yields_none() {
        assert_eq!(parse(""), None);
        assert_eq!(parse("<w:settings/>"), None);
        assert_eq!(
            parse(r#"<w:settings><w:defaultTabStop w:val="-3"/></w:settings>"#),
            None,
            "非正值容错"
        );
        assert_eq!(parse("not xml"), None);
    }
}
