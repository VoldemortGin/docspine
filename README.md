# docspine

[![PyPI](https://img.shields.io/pypi/v/docspine.svg)](https://pypi.org/project/docspine/)

A pure-Rust Word (`.docx`) parser with Python bindings (PyO3 / maturin,
abi3-py311). A `.docx` file is OOXML — a zip archive of XML parts — and docspine
walks `word/document.xml` directly to produce a structured, information-preserving
model: paragraphs (styled runs), **tables** (rows, cells, merges, fills, nesting),
and embedded pictures. Tables are a first-class focus. Embedded images can
additionally be OCR'd locally, offline, and deterministically via the sibling
[`ocrspine`](https://github.com/VoldemortGin/ocrspine) crate (PP-OCRv5 through
`tract-onnx` — no cloud, no network), and an image that *is* a table can be
reconstructed into a grid from its OCR word boxes. A parsed document can also
be **exported to PDF** (`to_pdf()` / `save_pdf()`) with flowed layout and
pagination through the shared pure-Rust `pdf-typeset` engine from
[`pdfspine`](https://github.com/VoldemortGin/pdfspine) — no LibreOffice, no
cloud converter.

docspine is the document-engine sibling of [`pdfspine`](https://github.com/VoldemortGin/pdfspine)
(PDF) and [`pptspine`](https://github.com/VoldemortGin/pptspine) (PowerPoint),
all sharing the same `ocrspine` OCR core.

## Spine 家族 / Spine family

本仓库是 Spine 家族的成员之一（角色：L2 文档引擎）。家族全部成员、分层、依赖方向、依赖形式与当前差距见 [`docs/spine-family.md`](docs/spine-family.md)；该文件在每个家族仓库中的副本内容相同，真源在家族根目录 `~/startup/spine/docs/spine-family.md`，用根目录 `make family-doc-sync` 同步。

## Capabilities

| Area | Status |
| --- | --- |
| Body blocks: paragraphs + tables in document order | parsed |
| Paragraphs: runs, text, style name, alignment, list level | parsed |
| Run styling: font, size, bold, italic, underline, color | parsed |
| **Tables: rows, cells, cell paragraphs** | parsed |
| **Table merges: `gridSpan` (horizontal)** (clamped to Word's 63-column limit) | parsed |
| **Table merges: `vMerge` restart / continue (vertical)** | parsed |
| **Nested tables (a table inside a cell)** | parsed |
| Cell shading/fill, cell width (dxa), table grid columns | parsed |
| Row height, header rows, `gridBefore` / `gridAfter` (leading / trailing skipped grid columns; orphan `vMerge continue` cells are kept as ordinary cells) | parsed |
| Embedded pictures: `r:embed` rel → media name + raw bytes + EMU extent + alt text (`wp:docPr@descr`, else `@title` → `pic["alt"]`) | parsed |
| Transparent wrappers: `w:sdt`, `w:customXml` (block + inline), `w:smartTag`, `w:hyperlink`, `w:fldSimple` (cached result), including row-level and cell-level `w:sdt` / `w:customXml` inside tables; complex fields keep the cached result, `w:instrText` never leaks | parsed |
| Ruby `w:ruby` (base text kept, reading `w:rt` not emitted), bidi containers `w:dir` / `w:bdo` | parsed |
| External content blocks `w:altChunk`: **not imported**; `doc.alt_chunk_count` counts them and `to_pdf` warns `alt-chunk-skipped` | detected |
| Document properties (`docProps/core.xml` → `doc.core_properties()`: title, creator, subject, keywords, description, category, last_modified_by, revision, created, modified, language; missing → `None`) | parsed |
| Parse diagnostics: `doc.diagnostics()` lists silent truncation / skipping / clamping (`xml-truncated`, `nesting-depth-exceeded`, `table-columns-clamped`, `grid-span-clamped`, `numbering-value-clamped`, `missing-part`, `alt-chunk-not-imported`) as `{kind, part, count}`, never document text | working |
| Math `m:oMath` / `m:oMathPara`: `m:t` text extracted in order (fractions / scripts / radicals as `1/2`, `x^2`, `x_i`, `sqrt(x)`; `run["is_math"]`; no layout; PDF draws plain text, `math-flattened` warning) | extracted |
| Headers / footers (`word/header*.xml` / `footer*.xml`; `default` / `first` / `even`, tables included): `sections()[i]["headers"|"footers"]` = `[{type, rel_id, blocks}]` (plus `title_pg`, `page_number_start`, `page_number_format` per section, `Document.even_and_odd_headers`, and `run["field"]` = field instruction or `None`); `to_text` / `to_markdown` / `to_html` emit each distinct part once (headers at the top as `[Header: default]` / `<header data-type="default">`, footers at the end); **drawn on every page** in the PDF (`w:titlePg` first page, `w:evenAndOddHeaders` even pages, missing types inherited from earlier sections; body pushed clear of tall headers/footers, `header-footer-overflow` warning when it cannot be; `PAGE` / `NUMPAGES` computed — `PAGE` honours `w:pgNumType` start / format and `\* roman`-style switches, odd/even headers follow the displayed number, `NUMPAGES` stays decimal; other fields use the cached result) | extracted + rendered |
| Comments (`word/comments.xml`): `comments()` = `[{id, author, date, initials, blocks}]` (missing attributes `None`), anchors are `kind == "comment_ref"` run segments (reference point only, no range); review metadata, so **not** in `to_text` / `to_markdown` / `to_html` and not drawn in PDF | extracted |
| Footnotes / endnotes (`word/footnotes.xml` / `endnotes.xml`; separator notes skipped): `footnotes()` / `endnotes()` = `[{id, blocks}]`, references are `kind == "note_ref"` run segments; `to_text` marks `[1]` / `[e1]` + trailing list, `to_markdown` uses `[^1]` / `[^e1]` footnote syntax, `to_html` uses `<sup>` anchors with back-linked `<div class="note">` entries; **not drawn** in PDF (`notes-not-rendered` warning) | extracted |
| Revisions (accept-all semantics): `w:ins` / `w:moveTo` kept, `w:del` / `w:moveFrom` dropped; old properties in `w:*PrChange` / `w:tblGridChange` ignored | parsed |
| `mc:AlternateContent`: first `mc:Choice` that yields content, else `mc:Fallback` (never both) | parsed |
| Text exports (`to_text` / `to_markdown` / `to_html`): list labels from the numbering engine (incl. style-level numbering such as numbered headings; Markdown uses `- ` and `1.` list syntax, other labels become text prefixes), hyperlinks (`http` / `https` / `mailto` only; Markdown `[text](url)`, HTML `<a>`), pictures (`![alt](media)` / `<img>`; plain text `[图片: alt]` only when alt exists); headings come from the style sheet (`outlineLvl`, style name, styleId) | extracted |
| Floating text boxes (`wps:txbx` / VML `v:textbox` → `w:txbxContent`): extracted as `run["text_boxes"]`, emitted right after the anchoring paragraph in `to_text` / `to_markdown` / `to_html`; **not drawn** in PDF (`text-box-not-rendered` warning) | extracted |
| Special run content: `w:sym` (code point kept as-is, incl. `U+F0xx` symbol-font PUA), `w:softHyphen` → U+00AD (invisible in PDF), `w:noBreakHyphen` → U+2011, `w:ptab` → tab | parsed |
| Image OCR (embedded pictures → words + boxes) | working (`ocr_image`) |
| Image-table reconstruction from OCR boxes → grid | working (`reconstruct_image_table`) |
| PDF export: `to_pdf()` / `save_pdf()` — flowed layout + pagination; per-section page geometry (`sectPr`), styles.xml + theme effective styles, numbering engine, table fidelity (borders/merges/margins, cross-page; cell vAlign top/center/bottom), paragraph borders/shading (native engine paragraph borders, drawn on every page fragment; `w:between` not drawn), inline images + absolutely-positioned anchored images (no text wrap), hyperlinks as PDF link annotations, `defaultTabStop` tab advance, superscript/subscript with a real baseline shift + `w:position`, character spacing `w:spacing` (expanded and condensed) | working |
| Legacy binary `.doc` (OLE/CFB) | probe + typed downgrade (full body deferred) |

Parsing is tolerant: unknown elements are skipped, missing attributes become
`None`, and malformed input yields a typed `DocError` rather than a panic.

Hostile input is bounded. Reading the zip package enforces `doc_parse::ZipLimits`
(defaults: 10,000 entries, 256 MiB per entry, 1 GiB total decompressed, a
10,000:1 compression ratio for entries over 1 MiB — unreachable by deflate's
~1032:1 ceiling, so it only catches bombs using other methods — 1024-byte entry
names); declared sizes are never trusted for allocation, and absolute or `..`
entry paths are rejected. Hitting a zip limit raises `DocZipError` in Python
(the message names the limit, e.g. `limit exceeded: entry-bytes (...)`); Rust
callers get `DocError::LimitExceeded` and can pass custom limits via
`parse_bytes_with_limits` / `parse_path_with_limits`. Nested tables / content
controls / inline run containers are capped at 64 levels; deeper subtrees are
skipped silently (no stack overflow, no error).

### docx first; legacy `.doc` deferred

Modern `.docx` (OOXML) is the **primary target**. The old binary `.doc` is a
Microsoft compound document (OLE/CFB): rebuilding its body from the binary FIB +
piece table is large, fiddly, and shares almost nothing with the docx path. So
docspine ships **detection + a clean typed downgrade** today (a `.doc` byte
stream yields `DocUnsupportedError`, and `probe_doc` reports the CFB streams when
built with the `legacy-doc` feature); full `.doc` body reconstruction is a
follow-up, not a blocker.

## Install

```bash
pip install docspine
```

docspine is **on PyPI**. OCR works out of the box: the PP-OCRv5 weights ship in
the shared [`ocrspine-models`](https://pypi.org/project/ocrspine-models/) data
package — a runtime dependency `pip` pulls in automatically — so a plain
`pip install docspine` finds the OCR models with no extra setup (it no longer
needs a sibling `../ocrspine/models` checkout or `OCRSPINE_MODELS`). To build from
source instead, see below.

## Build (from the package root)

```bash
uv venv --python 3.12 .venv
uv pip install --python .venv/bin/python maturin pytest "pdfspine>=0.8,<0.13" ocrspine-models
VIRTUAL_ENV="$(pwd)/.venv" .venv/bin/maturin develop --release --locked --uv
```

Cargo fetches `pdf-typeset` and its test font dependency from the same official
pdfspine v0.11.2 commit, `78a64d6e252ab739fcbad66c0d7f5328a080d667`, and
`ocrspine` from commit `041958aa6f8d70d3957e8f9e27896cf0cbc42511`;
no sibling pdfspine checkout is needed. Dependency installation may use the
network; document parsing, PDF export and OCR run locally. The installed
`ocrspine-models` package supplies OCR weights. See the
[migration validation](docs/pdfspine-v0112-validation.md) for wheel/export checks
and their coverage limits.

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

## Export to PDF

```python
doc = docspine.open("report.docx")
doc.save_pdf("report.pdf")         # flowed layout + pagination
pdf_bytes = doc.to_pdf()           # or in-memory bytes

# Optional: map a requested font family to a local font file (or to another
# installed family), layered on top of the built-in substitution table:
doc.save_pdf("report.pdf", font_map={"Calibri": "/path/to/Carlito.ttf"})
```

Per-section page geometry from `sectPr` is honored (paper size, orientation,
margins per section). Rendering is deterministic and fully offline. Missing
fonts degrade gracefully: an available face is substituted and a Python
`UserWarning` is emitted **once per warning kind** — the export never fails on
a missing font.

## Rust workspace

```
crates/
  doc-core    domain model + geometry (twip/EMU) + typed DocError. No IO/zip/XML.
  doc-parse   OOXML reader: zip extract + quick-xml walk -> Document.
              Legacy binary .doc probing behind the `legacy-doc` feature.
  doc-ocr     image-OCR bridge over ocrspine (PaddleOcr) + image-table reconstruction.
  doc-render  docx -> PDF renderer over the shared pdf-typeset engine (from pdfspine).
  py-bindings PyO3 _core extension (the FFI chokepoint); `ocr` feature gates OCR.
```

## Fuzzing

`fuzz/` is a standalone cargo-fuzz package (excluded from the workspace; needs nightly and
`cargo install cargo-fuzz`). Only panics / aborts / OOM count as failures — any `Err` is fine.
A daily CI job (`.github/workflows/fuzz.yml`) runs every target; it is not part of `ci.yml`.

```bash
cargo run --manifest-path fuzz/Cargo.toml --bin make_seeds   # synthesize seeds into fuzz/corpus/ (git-ignored)
cargo +nightly fuzz run parse_document_xml -- -max_total_time=120 -rss_limit_mb=2048
cargo +nightly fuzz run parse_docx         -- -max_total_time=120 -rss_limit_mb=2048
cargo +nightly fuzz run parse_parts        -- -max_total_time=120 -rss_limit_mb=2048
cargo +nightly fuzz run render_pdf         -- -max_total_time=120 -rss_limit_mb=2048
```

Reproduce a crash with `cargo +nightly fuzz run <target> fuzz/artifacts/<target>/<crash-file>` and
shrink it with `cargo +nightly fuzz tmin <target> <crash-file>`. To add a target: create
`fuzz/fuzz_targets/<name>.rs`, register a `[[bin]]` in `fuzz/Cargo.toml`, add it to the matrix in
`fuzz.yml`, and add seeds in `fuzz/seed.rs`. Fixed crashes get a regression test built in code
(see `crates/doc-parse/tests/fuzz_regressions.rs`), never a committed binary.

## Deferred / follow-up

- Full legacy binary `.doc` (OLE/CFB / `[MS-DOC]`) body reconstruction (FIB,
  piece table, CHPX/PAPX). Today: detection + typed downgrade + `probe_doc`.
- Richer styling, comment ranges / threaded replies, fields (beyond `PAGE` / `NUMPAGES` in headers/footers), drawing footnotes in the PDF,
  hyperlinks targets, SmartArt/charts.
