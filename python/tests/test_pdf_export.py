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


# ============================================================ C-9:制表位推进 + 降级


def _post_tab_word_x0(data: bytes) -> float:
    """渲染含 ``A<w:tab/>B`` 的段落,回读后返回制表位后单词 'B' 的 x0(磅)。"""
    d = _open_pdf(_render(data))
    words = d[0].get_text_words()
    b = next(w for w in words if "B" in w[4])
    return b[0]


def test_c9_tab_advances_to_default_stop():
    r"""无 settings.xml:``\t`` 按引擎缺省 36pt 间隔推进;制表位后 x0 落在 72+36k ±1pt。"""
    data = _body("<w:p><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t></w:r></w:p>")
    rel = _post_tab_word_x0(data) - 72.0  # 页左边距(Word 缺省 1 英寸)
    nearest = round(rel / 36.0) * 36.0
    assert abs(rel - nearest) <= 1.0, f"制表位后 x0 rel={rel} 不在 36pt 停位"
    assert rel >= 36.0 - 1.0, f"tab 至少推进一个 36pt 停位,rel={rel}"


def test_c9_default_tab_stop_from_settings():
    """settings.xml defaultTabStop=1440(72pt):制表位后 x0 落在 72+72k ±1pt。"""
    settings = (
        '<?xml version="1.0" encoding="UTF-8" standalone="yes"?>'
        '<w:settings xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">'
        '<w:defaultTabStop w:val="1440"/></w:settings>'
    )
    data = build_docx(
        _DOC_HEADER
        + "\n  <w:body><w:p><w:r><w:t>A</w:t><w:tab/><w:t>B</w:t></w:r></w:p></w:body>"
        + "\n</w:document>",
        settings_xml=settings,
    )
    rel = _post_tab_word_x0(data) - 72.0
    nearest = round(rel / 72.0) * 72.0
    assert abs(rel - nearest) <= 1.0, f"制表位后 x0 rel={rel} 不在 72pt 停位"
    assert rel >= 72.0 - 1.0, f"defaultTabStop=1440 应推进到 72pt,rel={rel}"


def test_c9_custom_tab_stops_warn_once():
    """段落声明 w:tabs 自定义制表位:恰好一条 custom-tab-stops UserWarning。"""
    body = (
        '<w:p><w:pPr><w:tabs><w:tab w:val="left" w:pos="2160"/></w:tabs></w:pPr>'
        "<w:r><w:t>A</w:t><w:tab/><w:t>B</w:t></w:r></w:p>"
    )
    doc = docspine.open_bytes(_body(body))
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        doc.to_pdf()
    tab_warnings = [w for w in ws if "tab stop" in str(w.message)]
    assert len(tab_warnings) == 1, [str(w.message) for w in ws]
    assert issubclass(tab_warnings[0].category, UserWarning)


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


# ============================================================ C-6:numbering 列表渲染门

# 三级编号:十进制 %1. / 小写字母 %2. / 小写罗马 %3.,各带层级缩进(悬挂 360)。
_NUMBERING_3LVL = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:numbering xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:abstractNum w:abstractNumId="0">
    <w:lvl w:ilvl="0">
      <w:start w:val="1"/><w:numFmt w:val="decimal"/><w:lvlText w:val="%1."/>
      <w:pPr><w:ind w:left="720" w:hanging="360"/></w:pPr>
    </w:lvl>
    <w:lvl w:ilvl="1">
      <w:start w:val="1"/><w:numFmt w:val="lowerLetter"/><w:lvlText w:val="%2."/>
      <w:pPr><w:ind w:left="1440" w:hanging="360"/></w:pPr>
    </w:lvl>
    <w:lvl w:ilvl="2">
      <w:start w:val="1"/><w:numFmt w:val="lowerRoman"/><w:lvlText w:val="%3."/>
      <w:pPr><w:ind w:left="2160" w:hanging="360"/></w:pPr>
    </w:lvl>
  </w:abstractNum>
  <w:num w:numId="1"><w:abstractNumId w:val="0"/></w:num>
  <w:num w:numId="2"><w:abstractNumId w:val="0"/></w:num>
  <w:num w:numId="3">
    <w:abstractNumId w:val="0"/>
    <w:lvlOverride w:ilvl="0"><w:startOverride w:val="5"/></w:lvlOverride>
  </w:num>
</w:numbering>"""


def _list_p(text: str, num_id: int, ilvl: int) -> str:
    return (
        f'<w:p><w:pPr><w:numPr><w:ilvl w:val="{ilvl}"/><w:numId w:val="{num_id}"/>'
        f"</w:numPr></w:pPr><w:r><w:t>{text}</w:t></w:r></w:p>"
    )


def _list_docx(body_xml: str) -> bytes:
    return build_docx(
        _DOC_HEADER + f"\n  <w:body>{body_xml}</w:body>\n</w:document>",
        numbering_xml=_NUMBERING_3LVL,
    )


def test_three_level_list_labels_render_in_reading_order():
    """C-6 门:三级嵌套 1. / a. / i. 标签按文档序与正文交错读回;计数与重置正确。"""
    body = (
        _list_p("Alpha", 1, 0)
        + _list_p("Beta", 1, 1)
        + _list_p("Gamma", 1, 2)
        + _list_p("Delta", 1, 2)
        + _list_p("Echo", 1, 0)
        + _list_p("Foxtrot", 1, 1)
    )
    d = _open_pdf(_render(_list_docx(body)))
    got = d[0].get_text().split()
    want = [
        "1.", "Alpha", "a.", "Beta", "i.", "Gamma", "ii.", "Delta",
        "2.", "Echo", "a.", "Foxtrot",  # 上层推进后下层重置。
    ]
    assert got == want, got


def test_list_label_sits_left_of_text_with_level_indent():
    """C-6 门:标签 x0 < 正文 x0(gutter);层级缩进生效(正文落 边距+left)。"""
    d = _open_pdf(_render(_list_docx(_list_p("Alpha", 1, 0) + _list_p("Beta", 1, 1))))
    words = {w[4]: (w[0], w[1]) for w in d[0].get_text_words()}
    label_x, alpha_x = words["1."][0], words["Alpha"][0]
    assert label_x < alpha_x, "标签画在正文起点左侧"
    assert abs(alpha_x - (72.0 + 36.0)) < 2.0, "一级正文对齐 left=720twip"
    assert abs(words["Beta"][0] - (72.0 + 72.0)) < 2.0, "二级正文对齐 left=1440twip"
    assert abs(words["a."][1] - words["Beta"][1]) < 1.0, "标签与正文同基线行"


def test_independent_lists_restart_and_start_override():
    """C-6 门:独立 numId 各自计数;startOverride 改写起值(restart 语义)。"""
    body = (
        _list_p("one", 1, 0)
        + _list_p("two", 1, 0)
        + _list_p("fresh", 2, 0)
        + _list_p("fifth", 3, 0)
    )
    d = _open_pdf(_render(_list_docx(body)))
    got = d[0].get_text().split()
    assert got == ["1.", "one", "2.", "two", "1.", "fresh", "5.", "fifth"], got


# ============================================================ C-7:表格保真渲染门

_FULL_BORDERS = """<w:tblBorders>
  <w:top w:val="single" w:sz="8"/><w:bottom w:val="single" w:sz="8"/>
  <w:left w:val="single" w:sz="8"/><w:right w:val="single" w:sz="8"/>
  <w:insideH w:val="single" w:sz="8"/><w:insideV w:val="single" w:sz="8"/>
</w:tblBorders>"""


def _stroke_segments(page) -> list:
    """页上全部描边线段(引擎逐边独立线 op → 每条物理边一段)。"""
    return [
        it
        for dr in page.get_drawings()
        if dr.get("type") == "s"
        for it in dr.get("items", [])
        if it[0] == "l"
    ]


def test_full_borders_paint_conflict_resolved_edge_count():
    """C-7 门:全边框 + gridSpan/vMerge 合并——线段数恰为消解后的物理边数,
    合并区内无线;每格文字都在其网格矩形内(±2pt)。"""
    body = f"""<w:tbl>
      <w:tblPr>{_FULL_BORDERS}</w:tblPr>
      <w:tblGrid><w:gridCol w:w="2400"/><w:gridCol w:w="2400"/></w:tblGrid>
      <w:tr>
        <w:tc><w:tcPr><w:gridSpan w:val="2"/><w:vMerge w:val="restart"/></w:tcPr>
          <w:p><w:r><w:t>Wide</w:t></w:r></w:p></w:tc>
      </w:tr>
      <w:tr>
        <w:tc><w:tcPr><w:gridSpan w:val="2"/><w:vMerge/></w:tcPr><w:p/></w:tc>
      </w:tr>
      <w:tr>
        <w:tc><w:p><w:r><w:t>Ada</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>Bob</w:t></w:r></w:p></w:tc>
      </w:tr>
    </w:tbl>"""
    page = _open_pdf(_render(_body(body)))[0]
    # 物理边:横 4×2=8 段,再减合并区内横线 2 段 = 6;竖:行0/行1 各 2(内竖线被合并
    # 压掉)+ 行2 3 段 = 7;共 13。
    assert len(_stroke_segments(page)) == 13
    assert page.get_text().split() == ["Wide", "Ada", "Bob"]
    # 词落在各自网格矩形内:表自边距 72 起,列宽 120pt。
    words = {w[4]: w for w in page.get_text_words()}
    assert 72.0 - 2.0 < words["Ada"][0] and words["Ada"][2] < 192.0 + 2.0
    assert 192.0 - 2.0 < words["Bob"][0] and words["Bob"][2] < 312.0 + 2.0


def test_tc_borders_override_tbl_borders_with_color():
    """C-7 门:tcBorders(红,sz16)盖过 tblBorders(黑,sz8)——出现 2pt 红线。"""
    body = f"""<w:tbl>
      <w:tblPr>{_FULL_BORDERS}</w:tblPr>
      <w:tblGrid><w:gridCol w:w="2400"/></w:tblGrid>
      <w:tr>
        <w:tc>
          <w:tcPr><w:tcBorders><w:top w:val="single" w:sz="16" w:color="FF0000"/></w:tcBorders></w:tcPr>
          <w:p><w:r><w:t>hot</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
    </w:tbl>"""
    page = _open_pdf(_render(_body(body)))[0]
    strokes = [dr for dr in page.get_drawings() if dr.get("type") == "s"]
    red = [dr for dr in strokes if dr.get("color") == (1.0, 0.0, 0.0)]
    assert red, [dr.get("color") for dr in strokes]
    assert abs(red[0].get("width") - 2.0) < 1e-6, "sz=16 八分之一磅 → 2pt"


def test_cell_margins_offset_text_x0():
    """C-7 门:缺省左边距 108twip=5.4pt;tcMar left=288twip=14.4pt 逐边生效。"""

    def first_word_x0(tcpr: str) -> float:
        body = f"""<w:tbl>
          <w:tblGrid><w:gridCol w:w="4800"/></w:tblGrid>
          <w:tr><w:tc>{tcpr}<w:p><w:r><w:t>margined</w:t></w:r></w:p></w:tc></w:tr>
        </w:tbl>"""
        page = _open_pdf(_render(_body(body)))[0]
        return page.get_text_words()[0][0]

    assert abs(first_word_x0("") - (72.0 + 5.4)) < 2.0
    wide = first_word_x0(
        '<w:tcPr><w:tcMar><w:start w:w="288" w:type="dxa"/></w:tcMar></w:tcPr>'
    )
    assert abs(wide - (72.0 + 14.4)) < 2.0


def test_valign_and_row_too_tall_warn_once():
    """C-7 门:vAlign(center)与超页高行各浮一次降级告警。"""
    body = """<w:tbl>
      <w:tblGrid><w:gridCol w:w="2400"/></w:tblGrid>
      <w:tr>
        <w:trPr><w:trHeight w:val="20000" w:hRule="exact"/></w:trPr>
        <w:tc><w:tcPr><w:vAlign w:val="center"/></w:tcPr>
          <w:p><w:r><w:t>deep</w:t></w:r></w:p></w:tc>
      </w:tr>
    </w:tbl>"""
    doc = docspine.open_bytes(_body(body))
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        doc.to_pdf()
    msgs = [str(w.message) for w in ws]
    assert sum("vertical alignment" in m for m in msgs) == 1, msgs
    assert sum("taller than the page body" in m for m in msgs) == 1, msgs


def test_thirty_row_table_paginates_rows_whole():
    """C-7 门:30 行表跨页——行整行挪页,同一行的左右两格永不分家。"""
    rows = "".join(
        f"""<w:tr><w:trPr><w:trHeight w:val="600"/></w:trPr>
          <w:tc><w:p><w:r><w:t>L{i:02d}</w:t></w:r></w:p></w:tc>
          <w:tc><w:p><w:r><w:t>R{i:02d}</w:t></w:r></w:p></w:tc></w:tr>"""
        for i in range(30)
    )
    body = f"""<w:tbl>
      <w:tblGrid><w:gridCol w:w="2400"/><w:gridCol w:w="2400"/></w:tblGrid>
      {rows}
    </w:tbl>"""
    d = _open_pdf(_render(_body(body)))
    assert d.page_count == 2, "30 行 × 30pt > 一页正文高"
    pages = [d[i].get_text().split() for i in range(d.page_count)]
    for i in range(30):
        on = [n for n, toks in enumerate(pages) if f"L{i:02d}" in toks]
        assert len(on) == 1, f"行 {i} 应恰好落在一页"
        assert f"R{i:02d}" in pages[on[0]], f"行 {i} 的两格必须同页"


# ============================================================ C-8:内嵌图片渲染


def test_inline_image_is_embedded_in_pdf(minimal_docx_bytes):
    """C-8:含内嵌图片的 docx 导出后,图片真正嵌入 PDF(不再只发 skip 告警)。

    ``minimal_docx_bytes`` 带一张 ``wp:inline`` 的 1×1 PNG(``wp:extent`` 1in×1in)。
    设 ``DOCSPINE_E2E_PNG`` 环境变量时另存一张光栅供人工目检。
    """
    import os

    pdf = _render(minimal_docx_bytes)
    doc = _open_pdf(pdf)
    assert doc.page_count >= 1
    imgs = doc[0].get_images()
    assert len(imgs) >= 1, "内嵌图片应出现在导出的 PDF 里"
    if os.environ.get("DOCSPINE_E2E_PNG"):
        doc[0].get_pixmap(dpi=120).save(os.environ["DOCSPINE_E2E_PNG"])


def test_emf_image_draws_placeholder_and_warns_once(emf_docx_bytes):
    """C-8:EMF 矢量图无法解码 → 画浅灰占位框(``get_drawings`` 见填充 + 描边),
    并发恰一条 ``UnsupportedImageFormat``(``UserWarning``);PDF 仍产出、无 panic。"""
    doc = docspine.open_bytes(emf_docx_bytes)
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        pdf = doc.to_pdf()
    assert pdf.startswith(b"%PDF-")
    d = _open_pdf(pdf)
    assert d.page_count >= 1
    # EMF 不解码 → 不作为图片 XObject 嵌入,而是画一个占位框(填充 + 四边线)。
    assert not d[0].get_images(), "EMF 不应作为图片嵌入 PDF"
    kinds = [dr.get("type") for dr in d[0].get_drawings()]
    assert "f" in kinds, f"占位框应画填充: {kinds}"
    assert "s" in kinds, f"占位框应画描边: {kinds}"
    # 首页光栅非空白(占位框可见)。
    assert any(b != 0xFF for b in bytes(d[0].get_pixmap().samples)), "占位框应使首页非全白"
    # 告警恰一条,且是 UnsupportedImageFormat 那一类(UserWarning)。
    fmt = [w for w in ws if "EMF" in str(w.message)]
    assert len(fmt) == 1, [str(w.message) for w in ws]
    assert issubclass(fmt[0].category, UserWarning)


def test_anchored_image_placed_at_pos_offset(anchored_image_docx_bytes):
    """C-8 收口:锚定浮动图按 ``wp:anchor`` 的 posOffset 绝对定位成覆盖层——落点
    (左边距+72, 上边距+36)=(144,108)pt、尺寸 72×36pt(±1pt);并发一条 FloatingNoWrap
    (``UserWarning``,文字不环绕)。"""
    doc = docspine.open_bytes(anchored_image_docx_bytes)
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        pdf = doc.to_pdf()
    d = _open_pdf(pdf)
    rects = d[0].get_image_rects()
    assert len(rects) == 1, "锚定图应作为一张图片 XObject 落在首页"
    r = rects[0]
    assert abs(r.x0 - 144.0) < 1.0, f"x0={r.x0}"
    assert abs(r.y0 - 108.0) < 1.0, f"y0={r.y0}"
    assert abs(r.width - 72.0) < 1.0 and abs(r.height - 36.0) < 1.0
    floating = [w for w in ws if "float" in str(w.message).lower()]
    assert len(floating) == 1, [str(w.message) for w in ws]
    assert issubclass(floating[0].category, UserWarning)


def test_paragraph_border_and_shading_are_drawn():
    """段落 ``pBdr`` + ``shd`` 真画:底纹铺填充矩形、四周边框画描边线
    (``get_drawings`` 见 ``f`` 与 ``s``);正文照常读回;不发 para-border/shading 告警。"""
    data = _body(
        "<w:p><w:pPr>"
        '<w:pBdr>'
        '<w:top w:val="single" w:sz="8" w:space="4" w:color="000000"/>'
        '<w:bottom w:val="single" w:sz="8" w:space="4" w:color="000000"/>'
        '<w:left w:val="single" w:sz="8" w:space="4" w:color="000000"/>'
        '<w:right w:val="single" w:sz="8" w:space="4" w:color="000000"/>'
        '</w:pBdr>'
        '<w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/>'
        "</w:pPr><w:r><w:t>Boxed and shaded paragraph.</w:t></w:r></w:p>"
    )
    with warnings.catch_warnings(record=True) as ws:
        warnings.simplefilter("always")
        pdf = docspine.open_bytes(data).to_pdf()
    d = _open_pdf(pdf)
    kinds = [dr.get("type") for dr in d[0].get_drawings()]
    assert "f" in kinds, f"段落底纹应画填充矩形: {kinds}"
    assert "s" in kinds, f"段落边框应画描边线: {kinds}"
    assert "Boxed and shaded paragraph." in d[0].get_text()
    msgs = " ".join(str(w.message) for w in ws)
    assert "border" not in msgs and "shading" not in msgs, msgs
