# pdfspine v0.11.2 / ocrspine 041958a dependency alignment — 2026-10-02

Behavior-preserving bump of docspine's three git dependency revs. No Rust or
Python source changed; no new pdfspine capability (paragraph borders,
super/subscript, character spacing, ...) is wired in. That migration is
separate work (see pdfspine `docs/typeset-paragraph-borders.md`).

## Rev changes

| Dependency | Before | After |
| --- | --- | --- |
| `ocrspine` | `732975f0233cd6500edfbbb82bc06c2332369871` | `041958aa6f8d70d3957e8f9e27896cf0cbc42511` (fixes the reading-order sort panic; same rev pdfspine v0.11.1 pins) |
| `pdf-typeset` | `f1f6ab4208876b0ba867edd76cc4e5da7ad8add2` (v0.8.0) | `78a64d6e252ab739fcbad66c0d7f5328a080d667` (v0.11.2) |
| `pdf-fonts` | `f1f6ab4208876b0ba867edd76cc4e5da7ad8add2` (v0.8.0) | `78a64d6e252ab739fcbad66c0d7f5328a080d667` (v0.11.2) |

The v0.11.2 hash was checked with `git -C ../pdfspine rev-parse v0.11.2^{commit}`
and matches. `Cargo.lock` was updated with `cargo update -p ... --precise`;
all six pdfspine crates (`pdf-core`, `pdf-edit`, `pdf-fonts`, `pdf-image`,
`pdf-text`, `pdf-typeset`) resolve to `0.11.2` at the single commit above, and
`ocrspine` to the new rev. The only other lock change is the new `sha2`
dependency edge of a pdfspine crate (`sha2 0.10.9` was already locked). No other
crate versions moved.

## Validation

On macOS arm64, Rust 1.96, Python 3.12, installed `pdfspine==0.11.2`,
`ocrspine-models==0.0.3`, maturin 1.15.0, pytest 9.1.1:

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | passed, no source change needed |
| `cargo test -p doc-core -p doc-parse -p doc-ocr -p doc-render --all-features --locked` | 115 passed (core 39, OCR geometry 4, parser 40, render 32), same as v0.8.0 |
| `cargo check -p py-bindings --features ocr,legacy-doc,pdf --locked` | passed |
| `maturin develop --release --locked` | built and installed |
| `pytest python/tests -q -ra` (default layout) | 59 passed, 4 skipped |
| `pytest python/tests/test_ocr.py` with sibling fixture reachable | 5 passed |
| Deterministic self-reference SSIM, gate 0.97 | all eight fixtures 1.0000 |

SSIM, per fixture (baselines not regenerated): minimal_paragraph 1.0000,
two_section_geometry 1.0000, sections_letter_a4 1.0000, content_loss_br_sdt
1.0000, merged_cell_table 1.0000, emf_placeholder 1.0000, anchored_image 1.0000,
para_box 1.0000.

## OCR output

`test_ocr.py` reads `ocrspine/tests/fixtures/ocr_sample.png` from a sibling
checkout located at `<docspine root>/../ocrspine`. The validation ran in a
worktree under `/Volumes/ExternalSSD/wt/`, where that sibling does not exist, so
4 OCR tests skipped in the default run. They were re-run with a temporary
symlink to the real sibling checkout (removed afterwards) and all 5 passed; the
three reference lines are still found, so no reference line was changed. Output
with the new rev (text, confidence): `pdfspine OCR test 2026` 83.0,
`纯Rust实现的PDF文字识别` 88.4, `PaddleOCR via tract` 88.5. The previous-rev
raw output was not captured, so the comparison is limited to the test's
reference-line assertions; the ocrspine `e810a9c` mid-gray fill change did not
break any of them.

## Limits

One host/Python combination only; not the Linux/macOS/Windows and Python
3.12-3.14 CI matrix, not a wheel install check, not a package upload. SSIM is the
self-reference gate against committed baselines (it detects change from the
previous docspine output, not fidelity against Word/LibreOffice); the
LibreOffice advisory comparison was not rerun. The Python reader installed was
`pdfspine==0.11.2` (allowed range `>=0.8,<0.12`).
