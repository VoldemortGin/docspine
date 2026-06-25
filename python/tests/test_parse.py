"""docspine 结构化解析的验收测试 —— 对合成的最小 ``.docx`` 断言文字 / 表格(重点)/ 图片。"""

from __future__ import annotations

import pytest

import docspine


def test_open_path_basic(minimal_docx_path):
    doc = docspine.open(minimal_docx_path)
    # 顶层块:标题段 + 表格 + 图片段。
    assert doc.block_count == 3
    assert len(doc) == 3


def test_open_bytes_matches_path(minimal_docx_bytes):
    doc = docspine.open_bytes(minimal_docx_bytes)
    assert doc.block_count == 3


def test_paragraph_runs_and_styling(minimal_docx_bytes):
    doc = docspine.open_bytes(minimal_docx_bytes)
    paras = doc.paragraphs()
    # 标题段 + 图片所在的空文字段。
    assert len(paras) >= 1
    title = paras[0]
    assert title["kind"] == "paragraph"
    assert title["text"] == "Hello docspine"
    assert title["style"] == "Heading1"
    assert title["align"] == "center"

    run = title["runs"][0]
    assert run["text"] == "Hello docspine"
    assert run["bold"] is True
    assert run["italic"] is True
    assert run["size_pt"] == pytest.approx(24.0)  # w:sz="48" 半磅 / 2
    assert run["font"] == "Calibri"
    assert run["color"] == "1F4E79"


def test_table_grid_and_header(minimal_docx_bytes):
    doc = docspine.open_bytes(minimal_docx_bytes)
    tables = doc.tables()
    assert len(tables) == 1
    table = tables[0]

    assert table["style"] == "TableGrid"
    assert table["grid_cols"] == [2400, 2400, 2400]
    assert table["col_count"] == 3
    assert table["row_count"] == 2

    header = table["rows"][0]
    assert header["is_header"] is True
    assert header["height"] == 400


def test_table_horizontal_grid_span_and_fill(minimal_docx_bytes):
    """横向合并(gridSpan)+ 单元格填充 —— 用户重点。"""
    table = docspine.open_bytes(minimal_docx_bytes).tables()[0]
    merged = table["rows"][0]["cells"][0]
    assert merged["text"] == "Merged Header"
    assert merged["grid_span"] == 2
    assert merged["fill"] == "FFCC00"
    # 未合并 / 无填充的格其 fill 为 None。
    a2 = table["rows"][1]["cells"][0]
    assert a2["grid_span"] == 1
    assert a2["fill"] is None


def test_table_vertical_v_merge_restart_continue(minimal_docx_bytes):
    """纵向合并(vMerge restart/continue)—— 用户重点。"""
    table = docspine.open_bytes(minimal_docx_bytes).tables()[0]
    restart = table["rows"][0]["cells"][1]
    assert restart["v_merge"] == "restart"
    assert restart["merged"] is False
    assert restart["text"] == "Spanning Down"

    cont = table["rows"][1]["cells"][2]
    assert cont["v_merge"] == "continue"
    assert cont["merged"] is True


def test_table_cell_width_dxa(minimal_docx_bytes):
    table = docspine.open_bytes(minimal_docx_bytes).tables()[0]
    a2 = table["rows"][1]["cells"][0]
    assert a2["width"] == 2400
    # 2400 twip / 20 = 120 pt。
    assert a2["width_points"] == pytest.approx(120.0)


def test_table_nested_table_inside_cell(minimal_docx_bytes):
    """嵌套表(单元格里再放表)—— 用户重点。"""
    table = docspine.open_bytes(minimal_docx_bytes).tables()[0]
    b2 = table["rows"][1]["cells"][1]
    # 直接段落文字 = "B2";嵌套表在 blocks 里。
    assert b2["text"] == "B2"
    nested = [blk for blk in b2["blocks"] if blk["kind"] == "table"]
    assert len(nested) == 1
    nested_table = nested[0]
    assert nested_table["row_count"] == 1
    assert nested_table["rows"][0]["cells"][0]["text"] == "nested"


def test_row_text_convenience(minimal_docx_bytes):
    table = docspine.open_bytes(minimal_docx_bytes).tables()[0]
    # 便利的逐行文字列表。
    assert table["rows"][0]["text"] == ["Merged Header", "Spanning Down"]


def test_embedded_picture_extracted(minimal_docx_bytes, image1_png_bytes):
    """内嵌图片:经 word/_rels 关系定位到 word/media,回填 media 名 + 字节长度 + EMU 尺寸。"""
    doc = docspine.open_bytes(minimal_docx_bytes)
    pics = []
    for blk in doc.body():
        if blk["kind"] == "paragraph":
            for run in blk["runs"]:
                pics.extend(run["pictures"])
    assert len(pics) == 1
    pic = pics[0]
    assert pic["rel_id"] == "rId10"
    assert pic["media"] == "image1.png"
    assert pic["image_bytes_len"] == len(image1_png_bytes)
    # wp:extent cx=cy=914400 EMU = 1 inch = 72 pt。
    assert pic["extent"] == (914400, 914400)
    assert pic["extent_points"] == pytest.approx((72.0, 72.0))


def test_document_text_convenience(minimal_docx_bytes):
    text = docspine.open_bytes(minimal_docx_bytes).text()
    assert "Hello docspine" in text
    # 表格行以 tab 连接、块以换行连接。
    assert "Merged Header\tSpanning Down" in text


def test_malformed_input_raises_typed_error():
    with pytest.raises(docspine.DocError):
        docspine.open_bytes(b"this is definitely not a docx zip")
    with pytest.raises(docspine.DocZipError):
        docspine.open_bytes(b"\x00\x01\x02\x03 not a zip")


def test_legacy_doc_bytes_raise_unsupported():
    # CFB 魔数(旧二进制 .doc)-> 清晰的 DocUnsupportedError(docx 优先)。
    cfb = bytes([0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1]) + b"\x00" * 64
    with pytest.raises(docspine.DocUnsupportedError):
        docspine.open_bytes(cfb)


def test_probe_doc_on_non_cfb(minimal_docx_bytes):
    # docx(zip)不是 CFB:probe 返回 is_cfb=False,不报错。
    probe = docspine.probe_doc(minimal_docx_bytes)
    assert probe["is_cfb"] is False
    assert probe["has_word_stream"] is False


def test_open_missing_file_raises():
    with pytest.raises((FileNotFoundError, OSError)):
        docspine.open("/no/such/doc-12345.docx")
