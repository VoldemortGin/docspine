# Changelog

All notable changes to **docspine** are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

docspine is an Apache-2.0-licensed, pure-Rust Word (`.docx`) reader with PyO3
Python bindings and a fidelity-preserving **PDF export** built on the shared
`pdf-typeset` engine from [pdfspine](https://pypi.org/project/pdfspine/). It is
**alpha / pre-1.0**: the core is feature-complete, but the public API may still
change.

## [Unreleased]

### Changed

- **PDF export dependencies aligned to pdfspine v0.8.0** (`249a3f7`): `pdf-typeset`
  and dev `pdf-fonts` now share one workspace git rev; README and a migration
  validation report updated.
- **Git dependencies aligned**: `ocrspine` to `041958a` (fixes a reading-order sort
  panic) and `pdf-typeset`/`pdf-fonts` to pdfspine v0.11.2 (`78a64d6`).
  Behavior-preserving; no new typesetting capability adopted. All 8 SSIM
  self-reference baselines are unchanged (1.0000).
- **PDF export draws paragraph borders (`w:pBdr`) and shading (`w:shd`) with
  pdf-typeset's native paragraph borders** (pdfspine v0.11.2) instead of a
  one-cell table wrapper. Bordered or shaded paragraphs that split across pages,
  including in-paragraph page breaks, are now drawn on every page. `dashed` and
  `dashSmallGap` render as dashes; other non-solid styles still draw solid.
  Matching adjacent paragraphs share one box. `w:between` is still not drawn and
  emits one `para-border-omitted` warning. The `para-shading-omitted` warning was
  removed because it no longer fires. The `para_box` SSIM baseline
  (`conformance/ssimref/para_box.pdf`) was regenerated on 2026-10-02 for the native
  geometry (self-reference was 0.9739 against the old baseline, now 1.0000).
- **PDF export: superscript/subscript render with a real baseline shift** via
  pdf-typeset `ResolvedScriptPlacement` (glyph ×0.65, +0.33em / −0.11em; nominal
  size kept as line strut) instead of size-only shrinking.
- **Test extra allows pdfspine 0.12**: `pdfspine>=0.8,<0.13` (was `<0.12`) in
  `pyproject.toml`, README and CLAUDE.md, after the full pytest suite (81 passed,
  SSIM gate included) ran green against pdfspine 0.12.0. Cargo git revs unchanged.

### Added

- **PDF export draws headers and footers on every page.** The effective part per
  page comes from the new pure `Document::header_footer_for_page(section,
  page_in_section, page_number)`: `w:titlePg` selects `first` on a section's first
  page, `w:evenAndOddHeaders` (`word/settings.xml`) selects `even` on even page
  numbers, otherwise `default`; a type a section lacks is inherited from the
  nearest earlier section, and none means no header/footer. Parts go through the
  body's block mapping and the engine's `layout_text_box` (paragraphs, tables,
  inline and anchored images; an image is embedded once however many pages repeat
  it), placed at `w:pgMar@w:header` from the top / `@w:footer` from the bottom.
  Part heights are measured before the body is laid out, and each page's body is
  pushed below its header / above its footer like Word (negative top / bottom
  margins keep the body fixed); when the push would leave less than
  min(72pt, original body height) the page keeps its margins and one
  `header-footer-overflow` warning fires. `PAGE` and `NUMPAGES` are computed in
  headers/footers (one body pass, then headers/footers per page with the known
  total; part heights use the cached field text). Other fields, and all fields in
  the body, keep their cached result. Parts holding only empty paragraphs are
  treated as absent. SSIM baselines are unchanged (no fixture has headers).
- **Fields are marked in the model**: `TextRun.field` holds the field instruction
  (`w:fldSimple@w:instr`, or the joined `w:instrText` of a `w:fldChar`
  begin/separate/end field, nesting-aware and spanning paragraphs) on runs that
  are part of a visible field result; a field without a cached result leaves one
  empty run carrying only the mark. Text and exports are unchanged.
  `Section.title_pg` (`w:titlePg`) and `Document.even_and_odd_headers` are parsed.
- **Page number start and format (`w:pgNumType`).** `Section.page_number_start`
  (`w:start`; negative, non-numeric or out-of-range values count as missing, `0` is
  valid) and `Section.page_number_format` (`PageNumFormat`: `decimal`,
  `lowerRoman`, `upperRoman`, `lowerLetter`, `upperLetter`; any other `w:fmt` value
  becomes `Other`, printed as decimal with one `page-number-format-unsupported`
  warning per document) are parsed. A section with `w:start` begins its first page
  at that number, otherwise numbering continues from the previous section; `PAGE`
  in headers/footers prints the displayed number in the section's format, and a
  field switch (`\* roman` / `ROMAN` / `alphabetic` / `ALPHABETIC` / `Arabic`)
  overrides the section format (unknown switches are ignored). Roman numerals cover
  1..=3999 and letters n >= 1 (27 is `aa`, 53 is `aaa`); anything outside degrades
  to decimal. `NUMPAGES` stays the physical page count in decimal. Odd/even header
  selection follows the **displayed** page number, as Word does, so `w:start`
  shifts which pages are even. New pure `format_page_number` and
  `Section::first_page_number`. `oddPage` / `evenPage` section breaks still insert
  no blank page.
- **Python exposes the header/footer rule and page numbering model.**
  `run["field"]` (field instruction or `None`), `sections()[i]["title_pg"]`,
  `sections()[i]["page_number_start"]` (`int | None`),
  `sections()[i]["page_number_format"]` (`"decimal"`, `"lowerRoman"`, `"upperRoman"`,
  `"lowerLetter"`, `"upperLetter"` or `"other"`) and `Document.even_and_odd_headers`.

- **Parse + cascade rPr `w:spacing`** (character spacing, twips, signed) **and
  `w:position`** (manual baseline shift, half-points); mapped to engine
  `CharacterSpacing` (condensed via `resolved_signed`) and an additive baseline
  shift. Over-condensed paragraphs fall back to zero spacing with a
  `signed-spacing-fallback` warning. Default documents render byte-identical PDFs.

- **cargo-fuzz harness under `fuzz/`** (`parse_docx`, `parse_document_xml`,
  `render_pdf`; seeds generated by `make_seeds`, corpus/artifacts git-ignored) and
  a daily `fuzz.yml` workflow (300 s per target, artifacts uploaded on crash).
  `fuzz/` is excluded from the workspace so the main gates are unaffected.

- **Row-level and cell-level `w:sdt` / `w:customXml` are expanded transparently**
  (`w:tbl > w:sdt > w:sdtContent > w:tr`, `w:tr > w:sdt > w:sdtContent > w:tc`):
  the wrapped rows/cells enter the table model as if unwrapped, so
  `gridSpan` / `vMerge` and column indices stay correct. Shares the existing
  `MAX_NEST_DEPTH` cap.
- **Math text is no longer lost**: the `m:t` text of `m:oMath` / `m:oMathPara`
  is extracted in document order as a plain-text run (`TextRun.is_math`) and flows
  into the paragraph text, `to_text` and `to_markdown`. No math layout or LaTeX;
  PDF export draws it as plain text and emits one `math-flattened` warning.
- **Math fractions, scripts and radicals no longer glue digits together**: `m:f`
  extracts as `num/den` (`1/2`, not `12`), `m:sSup` as `x^2`, `m:sSub` as `x_i`,
  `m:rad` as `sqrt(x)` (`root(3,x)` when a degree is present). An operand made of
  more than one text fragment is parenthesised, e.g. `(a+b)/c`. Other math
  structures keep plain concatenation; still no LaTeX. Iterative walk, structure
  depth capped by `MAX_NEST_DEPTH`.
- **Comments (`word/comments.xml`) enter the model and the Python API.**
  `Document.comments: BTreeMap<i64, Comment>` keeps `w:id`, `w:author`, `w:date`,
  `w:initials` (missing -> `None`) and the content blocks (same block parser and
  depth guard; duplicate / missing / non-numeric ids skipped). The part is located
  by its fixed name like footnotes. Body anchors are `RunSegment::CommentRef { id }`
  (reference point only; `commentRangeStart` / `commentRangeEnd` are not modelled).
  Comments are review metadata: they are **not** emitted by `to_text` /
  `to_markdown` / `to_html` (the export functions have no options struct, so no
  switch was added), not drawn in PDF, and never appear in warnings. Python:
  `Document.comments()` and `kind == "comment_ref"` run segments.
- **Images inside headers, footers and notes are pinned by tests to resolve
  through their own part's rels.** Each part (`word/_rels/header1.xml.rels`, ...)
  can reuse `rId1` for a different media file than `document.xml.rels`; the parser
  already resolves `r:embed` with the owning part's rels into a scope-independent
  `media_name` (no parse bug was found). The only trap was `Document.image_bytes(rel_id)`
  in Python, which indexes body pictures only: for pictures from headers / footers /
  notes use the picture dict's `media`. Documented on `image_bytes` and covered by
  `part_scoped_images.rs` and a pytest (header and body both `rId1`, different bytes).
- **`to_html` now emits headers, footers, footnotes and endnotes** (it silently
  dropped them before). Each distinct header/footer part is emitted once as
  `<header data-type="default|first|even">` (top) / `<footer ...>` (end), empty
  parts skipped. Note references are `<sup id="fnref-1"><a href="#fn-1">[1]</a></sup>`
  (endnotes `e1`), numbered by first reference with the same logic as
  `to_markdown`; repeated references carry the `id` only once; dangling
  references leave no marker. Referenced notes are listed at the end as
  `<div class="note" id="fn-1">` with a back-link to `#fnref-1`; unreferenced
  notes are not emitted.
- **Headers, footers, footnotes and endnotes enter the model and text exports.**
  `word/header*.xml` / `footer*.xml` are located through the `w:headerReference` /
  `w:footerReference` entries of each `w:sectPr` (types `default` / `first` /
  `even`) and the main-document rels, then parsed with the existing block parser
  (paragraphs and tables). `word/footnotes.xml` / `endnotes.xml` are keyed by
  `w:id` (separator / continuationSeparator / continuationNotice notes skipped);
  body references stay in the run sequence as `RunSegment::NoteRef`. `to_text`
  and `to_markdown` emit each distinct header/footer part once (headers first,
  footers last, labelled `[Header: default]` / `**Header (default)**`) and number
  notes by first reference (`[1]` / `[e1]` plus a trailing list; Markdown
  `[^1]` / `[^e1]` with `[^1]: ...` definitions). References to missing ids,
  rels to missing parts and malformed parts degrade without panicking. PDF export still does not draw them and emits one
  `header-footer-not-rendered` (removed again later in this cycle, see Breaking) and one `notes-not-rendered` warning; SSIM
  baselines are unchanged. Python: `sections()[i]["headers"|"footers"]`,
  `Document.footnotes()` / `endnotes()`, `kind == "note_ref"` run segments, and
  the previously missing `run["is_math"]`.

### Breaking (Rust API, pre-1.0)

- `Document::header_footer_for_page`'s `page_number` is now the **displayed** page
  number as `i64` (was the physical page index as `usize`); `Section` gains
  `page_number_start` and `page_number_format`; `RenderWarning` gains
  `PageNumFormatUnsupported`. Struct literals without `..Default::default()` and
  exhaustive matches on `RenderWarning` must be updated.
- `Document` gains `header_footers`, `footnotes`, `endnotes`, `comments`; `Section` gains
  `headers`, `footers`; `RunSegment` gains the `NoteRef { kind, id }` and `CommentRef { id }` variants;
  `RenderWarning` gains `HeaderFooterNotRendered` / `NotesNotRendered`. Struct
  literals without `..Default::default()` and exhaustive matches on `RunSegment` /
  `RenderWarning` must be updated.
- Headers/footers are now drawn, so `RenderWarning::HeaderFooterNotRendered`
  (`header-footer-not-rendered`) is removed; `RenderWarning::HeaderFooterOverflow`
  (`header-footer-overflow`) is added. `Document` gains `even_and_odd_headers`,
  `Section` gains `title_pg`, `TextRun` gains `field`: struct literals without
  `..Default::default()` and exhaustive matches on `RenderWarning` must be updated.
  Python run lists can now contain an empty run marking a field that has no
  cached result.

### Fixed

- **Complex-field state no longer leaks out of a rejected `mc:Choice`.** The
  `mc:AlternateContent` try-parse runs each `mc:Choice` until one yields content;
  a Choice that yielded nothing but had already advanced the `w:fldChar` stack
  (an unpaired `begin` / `separate`) left its frame behind, so the text after the
  `mc:AlternateContent` was marked as field result (`TextRun.field`) or the stack
  stayed stuck in the instruction region and later fields were never marked. The
  field stack is now snapshotted before each Choice and restored when that Choice
  is rejected, so only the selected branch advances it.
- **Parsing no longer silently drops content**: `w:moveTo` (kept; `w:moveFrom`
  dropped), `w:smartTag`, `w:customXml` (block + inline), `mc:AlternateContent`
  (first `mc:Choice` that yields content, else `mc:Fallback`), and floating text
  boxes (`wps:txbx` / VML `v:textbox`; extracted into `run["text_boxes"]` and
  emitted after the anchoring paragraph in `to_text` / `to_markdown` / `to_html`,
  not drawn in PDF — new `text-box-not-rendered` warning). `w:sym`,
  `w:softHyphen`, `w:noBreakHyphen` and `w:ptab` now produce characters. A VML
  `w:pict` containing a nested shape no longer truncates the rest of the document
  (previously a résumé with a nested VML image extracted 9 tokens instead of 1446).
- **`doc-ocr` panic isolation at the engine boundary** (engine call, image decode,
  engine construction): panics now surface as `DocOcrError` instead of escaping to
  Python as `PanicException`. Image-table comparators use `f64::total_cmp`, and
  words with NaN/Inf bbox or confidence are dropped.
- **CI Python matrix** never got past `maturin develop` ("Couldn't find a
  virtualenv"), so pytest and the `.ssimref` SSIM gate had not run in CI before.
  It now builds via `pip install -e ".[test]"` with `pytest -ra`; the `test` extra
  declares `pdfspine>=0.8,<0.12`. The eastAsia font-slot read-back test no longer
  hardcodes macOS Hiragino: it picks a preinstalled non-fallback CJK font per OS
  (Hiragino Sans GB / SimSun / Noto Sans CJK SC) and skips when none is installed.

- **`doc-core::Color::from_hex` panicked on 6-byte non-ASCII input** (e.g.
  `w:shd@w:fill="DD…"` split inside a multi-byte char), found by fuzzing; it now
  rejects non-ASCII input.

### Security

- `.docx` zip reads are now bounded by `doc_parse::ZipLimits` (defaults: 10,000 entries, 256 MiB per entry, 1 GiB total decompressed, 10,000:1 compression ratio for entries > 1 MiB — unreachable by deflate's ~1032:1 ceiling, so it only catches bombs using other compression methods — and 1024-byte names). Declared sizes are no longer trusted for allocation (forged headers are caught while streaming), and absolute / drive / `..` entry paths are rejected. Violations raise `DocError::LimitExceeded` (Python: `DocZipError`, message names the limit). New Rust entry points `parse_bytes_with_limits` / `parse_path_with_limits`. Nested tables / content controls / inline run containers deeper than 64 levels are now skipped instead of overflowing the stack.

## [0.5.1] — 2026-07-30

### Changed

- **Python floor relaxed to 3.12.** `requires-python` is now `>=3.12` (no upper
  bound), undoing 0.5.0's `>=3.14,<3.15` restriction; classifiers cover
  3.12/3.13/3.14 and CI tests all three. The abi3 wheels are unchanged.

## [0.5.0] — 2026-07-30

### Changed

- **BREAKING: Python 3.14 only.** `requires-python` is now `>=3.14,<3.15`
  (was `>=3.11`); classifiers and CI/release workflows target CPython 3.14.
  The wheel remains abi3, but the published metadata no longer allows
  installation on 3.11–3.13.

## [0.4.0] — 2026-07-13

### Added

- **Cell vertical alignment renders (C-7 wrap-up).** `w:vAlign` `center` / `bottom`
  now map to the engine's `TableCell.v_align` (`VAnchor`, pdf-typeset TS-11), which
  offsets cell content after the row height settles; the previous top-only
  `CellVAlignIgnored` degradation is removed.
- **Hyperlinks become PDF link annotations (§3j).** `w:hyperlink@r:id` resolves
  through `word/_rels` into `TextRun.link_target` and renders as a `/Link` URI
  annotation via the engine's `RunStyle.link` (TS-11; readable with pdfspine
  `get_links`). Document-internal `w:hyperlink@w:anchor` bookmarks are stored as
  `"#name"` but not drawn as links — one-time `InternalLinkNotRendered` warning.
- **Anchored images are absolutely positioned (C-8 wrap-up).** A floating
  (`wp:anchor`) raster picture no longer degrades to an inline block: it is
  drawn as a page overlay at its `wp:positionH/V` `posOffset` (relative to the
  page or the section margins), on the section's first page. Text does **not**
  wrap around it — that emits a `FloatingNoWrap` warning (per-line exclusion
  rectangles remain out of v1). Vector-format or byte-missing anchored pictures
  fall back to the existing inline-placeholder / skip paths.
- **Paragraph borders & shading are drawn (C-4 wrap-up).** A paragraph carrying
  `w:pBdr` (top/right/bottom/left) and/or `w:shd` is wrapped in a single-cell
  table so the engine paints the shading fill and the four border edges (visible
  in `get_drawings`); the border `@w:space` folds into cell padding. The
  `w:between` edge (between consecutive same-bordered paragraphs) and complex
  cases (an intra-paragraph page break) still degrade with a single
  `ParaBorderOmitted` / `ParaShadingOmitted` warning.
- **Committed `.ssimref` self-render CI gate (C-10 layer 4).**
  `scripts/ssim_selfref.py` re-renders the export fixture matrix deterministically
  (`DOCSPINE_DETERMINISTIC_FONTS=1`, bundled fonts only) and SSIM-compares it
  against committed baseline PDFs under `conformance/ssimref/`, both rasterised by
  the runner's `pdfspine` so the gate is raster-version-independent. It runs
  CI-blocking on one runner at `--min-ssim 0.97`; regenerate with `--update`.
  Unlike the LibreOffice oracle (advisory, never CI), this gate is a self-baseline
  and blocks on layout regressions.

### Changed

- **`FloatingImageInlined` warning renamed to `FloatingNoWrap`** (kind
  `floating-no-wrap`) to reflect that anchored images are now absolutely
  positioned rather than inlined; only the text-wrap degradation remains.
- **Bumped the `pdf-typeset` engine to rev `509a932`** (pdfspine TS-11: cell
  `v_align` + run-level `link`), enabling the two items above.

## [0.3.0] — 2026-07-08

### Added

- **Inline & anchored images in PDF export (C-8).** Embedded pictures are
  drawn into the PDF with EMU→pt extents; anchored images render inline with a
  `FloatingImageInlined` warning; EMF/WMF vector formats draw a sized grey
  placeholder with an `UnsupportedImageFormat` warning (never a panic).
- **`font_map` filesystem paths.** A requested family can map to a local font
  file (embedded into the PDF) as well as to another installed family.
- **Tab-stop advance (C-9).** A `w:tab` advances the pen to the next tab stop;
  the interval comes from `settings.xml`'s `w:defaultTabStop` (0.5-inch Word
  default when absent). Custom per-paragraph tab stops (`w:tabs` with
  pos/leader/alignment) are out of v1 scope and degrade to the default interval
  with a single `CustomTabStopsIgnored` warning.
- **LibreOffice oracle SSIM advisory** (`scripts/lo_oracle_ssim.py`) — a
  local-only, never-CI script that rasterises our export and a `soffice
  --headless` reference through pdfspine and reports a windowed SSIM per fixture
  (advisory band 0.80–0.90). The synthetic fixture matrix currently scores
  0.97–1.00 against LibreOffice.

## [0.2.0] — 2026-07-04

### Added

- **Fidelity-preserving PDF export** — `Document.to_pdf()` / `save_pdf()`,
  flowed layout with pagination, drawn through the shared pure-Rust
  `pdf-typeset` engine (git-pinned pdfspine crates).
  - Per-section page geometry from `w:sectPr` (`pgSz`/`pgMar`/`orient`);
    section break ⇒ page break; multi-column flattened with a warning (C-2).
  - Run segment model (`Text`/`Tab`/`Break`) with `w:br@w:type`, plus `w:sdt`
    and `w:fldSimple` transparency — content-loss fixes (C-3).
  - Direct paragraph & run formatting: spacing, indents, keep-flags,
    strike/highlight/vertAlign, 4-slot `rFonts`, CJK eastAsia slot (C-4).
  - `styles.xml` + `theme1.xml` effective-style resolver: docDefaults →
    basedOn chain (cycle-safe) → table-style overlay → direct, with theme
    font/color indirection (C-5).
  - `numbering.xml` list engine — labels + hanging indents, restart counters
    (C-6).
  - Table fidelity: borders/shading/vAlign/margins, `gridSpan`/`vMerge`
    flattening, border-conflict resolution, cross-page row pagination (C-7).
  - `font_map` override and per-kind degradation warnings — export never fails
    on a missing font.
- Embedded-image byte round-trip, OCR engine caching, structured export
  (`to_text()` / `to_markdown()` / HTML tables), and `w:ins` revision fixes.

### Fixed

- Intel-mac wheels build via `macos-14` cross-compilation so releases cover the
  full platform matrix.

## [0.1.1] — 2026-06-30

### Fixed

- Corrected `NOTICE`: OCR models ship via the `ocrspine-models` package, not
  bundled into the wheel.

## [0.1.0] — 2026-06-26

### Added

- Initial release: pure-Rust `.docx` reader with paragraph, styled-run, and
  **table** (rows/cells/`gridSpan`/`vMerge`/nested/fills) extraction;
  `to_text()` / `to_markdown()` structured export with HTML tables for merges;
  optional OCR of embedded raster images and image-table reconstruction via the
  shared `ocrspine` engine; PyO3 bindings with abi3 wheels for macOS, Linux,
  and Windows.
