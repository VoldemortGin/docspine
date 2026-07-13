"""测试夹具:用纯 Python ``zipfile`` 合成最小但合法的 ``.docx``,**不落二进制 fixture**。

一个 ``.docx`` 是 OOXML —— 一个装着 XML 部件的 zip 包。这里手写最小部件集合:
``[Content_Types].xml`` + 根关系 + ``word/document.xml`` 及其关系 + 一张内嵌图片(``word/media/``)。

文档内容覆盖解析层的关键路径:
- 一个带样式 run 的标题段(字体/字号/粗/斜/颜色/对齐/样式)。
- 一张表格,做扎实:**横向 gridSpan 合并** + **纵向 vMerge restart/continue 合并** +
  **嵌套表** + 单元格填充 + 单元格宽度 + 表头行。
- 一个内嵌图片段(``w:drawing`` > ``a:blip@r:embed`` -> ``word/media/image1.png``)。
"""

from __future__ import annotations

import io
import struct
import zipfile
import zlib

import pytest

_CONTENT_TYPES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
  <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
  <Default Extension="xml" ContentType="application/xml"/>
  <Default Extension="png" ContentType="image/png"/>
  <Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/>
</Types>"""

# styles / numbering / theme 部件的 Content-Types Override(build_docx 按需拼接)。
_STYLES_OVERRIDE = '  <Override PartName="/word/styles.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.styles+xml"/>'
_NUMBERING_OVERRIDE = '  <Override PartName="/word/numbering.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.numbering+xml"/>'
_THEME_OVERRIDE = '  <Override PartName="/word/theme/theme1.xml" ContentType="application/vnd.openxmlformats-officedocument.theme+xml"/>'
_SETTINGS_OVERRIDE = '  <Override PartName="/word/settings.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.settings+xml"/>'

_ROOT_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/>
</Relationships>"""

# 主文档关系:把图片 r:embed="rId10" 映射到 media/image1.png。
_DOC_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.png"/>
</Relationships>"""

# 一份文档:标题段 + 重型表格 + 内嵌图片段。
_DOCUMENT = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <w:body>
    <w:p>
      <w:pPr><w:pStyle w:val="Heading1"/><w:jc w:val="center"/></w:pPr>
      <w:r>
        <w:rPr>
          <w:rFonts w:ascii="Calibri"/>
          <w:b/><w:i/><w:sz w:val="48"/>
          <w:color w:val="1F4E79"/>
        </w:rPr>
        <w:t>Hello docspine</w:t>
      </w:r>
    </w:p>
    <w:tbl>
      <w:tblPr><w:tblStyle w:val="TableGrid"/></w:tblPr>
      <w:tblGrid>
        <w:gridCol w:w="2400"/>
        <w:gridCol w:w="2400"/>
        <w:gridCol w:w="2400"/>
      </w:tblGrid>
      <w:tr>
        <w:trPr><w:trHeight w:val="400"/><w:tblHeader/></w:trPr>
        <w:tc>
          <w:tcPr><w:gridSpan w:val="2"/><w:shd w:fill="FFCC00"/></w:tcPr>
          <w:p><w:r><w:t>Merged Header</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:tcPr><w:vMerge w:val="restart"/></w:tcPr>
          <w:p><w:r><w:t>Spanning Down</w:t></w:r></w:p>
        </w:tc>
      </w:tr>
      <w:tr>
        <w:tc>
          <w:tcPr><w:tcW w:w="2400" w:type="dxa"/></w:tcPr>
          <w:p><w:r><w:t>A2</w:t></w:r></w:p>
        </w:tc>
        <w:tc>
          <w:p><w:r><w:t>B2</w:t></w:r></w:p>
          <w:tbl>
            <w:tblGrid><w:gridCol w:w="1200"/></w:tblGrid>
            <w:tr><w:tc><w:p><w:r><w:t>nested</w:t></w:r></w:p></w:tc></w:tr>
          </w:tbl>
        </w:tc>
        <w:tc>
          <w:tcPr><w:vMerge w:val="continue"/></w:tcPr>
          <w:p/>
        </w:tc>
      </w:tr>
    </w:tbl>
    <w:p>
      <w:r>
        <w:drawing>
          <wp:inline>
            <wp:extent cx="914400" cy="914400"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
                  <pic:blipFill><a:blip r:embed="rId10"/></pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:inline>
        </w:drawing>
      </w:r>
    </w:p>
  </w:body>
</w:document>"""


def _png_1x1() -> bytes:
    """造一个最小的合法 1x1 PNG(纯 zlib + 手写块,不依赖 Pillow)。"""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = struct.pack(">IIBBBBB", 1, 1, 8, 2, 0, 0, 0)  # 1x1, 8-bit, truecolor
    raw = b"\x00\xff\xff\xff"  # one filtered scanline: filter 0 + white pixel
    idat = zlib.compress(raw)
    return sig + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def _build_minimal_docx() -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", _CONTENT_TYPES)
        z.writestr("_rels/.rels", _ROOT_RELS)
        z.writestr("word/document.xml", _DOCUMENT)
        z.writestr("word/_rels/document.xml.rels", _DOC_RELS)
        z.writestr("word/media/image1.png", _png_1x1())
    return buf.getvalue()


@pytest.fixture(scope="session")
def minimal_docx_bytes() -> bytes:
    """一个合成的最小 ``.docx`` 字节串(标题段 + 重型表格 + 内嵌图片)。"""
    return _build_minimal_docx()


@pytest.fixture
def minimal_docx_path(minimal_docx_bytes: bytes, tmp_path) -> str:
    """把合成的 ``.docx`` 落到临时文件,返回其路径(测 ``open(path)`` 路径)。"""
    p = tmp_path / "doc.docx"
    p.write_bytes(minimal_docx_bytes)
    return str(p)


@pytest.fixture(scope="session")
def image1_png_bytes() -> bytes:
    """合成 docx 内嵌的那张 1x1 PNG 的字节长度,供断言 image_bytes_len。"""
    return _png_1x1()


# --- 通用 docx 构造器(供新增的导出 / 修订 / 图片闭环测试复用) ----------------

# w:document 根(各测试文档共用的命名空间声明)。
_DOC_HEADER = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"
            xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"
            xmlns:wp="http://schemas.openxmlformats.org/drawingml/2006/wordprocessingDrawing"
            xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">"""


def build_docx(
    document_xml: str,
    *,
    image: bytes | None = None,
    styles_xml: str | None = None,
    numbering_xml: str | None = None,
    theme_xml: str | None = None,
    settings_xml: str | None = None,
) -> bytes:
    """用纯 ``zipfile`` 把给定的 ``word/document.xml`` 包成最小合法 ``.docx``。

    ``image`` 给定时写入 ``word/media/image1.png``(主文档关系把 ``rId10`` 指向它);
    不给时该关系悬空、无伤(图片测试才用)。``styles_xml`` / ``numbering_xml`` /
    ``theme_xml`` 给定时写入对应部件(含 Content-Types Override),供样式级联 /
    列表编号 / 导出测试合成带样式的文档。
    """
    types = _CONTENT_TYPES
    overrides = "".join(
        f"\n{line}"
        for part, line in [
            (styles_xml, _STYLES_OVERRIDE),
            (numbering_xml, _NUMBERING_OVERRIDE),
            (theme_xml, _THEME_OVERRIDE),
            (settings_xml, _SETTINGS_OVERRIDE),
        ]
        if part is not None
    )
    if overrides:
        types = types.replace("\n</Types>", f"{overrides}\n</Types>")
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", types)
        z.writestr("_rels/.rels", _ROOT_RELS)
        z.writestr("word/document.xml", document_xml)
        z.writestr("word/_rels/document.xml.rels", _DOC_RELS)
        if styles_xml is not None:
            z.writestr("word/styles.xml", styles_xml)
        if numbering_xml is not None:
            z.writestr("word/numbering.xml", numbering_xml)
        if theme_xml is not None:
            z.writestr("word/theme/theme1.xml", theme_xml)
        if settings_xml is not None:
            z.writestr("word/settings.xml", settings_xml)
        if image is not None:
            z.writestr("word/media/image1.png", image)
    return buf.getvalue()


# 含修订:w:ins(插入,接受后应保留文字)+ w:del(删除,接受后应丢弃文字)。
_REVISIONS_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p>
      <w:r><w:t>Start </w:t></w:r>
      <w:ins w:id="1" w:author="A" w:date="2026-01-01T00:00:00Z">
        <w:r><w:t>INSERTED</w:t></w:r>
      </w:ins>
      <w:r><w:t> middle </w:t></w:r>
      <w:del w:id="2" w:author="A" w:date="2026-01-01T00:00:00Z">
        <w:r><w:delText>DELETED</w:delText></w:r>
      </w:del>
      <w:r><w:t>end</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"""
)

# 一个不含任何合并的朴素 2x2 表(+ 二级标题),用于验证 to_markdown 走 GFM 管道表分支。
_SIMPLE_TABLE_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading2"/></w:pPr><w:r><w:t>Sub</w:t></w:r></w:p>
    <w:tbl>
      <w:tblGrid><w:gridCol w:w="1000"/><w:gridCol w:w="1000"/></w:tblGrid>
      <w:tr>
        <w:tc><w:p><w:r><w:t>H1</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>H2</w:t></w:r></w:p></w:tc>
      </w:tr>
      <w:tr>
        <w:tc><w:p><w:r><w:t>x</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>y</w:t></w:r></w:p></w:tc>
      </w:tr>
    </w:tbl>
  </w:body>
</w:document>"""
)

# 仅含一张内嵌图片(r:embed=rId10 -> media/image1.png),供“取字节 -> OCR”闭环测试。
_IMAGE_ONLY_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p>
      <w:r>
        <w:drawing>
          <wp:inline>
            <wp:extent cx="3000000" cy="1000000"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
                  <pic:blipFill><a:blip r:embed="rId10"/></pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:inline>
        </w:drawing>
      </w:r>
    </w:p>
  </w:body>
</w:document>"""
)


# 两节文档(C-2):段内 pPr>sectPr(Letter 纵向,结束第一节)+ body 末尾 sectPr
# (A4 横向、自定义边距、两栏,定义最后一节)。
_SECTIONS_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p><w:r><w:t>section one</w:t></w:r></w:p>
    <w:p>
      <w:pPr>
        <w:sectPr>
          <w:pgSz w:w="12240" w:h="15840"/>
          <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"
                   w:header="720" w:footer="720" w:gutter="0"/>
        </w:sectPr>
      </w:pPr>
    </w:p>
    <w:p><w:r><w:t>section two</w:t></w:r></w:p>
    <w:sectPr>
      <w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>
      <w:pgMar w:top="720" w:right="1080" w:bottom="360" w:left="1800"
               w:header="500" w:footer="400" w:gutter="100"/>
      <w:cols w:num="2" w:space="708"/>
    </w:sectPr>
  </w:body>
</w:document>"""
)

# 内容丢失修复三连(C-3):块级/行内 w:sdt、w:fldSimple 缓存结果、w:br@w:type="page"。
_CONTENT_LOSS_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:sdt>
      <w:sdtPr><w:alias w:val="Cover"/></w:sdtPr>
      <w:sdtContent><w:p><w:r><w:t>COVER-TITLE</w:t></w:r></w:p></w:sdtContent>
    </w:sdt>
    <w:p>
      <w:r><w:t>Updated </w:t></w:r>
      <w:sdt><w:sdtContent><w:r><w:t>2026-07-02</w:t></w:r></w:sdtContent></w:sdt>
    </w:p>
    <w:p>
      <w:r><w:t>Page </w:t></w:r>
      <w:fldSimple w:instr=" PAGE \\* MERGEFORMAT "><w:r><w:t>7</w:t></w:r></w:fldSimple>
    </w:p>
    <w:p><w:r><w:t>before</w:t><w:br w:type="page"/><w:t>after</w:t></w:r></w:p>
  </w:body>
</w:document>"""
)


# PDF 导出端到端 fixture(C-1/C-4):Heading1(样式级联)+ 两段直格正文(bold /
# italic / color)+ 无边框表格 + 段内 sectPr 分节(第二节 A4 横向换页面几何)。
_PDF_EXPORT_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Export Chapter</w:t></w:r></w:p>
    <w:p>
      <w:r><w:rPr><w:b/></w:rPr><w:t>Bold lead </w:t></w:r>
      <w:r><w:rPr><w:i/><w:color w:val="FF0000"/></w:rPr><w:t>with red italic tail.</w:t></w:r>
    </w:p>
    <w:p><w:r><w:t>Second body paragraph flows plainly after it.</w:t></w:r></w:p>
    <w:tbl>
      <w:tblGrid><w:gridCol w:w="3600"/><w:gridCol w:w="3600"/></w:tblGrid>
      <w:tr>
        <w:tc><w:tcPr><w:shd w:fill="D9E2F3"/></w:tcPr><w:p><w:r><w:t>Alpha</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>Beta</w:t></w:r></w:p></w:tc>
      </w:tr>
      <w:tr>
        <w:tc><w:p><w:r><w:t>Gamma</w:t></w:r></w:p></w:tc>
        <w:tc><w:p><w:r><w:t>Delta</w:t></w:r></w:p></w:tc>
      </w:tr>
    </w:tbl>
    <w:p>
      <w:pPr>
        <w:sectPr>
          <w:pgSz w:w="12240" w:h="15840"/>
          <w:pgMar w:top="1440" w:right="1440" w:bottom="1440" w:left="1440"
                   w:header="720" w:footer="720" w:gutter="0"/>
        </w:sectPr>
      </w:pPr>
    </w:p>
    <w:p><w:r><w:t>Landscape section closes the document.</w:t></w:r></w:p>
    <w:sectPr>
      <w:pgSz w:w="16838" w:h="11906" w:orient="landscape"/>
      <w:pgMar w:top="720" w:right="1080" w:bottom="720" w:left="1080"
               w:header="500" w:footer="400" w:gutter="0"/>
    </w:sectPr>
  </w:body>
</w:document>"""
)

# 导出 fixture 的 styles.xml:docDefaults 22 半磅(11pt)+ Normal(缺省)+
# Heading1(basedOn Normal:b / 32 半磅 = 16pt)。零 theme(落硬编码兜底)。
_PDF_EXPORT_STYLES = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:styles xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main">
  <w:docDefaults>
    <w:rPrDefault><w:rPr><w:sz w:val="22"/></w:rPr></w:rPrDefault>
    <w:pPrDefault><w:pPr/></w:pPrDefault>
  </w:docDefaults>
  <w:style w:type="paragraph" w:default="1" w:styleId="Normal"><w:name w:val="Normal"/></w:style>
  <w:style w:type="paragraph" w:styleId="Heading1">
    <w:name w:val="heading 1"/>
    <w:basedOn w:val="Normal"/>
    <w:rPr><w:b/><w:sz w:val="32"/></w:rPr>
  </w:style>
</w:styles>"""


@pytest.fixture(scope="session")
def pdf_export_docx_bytes() -> bytes:
    """PDF 导出端到端 fixture(C-1/C-4):标题 + 直格正文 + 表格 + 两节换几何。"""
    return build_docx(_PDF_EXPORT_DOCUMENT, styles_xml=_PDF_EXPORT_STYLES)


@pytest.fixture(scope="session")
def revisions_docx_bytes() -> bytes:
    """含 w:ins / w:del 修订标记的合成 ``.docx`` 字节。"""
    return build_docx(_REVISIONS_DOCUMENT)


@pytest.fixture(scope="session")
def sections_docx_bytes() -> bytes:
    """含段内 + body 末尾 ``w:sectPr`` 的两节合成 ``.docx`` 字节(C-2)。"""
    return build_docx(_SECTIONS_DOCUMENT)


@pytest.fixture(scope="session")
def content_loss_docx_bytes() -> bytes:
    """含 ``w:sdt`` / ``w:fldSimple`` / ``w:br@w:type`` 的合成 ``.docx`` 字节(C-3)。"""
    return build_docx(_CONTENT_LOSS_DOCUMENT)


@pytest.fixture(scope="session")
def simple_table_docx_bytes() -> bytes:
    """含一张无合并 2x2 表的合成 ``.docx`` 字节(测 GFM 管道表导出)。"""
    return build_docx(_SIMPLE_TABLE_DOCUMENT)


@pytest.fixture
def make_docx_with_image():
    """返回一个工厂:把给定图片字节嵌进一个只含该图的最小 ``.docx``。"""

    def _make(png: bytes) -> bytes:
        return build_docx(_IMAGE_ONLY_DOCUMENT, image=png)

    return _make


# 一张锚定浮动图(``wp:anchor``,relativeFrom="margin"):水平偏移 914400 EMU=72pt、
# 垂直偏移 457200 EMU=36pt;extent 914400×457200 EMU=72×36pt(rId10 → image1.png)。
# C-8:导出后应作为绝对定位覆盖层落在(左边距+72, 上边距+36)= (144,108)pt。
_ANCHORED_IMAGE_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p><w:r><w:t>Body text above the floating image.</w:t></w:r></w:p>
    <w:p>
      <w:r>
        <w:drawing>
          <wp:anchor behindDoc="0" allowOverlap="1" relativeHeight="0">
            <wp:simplePos x="0" y="0"/>
            <wp:positionH relativeFrom="margin"><wp:posOffset>914400</wp:posOffset></wp:positionH>
            <wp:positionV relativeFrom="margin"><wp:posOffset>457200</wp:posOffset></wp:positionV>
            <wp:extent cx="914400" cy="457200"/>
            <wp:wrapNone/>
            <a:graphic>
              <a:graphicData>
                <pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
                  <pic:blipFill><a:blip r:embed="rId10"/></pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:anchor>
        </w:drawing>
      </w:r>
    </w:p>
  </w:body>
</w:document>"""
)


@pytest.fixture(scope="session")
def anchored_image_docx_bytes() -> bytes:
    """含一张锚定浮动图(``wp:anchor`` + posOffset)的合成 ``.docx`` 字节(C-8:覆盖层)。"""
    return build_docx(_ANCHORED_IMAGE_DOCUMENT, image=_png_1x1())


# 一段带四周边框(``pBdr``)+ 底纹(``shd`` fill=D9E2F3)的段落:真画的填充矩形 +
# 四边描边线(渲染层把该段包成单格表)。
_PARA_BOX_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p>
      <w:pPr>
        <w:pBdr>
          <w:top w:val="single" w:sz="8" w:space="4" w:color="1F4E79"/>
          <w:bottom w:val="single" w:sz="8" w:space="4" w:color="1F4E79"/>
          <w:left w:val="single" w:sz="8" w:space="4" w:color="1F4E79"/>
          <w:right w:val="single" w:sz="8" w:space="4" w:color="1F4E79"/>
        </w:pBdr>
        <w:shd w:val="clear" w:color="auto" w:fill="D9E2F3"/>
      </w:pPr>
      <w:r><w:t>Boxed and shaded paragraph, drawn as fill + four border lines.</w:t></w:r>
    </w:p>
  </w:body>
</w:document>"""
)


@pytest.fixture(scope="session")
def para_box_docx_bytes() -> bytes:
    """含一段 ``pBdr`` + ``shd`` 段落的合成 ``.docx`` 字节(C-4 收口:底纹/边框真画)。"""
    return build_docx(_PARA_BOX_DOCUMENT)


# --- EMF 矢量图 fixture(C-8:不支持格式 → 占位框 + UnsupportedImageFormat) --------

# 主文档关系:把 r:embed="rId10" 指向一张 EMF(而非 png)。
_EMF_DOC_RELS = """<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
  <Relationship Id="rId10" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="media/image1.emf"/>
</Relationships>"""

# 仅含一张内嵌 EMF(r:embed=rId10 -> media/image1.emf;wp:extent 2in×1in)。
_EMF_IMAGE_DOCUMENT = (
    _DOC_HEADER
    + """
  <w:body>
    <w:p>
      <w:r>
        <w:drawing>
          <wp:inline>
            <wp:extent cx="1828800" cy="914400"/>
            <a:graphic>
              <a:graphicData>
                <pic:pic xmlns:pic="http://schemas.openxmlformats.org/drawingml/2006/picture">
                  <pic:blipFill><a:blip r:embed="rId10"/></pic:blipFill>
                </pic:pic>
              </a:graphicData>
            </a:graphic>
          </wp:inline>
        </w:drawing>
      </w:r>
    </w:p>
  </w:body>
</w:document>"""
)


def _emf_bytes() -> bytes:
    """造一个最小合法 EMF 头(ENHMETAHEADER):含 iType=1(EMR_HEADER)与偏移 40 处
    ``" EMF"`` 签名,足以被魔数识别且引擎解码失败时不 panic(纯 struct,不落二进制)。"""
    header = bytearray(88)
    struct.pack_into("<I", header, 0, 1)  # iType = EMR_HEADER
    struct.pack_into("<I", header, 4, 88)  # nSize
    struct.pack_into("<I", header, 40, 0x464D4520)  # dSignature = " EMF"
    struct.pack_into("<I", header, 48, 88)  # nBytes
    struct.pack_into("<I", header, 52, 1)  # nRecords
    return bytes(header)


def _build_emf_docx() -> bytes:
    types = _CONTENT_TYPES.replace(
        '  <Default Extension="png"',
        '  <Default Extension="emf" ContentType="image/x-emf"/>\n  <Default Extension="png"',
    )
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", types)
        z.writestr("_rels/.rels", _ROOT_RELS)
        z.writestr("word/document.xml", _EMF_IMAGE_DOCUMENT)
        z.writestr("word/_rels/document.xml.rels", _EMF_DOC_RELS)
        z.writestr("word/media/image1.emf", _emf_bytes())
    return buf.getvalue()


@pytest.fixture(scope="session")
def emf_docx_bytes() -> bytes:
    """含一张内嵌 EMF 矢量图的合成 ``.docx`` 字节(C-8:占位框渲染)。"""
    return _build_emf_docx()
