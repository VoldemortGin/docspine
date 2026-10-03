#![no_main]
//! 任意字节 -> `parse_bytes`(缺省 `ZipLimits`)。`Err` 都可接受;只有 panic/abort/OOM 算失败。

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = doc_parse::parse_bytes(data);
});
