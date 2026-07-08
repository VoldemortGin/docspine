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
