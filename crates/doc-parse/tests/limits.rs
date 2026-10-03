//! zip 包读取限额([`ZipLimits`])与 XML 嵌套深度守卫的验收测试。
//!
//! 恶意包全部在测试里用 `zip` 现场构造(必要时改写头部的声明大小),不落二进制 fixture。
//! zip 限额用例断言返回对应的 [`DocError::LimitExceeded`];嵌套深度用例断言超限子树被静默
//! 跳过 —— 全程不 panic、不 OOM、不栈溢出。

use std::io::{Cursor, Write};

use doc_core::model::Block;
use doc_core::DocError;
use doc_parse::{parse_bytes, parse_bytes_with_limits, LimitKind, ZipLimits};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

const W_NS: &str = "http://schemas.openxmlformats.org/wordprocessingml/2006/main";

fn document_xml(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?><w:document xmlns:w="{W_NS}"><w:body>{body}</w:body></w:document>"#
    )
}

/// 用给定条目 `(名字, 内容, 压缩方式)` 写一个 zip。
fn build_zip(entries: &[(&str, &[u8], CompressionMethod)]) -> Vec<u8> {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data, method) in entries {
        zip.start_file(
            *name,
            SimpleFileOptions::default().compression_method(*method),
        )
        .unwrap();
        zip.write_all(data).unwrap();
    }
    zip.finish().unwrap().into_inner()
}

/// 一个合法的最小 docx,外加若干额外条目。
fn docx_with(extra: &[(&str, &[u8], CompressionMethod)]) -> Vec<u8> {
    let doc = document_xml("<w:p><w:r><w:t>hi</w:t></w:r></w:p>");
    let mut entries: Vec<(&str, &[u8], CompressionMethod)> = vec![(
        "word/document.xml",
        doc.as_bytes(),
        CompressionMethod::Deflated,
    )];
    entries.extend_from_slice(extra);
    build_zip(&entries)
}

/// 把 zip 里**所有**本地头与中央目录头的“未压缩大小”字段改写成 `size`(伪造声明)。
fn patch_declared_size(mut bytes: Vec<u8>, size: u32) -> Vec<u8> {
    let le = size.to_le_bytes();
    let mut i = 0;
    while i + 4 <= bytes.len() {
        match &bytes[i..i + 4] {
            // 本地文件头:uncompressed size @ +22。
            [0x50, 0x4b, 0x03, 0x04] if i + 26 <= bytes.len() => {
                bytes[i + 22..i + 26].copy_from_slice(&le);
            }
            // 中央目录头:uncompressed size @ +24。
            [0x50, 0x4b, 0x01, 0x02] if i + 28 <= bytes.len() => {
                bytes[i + 24..i + 28].copy_from_slice(&le);
            }
            _ => {}
        }
        i += 1;
    }
    bytes
}

fn expect_limit(res: doc_core::Result<doc_parse::ParsedDoc>, want: LimitKind) -> (u64, u64) {
    match res {
        Err(DocError::LimitExceeded {
            kind,
            limit,
            actual,
        }) => {
            assert_eq!(kind, want, "wrong limit kind");
            assert!(
                actual > limit,
                "actual {actual} should exceed limit {limit}"
            );
            (limit, actual)
        }
        Err(e) => panic!("expected LimitExceeded({want:?}), got error {e}"),
        Ok(_) => panic!("expected LimitExceeded({want:?}), got Ok"),
    }
}

#[test]
fn default_limits_values() {
    let l = ZipLimits::default();
    assert_eq!(l.max_entries, 10_000);
    assert_eq!(l.max_entry_bytes, 256 * 1024 * 1024);
    assert_eq!(l.max_total_bytes, 1024 * 1024 * 1024);
    assert_eq!(l.max_compression_ratio, 10_000);
    assert_eq!(l.max_name_len, 1024);
}

#[test]
fn normal_docx_unchanged_under_default_limits() {
    let bytes = docx_with(&[]);
    let a = parse_bytes(&bytes).unwrap();
    let b = parse_bytes_with_limits(&bytes, &ZipLimits::default()).unwrap();
    assert_eq!(a.document.body.len(), 1);
    assert_eq!(format!("{:?}", a.document), format!("{:?}", b.document));
}

#[test]
fn too_many_entries() {
    let names: Vec<String> = (0..10_001).map(|i| format!("junk/{i}.bin")).collect();
    let extra: Vec<(&str, &[u8], CompressionMethod)> = names
        .iter()
        .map(|n| (n.as_str(), &b""[..], CompressionMethod::Stored))
        .collect();
    let (limit, actual) = expect_limit(parse_bytes(&docx_with(&extra)), LimitKind::Entries);
    assert_eq!((limit, actual), (10_000, 10_002));
}

#[test]
fn name_too_long() {
    let name = format!("word/{}.xml", "a".repeat(1100));
    let bytes = docx_with(&[(&name, b"x", CompressionMethod::Stored)]);
    let (limit, actual) = expect_limit(parse_bytes(&bytes), LimitKind::NameLength);
    assert_eq!((limit, actual), (1024, name.len() as u64));
}

#[test]
fn unsafe_paths_rejected() {
    for name in [
        "word/../../evil.xml",
        "../evil.xml",
        "/etc/evil.xml",
        "word\\..\\evil.xml",
        "C:/evil.xml",
    ] {
        let bytes = docx_with(&[(name, b"x", CompressionMethod::Stored)]);
        match parse_bytes(&bytes) {
            Err(DocError::Zip(msg)) => assert!(msg.contains("unsafe entry path"), "{msg}"),
            Err(e) => panic!("{name}: expected Zip(unsafe path), got {e}"),
            Ok(_) => panic!("{name}: expected Zip(unsafe path), got Ok"),
        }
    }
}

#[test]
fn declared_entry_size_over_limit() {
    // 声明 512 MiB(> 256 MiB 缺省),实际只有几字节:声明检查先拦,绝不按声明预分配。
    let bytes = patch_declared_size(
        docx_with(&[("word/media/big.bin", b"tiny", CompressionMethod::Stored)]),
        512 * 1024 * 1024,
    );
    let (limit, actual) = expect_limit(parse_bytes(&bytes), LimitKind::EntryBytes);
    assert_eq!((limit, actual), (256 * 1024 * 1024, 512 * 1024 * 1024));
}

#[test]
fn forged_4gib_declared_size() {
    // 伪造接近 4 GiB 的声明(0xFFFF_FFFF 是 zip64 标记,取其下一个值)。
    let bytes = patch_declared_size(
        docx_with(&[("word/media/big.bin", b"tiny", CompressionMethod::Stored)]),
        0xFFFF_FFFE,
    );
    let (limit, actual) = expect_limit(parse_bytes(&bytes), LimitKind::EntryBytes);
    assert_eq!((limit, actual), (256 * 1024 * 1024, 0xFFFF_FFFE));
}

#[test]
fn honest_entry_over_custom_limit() {
    let data = vec![b'x'; 4096];
    let bytes = docx_with(&[("word/media/a.bin", &data, CompressionMethod::Deflated)]);
    let limits = ZipLimits {
        max_entry_bytes: 2048,
        ..ZipLimits::default()
    };
    let (limit, actual) = expect_limit(
        parse_bytes_with_limits(&bytes, &limits),
        LimitKind::EntryBytes,
    );
    assert_eq!((limit, actual), (2048, 4096));
}

#[test]
fn forged_small_declared_size_caught_while_reading() {
    // 声明 16 字节,实际解压出 64 KiB:按 take(limit + 1) 流式读,多出 1 字节即报。
    let data = vec![b'z'; 64 * 1024];
    let bytes = patch_declared_size(
        docx_with(&[("word/media/a.bin", &data, CompressionMethod::Deflated)]),
        16,
    );
    let limits = ZipLimits {
        max_entry_bytes: 4096,
        ..ZipLimits::default()
    };
    let (limit, actual) = expect_limit(
        parse_bytes_with_limits(&bytes, &limits),
        LimitKind::EntryBytes,
    );
    assert_eq!((limit, actual), (4096, 4097));
}

#[test]
fn compression_ratio_bomb_declared() {
    // 经典炸弹形态:几 KiB 的压缩数据声明解压出 200 MiB(未超单条目上限,但比值离谱)。
    let zeros = vec![0u8; 64 * 1024];
    let bytes = patch_declared_size(
        docx_with(&[("word/media/bomb.bin", &zeros, CompressionMethod::Deflated)]),
        200 * 1024 * 1024,
    );
    let (limit, actual) = expect_limit(parse_bytes(&bytes), LimitKind::CompressionRatio);
    assert_eq!(limit, 10_000);
    assert!(actual > 10_000);
}

#[test]
fn compression_ratio_real_zeros() {
    // 真实的大块零:4 MiB 零经 deflate 压到 ~4 KiB,比值 ~1028:1(deflate 理论上限 ~1032)。
    // 缺省限额(10 000)下合法,不误伤纯色位图之类的 media。
    let zeros = vec![0u8; 4 * 1024 * 1024];
    let bytes = docx_with(&[("word/media/zeros.bin", &zeros, CompressionMethod::Deflated)]);
    parse_bytes(&bytes).unwrap();
    // 收紧到 1000 后同一个包被拦。
    let strict = ZipLimits {
        max_compression_ratio: 1000,
        ..ZipLimits::default()
    };
    let (limit, actual) = expect_limit(
        parse_bytes_with_limits(&bytes, &strict),
        LimitKind::CompressionRatio,
    );
    assert_eq!(limit, 1000);
    assert!(actual > 1000);
}

#[test]
fn small_compressible_file_not_flagged_by_ratio() {
    // 512 KiB 零的比值也 > 1000,但未压缩量 ≤ 1 MiB,即便限额收紧到 1 也不判定压缩比。
    let zeros = vec![0u8; 512 * 1024];
    let bytes = docx_with(&[("word/media/zeros.bin", &zeros, CompressionMethod::Deflated)]);
    let strict = ZipLimits {
        max_compression_ratio: 1,
        ..ZipLimits::default()
    };
    parse_bytes_with_limits(&bytes, &strict).unwrap();
}

#[test]
fn ratio_rechecked_on_actual_bytes() {
    // 声明 16 字节(过得了声明期比值检查),实际解压出 2 MiB 零(~1028:1):读完后的复查
    // 按收紧的 1000 拦下。
    let zeros = vec![0u8; 2 * 1024 * 1024];
    let bytes = patch_declared_size(
        docx_with(&[("word/media/zeros.bin", &zeros, CompressionMethod::Deflated)]),
        16,
    );
    let strict = ZipLimits {
        max_compression_ratio: 1000,
        ..ZipLimits::default()
    };
    let (limit, actual) = expect_limit(
        parse_bytes_with_limits(&bytes, &strict),
        LimitKind::CompressionRatio,
    );
    assert_eq!(limit, 1000);
    assert!(actual > 1000);
}

#[test]
fn limit_error_message_text() {
    let err = parse_bytes(&patch_declared_size(
        docx_with(&[("word/media/big.bin", b"tiny", CompressionMethod::Stored)]),
        512 * 1024 * 1024,
    ))
    .unwrap_err();
    assert_eq!(err.kind(), "limit-exceeded");
    assert_eq!(
        err.to_string(),
        "limit exceeded: entry-bytes (limit 268435456, actual 536870912)"
    );
    assert_eq!(LimitKind::Entries.to_string(), "entries");
    assert_eq!(LimitKind::TotalBytes.as_str(), "total-bytes");
    assert_eq!(LimitKind::CompressionRatio.as_str(), "compression-ratio");
    assert_eq!(LimitKind::NameLength.as_str(), "name-length");
}

#[test]
fn total_bytes_over_limit() {
    let chunk = vec![b'q'; 4000];
    let bytes = docx_with(&[
        ("word/media/1.bin", &chunk, CompressionMethod::Deflated),
        ("word/media/2.bin", &chunk, CompressionMethod::Deflated),
        ("word/media/3.bin", &chunk, CompressionMethod::Deflated),
    ]);
    let limits = ZipLimits {
        max_total_bytes: 10_000,
        ..ZipLimits::default()
    };
    let (limit, actual) = expect_limit(
        parse_bytes_with_limits(&bytes, &limits),
        LimitKind::TotalBytes,
    );
    assert_eq!((limit, actual), (10_000, 10_001));
}

// ------------------------------------------------------------------ 嵌套深度

fn nested_tables(levels: usize) -> String {
    let open = "<w:tbl><w:tr><w:tc>";
    let close = "<w:p/></w:tc></w:tr></w:tbl>";
    format!(
        "{}<w:p><w:r><w:t>core</w:t></w:r></w:p>{}",
        open.repeat(levels),
        close.repeat(levels)
    )
}

fn docx_body(body: &str) -> Vec<u8> {
    let doc = document_xml(body);
    build_zip(&[(
        "word/document.xml",
        doc.as_bytes(),
        CompressionMethod::Deflated,
    )])
}

/// 嵌套表的最大深度(顶层表算 1)。
fn table_depth(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            Block::Table(t) => {
                1 + t
                    .rows
                    .iter()
                    .flat_map(|r| &r.cells)
                    .map(|c| table_depth(&c.blocks))
                    .max()
                    .unwrap_or(0)
            }
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

const AFTER: &str = "<w:p><w:r><w:t>after</w:t></w:r></w:p>";

#[test]
fn nested_tables_at_limit_kept_whole() {
    let parsed = parse_bytes(&docx_body(&format!("{}{AFTER}", nested_tables(64)))).unwrap();
    let body = &parsed.document.body;
    assert_eq!(body.len(), 2);
    assert_eq!(table_depth(body), 64);
    let dbg = format!("{body:?}");
    assert!(dbg.contains("core") && dbg.contains("after"));
}

#[test]
fn nested_tables_70_levels_truncated_not_panic() {
    // 第 65 层起的子树整棵跳过;外层 64 层与其后的正文照常解析。
    let parsed = parse_bytes(&docx_body(&format!("{}{AFTER}", nested_tables(70)))).unwrap();
    let body = &parsed.document.body;
    assert_eq!(body.len(), 2);
    assert_eq!(table_depth(body), 64);
    let dbg = format!("{body:?}");
    assert!(
        !dbg.contains("core"),
        "content beyond depth 64 must be skipped"
    );
    assert!(dbg.contains("after"));
}

#[test]
fn nested_tables_10k_levels_no_stack_overflow() {
    let parsed = parse_bytes(&docx_body(&format!("{}{AFTER}", nested_tables(10_000)))).unwrap();
    assert_eq!(table_depth(&parsed.document.body), 64);
}

#[test]
fn nested_sdt_10k_levels_no_stack_overflow() {
    let levels = 10_000;
    let body = format!(
        "{}<w:p><w:r><w:t>core</w:t></w:r></w:p>{}{AFTER}",
        "<w:sdt><w:sdtContent>".repeat(levels),
        "</w:sdtContent></w:sdt>".repeat(levels)
    );
    let parsed = parse_bytes(&docx_body(&body)).unwrap();
    let dbg = format!("{:?}", parsed.document.body);
    assert!(!dbg.contains("core") && dbg.contains("after"));
}

#[test]
fn nested_inline_containers_10k_levels_no_stack_overflow() {
    let levels = 5_000;
    let body = format!(
        "<w:p>{}<w:r><w:t>core</w:t></w:r>{}</w:p>{AFTER}",
        "<w:hyperlink><w:ins>".repeat(levels),
        "</w:ins></w:hyperlink>".repeat(levels)
    );
    let parsed = parse_bytes(&docx_body(&body)).unwrap();
    let dbg = format!("{:?}", parsed.document.body);
    assert!(!dbg.contains("core") && dbg.contains("after"));
}
