"""docspine —— 纯 Rust 的 Word(.docx / OOXML)结构化解析器 + 本地图片 OCR。

这是由 Rust ``_core`` 扩展模块(PyO3 / maturin,abi3-py311)支撑的 Python 包。
解析面只读::func:`open` / :func:`open_bytes` 返回一个 :class:`Document` 句柄,其上
``body()`` 返回 ``list[dict]``(段落 / 表格,可自省、稳定)。**表格**给出 ``rows`` / 每行
``cells``,每个 cell 携带 ``grid_span``(横向合并)/ ``v_merge``(纵向合并)/ ``fill`` /
``width`` 与递归的 ``blocks``(嵌套表)。

:func:`ocr_image` 把图片字节交给姊妹 crate ``ocrspine``(PP-OCRv5,本地、离线、确定性)
做 OCR;:func:`reconstruct_image_table` 把一张**图片里的表格**(扫描件/截图)从 OCR 文字框
重建成网格 —— 无云端、无网络。OCR 入口仅在以 ``--features ocr`` 构建时存在。
"""

from __future__ import annotations

from importlib.metadata import PackageNotFoundError, version as _pkg_version

from . import _core
from ._core import (
    Document,
    DocError,
    DocOcrError,
    DocUnsupportedError,
    DocXmlError,
    DocZipError,
    open,
    open_bytes,
    probe_doc,
)

# OCR 入口仅在 `--features ocr` 构建时编入扩展;否则优雅缺省为 None。
try:  # pragma: no cover - 取决于构建特性
    from ._core import ocr_image, reconstruct_image_table
except ImportError:  # pragma: no cover
    ocr_image = None  # type: ignore[assignment]
    reconstruct_image_table = None  # type: ignore[assignment]

try:
    __version__ = _pkg_version("docspine")
except PackageNotFoundError:  # 源码树里未安装时回退到扩展自带版本。
    __version__ = _core.__version__

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
