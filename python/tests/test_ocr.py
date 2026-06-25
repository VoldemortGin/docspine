"""图片 OCR 桥 + 图像表格重建的验收测试。

OCR 走姊妹 crate ``ocrspine``(PP-OCRv5 / tract-onnx,本地、离线、确定性)。模型由扩展在
编译期烘进的 ``CARGO_MANIFEST_DIR/models``(即 ocrspine git checkout 的 ``models``),或由
``OCRSPINE_MODELS`` 环境变量覆盖。复用 ocrspine 自带、已验证的 ``ocr_sample.png`` fixture
(含 "pdfspine OCR test 2026" 等参考行),不另落二进制。

OCR 入口仅在以 ``--features ocr`` 构建扩展时存在;否则整体 skip。
"""

from __future__ import annotations

from pathlib import Path

import pytest

import docspine

# 没有以 ocr 特性构建时,这两个入口是 None;整模块 skip。
pytestmark = pytest.mark.skipif(
    docspine.ocr_image is None,
    reason="extension built without the `ocr` feature (maturin develop --features ocr)",
)

# ocrspine 是 docspine 的姊妹包,布局为 spine/ocrspine 与 spine/docspine 平级。
_OCR_SAMPLE = (
    Path(__file__).resolve().parents[3] / "ocrspine" / "tests" / "fixtures" / "ocr_sample.png"
)


@pytest.fixture(scope="session")
def ocr_sample_bytes() -> bytes:
    if not _OCR_SAMPLE.is_file():
        pytest.skip(f"OCR sample fixture not found at {_OCR_SAMPLE}")
    return _OCR_SAMPLE.read_bytes()


def test_ocr_image_recognizes_reference_lines(ocr_sample_bytes):
    items = docspine.ocr_image(ocr_sample_bytes)
    assert isinstance(items, list)
    assert items, "OCR returned no items at all"

    first = items[0]
    assert set(first) == {"text", "bbox", "confidence"}
    assert isinstance(first["text"], str)
    assert len(first["bbox"]) == 4
    assert 0.0 <= first["confidence"] <= 100.0

    joined = "".join(ch for it in items for ch in it["text"] if not ch.isspace())
    for ref in ("pdfspineOCRtest2026", "纯Rust实现的PDF文字识别", "PaddleOCRviatract"):
        assert ref in joined, f"reference line {ref!r} not found in {joined!r}"


def test_reconstruct_image_table_returns_grid(ocr_sample_bytes):
    """图像表格重建端到端:OCR 框 -> 行列网格。样本不是严格表格,故只断言形状契约稳定。"""
    tables = docspine.reconstruct_image_table(ocr_sample_bytes)
    assert isinstance(tables, list)
    for t in tables:
        assert set(t) >= {"bbox", "row_count", "col_count", "cols", "rows", "cells"}
        assert len(t["cols"]) == t["col_count"] + 1
        assert len(t["rows"]) == t["row_count"] + 1
        for c in t["cells"]:
            assert set(c) >= {"row", "col", "row_span", "col_span", "bbox", "text", "confidence"}
            assert c["row_span"] >= 1 and c["col_span"] >= 1


def test_ocr_image_bad_bytes_raises():
    with pytest.raises(docspine.DocError):
        docspine.ocr_image(b"not an image at all")
