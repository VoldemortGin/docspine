# PRD-PDF-EXPORT — Faithful .docx → PDF export (Phase C, flow layout + pagination)

Status: design (code-verified 2026-07-02). Effort scale (family convention): **S** ≈ hours · **M** ≈ 1–2
days · **L** ≈ multi-day. Every task lists **why · files · effort · Acceptance** (the green condition that
means "done"). Family phase order: **Phase A** = pdfspine `crates/pdf-typeset` engine (pdfspine
`docs/PRD-NEXT.md` §10, tasks TS-1..TS-7) → **Phase B** = pptspine → **Phase C** = docspine (**this PRD**,
tasks C-1..C-10). Phase C's engine prerequisites: **TS-2** (system fonts), **TS-3** (multi-face +
subsetter), **TS-4** (flow layout + table primitives).

---

## 1. Goal & non-goals

**Goal.** `Document.to_pdf()` produces a PDF that is *layout-faithful* to what Word shows for the source
`.docx`: (a) all body content survives in reading order (read-back token-F1 & order ≥ 0.99 vs `to_text()`);
(b) page count and `Page.rect` match the sectPr-derived geometry; (c) fonts / sizes / bold / italic /
decorations match the **effective** formatting after the full styles cascade (docDefaults → basedOn chain →
direct formatting); (d) block positions (margins, indents, spacing, table grids) sit within tolerance of a
Word/LibreOffice reference (advisory SSIM band 0.80–0.90, later committed `.ssimref` at `--min-ssim 0.97`).
Deterministic per font environment: same machine + same fonts ⇒ identical bytes.

**Disclaimer — the markdown path does NOT count.** `doc.to_markdown()` piped into pdfspine's
`markdown_to_pdf()` already yields *a* PDF today, but it discards page geometry, pagination, styles.xml,
exact fonts, tables-with-merges, and images-in-position (`crates/doc-core/src/export.rs:38-61` is a lossy
string serializer). That content-level path is explicitly **not** export fidelity; this PRD is about true
layout-faithful rendering via a new `doc-render` crate.

**v1 IN scope** (each has a task in §7): sectPr page size/margins/orientation per section, section breaks as
page breaks; **styles.xml full inheritance chain** (docDefaults → basedOn → direct — the single biggest
parse gap, §3b); numbering.xml lists; paragraph spacing/indent/borders/shading; run underline styles /
highlight / vertAlign / strike / all-four-slot `rFonts` incl. eastAsia; per-edge table & cell borders +
shading + vAlign + cell margins; inline AND floating image placement (no wrap); explicit page breaks (`w:br
w:type="page"`); tab-to-default-stops; theme1.xml fontScheme (major/minor latin + eastAsia).

**v1 OUT of scope** (declared, with rationale; every degradation emits a structured warning):

- **Headers/footers** — declared out for v1. Parts never read today (§3j); requires per-section
  header/footer parts + a PAGE/NUMPAGES field engine (M–L on top of everything else).
- **Footnotes/endnotes** — per-page float layout with split/continuation is L-hard; parts never read
  (`w:footnoteReference` skipped, `document.rs:224`).
- **Multi-column sections** — needs a column-balancing engine; `w:cols` parsed (C-2) but rendered
  single-column + warning.
- **Full floating-object text-wrap semantics** — v1 draws floating/anchored images at their anchored
  absolute position as an overlay, **no text wrap**, + warning. Real wrap requires per-line exclusion
  rectangles in the engine.
- **Field recalculation** — render the cached field result only. NOTE: doc-parse today **drops `w:fldSimple`
  results and `w:sdt` content entirely** — both are S fixes and **IN scope** (C-3); recalculation (dates,
  refs, page numbers) stays out.
- **Charts / SmartArt / WordArt / OLE** — each needs its own DrawingML interpreter; bounding-box placeholder
  + warning.
- **Gradients / shadows** — pdf-edit authoring has no `/Shading`/`/Pattern` (grep zero hits); degrade to
  first-stop solid + warning.
- **Comments / revision marks** — render accepted text per existing parser semantics (`w:ins` expanded,
  `w:del` dropped, `document.rs:118-127`); no markup rendering.
- **Custom tab stops** — v1 uses default 0.5" interval tabs (C-9); `w:tabs` pos/leader/alignment is a
  stretch goal.
- **Intra-row table page splitting** — v1 moves unsplittable rows whole to the next page, + warning if a row
  is taller than a page (matches pdf-markdown's documented limitation, pdfspine `layout.rs:832-833`).

---

## 2. Current state (docspine today)

- Cargo workspace, 4 crates (`Cargo.toml:1-8`): `doc-core` (dependency-free IR + twip/EMU geometry +
  text/md/html export), `doc-parse` (zip + quick-xml → `Document`), `doc-ocr`, `py-bindings` (PyO3 `_core`,
  abi3-py311). All non-FFI crates `#![forbid(unsafe_code)]`.
- The IR is a **content** model, not a layout model: `Document { body: Vec<Block> }` only
  (`crates/doc-core/src/model.rs:16-20`) — no sections, styles, numbering, or theme. `Paragraph` carries
  runs + raw `style`/`align`/`list_level` (`model.rs:32-41`); `TextRun` carries
  text/font/size_pt/bold/italic/underline/color/pictures (`model.rs:51-66`) — verified in source.
- The zip layer reads exactly three things: `word/document.xml`, `word/_rels/document.xml.rels`,
  `word/media/*` (`crates/doc-parse/src/zip_pkg.rs:50-72`, verified). All zip parts are already in memory
  (`Package.parts`, `zip_pkg.rs:20-41`), so new part readers are only new `part_str()` calls + walkers.
- The body walker skips every unknown element wholesale, including `w:sectPr` and `w:sdt`
  (`crates/doc-parse/src/xml/document.rs:82-83`, verified); `parse_ppr` handles exactly `pStyle`/`jc`/`ilvl`
  (`document.rs:141-164`, verified — `numId` unread).
- Tables are structurally solid (grid, `gridSpan`, `vMerge`, nesting, dxa widths, shading fill —
  `document.rs:354-373, 460-505`) but carry **zero border/margin/vAlign data**.
- Existing exports are pure model→string serializers (`export.rs:18-131`), exposed as
  `to_text()/to_markdown()/to_html()` (`crates/py-bindings/src/lib.rs:284-298`; stubs
  `python/docspine/_core.pyi:29-32`, verified). **No file-writing method exists**; media bytes are reachable
  from the Python handle (`ParsedDoc { document, media }`, `doc-parse/src/lib.rs:24-28`; handle at
  `lib.rs:202-207`).
- `geom.rs` already has exactly the conversions a PDF mapper needs: `TWIPS_PER_POINT=20.0`,
  `EMU_PER_POINT=12700.0`, `twips_to_points`, `emu_to_points` (`crates/doc-core/src/geom.rs:11-39`).

---

## 3. Parse-gap inventory for fidelity (THE core section)

Verdicts against `crates/doc-parse` as of 2026-07-02. "Model growth" = which doc-core types must grow.

| # | Feature | Verdict | Evidence | Effort | Model growth |
|---|---|---|---|---|---|
| a | sectPr: `pgSz`/`pgMar`/`orient`/`cols`, section breaks | **MISSING** (skipped wholesale) | `document.rs:82-83`; no Section type (`model.rs:16-20`); grep `sectPr\|pgSz\|pgMar` → only the skip comment | M (S for single section) | new `Section { page_w, page_h, orient, margins, cols }`; `Document.sections`; paragraph→section attribution |
| b | styles.xml: definitions, `basedOn`, `docDefaults`, effective resolution | **MISSING entirely** | never opened (`zip_pkg.rs:50-59`; `doc-parse/src/lib.rs:40-69`); grep `styles.xml\|docDefaults\|basedOn` → 0 hits; only raw ids kept (`document.rs:149`, `340-342`) | **L** (top priority) | new `StyleTable` (id → pPr/rPr fragments + basedOn + type + default); `ParaProps`/`RunProps` structs shared with direct formatting |
| c | numbering.xml; **`w:numId` unread — bonus bug** | **MISSING** (only `ilvl` read) | `document.rs:151-153` — no `numId` arm; grep `numbering\|abstractNum\|lvlText` → 0 hits | M–L (numId capture itself S) | `Paragraph.num_id`; new `NumberingTable` (numId → abstractNum → per-level numFmt/lvlText/start/ind) |
| d | pPr `jc` (incl. justify `both`) | **PARSED** (raw string) | `document.rs:150`; `model.rs:37` | — | normalize enum in resolver |
| d | pPr `spacing` (before/after/line/lineRule) | **MISSING** | no arm in `document.rs:146-155` | S | `ParaProps.spacing` |
| d | pPr `ind` (left/right/firstLine/hanging) | **MISSING** | same | S | `ParaProps.indent` |
| d | pPr `pBdr` / `shd` | **MISSING** (only cell shd exists, `document.rs:490-494`) | same | S–M | shared `Border` struct + `ParaProps` |
| d | pPr `keepNext`/`keepLines`/`pageBreakBefore`/`widowControl`/`contextualSpacing` | **MISSING** | same | S each | `ParaProps` flags |
| e | rPr `rFonts` 4-slot (ascii/hAnsi/eastAsia/cs + theme attrs) | **PARTIAL** — collapsed to ONE font, **`@eastAsia` never read** | `document.rs:275-280`; `model.rs:54` | S | `RunProps.fonts: [Option<String>; 4]` + theme-slot enums |
| e | rPr `sz` / `b` / `i` / `color` | **PARSED** (sz half-pt÷2; on/off semantics correct; `auto`→None) | `document.rs:281-289, 296-300`; `xml/mod.rs:106-115` | — (`szCs`/`bCs`/`iCs` S) | — |
| e | rPr `u` underline style variants + color | **PARTIAL** (bool only; style discarded) | `document.rs:290-295`; `model.rs:60-61` | S | `RunProps.underline: Option<UnderlineKind>` |
| e | rPr `strike`/`dstrike`, `highlight`, `vertAlign`, char `spacing`, `caps`/`smallCaps`, `rStyle` | **MISSING** | no arms in `document.rs:273-303` | S each | `RunProps` fields |
| f | tblGrid, gridSpan, vMerge, tcW(dxa), cell shd fill, trHeight, tblHeader, nested tables | **PARSED** | `document.rs:354-373, 400-424, 460-505` | — | — |
| f | `tblBorders`/`tcBorders` (per-edge), `vAlign`, `tcMar`/`tblCellMar`, `tblW`, tcW `pct`, trHeight `@hRule`, table `jc`/`tblInd`/`tblLayout`, `cantSplit` | **MISSING** — borders exist nowhere in the pipeline (grep `tblBorders\|tcBorders\|vAlign\|tcMar` → 0) | tblPr matches only `tblStyle` (`document.rs:338-343`); tcPr walker `document.rs:460-505`; hRule unread (`document.rs:408-412`) | M total | `TableProps`/`CellProps` with per-edge `Border`; `Row.h_rule`, `Row.cant_split` |
| g | Inline picture: extent (EMU) + bytes + rel resolution | **PARSED** (all media formats collected) | `document.rs:511-544, 574-592`; `zip_pkg.rs:62-72` | — | — |
| g | Inline-vs-anchor flag, anchor `positionH/V` offsets, wrap type, z-order; VML size; crop | **MISSING/PARTIAL** — both captured but indistinguishable | `document.rs:511-544` (no anchor/position/wrap names); VML rel-only `document.rs:548-570` | S parse / M render | `Picture.placement: Inline\|Anchored{x,y,relative_from,behind}` |
| h | theme1.xml fontScheme (major/minor latin+eastAsia) + clrScheme | **MISSING entirely** | grep `theme\|fontScheme` → 0; `zip_pkg.rs:44-72` | M | new `Theme { fonts, colors }` — without it Word's default body font (`minorHAnsi` → Calibri) is unresolvable |
| i | `w:br`/`w:cr` **folded to `'\n'`; `@w:type` never read — bonus bug**: explicit page breaks invisible | **PARTIAL (lossy)** | `document.rs:210-212, 230-232` | S–M | run content → `Vec<RunSegment>` (Text/Tab/Break{Line,Page,Column}) — ripples into `export.rs` |
| i | Tab stops (`w:tabs`, settings.xml `defaultTabStop`) | **MISSING** (`w:tab` folded to `'\t'`, `document.rs:206-209`; settings.xml never read) | no `tabs` arm in `parse_ppr` | M (parse S, render M) | `ParaProps.tabs`; `Document.default_tab_stop` |
| j | Hyperlink targets | **DONE** — `@r:id`→rels URI in `TextRun.link_target`; renders as a PDF `/Link` annotation via the engine's `RunStyle.link` (TS-11); internal `@w:anchor` stored as `"#name"`, not drawn + one-time warning | `document.rs` `hyperlink_target`; `map.rs` `push_runs` link threading | done | `TextRun.link_target` (landed) |
| j | Footnotes/endnotes | **MISSING** | parts never read; refs skipped (`document.rs:224`) | L — **OUT v1** | — |
| j | Headers/footers | **MISSING** | inside skipped sectPr (`document.rs:82-83`); parts never read | M–L — **OUT v1** | — |
| + | **Bonus bug: `w:sdt` content dropped wholesale** — cover pages / TOC text LOST | **MISSING** | `document.rs:82-83` + `skip_element` (`document.rs:620-638`) | S — fix in C-3 | none (transparent container) |
| + | **Bonus bug: `w:fldSimple` cached result dropped** | **MISSING** | falls into `document.rs:127` skip | S — fix in C-3 | none (transparent container) |

**Reading of the table:** structure (blocks, runs, tables-with-merges, images-with-bytes) is solid;
everything *visual* beyond `b/i/u/sz/color/jc` is absent, and the three cross-part tables
(styles/numbering/theme) don't exist at all. The extended pPr/rPr walkers must be refactored into reusable
prop-struct parsers shared by document.xml **and** styles.xml (same grammar in both parts).

---

## 4. `doc-render` crate design

**One new crate** `crates/doc-render`, mapping the (extended) doc-core IR + `ParsedDoc.media` onto
`pdf-typeset` input. It must NOT live in doc-core (doc-core is "No IO, no zip, no XML", sole dep thiserror —
`doc-core/src/lib.rs:2-5`) and shares nothing with `export.rs` except the input `Document`.

**Dependency form (LOCKED)** — copy the ocrspine precedent verbatim-style. The existing comment at
`/Users/linhan/workspace/spine/docspine/Cargo.toml:24-29`:

```toml
# --- sibling family crate: domain-neutral OCR (single source of truth for the
# path; doc-ocr uses `ocrspine.workspace = true` so no per-crate path math).
# Git dep (NOT path): the family publishes uniformly via git, and CI's
# `maturin build` then `cargo fetch`es ocrspine itself — no sibling checkout
# needed on the runner. ---
ocrspine = { git = "https://github.com/VoldemortGin/ocrspine", rev = "732975f0233cd6500edfbbb82bc06c2332369871" }
```

New entry, same pattern: `pdf-typeset = { git = "https://github.com/VoldemortGin/pdfspine", rev = "<pinned>"
}` in `[workspace.dependencies]`; `doc-render` consumes `pdf-typeset.workspace = true`. `../pdfspine/` stays
READ-ONLY (`CLAUDE.md:34-40`). Dependency graph: `doc-core` stays dependency-free; `doc-render` = doc-core +
pdf-typeset; `py-bindings` += doc-render behind a default-on `pdf` feature (mirrors the `ocr` feature
pattern, `pyproject.toml:58-60`).

**Module sketch:**

```
crates/doc-render/
  src/lib.rs       render_pdf(&Document, &BTreeMap<String,Vec<u8>>, &RenderOptions) -> ExportResult
                   RenderOptions { font_map: BTreeMap<String,String> }   # user substitution overrides
  src/map.rs       block walk: Paragraph → pdf-typeset styled-run paragraph input; drives the resolver
  src/section.rs   Section chain → per-page geometry (page box, margins, orientation); break policy
  src/table.rs     merge/span flattening + border conflict resolution → table-primitive input
  src/image.rs     inline atoms + anchored overlays; PNG/JPEG passthrough; EMF/WMF degrade
  src/warn.rs      docspine-side ExportWarning kinds, merged with engine warnings
```

**Styles resolution happens WHERE — recommendation (clear): a new doc-style module in doc-core,
`crates/doc-core/src/style.rs`,** feeding doc-render. Rationale: (1) the resolver's inputs are pure model
data (StyleTable/Theme + direct props) — zero deps, so doc-core's charter holds; (2) the locked dependency
graph says doc-render deps = doc-core + pdf-typeset only, so a doc-parse-hosted resolver would be
unreachable at render time without pre-baking resolved props into every paragraph (duplicating raw +
resolved state and silently changing `to_markdown`'s raw-id semantics); (3) doc-core tests compile in
seconds, doc-render pulls the whole pdfspine graph — the cascade's many unit tests belong on the fast side;
(4) `export.rs` can later reuse effective props (e.g. real heading sizes in HTML). doc-parse's job stays
*mechanical*: walk styles.xml/numbering.xml/theme1.xml into plain-data tables on `Document`. `style.rs`
exposes `resolve_para(&doc, &para) -> EffectiveParaProps` and `resolve_run(&doc, &para, &run) ->
EffectiveRunProps` implementing docDefaults → basedOn chain (cycle-safe, visited-set) → table-style overlay
→ direct formatting, plus theme font/color indirection. The numbering counter engine (per-numId per-level
state machine, restart semantics, `%1.%2` label formatting) is its sibling `doc-core/src/numbering.rs`.

**Section / page geometry.** `Document.sections: Vec<Section>` — body-final `w:sectPr` is the last section;
each mid-body `w:pPr > w:sectPr` closes the section *containing* it. Word defaults when absent: 12240×15840
twips page, 1440 twips margins. doc-render feeds pdf-typeset's pagination-callback API a per-section
geometry provider; a section boundary forces a page break and swaps the page box (v1 treats all
section-start types as next-page). All conversions via `twips_to_points` (`geom.rs:24-27`).

**Table flow strategy.** doc-render pre-computes what the engine's primitives don't know about OOXML: (1)
grid from `w:tblGrid` (fixed layout) or measured (auto), pct/dxa `tcW` resolution against the text width;
(2) a **span map** flattening `gridSpan`/`vMerge` — vMerge continuation cells collapse into the restart
cell, whose block content lays out once with row-span height; (3) **border conflict resolution** on the
docspine side (`tcBorders` > `tblBorders` > table-style borders; shared-edge dominance) so pdf-typeset
receives final per-edge strokes per cell and just paints; (4) row pagination: whole-row move to next page
when it doesn't fit (`cantSplit` semantics for all rows in v1), `RowTooTall` warning when a row exceeds page
height; nested tables recurse through cell block layout.

**Image placement.** Inline pictures become inline atoms with extent EMU→pt (`emu_to_points`,
`geom.rs:33-36`), participating in line height. Anchored pictures are collected per page and drawn as
overlays at `positionH/V` offsets relative to page/margin (v1: no text wrap, `FloatingNoWrap` warning). JPEG
passes through; PNG et al. decode via the engine's pdf-image path; EMF/WMF → gray placeholder rect +
`UnsupportedImageFormat` warning. VML pictures without extent fall back to intrinsic bitmap size.

**Warning propagation.** pdf-typeset returns `ExportResult { bytes, warnings: Vec<ExportWarning> }` (engine
kinds: FontSubstituted, MissingGlyph, …). doc-render appends its own kinds (FloatingNoWrap, RowTooTall,
UnsupportedImageFormat, MultiColumnFlattened, GradientDegraded, CustomTabStopsIgnored, …) and returns the
merged vector; py-bindings dedupes by kind and surfaces each unique kind once.

---

## 5. Python API (v1, LOCKED)

```python
class Document:
    def to_pdf(self, *, font_map: dict[str, str] | None = None) -> bytes: ...
    def save_pdf(self, path: str | os.PathLike, *, font_map: dict[str, str] | None = None) -> None: ...
```

Zero required args; `font_map` is the only option (family substitution overrides fed to the engine's
resolver, e.g. `{"宋体": "Songti SC"}`). Matches the existing `to_text()/to_markdown()/to_html()` convention
(`python/docspine/_core.pyi:29-32`). Degradation warnings surface via `warnings.warn` — **one per unique
ExportWarning kind**, category `UserWarning`.

**Wiring sketch** (`crates/py-bindings/src/lib.rs`): new methods on the frozen `Document` pyclass next to
`to_html` (`lib.rs:296-298`); the handle already holds `Arc<CoreDocument>` **and** the media map
(`lib.rs:202-207`) — everything `render_pdf` needs. Render under `py.detach` (pattern `lib.rs:331`);
re-acquire the GIL to emit `PyErr::warn` per unique kind, return `PyBytes`. `save_pdf` = same path +
`std::fs::write`, mapping IO errors into the existing `DocError` hierarchy (`lib.rs:43-61`). Stubs: extend
`_core.pyi` after line 33; no pure-Python wrapper needed (methods live on the pyclass, like every existing
method). Feature gating: `pdf` feature on py-bindings (default-on in `[tool.maturin] features`, mirroring
`ocr`).

---

## 6. Reuse map (落点地图 — don't re-investigate)

All pdfspine anchors verified 2026-07-02 against `/Users/linhan/workspace/spine/pdfspine`.

- **New crate `crates/doc-render`** (deps: doc-core + pdf-typeset via git+rev workspace dep, §4).
  pdf-typeset re-exports the pdf-edit surface consumers need — do NOT depend on pdf-edit/pdf-core directly.
- **Engine input model (TS-4 delivers):** styled runs
  (family/size/bold/italic/underline/strike/color/highlight) + paragraph props (align incl. justify, line
  spacing multiple+exact, space before/after, first-line/hanging + left/right indent, list labels) +
  pagination callbacks. What exists today that TS-4 generalizes: greedy wrap
  `crates/pdf-markdown/src/layout.rs:310-394`; pending-gap space-before/after machinery `layout.rs:442-452`;
  align_offset (L/C/R only) `layout.rs:516-522`; per-char fallback
  `crates/pdf-markdown/src/fonts.rs:169-205`.
- **⚠ TRAP:** `Frag` has **no per-frag size** — one size per paragraph is assumed throughout
  (`layout.rs:148-156, 468-476`), and baseline is the `0.8` heuristic not real ascent (`layout.rs:23-24`).
  Mixed-size lines + real metrics are TS-4 deliverables; doc-render must not land C-1 against a pre-TS-4
  engine rev.
- **⚠ TRAP:** justify cannot use the PDF `Tw` operator — Identity-H uses 2-byte codes and `Tw` only applies
  to single-byte code 32. TS-4 implements justification by redistributing space-frag widths; docspine only
  passes `align="both"` through.
- **⚠ TRAP:** `insert_textbox` (`crates/pdf-edit/src/text.rs:238`) **drops lines past the bottom edge and
  does not return the overflow** → unusable for cross-page flow (same trap PRD-NEXT §9 flags at
  `docs/PRD-NEXT.md:361-363`). Flow must go through pdf-typeset's paginating layout, never insert_textbox.
- **Fonts (TS-2/TS-3 deliver):** system resolution via fontdb 0.23 + substitution tables (宋体→Songti SC etc.)
  + per-char fallback chain + bundled Liberation/Noto last resort
  (`crates/pdf-fonts/src/liberation.rs:36-52`); multi-face R/B/I/BI as 4 distinct embedded fonts with TTC
  face index; usage-based glyph subsetter for CJK. Existing base: `pdf_edit::fontfile::EmbeddedFont` —
  `parse` / `glyph_id` / `char_advance` / `write_type0(doc, used)` (`crates/pdf-edit/src/fontfile.rs`).
- **⚠ TRAP:** today the **whole font program embeds verbatim** (`fontfile.rs:36-37, 81`) and face index 0 is
  hardcoded (`fontfile.rs:62, 108`) — a fontdb-resolved `Songti.ttc` (~90 MB) would embed wholesale. TS-3's
  subsetter is a hard prerequisite for CJK documents; docspine must gate C-1 on it.
- **⚠ TRAP:** parse each font ONCE per document, accumulate `used` glyphs across all runs, `write_type0`
  ONCE at the end — per-run `insert_text(fontfile=)` re-embeds the program per call (PRD-NEXT
  `docs/PRD-NEXT.md:349-351`).
- **⚠ TRAP:** determinism becomes **per font environment** once system fonts resolve — same machine + fonts
  ⇒ same bytes; cross-machine bytes differ. CI byte-equality asserts must use bundled-font fixtures only.
- **Draw (exists today, reached via pdf-typeset re-exports):** `Shape`/`draw_rect`
  (`crates/pdf-edit/src/drawing.rs:118`, one-shot `:368`) for shading fills; per-edge borders = 4 `Line`
  ops, not `StrokeRect` (precedent `layout.rs:910-929`); `insert_image_jpeg`/`insert_image_rgb`
  (`crates/pdf-edit/src/image.rs:33` / `:81`; PNG → decode via pdf-image first; alpha composited over white
  — no `/SMask`, `crates/pdf-markdown/src/images.rs:209-222`).
- **Read-back gates (all exist, zero new plumbing):** `Page.get_text`
  (`python/pdfspine/document.py:1813-1843`); scorer `conformance/gt/score.py` — `content_scores` (`:152`),
  `order_score` (`:198`); raster + SSIM `conformance/gt/render_diff.py` — `ssim` (`:242-281`), `_near_blank`
  (`:463-469`); committed-refs precedent `--min-ssim 0.97` (pdfspine `.github/workflows/ci.yml:189-194`);
  `get_text_words` for coordinate asserts; `get_drawings` for border read-back
  (`crates/pdf-edit/src/drawings.rs`).
- **LibreOffice oracle (local-only, advisory):** `/Applications/LibreOffice.app/Contents/MacOS/soffice`
  (25.2.1.2, verified present) `--headless --convert-to pdf`; rasterize both PDFs with pdfspine
  `get_pixmap`, SSIM via `render_diff.py`; advisory band 0.80–0.90; **never in CI**.
- **Fixtures (LOCKED, no binary fixtures):** extend `python/tests/conftest.py::build_docx(document_xml, *,
  image=None)` (`conftest.py:174-188`) with optional part kwargs (`styles_xml=`, `numbering_xml=`,
  `theme_xml=`, `settings_xml=`) + export-oriented synthetic docs (multi-page, styled runs, sectPr chains,
  bordered/merged tables, inline+anchored images). python-docx exists only in the global anaconda python —
  authoring aid, never a test dep. Real-user `.docx` = gitignored local spot-check corpus, advisory only.
- **Geometry:** `doc-core/src/geom.rs:11-39` already has `twips_to_points`/`emu_to_points` — the mapper's
  only unit math; do not duplicate.
- **Dep pattern:** the ocrspine git-dep comment (`Cargo.toml:24-29`, quoted in §4) is the template; CI
  relies on `cargo fetch` of git deps, no sibling checkout (`.github/workflows/ci.yml:2-4`).

---

## 7. Phased plan (C-1..C-10)

Sequenced so early tasks don't block on the full styles resolver: **direct-formatting documents render
first** (C-1..C-4); C-2..C-4 are parse-side and can proceed in parallel with pdfspine Phase A. C-1 blocks on
**TS-2/TS-3/TS-4** landing in a pinned pdfspine rev.

| ID | Title | Effort | Blocks on |
|---|---|---|---|
| C-1 | doc-render scaffold + minimal direct-format flow + Python API | M | TS-2..TS-4 |
| C-2 | sectPr sections & per-section page geometry | M | — (parse); C-1 for render gate |
| C-3 | Run segments + content-loss bug fixes (br/sdt/fldSimple) | M | — |
| C-4 | Direct paragraph & run formatting completion | M | C-1, C-3 |
| C-5 | styles.xml + theme1.xml + effective-style resolver | **L** | C-4 (shared prop parsers) |
| C-6 | numbering.xml lists | M | C-5 (level pPr merge) |
| C-7 | Table fidelity (borders/shading/vAlign/margins/merges/pagination) | M–L | C-1; TS-4 table primitives |
| C-8 | Images: inline atoms + anchored overlays | M | C-1 |
| C-9 | Tabs, degradation polish, warnings completeness | S–M | C-1..C-8 |
| C-10 | Conformance & CI gates — full family stack | M | all |

- **C-1 · Scaffold + minimal flow (M).** New `crates/doc-render` + workspace git dep on pdf-typeset (§4
  pattern); map existing TextRun fields (font/size/bold/italic/underline/color — `model.rs:51-66`) onto
  engine styled runs; Word-default Letter page; `to_pdf`/`save_pdf` + `font_map` + warnings on the pyclass
  (§5); stubs. **Green:** synthetic 2-paragraph direct-format fixture → `to_pdf()` → pdfspine opens it; page
  count 1; token-F1 & order ≥ 0.99 (`score.py`); `_near_blank` false; existing Rust + pytest suites
  unchanged; `cargo check -p py-bindings` green on 3 OSes.
- **C-2 · Sections & page geometry (M).** Parse body-final + pPr-embedded `w:sectPr`
  (`pgSz`/`pgMar`/`orient`/`cols`); `Section` model + Word defaults; section break ⇒ page break; `cols>1`
  flattened + warning. **Green:** synthetic 3-section fixture (Letter portrait / A4 landscape / Letter)
  exports 3 pages; each `Page.rect` matches its sectPr-derived size within **0.5 pt**; first
  `get_text_words` word origin within 2 pt of (left-margin, top-margin); MultiColumnFlattened warning fires.
- **C-3 · Run segments + content-loss fixes (M).** Run content becomes `Vec<RunSegment>`
  (Text/Tab/Break{Line,Page,Column→Page}) — `w:br@w:type` finally read (`document.rs:210-232`); `w:sdt >
  w:sdtContent` and `w:fldSimple` become transparent containers (bonus bugs, §3); `export.rs` updated
  behavior-preserving (`to_text` still emits `\n`/`\t`). **Green:** `<w:br w:type="page"/>` fixture yields 2
  pages; sdt + fldSimple text present in both `to_text()` **and** PDF read-back; full existing test floor
  green (no text regressions).
- **C-4 · Direct formatting completion (M).** pPr spacing/ind/pBdr/shd + keep-flags; rPr
  strike/highlight/vertAlign/underline-kind/4-slot rFonts/szCs; refactor pPr/rPr walkers into reusable
  prop-struct parsers (needed again in C-5). **Green:** hanging-indent fixture — first-line word x0 =
  margin+firstLine ± 2 pt, wrapped-line x0 = margin+left ± 2 pt (`get_text_words`); spacing fixture
  deterministically moves a page break (page count 2 vs 1 assert); highlight/strike visible as ≥ N painted
  ops via `get_drawings`; CJK run renders with eastAsia font slot (span font name assert).
- **C-5 · styles.xml + theme + resolver (L — the long pole).** Walkers for styles.xml (id → props + basedOn
  + type + default) and theme1.xml fontScheme; `doc-core/src/style.rs` resolver (docDefaults → basedOn
  chain, cycle-safe → table-style overlay → direct); theme font/color indirection (`asciiTheme="minorHAnsi"`
  etc.). **Green:** all-styles fixture (zero direct formatting; Heading1 basedOn Normal; docDefaults 11 pt +
  minorHAnsi theme font): read-back span sizes match resolved values within 0.5 pt; mutating docDefaults
  size in the fixture changes rendered spans (proves cascade live); a `basedOn` cycle fixture terminates
  with a warning, no hang.
- **C-6 · numbering.xml lists (M).** Capture `w:numId` (bonus bug); numbering.xml walker
  (numFmt/lvlText/start/lvlJc/per-level ind); counter engine `doc-core/src/numbering.rs`; labels + hanging
  indents into engine list-label props. styleLink/numStyleLink indirection deferred + warning. **Green:**
  3-level fixture renders `1.` / `a.` / `i.` labels, read-back order ≥ 0.99 with labels interleaved
  correctly; restart-numbering fixture resets counters; label x0 < paragraph text x0 (gutter assert).
- **C-7 · Table fidelity (M–L).** Parse
  tblBorders/tcBorders/vAlign/tcMar/tblCellMar/tblW/pct-tcW/hRule/table-jc/tblInd/cantSplit; span-map
  flattening + border conflict resolution (§4); unsplittable-row pagination + RowTooTall warning. **Green:**
  bordered merged-cell fixture — token-F1 ≥ 0.99 in reading order; `get_drawings` edge count equals the
  conflict-resolved expectation (merged edges suppressed); every cell's words lie inside its grid rect ± 2
  pt; 30-row table paginates with rows moved whole; taller-than-page row emits RowTooTall.
  **Status: done.** Cell `vAlign` center/bottom now renders via the engine's `TableCell.v_align`
  (`VAnchor`, TS-11) — the earlier `CellVAlignIgnored` top-only degradation is gone.
- **C-8 · Images (M).** Placement model (inline vs anchored + offsets, §3g); inline atoms with EMU→pt
  extents; anchored overlays (no wrap + warning); VML style-attr size parse; EMF/WMF placeholder + warning.
  **Green:** inline PNG fixture — exactly 1 image XObject on page 1 (`extract_image_info`), placed rect
  matches extent within 1 pt; anchored fixture — image at posOffset within 1 pt + FloatingNoWrap warning;
  EMF fixture — placeholder drawn, UnsupportedImageFormat warned, no panic; raster non-blank.
- **C-9 · Tabs + degradation polish (S–M).** Default 0.5" interval tab advance (settings.xml
  `defaultTabStop` when present); audit every OUT-scope construct degrades with exactly one warning kind;
  custom tab stops = stretch (warn when present). **Green:** tab fixture — post-tab word x0 lands on the
  next 36 pt multiple ± 1 pt; `pytest.warns` catches exactly one warning per unique kind across a
  kitchen-sink degradation fixture.
- **C-10 · Conformance & CI — full family stack (M).** Wire the four-layer gate stack: (1) **CI-blocking
  read-back** — every export fixture: `Document.to_text()` vs pdfspine `Page.get_text()` scored with
  `score.py`, token-F1 & order ≥ 0.99; (2) **CI-blocking structural asserts** — page count, `Page.rect` ==
  sectPr size, `get_text_words` margin/indent tolerances, image survival via `extract_image_info`, non-blank
  raster (`render_diff.py _near_blank`); (3) **local-only advisory** LibreOffice oracle script (`soffice
  --headless --convert-to pdf`, both rasterized via `get_pixmap`, SSIM via `render_diff.py`, advisory band
  0.80–0.90, never in CI); (4) follow-up: committed `.ssimref` refs at `--min-ssim 0.97` per pdfspine
  `ci.yml:189-194`. **Green:** full fixture corpus passes (1)+(2) in CI on 3 OSes; oracle script runs
  locally and reports the band; README capability table + CLAUDE.md module map updated; workspace
  fmt/clippy/test + pytest floors green.

---

## 8. Risks & mitigations

1. **styles.xml inheritance correctness (top risk).** Word's cascade (docDefaults → style chain →
   table-style overlay → direct, with toggle-property XOR semantics for b/i and theme indirection) is
   subtle; a wrong merge order mis-renders *every* Word-authored file. *Mitigation:* pure resolver in
   doc-core with exhaustive unit fixtures per merge rule (C-5 gates prove the cascade is live, not just
   parsed); LibreOffice oracle as semantic cross-check; toggle-property semantics get dedicated tests; cycle
   guard mandatory.
2. **Line-metrics parity with Word.** Word's line height derives from font ascent/descent + grid/lineRule
   rules; the engine's metrics (TS-4 replaces the 0.8-baseline heuristic) will not match Word
   glyph-for-glyph, shifting page breaks on dense documents. *Mitigation:* structural gates assert geometry
   we control (margins, indents, page boxes) with tolerances, content gates assert order not position; SSIM
   stays advisory (0.80–0.90) until metrics stabilize; `lineRule="exact"` honored verbatim (no metric
   dependence); accept ±1-line break drift as in-tolerance for v1.
3. **Table flow complexity.** Merges × per-edge conflict rules × pagination interact combinatorially;
   pdfspine's engine has never done spans (`layout.rs:832-833`). *Mitigation:* docspine pre-resolves
   everything OOXML-specific (span map, final per-edge strokes) so the engine only measures/paints;
   property-style tests generate random merge patterns and assert grid invariants (cell rects tile the
   table, no orphan edges); intra-row splitting explicitly deferred.
4. **Engine-schedule coupling.** C-1/C-7/C-8 block on TS-2..TS-4 in another repo. *Mitigation:* pinned git
   rev per the locked dep pattern; C-2..C-4 (parse-side) are engine-independent and scheduled first in
   parallel; bump the rev deliberately, never track a branch.
5. **Font environment variance.** System-font resolution makes output machine-dependent; CJK docs on a
   font-poor CI runner substitute silently. *Mitigation:* CI fixtures restrict to bundled Liberation/Noto
   families (deterministic); substitution always warns; `font_map` gives users explicit control; document
   the per-environment determinism contract in README.
6. **`export.rs` regression from the run-segment refactor (C-3).** `to_text/to_markdown/to_html` and
   downstream RAG consumers depend on current text folding. *Mitigation:* segments serialize back to
   identical folded strings; the full existing test floor is a C-3 gate; `Paragraph.text()` keeps its
   signature.
