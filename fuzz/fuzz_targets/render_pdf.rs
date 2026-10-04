#![no_main]
//! 任意字节 -> `parse_bytes`;解析成功再走 `doc_render::render_pdf`。只有 panic/abort/OOM 算失败。
//! 输入以 `PK` 开头按完整 docx 解析;否则当作 `word/document.xml` 现场打包(zip 的 CRC 让
//! 变异几乎都止步于容器层,这样 fuzzer 才能真正打到排版引擎)。
//! 用 `DOCSPINE_DETERMINISTIC_FONTS` 只走内置兜底字体,不扫系统字体(快且跨机器确定)。

use std::sync::Once;

use libfuzzer_sys::fuzz_target;

static INIT: Once = Once::new();

fuzz_target!(|data: &[u8]| {
    INIT.call_once(|| std::env::set_var("DOCSPINE_DETERMINISTIC_FONTS", "1"));
    let parsed = if data.starts_with(b"PK") {
        doc_parse::parse_bytes(data)
    } else {
        doc_parse::parse_bytes(&docspine_fuzz::pack_document_xml(data))
    };
    if let Ok(parsed) = parsed {
        docspine_fuzz::exercise_exports(&parsed.document);
        let _ = doc_render::render_pdf(
            &parsed.document,
            &parsed.media,
            &doc_render::RenderOptions::default(),
        );
    }
});
