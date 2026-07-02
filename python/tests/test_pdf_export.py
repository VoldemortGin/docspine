"""PDF 导出(C-1/C-4)验收:``to_pdf`` / ``save_pdf`` 端到端 + 直接格式化渲染门。

回读器用姊妹包 ``pdfspine``(fitz 兼容面):文本回读 / 词坐标 / 画op / 光栅。
未安装 pdfspine 时整文件跳过(CI 的 C-10 阶段再把它钉进测试矩阵)。
几何断言只涉及我们自己控制的量(页面盒、边距、缩进),容差 2pt;
字体名断言依赖本机字体环境(macOS Hiragino),按 PRD 属地化为本地自证门。
"""

from __future__ import annotations

import warnings
from collections import Counter

import pytest

import docspine
from conftest import _DOC_HEADER, build_docx

pdfspine = pytest.importorskip("pdfspine")


def _render(data: bytes, **kw) -> bytes:
    doc = docspine.open_bytes(data)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")  # 降级告警另有专测
        return doc.to_pdf(**kw)


def _open_pdf(pdf: bytes):
    return pdfspine.open(stream=pdf, filetype="pdf")


def _body(body_xml: str) -> bytes:
    return build_docx(_DOC_HEADER + f"\n  <w:body>{body_xml}</w:body>\n</w:document>")


# ============================================================ C-1:端到端绿条


def test_to_pdf_produces_valid_pdf_with_section_geometry(pdf_export_docx_bytes):
    """两节 fixture:合法 PDF、2 页、每页 rect 与 sectPr 几何吻合(0.5pt 内)。"""
    pdf = _render(pdf_export_docx_bytes)
    assert pdf.startswith(b"%PDF-")
    d = _open_pdf(pdf)
    assert d.page_count == 2
    # 第一节 Letter 纵向 12240x15840 twip = 612x792pt。
    r0 = d[0].rect
    assert abs(r0.width - 612.0) < 0.5 and abs(r0.height - 792.0) < 0.5
    # 第二节 A4 横向 16838x11906 twip = 841.9x595.3pt。
    r1 = d[1].rect
    assert abs(r1.width - 841.9) < 0.5 and abs(r1.height - 595.3) < 0.5


def test_read_back_tokens_match_to_text_in_order(pdf_export_docx_bytes):
    """内容读回:PDF 文本 token 与 to_text() 逐 token 相等(F1 = 1、序一致)。"""
    doc = docspine.open_bytes(pdf_export_docx_bytes)
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        pdf = doc.to_pdf()
    d = _open_pdf(pdf)
    got = " ".join(d[i].get_text() for i in range(d.page_count)).split()
    want = doc.to_text().split()
    assert want, "fixture 不应为空"
    assert got == want, f"token 序/内容不一致: {got=} {want=}"
    # F1(冗余于逐 token 相等,按 PRD 门径显式给出)。
    ca, cb = Counter(want), Counter(got)
    overlap = sum(min(ca[t], cb[t]) for t in ca)
    f1 = 2 * overlap / (len(want) + len(got))
    assert f1 >= 0.99


def test_first_word_sits_at_margin_origin(pdf_export_docx_bytes):
    """首词落在(左边距, 上边距)附近(2pt 容差;基线/行高造成的纵向余量放宽到一行)。"""
    d = _open_pdf(_render(pdf_export_docx_bytes))
    words = d[0].get_text_words()
    x0, y0 = words[0][0], words[0][1]
    assert abs(x0 - 72.0) < 2.0
    assert 72.0 - 2.0 < y0 < 72.0 + 20.0  # 词框顶不高于上边距,且在首行内


def test_save_pdf_writes_file(tmp_path, pdf_export_docx_bytes):
    doc = docspine.open_bytes(pdf_export_docx_bytes)
    out = tmp_path / "out.pdf"
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        doc.save_pdf(out)
    data = out.read_bytes()
    assert data.startswith(b"%PDF-") and len(data) > 1000


def test_font_map_substitution_override():
    """font_map 把未安装的请求 family 指到候选;引擎报 font-substituted 降级。"""
    data = _body(
        '<w:p><w:r><w:rPr><w:rFonts w:ascii="No Such Family docspine"/></w:rPr>'
        "<w:t>mapped</w:t></w:r></w:p>"
    )
    doc = docspine.open_bytes(data)
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        pdf = doc.to_pdf(font_map={"No Such Family docspine": "Liberation Serif"})
    assert pdf.startswith(b"%PDF-")
    msgs = [str(w.message) for w in ws]
    assert any("Liberation Serif" in m for m in msgs), msgs


def test_raster_is_not_blank(pdf_export_docx_bytes):
    """光栅非空白(_near_blank 门的本地等价):首页像素不全白。"""
    d = _open_pdf(_render(pdf_export_docx_bytes))
    pix = d[0].get_pixmap()
    samples = bytes(pix.samples)
    assert any(b != 0xFF for b in samples), "首页光栅不应是全白"


def test_explicit_page_break_yields_two_pages():
    """C-3 渲染门:``<w:br w:type="page"/>`` 产出 2 页,断点前后文字各归其页。"""
    data = _body(
        "<w:p><w:r><w:t>before</w:t><w:br w:type=\"page\"/><w:t>after</w:t></w:r></w:p>"
    )
    d = _open_pdf(_render(data))
    assert d.page_count == 2
    assert "before" in d[0].get_text() and "after" not in d[0].get_text()
    assert "after" in d[1].get_text()


def test_degradation_warnings_dedupe_by_kind():
    """降级告警每类一次:两个多栏节 + 一张图片 → 各自恰好 1 条 UserWarning。"""
    body = (
        "<w:p><w:r><w:t>one</w:t></w:r></w:p>"
        "<w:p><w:pPr><w:sectPr><w:cols w:num=\"2\"/></w:sectPr></w:pPr></w:p>"
        "<w:p><w:r><w:t>two</w:t></w:r></w:p>"
        "<w:sectPr><w:cols w:num=\"3\"/></w:sectPr>"
    )
    doc = docspine.open_bytes(_body(body))
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        doc.to_pdf()
    col_warnings = [w for w in ws if "column" in str(w.message)]
    assert len(col_warnings) == 1, [str(w.message) for w in ws]
    assert issubclass(col_warnings[0].category, UserWarning)


# ============================================================ C-4:直接格式化渲染门


def test_hanging_indent_first_and_wrapped_line_x0():
    """悬挂缩进:首行 x0 = 边距+left−hanging ±2pt;续行 x0 = 边距+left ±2pt。"""
    data = _body(
        '<w:p><w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr>'
        "<w:r><w:t>Hang lead words that should wrap onto a second line because this "
        "sentence is deliberately long enough to overflow the first line width of a "
        "letter page.</w:t></w:r></w:p>"
    )
    d = _open_pdf(_render(data))
    words = d[0].get_text_words()
    lines: dict[float, list] = {}
    for w in words:
        lines.setdefault(round(w[1], 1), []).append(w)
    ys = sorted(lines)
    assert len(ys) >= 2, "fixture 必须换行"
    first_x0 = min(w[0] for w in lines[ys[0]])
    wrapped_x0 = min(w[0] for w in lines[ys[1]])
    assert abs(first_x0 - (72.0 + 36.0 - 18.0)) < 2.0  # 90pt
    assert abs(wrapped_x0 - (72.0 + 36.0)) < 2.0  # 108pt


def test_spacing_after_moves_page_break():
    """段后间距确定性地挪动分页:同样 40 段,无间距 1 页,段后 12pt 变 2 页。"""

    def doc_with_spacing(after_twips: int | None) -> bytes:
        spacing = (
            f'<w:pPr><w:spacing w:after="{after_twips}"/></w:pPr>'
            if after_twips
            else ""
        )
        paras = "".join(
            f"<w:p>{spacing}<w:r><w:t>Paragraph number {i} keeps the page busy.</w:t></w:r></w:p>"
            for i in range(40)
        )
        return _body(paras)

    tight = _open_pdf(_render(doc_with_spacing(None)))
    spaced = _open_pdf(_render(doc_with_spacing(240)))
    assert tight.page_count == 1
    assert spaced.page_count == 2


def test_highlight_and_strike_paint_ops():
    """高亮 + 删除线:get_drawings 至少各见一个填充 op 与描边 op。"""
    data = _body(
        '<w:p><w:r><w:rPr><w:highlight w:val="yellow"/><w:strike/></w:rPr>'
        "<w:t>marked text</w:t></w:r></w:p>"
    )
    d = _open_pdf(_render(data))
    drawings = d[0].get_drawings()
    kinds = [dr.get("type") for dr in drawings]
    assert "f" in kinds, f"高亮应画填充矩形: {kinds}"
    assert "s" in kinds, f"删除线应画描边线: {kinds}"


def test_cjk_run_uses_east_asia_font_slot():
    """CJK 字符落 eastAsia 字体槽(span 字体名断言;本机装有 Hiragino Sans GB)。"""
    data = _body(
        '<w:p><w:r><w:rPr><w:rFonts w:ascii="Helvetica" w:eastAsia="Hiragino Sans GB"/></w:rPr>'
        "<w:t>AB中文CD</w:t></w:r></w:p>"
    )
    d = _open_pdf(_render(data))
    spans = [
        (s["text"], s["font"])
        for b in d[0].get_text("dict")["blocks"]
        for line in b.get("lines", [])
        for s in line.get("spans", [])
    ]
    cjk = [f for t, f in spans if "中文" in t]
    latin = [f for t, f in spans if "AB" in t]
    assert cjk and latin, spans
    assert cjk[0] != latin[0], "CJK 段与拉丁段应是不同字体"
    assert "Hiragino" in cjk[0].replace(" ", ""), spans


def test_empty_paragraph_occupies_one_line():
    """空段落占一行高:两段之间夹一个空段,第三段的 y 应比紧邻排布低一行。"""
    with_blank = _body(
        "<w:p><w:r><w:t>alpha</w:t></w:r></w:p><w:p/>"
        "<w:p><w:r><w:t>omega</w:t></w:r></w:p>"
    )
    without_blank = _body(
        "<w:p><w:r><w:t>alpha</w:t></w:r></w:p>"
        "<w:p><w:r><w:t>omega</w:t></w:r></w:p>"
    )

    def omega_y(data: bytes) -> float:
        page = _open_pdf(_render(data))[0]
        return next(w[1] for w in page.get_text_words() if w[4] == "omega")

    dy = omega_y(with_blank) - omega_y(without_blank)
    assert 8.0 < dy < 25.0, f"空段应恰好垫高约一行: {dy=}"


def test_direct_bold_italic_color_render_distinct_spans(pdf_export_docx_bytes):
    """直格 bold/italic/color:同段两个 run 输出为不同字面/颜色的 span。"""
    d = _open_pdf(_render(pdf_export_docx_bytes))
    spans = [
        s
        for b in d[0].get_text("dict")["blocks"]
        for line in b.get("lines", [])
        for s in line.get("spans", [])
    ]
    bold = [s for s in spans if "Bold lead" in s["text"]]
    red = [s for s in spans if "red italic tail" in s["text"]]
    assert bold and red
    assert bold[0]["font"] != red[0]["font"], "粗体与斜体应是不同 face"
    assert red[0]["color"] != bold[0]["color"], "红色 run 颜色应异于黑色 run"
