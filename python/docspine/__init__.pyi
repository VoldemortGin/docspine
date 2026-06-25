"""``docspine`` 顶层 API 的类型存根(PEP 561)。"""

from __future__ import annotations

from ._core import (
    Document as Document,
    DocError as DocError,
    DocOcrError as DocOcrError,
    DocUnsupportedError as DocUnsupportedError,
    DocXmlError as DocXmlError,
    DocZipError as DocZipError,
    ocr_image as ocr_image,
    open as open,
    open_bytes as open_bytes,
    probe_doc as probe_doc,
    reconstruct_image_table as reconstruct_image_table,
)

__version__: str

__all__ = [
    "Document",
    "open",
    "open_bytes",
    "probe_doc",
    "ocr_image",
    "reconstruct_image_table",
    "DocError",
    "DocZipError",
    "DocXmlError",
    "DocUnsupportedError",
    "DocOcrError",
    "__version__",
]
