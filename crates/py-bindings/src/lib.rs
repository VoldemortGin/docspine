// `py-bindings` 是唯一的 FFI 关隘,也是唯一允许使用 `unsafe` 的一方(PyO3 会生成 FFI
// glue)。因此它不 `forbid(unsafe_code)`,而是要求 `unsafe` 必须被显式限定作用域。
#![deny(unsafe_op_in_unsafe_fn)]
//! 把 docspine 的 Rust 核暴露给 Python 的 `_core` 扩展模块(PyO3 / maturin,abi3-py311)。
//!
//! 暴露**只读**解析面:`open` / `open_bytes` 返回一个 [`Document`] 句柄,其上 `body()` 返回
//! `list[dict]`(段落 / 表格,可自省、稳定)。表格 dict 给出 `rows` / 每行 `cells`,每个 cell
//! 携带 `grid_span` / `v_merge` / `fill` / `width` 与递归的 `blocks`(嵌套表)。开启 `ocr`
//! 特性后另有 `ocr_image`(图片字节 -> 词)与 `reconstruct_image_table`(图片表格 -> 网格)。
//!
//! **句柄/索引模式**:`#[pyclass]` 持有 `Arc` 共享的已解析数据,绝不持有 Rust 借用。重活
//! (解析 / OCR)在 [`Python::detach`] 下释放 GIL 运行。错误折成以 `_core.DocError` 为根的
//! 类型化异常层级。

use std::path::PathBuf;
use std::sync::Arc;

use doc_core::geom::{emu_to_points, twips_to_points};
use doc_core::model::{
    Block, Cell, Color, Document as CoreDocument, Paragraph, Picture, Row, Table, TextRun, VMerge,
};
use doc_core::DocError;
use doc_parse::{parse_bytes, parse_path};
use pyo3::create_exception;
use pyo3::exceptions::{PyFileNotFoundError, PyOSError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};

/// 包版本(镜像 Rust workspace 版本)。
const VERSION: &str = env!("CARGO_PKG_VERSION");

// --- 异常层级 -------------------------------------------------------------

create_exception!(_core, DocError_, pyo3::exceptions::PyException);
create_exception!(_core, DocZipError, DocError_);
create_exception!(_core, DocXmlError, DocError_);
create_exception!(_core, DocUnsupportedError, DocError_);
create_exception!(_core, DocOcrError, DocError_);

/// 把 [`DocError`] 折成对应的 Python 异常(按 `kind()` 稳定标签分派)。
fn map_err(e: DocError) -> PyErr {
    let msg = e.to_string();
    match e.kind() {
        "zip" => DocZipError::new_err(msg),
        "xml" => DocXmlError::new_err(msg),
        "unsupported" => DocUnsupportedError::new_err(msg),
        "ocr" => DocOcrError::new_err(msg),
        "invalid-argument" => PyValueError::new_err(msg),
        "io" => {
            if let DocError::Io(io) = &e {
                if io.kind() == std::io::ErrorKind::NotFound {
                    return PyFileNotFoundError::new_err(msg);
                }
            }
            PyOSError::new_err(msg)
        }
        _ => DocError_::new_err(msg),
    }
}

// --- 颜色小工具 -----------------------------------------------------------

/// 把一个 [`Color`] 转成 `"RRGGBB"` 十六进制串。
fn color_hex(c: &Color) -> String {
    format!("{:02X}{:02X}{:02X}", c.rgb[0], c.rgb[1], c.rgb[2])
}

// --- dict 构造:把领域模型映射成可自省的 list[dict] ----------------------

/// 一个 [`TextRun`] -> dict。
fn run_dict<'py>(py: Python<'py>, run: &TextRun) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("text", &run.text)?;
    d.set_item("font", run.font.as_deref())?;
    d.set_item("size_pt", run.size_pt)?;
    d.set_item("bold", run.bold)?;
    d.set_item("italic", run.italic)?;
    d.set_item("underline", run.underline)?;
    d.set_item("color", run.color.as_ref().map(color_hex))?;
    // 内嵌图片(若有)。
    let pics = PyList::empty(py);
    for p in &run.pictures {
        pics.append(picture_dict(py, p)?)?;
    }
    d.set_item("pictures", pics)?;
    Ok(d)
}

/// 一张 [`Picture`] -> dict。
fn picture_dict<'py>(py: Python<'py>, pic: &Picture) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    d.set_item("rel_id", &pic.rel_id)?;
    d.set_item("media", pic.media_name.as_deref())?;
    match pic.extent {
        Some((cx, cy)) => {
            d.set_item("extent", (cx, cy))?;
            d.set_item("extent_points", (emu_to_points(cx), emu_to_points(cy)))?;
        }
        None => {
            d.set_item("extent", py.None())?;
            d.set_item("extent_points", py.None())?;
        }
    }
    d.set_item("image_bytes_len", pic.image_bytes_len)?;
    Ok(d)
}

/// 一个 [`Paragraph`] -> dict。
fn paragraph_dict<'py>(py: Python<'py>, para: &Paragraph) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    let runs = PyList::empty(py);
    for r in &para.runs {
        runs.append(run_dict(py, r)?)?;
    }
    d.set_item("kind", "paragraph")?;
    d.set_item("runs", runs)?;
    d.set_item("text", para.text())?;
    d.set_item("style", para.style.as_deref())?;
    d.set_item("align", para.align.as_deref())?;
    d.set_item("list_level", para.list_level)?;
    Ok(d)
}

/// 一个 [`Cell`] -> dict(`blocks` 递归,所以嵌套表天然展开)。
fn cell_dict<'py>(py: Python<'py>, cell: &Cell) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    let blocks = PyList::empty(py);
    for b in &cell.blocks {
        blocks.append(block_dict(py, b)?)?;
    }
    d.set_item("blocks", blocks)?;
    // 便利:单元格直接段落文字(忽略嵌套表)。
    d.set_item("text", cell.text())?;
    d.set_item("grid_span", cell.grid_span)?;
    d.set_item("v_merge", vmerge_str(cell.v_merge))?;
    d.set_item("merged", cell.is_vmerge_continuation())?;
    d.set_item("fill", cell.fill.as_ref().map(color_hex))?;
    d.set_item("width", cell.width)?;
    d.set_item("width_points", cell.width.map(twips_to_points))?;
    Ok(d)
}

/// [`VMerge`] -> 稳定字符串标签。
fn vmerge_str(v: VMerge) -> &'static str {
    match v {
        VMerge::None => "none",
        VMerge::Restart => "restart",
        VMerge::Continue => "continue",
    }
}

/// 一个 [`Row`] -> dict(`cells` + 便利的 `text` 列表)。
fn row_dict<'py>(py: Python<'py>, row: &Row) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    let cells = PyList::empty(py);
    let texts = PyList::empty(py);
    for c in &row.cells {
        let cd = cell_dict(py, c)?;
        texts.append(cd.get_item("text")?)?;
        cells.append(cd)?;
    }
    d.set_item("cells", cells)?;
    d.set_item("text", texts)?;
    d.set_item("height", row.height)?;
    d.set_item("is_header", row.is_header)?;
    Ok(d)
}

/// 一张 [`Table`] -> dict。
fn table_dict<'py>(py: Python<'py>, table: &Table) -> PyResult<Bound<'py, PyDict>> {
    let d = PyDict::new(py);
    let rows = PyList::empty(py);
    for r in &table.rows {
        rows.append(row_dict(py, r)?)?;
    }
    d.set_item("kind", "table")?;
    d.set_item("rows", rows)?;
    d.set_item("row_count", table.rows.len())?;
    d.set_item("col_count", table.col_count())?;
    d.set_item("grid_cols", table.grid_cols.clone())?;
    d.set_item("style", table.style.as_deref())?;
    Ok(d)
}

/// 一个 [`Block`] -> dict。
fn block_dict<'py>(py: Python<'py>, block: &Block) -> PyResult<Bound<'py, PyDict>> {
    match block {
        Block::Paragraph(p) => paragraph_dict(py, p),
        Block::Table(t) => table_dict(py, t),
    }
}

// --- pyclass 句柄 ---------------------------------------------------------

/// 一份已解析的 Word 文档句柄(`Arc` 共享底层数据)。
#[pyclass(name = "Document", module = "docspine._core", frozen)]
struct PyDocument {
    inner: Arc<CoreDocument>,
}

#[pymethods]
impl PyDocument {
    /// 正文块数量(顶层段落 + 表格)。
    #[getter]
    fn block_count(&self) -> usize {
        self.inner.body.len()
    }

    /// 顶层正文块,作为 `list[dict]`(段落 / 表格)。
    fn body<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for b in &self.inner.body {
            list.append(block_dict(py, b)?)?;
        }
        Ok(list)
    }

    /// 便利:顶层段落,作为 `list[dict]`(过滤掉表格)。
    fn paragraphs<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for b in &self.inner.body {
            if let Block::Paragraph(p) = b {
                list.append(paragraph_dict(py, p)?)?;
            }
        }
        Ok(list)
    }

    /// 便利:顶层表格,作为 `list[dict]`(过滤掉段落)。
    fn tables<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let list = PyList::empty(py);
        for b in &self.inner.body {
            if let Block::Table(t) = b {
                list.append(table_dict(py, t)?)?;
            }
        }
        Ok(list)
    }

    /// 便利:把全文按段落顺序拼成纯文本(表格按行、单元格按 tab 连接)。
    fn text(&self) -> String {
        let mut out: Vec<String> = Vec::new();
        for b in &self.inner.body {
            match b {
                Block::Paragraph(p) => out.push(p.text()),
                Block::Table(t) => {
                    for row in &t.rows {
                        let cells: Vec<String> = row.cells.iter().map(|c| c.text()).collect();
                        out.push(cells.join("\t"));
                    }
                }
            }
        }
        out.join("\n")
    }

    fn __len__(&self) -> usize {
        self.inner.body.len()
    }

    fn __repr__(&self) -> String {
        format!("<docspine.Document block_count={}>", self.inner.body.len())
    }
}

// --- 模块级函数:解析 -----------------------------------------------------

/// 从磁盘路径解析一个 `.docx`。解析在释放 GIL 下进行。
#[pyfunction]
fn open(py: Python<'_>, path: PathBuf) -> PyResult<PyDocument> {
    let parsed = py.detach(|| parse_path(&path)).map_err(map_err)?;
    Ok(PyDocument {
        inner: Arc::new(parsed.document),
    })
}

/// 从内存字节解析一个 `.docx`。解析在释放 GIL 下进行。
#[pyfunction]
fn open_bytes(py: Python<'_>, data: &[u8]) -> PyResult<PyDocument> {
    let owned = data.to_vec();
    let parsed = py.detach(|| parse_bytes(&owned)).map_err(map_err)?;
    Ok(PyDocument {
        inner: Arc::new(parsed.document),
    })
}

/// 探测一段字节是否为旧二进制 `.doc`(OLE/CFB),返回 `{is_cfb, has_word_stream, streams}`。
/// 需要 `legacy-doc` 特性才能列出流;否则 CFB 字节抛 `DocUnsupportedError`。
#[pyfunction]
fn probe_doc<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyDict>> {
    let owned = data.to_vec();
    let probe = py
        .detach(|| doc_parse::legacy::probe_doc(&owned))
        .map_err(map_err)?;
    let d = PyDict::new(py);
    d.set_item("is_cfb", probe.is_cfb)?;
    d.set_item("has_word_stream", probe.has_word_stream)?;
    d.set_item("streams", probe.streams)?;
    Ok(d)
}

// --- 模块级函数:OCR(仅 `ocr` 特性) -------------------------------------

#[cfg(feature = "ocr")]
mod ocr_api {
    use super::*;
    use doc_ocr::{ocr_image_bytes, reconstruct_table_from_image, ImageTableOptions, OcrItem};

    /// 把一个 [`OcrItem`] 折成 dict。
    fn ocr_item_dict<'py>(py: Python<'py>, it: &OcrItem) -> PyResult<Bound<'py, PyDict>> {
        let d = PyDict::new(py);
        d.set_item("text", &it.text)?;
        d.set_item("bbox", (it.x0, it.y0, it.x1, it.y1))?;
        d.set_item("confidence", it.confidence)?;
        Ok(d)
    }

    /// 对一张图片的编码字节(PNG / JPEG / TIFF / BMP)做本地 OCR,返回 `list[dict]`,每项含
    /// `text` / `bbox` / `confidence`。推理在释放 GIL 下进行(本地、离线、确定性)。
    #[pyfunction]
    pub fn ocr_image<'py>(py: Python<'py>, data: &[u8]) -> PyResult<Bound<'py, PyList>> {
        let owned = data.to_vec();
        let items = py.detach(|| ocr_image_bytes(&owned)).map_err(map_err)?;
        let list = PyList::empty(py);
        for it in &items {
            list.append(ocr_item_dict(py, it)?)?;
        }
        Ok(list)
    }

    /// 把一张**图片里的表格**(扫描件/截图)从 OCR 文字框重建成网格,返回 `list[dict]`,每张
    /// 表含 `row_count` / `col_count` / `cols` / `rows` / `cells`(每格 `row`/`col`/`row_span`/
    /// `col_span`/`bbox`/`text`/`confidence`)。OCR 在释放 GIL 下进行。
    #[pyfunction]
    pub fn reconstruct_image_table<'py>(
        py: Python<'py>,
        data: &[u8],
    ) -> PyResult<Bound<'py, PyList>> {
        let owned = data.to_vec();
        let opts = ImageTableOptions::default();
        let result = py
            .detach(|| reconstruct_table_from_image(&owned, &opts))
            .map_err(map_err)?;
        let list = PyList::empty(py);
        for t in &result.tables {
            let td = PyDict::new(py);
            td.set_item("bbox", t.bbox)?;
            td.set_item("row_count", t.row_count)?;
            td.set_item("col_count", t.col_count)?;
            td.set_item("cols", t.cols.clone())?;
            td.set_item("rows", t.rows.clone())?;
            let cells = PyList::empty(py);
            for c in &t.cells {
                let cd = PyDict::new(py);
                cd.set_item("row", c.row)?;
                cd.set_item("col", c.col)?;
                cd.set_item("row_span", c.row_span)?;
                cd.set_item("col_span", c.col_span)?;
                cd.set_item("bbox", c.bbox)?;
                cd.set_item("text", &c.text)?;
                cd.set_item("confidence", c.confidence)?;
                cells.append(cd)?;
            }
            td.set_item("cells", cells)?;
            list.append(td)?;
        }
        Ok(list)
    }
}

/// 包版本。
#[pyfunction]
fn version() -> &'static str {
    VERSION
}

// --- 模块注册 -------------------------------------------------------------

#[pymodule]
fn _core(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("__version__", VERSION)?;
    m.add_function(wrap_pyfunction!(version, m)?)?;
    m.add_function(wrap_pyfunction!(open, m)?)?;
    m.add_function(wrap_pyfunction!(open_bytes, m)?)?;
    m.add_function(wrap_pyfunction!(probe_doc, m)?)?;

    // OCR 入口仅在 `ocr` 特性开启时注册。
    #[cfg(feature = "ocr")]
    {
        m.add_function(wrap_pyfunction!(ocr_api::ocr_image, m)?)?;
        m.add_function(wrap_pyfunction!(ocr_api::reconstruct_image_table, m)?)?;
    }

    m.add_class::<PyDocument>()?;

    // 异常层级(根 `DocError`)。`DocError_` 的 Rust 标识符带下划线避免与
    // `doc_core::DocError` 撞名,但暴露给 Python 的名字是 `DocError`。
    m.add("DocError", py.get_type::<DocError_>())?;
    m.add("DocZipError", py.get_type::<DocZipError>())?;
    m.add("DocXmlError", py.get_type::<DocXmlError>())?;
    m.add("DocUnsupportedError", py.get_type::<DocUnsupportedError>())?;
    m.add("DocOcrError", py.get_type::<DocOcrError>())?;

    Ok(())
}
