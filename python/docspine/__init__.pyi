"""``docspine`` 顶层 API 的类型存根(PEP 561)。"""

from __future__ import annotations

from typing import Any

from ._core import (
    Document as Document,
    DocError as DocError,
    DocOcrError as DocOcrError,
    DocRenderError as DocRenderError,
    DocUnsupportedError as DocUnsupportedError,
    DocXmlError as DocXmlError,
    DocZipError as DocZipError,
    open as open,
    open_bytes as open_bytes,
    probe_doc as probe_doc,
)

# ``ocr_image`` / ``reconstruct_image_table`` 是顶层 Python 包装:委托给 ``_core`` 前
# 先把引擎指向共享数据包 ``ocrspine-models`` 里的 PP-OCRv5 权重(见 ``__init__.py`` 的
# ``_ensure_ocr_models_env``)。仅在以 ``--features ocr`` 构建时存在。
def ocr_image(data: bytes) -> list[dict[str, Any]]: ...
def reconstruct_image_table(data: bytes) -> list[dict[str, Any]]: ...

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
    "DocRenderError",
    "__version__",
]
