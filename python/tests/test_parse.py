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


# --- 内嵌图片字节(打通 OCR 闭环的前半段) -------------------------------------


def test_image_bytes_by_media_name_and_rel_id(minimal_docx_bytes, image1_png_bytes):
    """Document.image_bytes 既能按 media 裸文件名取,也能按图片 dict 的 rel_id 取。"""
    doc = docspine.open_bytes(minimal_docx_bytes)
    pic = None
    for blk in doc.body():
        if blk["kind"] == "paragraph":
            for run in blk["runs"]:
                if run["pictures"]:
                    pic = run["pictures"][0]
    assert pic is not None
    # 按 media 名取。
    by_name = doc.image_bytes(pic["media"])
    assert by_name == image1_png_bytes
    # 按 rel_id 取(同一份字节)。
    by_rel = doc.image_bytes(pic["rel_id"])
    assert by_rel == image1_png_bytes
    # 查不到 -> None,绝不抛错。
    assert doc.image_bytes("does-not-exist.png") is None


# --- 节(sectPr)页面几何:C-2 -------------------------------------------------


def test_sections_default_when_no_sectpr(minimal_docx_bytes):
    """整篇没有 sectPr -> 恰好一节 Word 默认页面设置(Letter 纵向、1 英寸边距)。"""
    doc = docspine.open_bytes(minimal_docx_bytes)
    sections = doc.sections()
    assert len(sections) == 1
    s = sections[0]
    assert (s["page_width"], s["page_height"]) == (12240, 15840)
    assert s["page_width_points"] == pytest.approx(612.0)
    assert s["page_height_points"] == pytest.approx(792.0)
    assert s["orientation"] == "portrait"
    assert s["margins"] == {
        "top": 1440,
        "right": 1440,
        "bottom": 1440,
        "left": 1440,
        "header": 720,
        "footer": 720,
        "gutter": 0,
    }
    assert s["margins_points"]["top"] == pytest.approx(72.0)
    assert s["cols"] == 1
    assert s["end_block_index"] == doc.block_count


def test_sections_geometry_and_attribution(sections_docx_bytes):
    """段内 pPr>sectPr 结束第一节;body 末尾 sectPr 定义最后一节(A4 横向、两栏)。"""
    doc = docspine.open_bytes(sections_docx_bytes)
    sections = doc.sections()
    assert len(sections) == 2

    first, last = sections
    assert (first["page_width"], first["page_height"]) == (12240, 15840)
    assert first["orientation"] == "portrait"
    # 第一节含块 0..2(正文段 + 承载 sectPr 的空段)。
    assert first["end_block_index"] == 2

    assert (last["page_width"], last["page_height"]) == (16838, 11906)  # A4 横向
    assert last["page_width_points"] == pytest.approx(841.9)
    assert last["page_height_points"] == pytest.approx(595.3)
    assert last["orientation"] == "landscape"
    assert last["margins"] == {
        "top": 720,
        "right": 1080,
        "bottom": 360,
        "left": 1800,
        "header": 500,
        "footer": 400,
        "gutter": 100,
    }
    assert last["cols"] == 2
    assert last["end_block_index"] == doc.block_count == 3


def test_content_after_mid_body_sectpr_not_truncated(sections_docx_bytes):
    """内容丢失修复:段内 sectPr 之后的正文不再被截断(旧 walker 会丢掉其后全部 body)。"""
    text = docspine.open_bytes(sections_docx_bytes).to_text()
    assert "section one" in text
    assert "section two" in text  # 修复前:段内 sectPr 之后的内容全部丢失。


# --- run 分段与内容丢失修复:C-3 ------------------------------------------------


def test_run_segments_exposed_with_break_types(content_loss_docx_bytes):
    """run dict 新增 segments:文字 / 制表 / 断(w:br@w:type 不再丢失);text 契约不变。"""
    doc = docspine.open_bytes(content_loss_docx_bytes)
    # 最后一段:before <w:br w:type="page"/> after。
    para = doc.paragraphs()[-1]
    run = para["runs"][0]
    assert run["segments"] == [
        {"kind": "text", "text": "before"},
        {"kind": "break", "break_type": "page"},
        {"kind": "text", "text": "after"},
    ]
    # 折叠后的 text 键契约不变:Break -> "\n"。
    assert run["text"] == "before\nafter"


def test_sdt_and_fldsimple_content_recovered(content_loss_docx_bytes):
    """w:sdt(块级 + 行内)与 w:fldSimple 的文字不再整体丢失(修复前 to_text 为空段)。"""
    doc = docspine.open_bytes(content_loss_docx_bytes)
    # 修复后的全文(修复前:COVER-TITLE / 2026-07-02 / 7 三处全部缺失)。
    assert doc.to_text() == "COVER-TITLE\nUpdated 2026-07-02\nPage 7\nbefore\nafter"
    md = doc.to_markdown()
    assert "COVER-TITLE" in md
    assert "Updated 2026-07-02" in md
    assert "Page 7" in md


# --- 修订:w:ins 插入文字保留、w:del 删除文字丢弃 -----------------------------


def test_revision_ins_text_kept_del_text_dropped(revisions_docx_bytes):
    """w:ins 内插入的文字按“接受修订”保留;w:del 内删除的文字丢弃。"""
    doc = docspine.open_bytes(revisions_docx_bytes)
    text = doc.text()
    assert "INSERTED" in text  # 修订插入的文字现在能提取到(修复前会整段丢失)。
    assert "DELETED" not in text  # 修订删除的文字不输出。
    assert "Start" in text and "end" in text  # 周围正常正文不受影响。


# --- 结构化导出:to_text / to_markdown / to_html ------------------------------


def test_to_text_equivalent_to_text(minimal_docx_bytes):
    doc = docspine.open_bytes(minimal_docx_bytes)
    assert doc.to_text() == doc.text()
    assert "Hello docspine" in doc.to_text()
    assert "Merged Header\tSpanning Down" in doc.to_text()


def test_to_markdown_heading_and_merged_table(minimal_docx_bytes):
    """标题映射成 #;含合并的表退回 HTML <table> 保真 colspan/rowspan。"""
    md = docspine.open_bytes(minimal_docx_bytes).to_markdown()
    assert "# Hello docspine" in md
    assert 'colspan="2"' in md
    assert 'rowspan="2"' in md
    assert "Merged Header" in md


def test_to_markdown_simple_table_is_gfm(simple_table_docx_bytes):
    """无合并的表输出 GFM 管道表(含分隔行)。"""
    md = docspine.open_bytes(simple_table_docx_bytes).to_markdown()
    assert "## Sub" in md
    assert "| H1 | H2 |" in md
    assert "| --- | --- |" in md
    assert "| x | y |" in md


def test_to_html_paragraph_heading_and_table_spans(minimal_docx_bytes):
    html = docspine.open_bytes(minimal_docx_bytes).to_html()
    assert "<h1>Hello docspine</h1>" in html
    assert "<table>" in html
    assert 'colspan="2"' in html
    assert 'rowspan="2"' in html
    assert "<td>A2</td>" in html
    assert "nested" in html  # 嵌套表内容也在(单元格内递归渲染)。


def test_to_html_simple_table(simple_table_docx_bytes):
    html = docspine.open_bytes(simple_table_docx_bytes).to_html()
    # 朴素表(无表头行标记)也渲染成 <table>,单元格为 <td>。
    assert "<table>" in html
    assert "<td>H1</td>" in html
    assert "<h2>Sub</h2>" in html
