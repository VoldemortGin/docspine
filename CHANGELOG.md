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

- **Python `text()` / `to_text()` / `to_markdown()` / `to_html()` now release the GIL** while exporting (parsing, OCR and PDF rendering already did), so a large export no longer stalls other Python threads. No output change.

- **Style-level numbering (`w:style > w:pPr > w:numPr`) now applies to PDF export and all three text exports** (behavior change: headings linked to a multilevel list gain chapter numbers `1` / `1.1` / `1.1.1`, in `to_text`, Markdown `# 1.1 Title`, HTML and the PDF label). A paragraph's effective numbering is its own `numPr` (`numId=0` cancels), else the nearest `numId` along the style `basedOn` chain (a derived style's `numId=0` cancels); the level comes from the paragraph's own `ilvl`, else the `numbering.xml` `w:lvl > w:pStyle` back-link (ECMA-376 §17.9.23: it overrides the style's `numPr` level), else the style's `ilvl`, else 0. One shared resolver (`doc_core::style::resolve_numbering`) feeds both the PDF mapping and the exports. API additions: `ParaProps.num_id` / `num_ilvl`, `NumLevel.p_style`, `NumberingTable::level_for_style`, `NumRef`. `Paragraph.num_id` / `list_level` (and the Python paragraph dict) still carry only the paragraph's *own* `numPr`.

- **`to_text` no longer drops tables nested in table cells** (behavior change; fixes silent text loss): a nested table's rows are flattened into the cell as extra lines (cells joined by `\t`, rows by `\n`, same style as multi-paragraph cells), recursively and bounded by the parser's nesting guard. Markdown already fell back to an HTML `<table>` and HTML already nested real `<table>`s; a table inside a text box inside a GFM cell now also forces the HTML fallback instead of being dropped.

- **`to_text` / `to_markdown` / `to_html` now include list labels, hyperlinks and pictures** (intentional behavior change; no existing assertion needed rewording): list labels are computed with the same `ListCounters` as the PDF path (body incl. table cells continuous; header/footer parts, each note and each text box counted separately). Markdown emits `- ` for bullets and real list syntax for `1.` / `2)` labels (indented 4 spaces per `ilvl`), other labels (`a)`, `1.2.3`, `(a)`) as text prefixes; plain text / HTML prefix the label. Hyperlinks become `[text](url)` / `<a href>` for `http` / `https` / `mailto` only (anchors and other schemes stay plain text; Markdown escaping of link text and URLs is described under Security); pictures become `![alt](media)` / `<img alt src>`, and plain text gets `[图片: alt]` only when alt text exists.

- **Markdown / HTML heading detection now goes through the style sheet** (behavior change): a paragraph is a heading by (1) its own `w:outlineLvl`, (2) the `w:outlineLvl` cascaded along its style's `basedOn` chain, (3) the style `w:name` (own or inherited, case/space-insensitive `heading N`, `Title`, `Subtitle`, localized `标题 N` etc.), (4) the literal styleId match as before. `outlineLvl` 9 means body text; levels 7-9 are emitted as level 6. Numeric styleIds with a `heading 1` name and custom styles based on a heading used to export as plain paragraphs. `ParaProps` gained `outline_lvl` (Rust API, additive); `Subtitle` / `标题` styleIds now also map to headings.

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

- **Document properties** (`docProps/core.xml`): `Document.core_properties` (`CoreProperties`: title / subject / creator / keywords / description / category / last_modified_by / revision / created / modified / language, raw strings) and Python `doc.core_properties()` returning a fixed-key dict (names parallel to pptspine's `core_properties()`, core.xml subset; missing field -> `None`). The part is located via the package `_rels/.rels` core-properties relationship, falling back to `docProps/core.xml`; a missing or malformed part yields all `None` without error. The values never enter any warning or diagnostic. `Document` gains a public field (Rust API).

- **Parse diagnostics channel** (`Document.diagnostics`, Python `doc.diagnostics()`): a structured list of `Diagnostic { kind, part, count }` (identical `(kind, part)` merged; **never contains document text**) so callers can tell when content was silently truncated, skipped or clamped. Kinds (`DiagnosticKind`, `#[non_exhaustive]`, stable `code()` strings in the `RenderWarning` kebab-case style): `xml-truncated` (a part's XML is broken / cut off; the parsed prefix is still returned), `nesting-depth-exceeded` (subtrees skipped by the `MAX_NEST_DEPTH` guard), `table-columns-clamped` (extra `gridCol`s dropped + `gridBefore` / `gridAfter` clamps), `grid-span-clamped`, `numbering-value-clamped` (`w:start` / `w:startOverride` above `MAX_LIST_NUMBER`), `missing-part` (header / footer relationships or parts that do not exist, pictures whose media is missing; `part` is the part holding the dangling reference), `alt-chunk-not-imported` (every `w:altChunk`, headers / notes included). Collected centrally: walkers only bump counters on their `Ctx`, and `doc-parse/src/lib.rs` turns them into diagnostics and runs one well-formedness pass per XML part. `alt_chunk_count` is kept unchanged (body only). `Document` gained a public field, so struct literals without `..Default::default()` need updating (Rust API).

- **`w:altChunk` is now surfaced**: its content is still not imported, but `Document.alt_chunk_count` (Python `doc.alt_chunk_count`) counts them and PDF export emits one `alt-chunk-skipped` warning, so callers know content is missing.

- **Picture alt text**: `Picture.alt` (Rust) / `pic["alt"]` (Python) from `wp:docPr@descr`, falling back to `@title` (VML: `v:shape@alt` / `v:imagedata@o:title`).

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

- **Regression tests that fail when the DoS guards are reverted** (second review: these guards could be broken while the suite stayed green). Counters now sit where the work happens: note numbering counts actual key comparisons (a linear scan is counted too), `mc:Choice` snapshots count every `FieldFrame` clone, header / footer lookup counts every section visited — including on the PDF render path via the test-only `doc-core` feature `step-counters` — the Start form `<w:footnote>…</w:footnote>` is covered by the note cap test, the style-chain diagnostic test checks that the chain is really cut, and the inline-export linearity test covers multi-run Markdown expansion. Each was checked by temporarily reverting the guard.

- **Scale regression tests** (`crates/doc-parse/tests/scale_regressions.rs`, plus counters inside `style.rs` / `export.rs` / `xml/document.rs` / `header.rs` / `model.rs` unit tests): small synthetic inputs with large expansion (4,000-deep `basedOn` chains, 40,000 footnote references, 100 KB field instructions x thousands of result runs, `mc:Choice` storms over deep field stacks, 1,000 sections inheriting one header) now have count / linearity / boundedness assertions that a wall-clock timeout would not catch; minimal inputs were also added to `fuzz_regressions.rs` and the fuzz seed generator (`fuzz/seed.rs`).

### Breaking (Rust API, pre-1.0)

- `Document` gains `diagnostics` and `core_properties`, `ParaProps` gains `num_id` / `num_ilvl`, and `NumLevel` gains `p_style` (struct literals without `..Default::default()` must be updated).

- `Row` gains `grid_before` and `grid_after` (struct literals without `..Default::default()` must be updated).

- `Document` gains `alt_chunk_count`, `Picture` gains `alt`, `ParaProps` gains `outline_lvl`, and `RenderWarning` gains `AltChunkSkipped` (`alt-chunk-skipped`). Struct literals without `..Default::default()` and exhaustive matches on `RenderWarning` must be updated.

- `RenderWarning` gains `TableOverBudget` (`table-over-budget`); exhaustive matches
  on `RenderWarning` must be updated. `doc_core::model` gains the constants
  `MAX_TABLE_COLS` (63) and `MAX_TABLE_CELLS` (250,000), and `doc_core::numbering`
  gains `MAX_LIST_NUMBER` (32767). `Table::col_count()` is now capped at 63 and
  `Cell::grid_span` is clamped to 63 at parse time (Python `grid_span` /
  `col_count` / `grid_cols` follow).
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
- **`TextRun.field` is now `Option<Arc<str>>`** (was `Option<String>`): code that built or compared the field as a `String` must use `Arc<str>` / `as_deref()`. `DiagnosticKind` is `#[non_exhaustive]`; new kinds `style-chain-truncated`, `notes-truncated`, `field-instr-truncated`. The Python `field` key is unchanged (`str | None`).
- New API `Document::header_footer_index()` / `HeaderFooterIndex`; new `DiagnosticKind::SectionsTruncated`; `MAX_SECTIONS`, `MAX_NOTES`, `MAX_FIELD_INSTR` / `MAX_STYLE_CHAIN` constants are public.

### Fixed

- **Package parts are located through relationships instead of hard-coded paths** (previously a valid package whose main part is not `word/document.xml`, e.g. `word/document2.xml`, failed with "missing word/document.xml"): the main part is found via the `_rels/.rels` relationship whose type ends with `/officeDocument` (falls back to `word/document.xml` when `.rels` is missing / malformed / points nowhere); styles, numbering, settings, footnotes, endnotes, comments and theme are located via the main part's rels by relationship type, falling back to the old fixed paths. Relative targets resolve against the main part's directory and are normalized; a target whose `..` escapes the package root is rejected (treated as missing). Header / footer targets use the same resolver, and media is also collected from `<main dir>/media/`. Behavior change only for packages that previously failed or silently lost parts.

- **`w:gridBefore` / `w:gridAfter` are now parsed** (`Row.grid_before` / `grid_after`, clamped to `MAX_TABLE_COLS`; Python row dict `grid_before` / `grid_after`). Column indices, `vMerge` pairing, HTML `rowspan` and the PDF table layout now count the leading skipped grid columns, so tables whose rows start with a gap no longer shift cells left or mis-pair `vMerge`. HTML emits an empty cell (`colspan` for several columns) for the gap; the Markdown pipe table and plain text pad leading empty cells; the PDF leaves the gap empty. `Table::col_count()` (no `tblGrid`) includes the first row's before/after. **Orphan `vMerge continue` cells** (no `restart` above, or first row) are now ordinary cells: HTML export used to drop their content and the PDF mapping used to swallow them into the cell above.

- **`w:ruby` base text is no longer dropped**: the `w:rubyBase` runs now join the paragraph text in place (Japanese / pinyin-annotated Chinese documents lost whole phrases); the `w:rt` reading is intentionally not emitted (no duplicated text, no model field). **`w:dir` / `w:bdo`** (bidirectional text containers) are now run containers like `w:hyperlink` / `w:smartTag`, so Arabic / Hebrew runs inside them are kept.

- **Formula structures inside `m:oMathPara > m:oMath` (and inside `m:d` / `m:nary` / `m:func` / `m:e`) are linearized**: the inner `m:oMath` and other transparent wrappers used to hide `m:f` / `m:sSup` / `m:sSub` / `m:rad`, so a display equation such as `1/2` came out as `12`. Wrappers are now transparent; two `m:oMath` in one `m:oMathPara` are still joined by a space.

- **`w:tblGridChange` no longer truncates the document.** A tracked column-width /
  column-insert edit nests an old `w:tblGrid` inside the current one; the walker
  returned at the first `</w:tblGrid>`, so the leftover end tags closed the table
  and then the body: every remaining row and all following content was lost, and the
  old widths leaked into `grid_cols`. The old grid is now skipped as a whole
  (accept-all-revisions); expanded `<w:gridCol></w:gridCol>` parses like the
  self-closed form.
- **Formatting revisions no longer overwrite current properties.** The old values
  inside `w:pPrChange` / `w:rPrChange` / `w:tcPrChange` / `w:tblPrChange` /
  `w:trPrChange` (also reached from `styles.xml` and `numbering.xml`) were applied
  last and won, reverting style, numbering, `gridSpan`, `vMerge`, bold and so on to
  the pre-revision state. Those containers are now skipped as a whole.
- **Huge `list` start values could abort or hang.** A `w:start` near `i64::MAX` with a
  letter or roman `numFmt` hit `repeat(n / 26)` (capacity overflow) or a roughly
  `n / 1000` loop; the counter's `+ 1` could overflow. Letter / roman formats now
  fall back to decimal beyond Word's 32767 list limit and the counter saturates. The
  page-number letter format had the same unbounded repeat and gets the same cap.
- **Fuzz coverage**: new `parse_parts` target (first byte selects document / styles /
  numbering / header / footnotes / comments / settings, the rest is that part's XML),
  and the parsing targets now also run `to_text` / `to_markdown` / `to_html`.
  Not run locally (needs nightly + cargo-fuzz); minimal trigger inputs for the fixes
  above are regression tests in `crates/doc-parse/tests/fuzz_regressions.rs`.
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

- **Style-level numbering and heading resolution no longer scale as chain length x paragraphs** (performance / DoS fix from review): a ~35 KB `.docx` with a 4,000-deep `basedOn` chain and 200 paragraphs made `to_text` walk the chain once per paragraph with an O(L²) cycle check. Cycle detection now uses a set (`style_chain_ids`), `to_text` / `to_markdown` / `to_html` resolve each style's numbering and heading level once per export through a call-local `StyleCache` (no shared state; `Document` stays immutable behind `Arc`), `StyleTable::validate` visits each style once, and every `basedOn` walk is capped at `MAX_STYLE_CHAIN = 64` (the most-base ancestors beyond the cap are ignored; the parse diagnostic `style-chain-truncated` counts styles with an over-deep chain).

- **Footnote / endnote numbering is no longer quadratic** (performance / DoS fix from review): exports looked up each reference in a `Vec` with `contains` / `position`, so 40,000 distinct references (a ~200 KB `.docx`) made `to_text` take seconds. The first-reference order now has an `id -> number` ordered index (O(log n) per reference). Footnote / endnote / comment parts are also capped at `MAX_NOTES = 100,000` entries each (real books have at most a few thousand notes); extra entries are dropped and counted by the new parse diagnostic `notes-truncated`.

- **Field instructions are no longer cloned per result run, and are length-capped** (memory DoS fix from review): every run in a field's result region used to get its own copy of the whole instruction text, so a 1.2 KB `.docx` (100 KB `w:instrText`, 2,000 result runs) used ~250 MB at parse. Runs now share one `Arc<str>` per field, and a single field instruction is truncated at `MAX_FIELD_INSTR = 4096` bytes (on a char boundary; counted by the new parse diagnostic `field-instr-truncated`). Rendering only needs the first word and `\*` switches, so nothing observable is lost.

- **Python bindings no longer copy shared data per run / per section** (memory DoS fix from the second review): runs of one field share a single `Arc<str>` instruction in Rust, but `paragraphs()` / `body()` built a new Python `str` for every run, so a 26 KB `.docx` (4 KB instruction x 200,000 result runs) grew RSS from 272 MB to 1.1 GB. Each conversion call now caches Python objects by shared pointer: all result runs of a field get the same `str` object (`runs[i]["field"] is runs[j]["field"]`), and in `sections()` sections referencing the same header / footer part get the same `blocks` list object instead of a converted copy per section. Measured on the review probe: RSS growth ~940 MB -> ~310 MB (the rest is the 200,000 run dicts themselves).

- **`mc:Choice` field-stack snapshots are now pointer-sized** (memory / CPU DoS fix from review): each non-empty `mc:Choice` deep-copied the whole complex-field stack (every frame's instruction text) so it could roll back a losing branch. Frames are now `Rc`-shared: a snapshot copies at most `MAX_NEST_DEPTH` pointers, and a frame is copied (copy-on-write, at most `MAX_FIELD_INSTR` bytes) only if the trial branch actually mutates it. Rollback semantics are unchanged.

- **Math linearization no longer changes the meaning of expressions** (behavior change; fixes from review). Parentheses are now decided by the *linearized text* of a slot (anything that is not a single alphanumeric atom, or a fully bracketed string, gets parenthesized), not by the number of `m:t` fragments: a single-run numerator `a+b` over `c` was `a+b/c`, now `(a+b)/c`; `x` with superscript `n+1` was `x^n+1`, now `x^(n+1)`. `m:d` now emits its delimiters from `m:dPr` (`begChr` / `endChr` / `sepChr`, defaults `(` `)` `|`; an explicit empty value means no bracket), so `2(x+1)` is no longer `2x+1`; `m:nary` emits `operator_(lower)^(upper) body` with the operator from `m:naryPr > m:chr` (default `∫`), so a sum is `∑_(i=1)^n x_i` instead of `i=1nx_i`; `m:sSubSup` is `x_i^2` instead of `xi2`. Also linearized now: `m:sPre` (`_6^14 C`), `m:limLow` / `m:limUpp` (`lim_(x→0)`), `m:func` (`sin x`, `sin(x+1)`), `m:acc`, `m:bar` (`overline(x)` / `underline(x)`), `m:m` matrices (`[a,b;c,d]`); other multi-slot structures (`m:eqArr`, `m:box`, ...) separate their children with one space. Still an explicit-stack implementation bounded by `MAX_NEST_DEPTH`. Existing expectations in `content_drop.rs` that encoded the old output (`m:d` / `m:nary` / `m:func` inside transparent containers, and the "other structures" test) were updated.

- **Text exports no longer scan back over the paragraph for every run** (performance / DoS fix from review; regression introduced by the Markdown escaping change): deciding whether a run starts a line re-scanned the whole paragraph built so far, in all three exports, so one paragraph with 800,000 runs (a ~100 KB `.docx`) took ~15 s per export and 100,000 references to one endnote took 8 s in `to_html`. The "line is blank so far" state is now maintained incrementally from the newly appended bytes only. No output change.

- **Math linearization no longer glues structures to their neighbours or treats multi-character strings as atoms** (behavior change; second review round). The previous rules were right inside each structure but wrong at the seams: `3` followed by the fraction 1/2 gave `31/2` (reads as 15.5), `x` + 1/2 gave `x1/2`, `1/(2x)` and `(1/2)·x` both gave `1/2x`, `(2x)²` and `2·x²` both gave `2x^2`, `x²` followed by `3` gave `x^23`, `sin x cos x` gave `sin xcos x`, `x` followed by a prescript gave `x_6^14 C`, and a sum over `x_i+1` gave `∑_(i=1)^n x_i+1`. New rules (full text in `crates/doc-parse/src/xml/math.rs`, which now holds the linearizer): an atom is only a single letter (optionally with combining marks), a number, or a fully bracketed group, and every other slot is parenthesized (`x^(2x)`, `(ab)^2`); a structure is separated from adjacent terms by a space unless an operator, punctuation or bracket already separates them (`2 x^2`, `x^2 3`, `e^x y`, `sin(x) cos x`); a fraction next to a product term is parenthesized as a whole (`3 (1/2)`, `(1/2) x`, while `3+1/2` stays); an n-ary operand or function argument is parenthesized unless it is an atom or a scripted atom (`∑_(i=1)^n x_i`, `∑_(i=1)^n (x_i+1)`, `∫_0^1 (x dx)`); `m:sPre` is written `(_6^14)C`; `m:d` with `sepChr=""` separates elements with a space; `m:box` and similar wrappers are transparent and `m:eqArr` rows are separated like `m:oMathPara` equations. A property test (`crates/doc-parse/tests/math_props.rs`) linearizes 28,622 generated formulas (atoms × fraction / scripts / radical / sum / function / delimiter, one and two levels deep, plus adjacent-term sequences) and asserts that formulas with different meaning never produce the same text; the old rules produced 313 collisions.

- **HTML export no longer nests `<a>` for a footnote reference inside a hyperlink** (invalid markup): the outer link is closed before the note marker and reopened after it, so text order is unchanged and no empty anchors are left.
- **Header / footer cost no longer scales as sections x work** (performance fix from review): a 1 KB `.docx` with 1,000 sections inheriting one header re-mapped and re-measured the same part once per section, and `Document::header_footer_for_page` walked back over all earlier sections for every page. The effective header / footer references are now precomputed once by prefix propagation (new `Document::header_footer_index()` / `HeaderFooterIndex::for_page`, O(1) per page; `header_footer_for_page` is kept, same semantics), and part measurement / layout / image-id normalization are cached by `(part, page geometry, header or footer distance)` instead of by section index. The section count is capped at `MAX_SECTIONS = 100,000` (see the next entry); extra middle sections are merged into the last one (which carries the document-level `sectPr`) and counted by the new parse diagnostic `sections-truncated`.

- **The section cap no longer drops header / footer content or page-number restarts below 100,000 sections** (second review): with the cap at 10,000, a mail-merge document (one section per letter, page numbers restarting at 1) above 10,000 letters got wrong PDF page numbers, and a header referenced only by a merged section vanished from `to_text`. Section count no longer has superlinear cost (measured `to_pdf` 0.11 s / 1.37 s for 10,000 / 100,000 sections), so the cap now only guards memory: each section forces at least one PDF page (~10 KB of render memory per page, ~1 GB at 100,000), while the zip limits would otherwise allow ~7 million minimal `sectPr`s. `MAX_SECTIONS` is now 100,000. When it does trigger, the merged sections' page geometry and page-number settings follow the last section (that is what `sections-truncated` reports), but their header / footer references are kept: the last section gets the references effective at the end of the merged range for the types it lacks (so its own pages resolve exactly as before), and every other referenced part is still loaded and exported.

- **Three degraded-input corrections from the second review.** (1) When a style `basedOn` chain is cut at `MAX_STYLE_CHAIN = 64`, toggle properties (`b`, `i`, `caps`, `smallCaps`, `strike`, `vanish`) no longer XOR along the truncated chain, whose parity may be flipped (a wrong result, not just an incomplete one); they take the explicit value nearest to the run, else the document default (the truncation is still reported as `style-chain-truncated`). (2) When a footnote / endnote / comment part exceeds `MAX_NOTES`, notes referenced from the body or headers / footers are kept first (they reserve up to `MAX_NOTES` slots, unreferenced notes use the rest) instead of dropping whatever comes last in the part; dropped notes are still counted by `notes-truncated` and their references still leave no dangling marker. (3) Field instructions are whitespace-folded (leading whitespace dropped, runs of whitespace collapsed to one space) before the `MAX_FIELD_INSTR` cap applies, so ~4 KB of leading blanks no longer cut off `PAGE` and lose the field; `TextRun.field` therefore carries the folded form (`PAGE  \* MERGEFORMAT` becomes `PAGE \* MERGEFORMAT`).

### Security

- Table size is now bounded: `gridSpan` and `w:tblGrid` are clamped to Word's 63-column
  limit at parse time, `Table::col_count()` is capped (saturating sum), and the PDF
  mapping budgets `columns x rows` per table at 250,000 grid slots (about 4,000 rows at
  63 columns). Over-budget tables render only their leading rows and emit one
  `table-over-budget` warning; text / Markdown / HTML export keep every row. Previously
  a tiny file (`gridSpan="4000000000"`, or 10,000 `gridCol` x 10,000 rows) made
  `to_pdf` abort on allocation failure.
- `.docx` zip reads are now bounded by `doc_parse::ZipLimits` (defaults: 10,000 entries, 256 MiB per entry, 1 GiB total decompressed, 10,000:1 compression ratio for entries > 1 MiB — unreachable by deflate's ~1032:1 ceiling, so it only catches bombs using other compression methods — and 1024-byte names). Declared sizes are no longer trusted for allocation (forged headers are caught while streaming), and absolute / drive / `..` entry paths are rejected. Violations raise `DocError::LimitExceeded` (Python: `DocZipError`, message names the limit). New Rust entry points `parse_bytes_with_limits` / `parse_path_with_limits`. Nested tables / content controls / inline run containers deeper than 64 levels are now skipped instead of overflowing the stack.

- **Markdown export: document text cannot inject Markdown structure, with a minimal escape set** (security fix from review, then narrowed after the second review; intentional behavior change). Before the first fix only `[` / `]` in link text and alt were escaped, so document content could inject links, images, raw HTML and block syntax (a link text `a\](javascript:alert(1))` inside an `https` link even produced a `javascript:` link). The first fix escaped every special character in every piece of text, which hurt RAG text (`f(x)` became `f\(x\)`, `AT&T` was escaped, table cells became `\-5`, `\+3%`, `\=A1`, `1\.`, and indented code lines lost their indentation) and still left gaps. Now each block (paragraph, heading, list item, table cell, footnote definition) is assembled into one buffer with a per-byte mask of exporter-generated syntax, then escaped in a single linear pass, so context that spans runs is seen (`&` + `#x3C;` in the next run, a generated `]` followed by document `(`). Only document text is escaped, and only where it could form syntax: `[` always; `]` only in link text / image alt; `(` and `:` only right after an exporter-generated `]`; `<` only before an ASCII letter, `/`, `!` or `?`; `>` only at line start; `&` only when it forms an entity reference; `_` only at word boundaries; `*` unless surrounded by whitespace; backticks always; `~` when a block has two or more; `\` only before ASCII punctuation, a line break or at block end. Line-start rules (ATX headings, `- + *` bullets, setext / thematic-break lines, `1.` / `1)` ordered markers, `>` quotes, table delimiter rows) apply to paragraph and heading-continuation lines but never inside table cells; 4+ columns of leading whitespace are removed only on a block's first line (CommonMark drops it when rendering anyway), continuation lines keep their indentation; `|` is escaped in cells, and in a paragraph only when one of its lines looks like a table delimiter row; a trailing ` #` sequence in a heading is escaped instead of being swallowed. Footnote definitions `[^n]: content` can no longer act as CommonMark link reference definitions (which turned the body `[^n]` into a link to a `javascript:` / `data:` URL): a leading `<` and a title-like second word are escaped, and single-word content gets a trailing space + U+200B (invisible, not CommonMark whitespace). Line breaks inside link text and alt fold to spaces (a double break used to cut the link), footnote markers inside link text are moved between two links like the HTML export, and `&` that forms an entity in a link destination is written `\&` (`?q=&copy;` stayed `©` before). Link / image destinations still percent-encode `\`, whitespace, parentheses and angle brackets. Verified end to end with markdown-it (CommonMark + table + strikethrough, permissive link validation): 87 payloads x 16 positions plus cross-run cases produce no injected link, image, raw HTML, table, heading, list, quote, code or emphasis. HTML export is unchanged except that alt text of a picture without a media name is now HTML-escaped (it was emitted raw). A renderer that allows raw HTML should still treat the output as untrusted content.

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
