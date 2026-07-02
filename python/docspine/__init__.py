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

import os
from importlib.metadata import PackageNotFoundError, version as _pkg_version

from . import _core
from ._core import (
    Document,
    DocError,
    DocOcrError,
    DocRenderError,
    DocUnsupportedError,
    DocXmlError,
    DocZipError,
    open,
    open_bytes,
    probe_doc,
)

# --- OCR 模型解析:把 Rust PaddleOCR 引擎指向模型权重 ---------------------------

# OCR 推理在姊妹 crate ``ocrspine`` 里,其 ``models_dir()`` 读这个变量。docspine 没有
# 自己的覆盖变量,直接用引擎变量本身既作覆盖又作目标。
_OCRSPINE_MODELS_ENV = "OCRSPINE_MODELS"


def _ensure_ocr_models_env() -> None:
    """把 Rust PaddleOCR 引擎指向模型权重(惰性、廉价、幂等、跨平台)。

    docspine 编进了 OCR 代码(``--features ocr``)但不带模型;~28 MB 的 PP-OCRv5 ONNX
    权重来自共享数据包 ``ocrspine-models``(硬依赖),所以裸 ``pip install docspine``
    即全功能 OCR、离线可跑。OCR 推理在姊妹 crate ``ocrspine`` 里,其 ``models_dir()``
    读 ``OCRSPINE_MODELS``。在调用 OCR 入口前解析:

    1. ``OCRSPINE_MODELS`` 已在环境里 → 原样保留(用户显式覆盖 / 源码 checkout 时手动指);
    2. 否则已安装的共享数据包 ``ocrspine_models`` → 从其 ``models_dir()`` 设置它;
    3. 否则什么都不做 —— 引擎回退到编译期烘进的 ``ocrspine/models`` 开发目录(源码
       checkout),或抛出清晰的 ``DocOcrError`` / ``DocUnsupportedError``。
    """
    if os.environ.get(_OCRSPINE_MODELS_ENV):
        return
    try:
        import ocrspine_models
    except ImportError:
        return
    try:
        os.environ[_OCRSPINE_MODELS_ENV] = os.fspath(ocrspine_models.models_dir())
    except Exception:
        # 数据包损坏/不全时不要掩盖引擎自身的清晰报错;保持 env 不变,交给 Rust 端报。
        pass


# OCR 入口仅在 `--features ocr` 构建时编入扩展;否则优雅缺省为 None。顶层 Python 包装
# 在委托给 Rust ``_core`` 前,先把引擎指向共享数据包里的 PP-OCRv5 权重(见
# :func:`_ensure_ocr_models_env`),使裸 ``pip install docspine`` 即可离线全功能 OCR。
try:  # pragma: no cover - 取决于构建特性
    from ._core import ocr_image as _core_ocr_image
    from ._core import reconstruct_image_table as _core_reconstruct_image_table
except ImportError:  # pragma: no cover
    _core_ocr_image = None  # type: ignore[assignment]
    _core_reconstruct_image_table = None  # type: ignore[assignment]

if _core_ocr_image is not None:

    def ocr_image(data: bytes) -> list[dict[str, object]]:
        """对图片字节做本地 OCR,返回 ``[{text, bbox, confidence}, ...]``。

        在委托给 Rust ``_core.ocr_image`` 前,先把引擎指向共享数据包里的 PP-OCRv5
        权重。非图片字节会抛出类型化的 :class:`DocOcrError`(:class:`DocError` 子类),
        绝不 panic。
        """
        _ensure_ocr_models_env()
        return _core_ocr_image(data)

    def reconstruct_image_table(data: bytes) -> list[dict[str, object]]:
        """把一张图片里的表格(扫描件/截图)从 OCR 词框几何重建成网格,返回 ``list[dict]``。

        在委托给 Rust ``_core.reconstruct_image_table`` 前,先把引擎指向共享数据包里
        的 PP-OCRv5 权重(见 :func:`_ensure_ocr_models_env`)。
        """
        _ensure_ocr_models_env()
        return _core_reconstruct_image_table(data)

else:  # pragma: no cover - 取决于构建特性
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
    "DocRenderError",
    "__version__",
]
