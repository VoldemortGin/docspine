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


def build_docx(document_xml: str, *, image: bytes | None = None) -> bytes:
    """用纯 ``zipfile`` 把给定的 ``word/document.xml`` 包成最小合法 ``.docx``。

    ``image`` 给定时写入 ``word/media/image1.png``(主文档关系把 ``rId10`` 指向它);
    不给时该关系悬空、无伤(图片测试才用)。
    """
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr("[Content_Types].xml", _CONTENT_TYPES)
        z.writestr("_rels/.rels", _ROOT_RELS)
        z.writestr("word/document.xml", document_xml)
        z.writestr("word/_rels/document.xml.rels", _DOC_RELS)
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


@pytest.fixture(scope="session")
def revisions_docx_bytes() -> bytes:
    """含 w:ins / w:del 修订标记的合成 ``.docx`` 字节。"""
    return build_docx(_REVISIONS_DOCUMENT)


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
