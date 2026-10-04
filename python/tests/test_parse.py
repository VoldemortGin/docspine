"""docspine 结构化解析的验收测试 —— 对合成的最小 ``.docx`` 断言文字 / 表格(重点)/ 图片。"""

from __future__ import annotations

import io
import zipfile

import pytest

import docspine
from conftest import _DOC_HEADER, _png_1x1, build_docx


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
    assert pic["alt"] == "a tiny pixel"  # wp:docPr@descr
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


_W_NS = "http://schemas.openxmlformats.org/wordprocessingml/2006/main"


def _docx(body: str, extra: int = 0) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr(
            "word/document.xml",
            f'<w:document xmlns:w="{_W_NS}"><w:body>{body}</w:body></w:document>',
        )
        for i in range(extra):
            z.writestr(f"junk/{i}.bin", b"")
    return buf.getvalue()


def test_zip_entry_limit_raises_zip_error():
    # 超过缺省 10 000 条目 -> 资源限额,按 zip/损坏输入抛出,信息里带限额种类。
    with pytest.raises(docspine.DocZipError, match="limit exceeded: entries"):
        docspine.open_bytes(_docx("<w:p/>", extra=10_000))


def test_deep_nesting_skipped_not_crash():
    # 70 层嵌套表:超过 64 层的子树静默跳过,外层与后续正文照常解析。
    levels = 70
    body = (
        "<w:tbl><w:tr><w:tc>" * levels
        + "<w:p><w:r><w:t>core</w:t></w:r></w:p>"
        + "</w:tc></w:tr></w:tbl>" * levels
        + "<w:p><w:r><w:t>after</w:t></w:r></w:p>"
    )
    doc = docspine.open_bytes(_docx(body))
    assert doc.block_count == 2
    text = doc.text()
    assert "after" in text
    assert "core" not in text


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



# --- 静默丢正文:moveTo / smartTag / customXml / AlternateContent / 文本框 / 符号 ---


def test_wrapped_and_alternate_content_recovered():
    """修订移动目标、smartTag、customXml、AlternateContent 回退、符号字符不再整段丢失。"""
    body = (
        '<w:customXml w:element="root"><w:p>'
        "<w:moveFrom><w:r><w:delText>OLD</w:delText></w:r></w:moveFrom>"
        "<w:moveTo><w:r><w:t>moved </w:t></w:r></w:moveTo>"
        "<w:smartTag><w:r><w:t>tagged </w:t></w:r></w:smartTag>"
        '<w:r xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006">'
        '<mc:AlternateContent><mc:Choice Requires="w16se"><w16se:symEx xmlns:w16se="urn:x"/></mc:Choice>'
        "<mc:Fallback><w:t>fb</w:t></mc:Fallback></mc:AlternateContent>"
        '<w:sym w:font="Symbol" w:char="03B1"/></w:r>'
        "</w:p></w:customXml>"
    )
    doc = docspine.open_bytes(build_docx(_DOC_HEADER + f"<w:body>{body}</w:body></w:document>"))
    assert doc.to_text() == "moved tagged fb\u03b1"


def test_text_box_exposed_on_run_dict_and_in_exports():
    """文本框内容挂在 run dict 的 ``text_boxes``(块 dict 同 body()),并紧随段落进导出。"""
    body = (
        '<w:p><w:r><w:t>Anchor</w:t></w:r><w:r><w:pict xmlns:v="urn:schemas-microsoft-com:vml">'
        '<v:shape><v:textbox><w:txbxContent><w:p><w:r><w:t>Boxed</w:t></w:r></w:p>'
        "</w:txbxContent></v:textbox></v:shape></w:pict></w:r></w:p>"
    )
    doc = docspine.open_bytes(build_docx(_DOC_HEADER + f"<w:body>{body}</w:body></w:document>"))
    para = doc.paragraphs()[0]
    assert para["text"] == "Anchor"
    boxes = [tb for run in para["runs"] for tb in run["text_boxes"]]
    assert len(boxes) == 1
    assert boxes[0]["blocks"][0]["kind"] == "paragraph"
    assert boxes[0]["blocks"][0]["text"] == "Boxed"
    assert doc.to_text() == "Anchor\nBoxed"
    assert doc.to_markdown() == "Anchor\n\nBoxed"
    assert doc.to_html() == "<p>Anchor</p>\n<p>Boxed</p>"

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


# --- 页眉页脚 / 脚注尾注 / 公式标记 --------------------------------------------

_W_NS = (
    'xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main" '
    'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"'
)
_REL_BASE = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"


def _with_parts(docx: bytes, parts: dict[str, str], rels: dict[str, tuple[str, str]]) -> bytes:
    """在 ``build_docx`` 产物上追加部件与主文档关系(``rId -> (类型后缀, Target)``)。"""
    src = zipfile.ZipFile(io.BytesIO(docx))
    out = io.BytesIO()
    extra = "".join(
        f'<Relationship Id="{rid}" Type="{_REL_BASE}/{ty}" Target="{target}"/>'
        for rid, (ty, target) in rels.items()
    )
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as dst:
        for info in src.infolist():
            data = src.read(info.filename)
            if info.filename == "word/_rels/document.xml.rels":
                data = data.decode().replace("</Relationships>", f"{extra}</Relationships>").encode()
            dst.writestr(info.filename, data)
        for name, xml in parts.items():
            dst.writestr(name, xml)
    return out.getvalue()


def _p(text: str) -> str:
    return f"<w:p><w:r><w:t>{text}</w:t></w:r></w:p>"


def test_header_footer_and_notes_exposed_and_exported():
    body = (
        "<w:body><w:p><w:r><w:t>Body</w:t></w:r>"
        '<w:r><w:footnoteReference w:id="1"/></w:r>'
        '<w:r><w:endnoteReference w:id="1"/></w:r></w:p>'
        '<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/>'
        '<w:headerReference w:type="first" r:id="rIdH2"/>'
        '<w:footerReference w:type="even" r:id="rIdF"/></w:sectPr></w:body>'
    )
    docx = _with_parts(
        build_docx(_DOC_HEADER + body + "</w:document>"),
        {
            "word/header1.xml": f"<w:hdr {_W_NS}>{_p('Top')}</w:hdr>",
            "word/header2.xml": f"<w:hdr {_W_NS}>{_p('Title page')}</w:hdr>",
            "word/footer1.xml": f"<w:ftr {_W_NS}>{_p('Bottom')}</w:ftr>",
            "word/footnotes.xml": (
                f'<w:footnotes {_W_NS}><w:footnote w:type="separator" w:id="-1">'
                f"{_p('SEP')}</w:footnote><w:footnote w:id=\"1\">{_p('Foot')}</w:footnote></w:footnotes>"
            ),
            "word/endnotes.xml": (
                f'<w:endnotes {_W_NS}><w:endnote w:id="1">{_p("End")}</w:endnote></w:endnotes>'
            ),
        },
        {
            "rIdH": ("header", "header1.xml"),
            "rIdH2": ("header", "header2.xml"),
            "rIdF": ("footer", "footer1.xml"),
        },
    )
    doc = docspine.open_bytes(docx)

    (sect,) = doc.sections()
    assert [h["type"] for h in sect["headers"]] == ["default", "first"]
    assert sect["headers"][0]["rel_id"] == "rIdH"
    assert sect["headers"][0]["blocks"][0]["text"] == "Top"
    assert [f["type"] for f in sect["footers"]] == ["even"]
    assert sect["footers"][0]["blocks"][0]["text"] == "Bottom"

    kinds = [
        (seg["note_kind"], seg["id"])
        for run in doc.paragraphs()[0]["runs"]
        for seg in run["segments"]
        if seg["kind"] == "note_ref"
    ]
    assert kinds == [("footnote", 1), ("endnote", 1)]
    assert [(n["id"], n["blocks"][0]["text"]) for n in doc.footnotes()] == [(1, "Foot")]
    assert [(n["id"], n["blocks"][0]["text"]) for n in doc.endnotes()] == [(1, "End")]

    assert doc.to_text() == (
        "[Header: default]\nTop\n[Header: first]\nTitle page\n"
        "Body[1][e1]\n[1] Foot\n[e1] End\n[Footer: even]\nBottom"
    )
    md = doc.to_markdown()
    assert "Body[^1][^e1]" in md and "[^1]: Foot" in md and "[^e1]: End" in md
    assert "SEP" not in doc.to_text()


def test_sections_without_header_footer_or_notes_have_empty_lists(minimal_docx_bytes):
    doc = docspine.open_bytes(minimal_docx_bytes)
    assert doc.sections()[0]["headers"] == []
    assert doc.sections()[0]["footers"] == []
    assert doc.footnotes() == []
    assert doc.endnotes() == []


def test_comments_exposed_with_anchor_and_kept_out_of_default_exports():
    body = (
        "<w:body><w:p>"
        '<w:commentRangeStart w:id="0"/><w:r><w:t>Body</w:t></w:r><w:commentRangeEnd w:id="0"/>'
        '<w:r><w:commentReference w:id="0"/></w:r>'
        '<w:r><w:commentReference w:id="9"/></w:r></w:p></w:body>'
    )
    docx = _with_parts(
        build_docx(_DOC_HEADER + body + "</w:document>"),
        {
            "word/comments.xml": (
                f"<w:comments {_W_NS}>"
                '<w:comment w:id="0" w:author="Ann" w:date="2026-10-03T09:30:00Z" w:initials="A">'
                f"{_p('SECRET-NOTE')}</w:comment>"
                f'<w:comment w:id="2">{_p("bare")}</w:comment></w:comments>'
            ),
        },
        {},
    )
    doc = docspine.open_bytes(docx)

    first, second = doc.comments()
    assert (first["id"], first["author"], first["date"], first["initials"]) == (
        0,
        "Ann",
        "2026-10-03T09:30:00Z",
        "A",
    )
    assert first["blocks"][0]["text"] == "SECRET-NOTE"
    assert (second["id"], second["author"], second["date"], second["initials"]) == (
        2,
        None,
        None,
        None,
    )

    refs = [
        seg["id"]
        for run in doc.paragraphs()[0]["runs"]
        for seg in run["segments"]
        if seg["kind"] == "comment_ref"
    ]
    assert refs == [0, 9]  # 悬空引用(9)保留引用点,不报错

    for out in (doc.to_text(), doc.to_markdown(), doc.to_html()):
        assert "Body" in out
        assert "SECRET-NOTE" not in out and "Ann" not in out


def test_comments_empty_without_part_and_malformed_part_does_not_raise(minimal_docx_bytes):
    assert docspine.open_bytes(minimal_docx_bytes).comments() == []
    docx = _with_parts(
        build_docx(_DOC_HEADER + "<w:body>" + _p("x") + "</w:body></w:document>"),
        {"word/comments.xml": f'<w:comments {_W_NS}><w:comment w:id="1"><w:p><w:r><w:t>Cut'},
        {},
    )
    assert docspine.open_bytes(docx).to_text() == "x"


def test_part_scoped_image_rel_ids_resolve_to_their_own_bytes():
    """页眉与正文都用 rId1 却指向不同图片:各自的 ``media`` 取到各自的字节;
    ``rel_id`` 反查只对正文有效(部件作用域内的 rel_id 在别的部件里含义不同)。"""
    body_png, head_png = _png_1x1((255, 0, 0)), _png_1x1((0, 0, 255))
    assert body_png != head_png

    def pic(rid: str) -> str:
        return (
            f'<w:r><w:drawing><wp:inline><wp:extent cx="914400" cy="914400"/><a:graphic><a:graphicData>'
            f'<pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">'
            f'<pic:blipFill><a:blip r:embed="{rid}"/></pic:blipFill></pic:pic>'
            f"</a:graphicData></a:graphic></wp:inline></w:drawing></w:r>"
        )

    body = (
        f"<w:body><w:p>{pic('rId1')}</w:p>"
        '<w:sectPr><w:headerReference w:type="default" r:id="rIdH"/></w:sectPr></w:body>'
    )
    docx = _with_parts(
        build_docx(_DOC_HEADER + body + "</w:document>"),
        {
            "word/header1.xml": (
                f'<w:hdr {_W_NS} xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/'
                f'wordprocessingDrawing" xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">'
                f"<w:p>{pic('rId1')}</w:p></w:hdr>"
            ),
            "word/_rels/header1.xml.rels": (
                '<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
                f'<Relationship Id="rId1" Type="{_REL_BASE}/image" Target="media/head.png"/></Relationships>'
            ),
            "word/media/body.png": body_png,
            "word/media/head.png": head_png,
        },
        {"rIdH": ("header", "header1.xml"), "rId1": ("image", "media/body.png")},
    )
    doc = docspine.open_bytes(docx)
    body_pic = doc.paragraphs()[0]["runs"][0]["pictures"][0]
    head_pic = doc.sections()[0]["headers"][0]["blocks"][0]["runs"][0]["pictures"][0]
    assert body_pic["rel_id"] == head_pic["rel_id"] == "rId1"
    assert (body_pic["media"], head_pic["media"]) == ("body.png", "head.png")
    assert doc.image_bytes(body_pic["media"]) == body_png
    assert doc.image_bytes(head_pic["media"]) == head_png
    assert doc.image_bytes(body_pic["rel_id"]) == body_png  # rel_id 反查按正文作用域


def test_math_run_exposes_is_math_flag():
    body = (
        '<w:body><w:p xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">'
        "<w:r><w:t>t</w:t></w:r><m:oMath><m:r><m:t>x</m:t></m:r></m:oMath></w:p></w:body>"
    )
    doc = docspine.open_bytes(build_docx(_DOC_HEADER + body + "</w:document>"))
    assert [r["is_math"] for r in doc.paragraphs()[0]["runs"]] == [False, True]


def test_field_run_title_pg_even_odd_and_page_numbering_exposed():
    """run["field"] / section 的 title_pg 与页码设置 / document 的 even_and_odd_headers。"""
    fld = (
        '<w:r><w:fldChar w:fldCharType="begin"/></w:r>'
        "<w:r><w:instrText> PAGE </w:instrText></w:r>"
        '<w:r><w:fldChar w:fldCharType="separate"/></w:r>'
        "<w:r><w:t>3</w:t></w:r>"
        '<w:r><w:fldChar w:fldCharType="end"/></w:r>'
    )
    body = (
        f"<w:body><w:p><w:r><w:t>plain</w:t></w:r>{fld}</w:p>"
        '<w:p><w:pPr><w:sectPr><w:titlePg/>'
        '<w:pgNumType w:fmt="lowerRoman" w:start="0"/></w:sectPr></w:pPr></w:p>'
        '<w:p/><w:sectPr><w:pgNumType w:fmt="ordinal" w:start="-2"/></w:sectPr></w:body>'
    )
    settings = f'<w:settings xmlns:w="{_W_NS}"><w:evenAndOddHeaders/></w:settings>'
    doc = docspine.open_bytes(
        build_docx(_DOC_HEADER + body + "</w:document>", settings_xml=settings)
    )

    runs = doc.paragraphs()[0]["runs"]
    assert [(r["text"], r["field"]) for r in runs if r["text"]] == [
        ("plain", None),
        ("3", "PAGE"),
    ]

    first, second = doc.sections()
    assert first["title_pg"] is True
    assert (first["page_number_start"], first["page_number_format"]) == (0, "lowerRoman")
    assert second["title_pg"] is False
    # 非法 start(负数)按缺失 = None;不支持的 fmt 暴露为 "other"。
    assert (second["page_number_start"], second["page_number_format"]) == (None, "other")
    assert doc.even_and_odd_headers is True


def test_page_numbering_defaults_when_not_declared(minimal_docx_bytes):
    doc = docspine.open_bytes(minimal_docx_bytes)
    (sect,) = doc.sections()
    assert sect["title_pg"] is False
    assert sect["page_number_start"] is None
    assert sect["page_number_format"] == "decimal"
    assert doc.even_and_odd_headers is False
    assert all(r["field"] is None for p in doc.paragraphs() for r in p["runs"])
