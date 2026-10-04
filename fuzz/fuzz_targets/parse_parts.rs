#![no_main]
//! 首字节选部件种类(document / styles / numbering / header / footnotes / comments / settings),
//! 其余字节当作该部件的 XML,其它部件取最小合法内容,打成 docx 再解析,跑三个文本导出器并
//! 渲染 PDF——让 fuzzer 直接打到 `parse_document_xml` target 够不着的附属部件(编号标签 / 样式级联 /
//! 页眉页脚只在渲染映射里展开)。只有 panic/abort/OOM 算失败。
//! 用 `DOCSPINE_DETERMINISTIC_FONTS` 只走内置兜底字体,不扫系统字体。

use std::sync::Once;

use libfuzzer_sys::fuzz_target;

static INIT: Once = Once::new();

fuzz_target!(|data: &[u8]| {
    INIT.call_once(|| std::env::set_var("DOCSPINE_DETERMINISTIC_FONTS", "1"));
    let Some((&selector, xml)) = data.split_first() else {
        return;
    };
    if let Ok(parsed) = doc_parse::parse_bytes(&docspine_fuzz::pack_part(selector, xml)) {
        docspine_fuzz::exercise_exports(&parsed.document);
        let _ = doc_render::render_pdf(
            &parsed.document,
            &parsed.media,
            &doc_render::RenderOptions::default(),
        );
    }
});
