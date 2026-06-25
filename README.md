# docspine

A pure-Rust Word (`.docx`) parser with Python bindings (PyO3 / maturin,
abi3-py311). A `.docx` file is OOXML — a zip archive of XML parts — and docspine
walks `word/document.xml` directly to produce a structured, information-preserving
model: paragraphs (styled runs), **tables** (rows, cells, merges, fills, nesting),
and embedded pictures. Tables are a first-class focus. Embedded images can
additionally be OCR'd locally, offline, and deterministically via the sibling
[`ocrspine`](https://github.com/VoldemortGin/ocrspine) crate (PP-OCRv5 through
`tract-onnx` — no cloud, no network), and an image that *is* a table can be
reconstructed into a grid from its OCR word boxes.

docspine is the document-engine sibling of [`pdfspine`](https://github.com/VoldemortGin/pdfspine)
(PDF) and [`pptspine`](https://github.com/VoldemortGin/pptspine) (PowerPoint),
all sharing the same `ocrspine` OCR core.

## Capabilities

| Area | Status |
| --- | --- |
| Body blocks: paragraphs + tables in document order | parsed |
| Paragraphs: runs, text, style name, alignment, list level | parsed |
| Run styling: font, size, bold, italic, underline, color | parsed |
| **Tables: rows, cells, cell paragraphs** | parsed |
| **Table merges: `gridSpan` (horizontal)** | parsed |
| **Table merges: `vMerge` restart / continue (vertical)** | parsed |
| **Nested tables (a table inside a cell)** | parsed |
| Cell shading/fill, cell width (dxa), table grid columns | parsed |
| Row height, header rows | parsed |
| Embedded pictures: `r:embed` rel → media name + raw bytes + EMU extent | parsed |
| Image OCR (embedded pictures → words + boxes) | working (`ocr_image`) |
| Image-table reconstruction from OCR boxes → grid | working (`reconstruct_image_table`) |
| Legacy binary `.doc` (OLE/CFB) | probe + typed downgrade (full body deferred) |

Parsing is tolerant: unknown elements are skipped, missing attributes become
`None`, and malformed input yields a typed `DocError` rather than a panic.

### docx first; legacy `.doc` deferred

Modern `.docx` (OOXML) is the **primary target**. The old binary `.doc` is a
Microsoft compound document (OLE/CFB): rebuilding its body from the binary FIB +
piece table is large, fiddly, and shares almost nothing with the docx path. So
docspine ships **detection + a clean typed downgrade** today (a `.doc` byte
stream yields `DocUnsupportedError`, and `probe_doc` reports the CFB streams when
built with the `legacy-doc` feature); full `.doc` body reconstruction is a
follow-up, not a blocker.

## Build (from the package root)

```bash
uv venv .venv
VIRTUAL_ENV="$(pwd)/.venv" uv pip install maturin pytest
# Structural parsing needs no models. The OCR path resolves models from a
# sibling ../ocrspine/models by default (or set OCRSPINE_MODELS).
OCRSPINE_MODELS="$(cd ../ocrspine && pwd)/models" \
  VIRTUAL_ENV="$(pwd)/.venv" .venv/bin/maturin develop --release
```

## Use from Python

```python
import docspine

doc = docspine.open("report.docx")
print(doc.block_count)

for block in doc.body():            # list[dict], introspectable
    if block["kind"] == "paragraph":
        for run in block["runs"]:
            print(run["text"], run["bold"], run["color"])
    elif block["kind"] == "table":
        for row in block["rows"]:
            for cell in row["cells"]:
                print(cell["text"], "span", cell["grid_span"], cell["v_merge"])

# Run OCR on raw image bytes (PNG/JPEG), offline:
items = docspine.ocr_image(open("scan.png", "rb").read())
print(" ".join(i["text"] for i in items))

# Reconstruct a table that lives inside an image into a grid:
for table in docspine.reconstruct_image_table(open("table.png", "rb").read()):
    for cell in table["cells"]:
        print(cell["row"], cell["col"], cell["text"])
```

## Rust workspace

```
crates/
  doc-core    domain model + geometry (twip/EMU) + typed DocError. No IO/zip/XML.
  doc-parse   OOXML reader: zip extract + quick-xml walk -> Document.
              Legacy binary .doc probing behind the `legacy-doc` feature.
  doc-ocr     image-OCR bridge over ocrspine (PaddleOcr) + image-table reconstruction.
  py-bindings PyO3 _core extension (the FFI chokepoint); `ocr` feature gates OCR.
```

## Deferred / follow-up

- Full legacy binary `.doc` (OLE/CFB / `[MS-DOC]`) body reconstruction (FIB,
  piece table, CHPX/PAPX). Today: detection + typed downgrade + `probe_doc`.
- Bundling the PP-OCRv5 ONNX weights into the published wheel (a CI task).
- Richer styling (theme colors, styles.xml inheritance), headers/footers,
  footnotes/endnotes, comments, fields, hyperlinks targets, SmartArt/charts.
