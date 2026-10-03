#![no_main]
//! 任意字节当作 `word/document.xml`,现场包进最小 zip 再解析——让 fuzzer 直接打到 XML 解析层,
//! 不必先"猜"出合法 zip 结构。只有 panic/abort/OOM 算失败。

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = doc_parse::parse_bytes(&docspine_fuzz::pack_document_xml(data));
});
