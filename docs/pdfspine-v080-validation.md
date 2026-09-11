# pdfspine v0.8.0 dependency migration — 2026-09-10

`doc-render` now uses the supported pdfspine v0.8.0 git source:
`https://github.com/VoldemortGin/pdfspine`, commit
`f1f6ab4208876b0ba867edd76cc4e5da7ad8add2`. Both `pdf-typeset` and
the test-only `pdf-fonts` dependency are declared in the workspace; the latter
inherits that declaration in `doc-render`. Cargo.lock resolves all six PDF
crates (`core`, `edit`, `fonts`, `image`, `text`, `typeset`) to this single
commit/version. No sibling source path or branch dependency is used.

The previous production PDF source was `509a932e92f6d804c2b5345040715d7fbf3d6326`;
the test font source was separately `93214453167535c424079470aefb13773e79d717`.
All non-PDF package identities in Cargo.lock remain unchanged, including
ocrspine at `732975f0233cd6500edfbbb82bc06c2332369871`. Existing features and
docspine's version metadata (`0.0.1`) remain unchanged. No Rust API adaptation
was necessary.

## Validation

On macOS 26.5.1 arm64, Rust 1.96.0, Python 3.12.11:

| Check | Result |
| --- | --- |
| `cargo fmt --all --check` | passed |
| `cargo clippy --workspace --all-targets --all-features --locked -- -D warnings` | passed |
| `cargo test -p doc-core -p doc-parse -p doc-ocr -p doc-render --all-features --locked` | 115 passed |
| `cargo check -p py-bindings --features ocr,legacy-doc,pdf --locked` | passed |
| `maturin build --release --locked` with existing pyproject features | wheel built |
| Full `python -m pytest python/tests -q -ra`, using installed wheel | 63 passed, no skips |
| `uv pip check` | passed |
| Deterministic self-reference SSIM, unchanged gate 0.97 | all eight fixtures 1.0000 |

The Rust count comprises core 39, OCR geometry 4, parser 40 and render 32.
Python checks exercise parsing, OCR, content/section geometry and the actual
`to_pdf()` / `save_pdf()` export/read-back path. The wheel was installed into a
new external environment, not with `maturin develop`. All five installed
`docspine/` payload files match the built wheel byte-for-byte; imports resolve
to that environment's site-packages, including `_core.abi3.so`.

Wheel: `docspine-0.0.1-cp311-abi3-macosx_11_0_arm64.whl`.
SHA-256: `cf6bab86e529785fd606ba81393a9e8cd4fe9c51f1fca6969748e860e6e675f5`.
This is a local candidate build, not a newly published docspine release.
The reader is installed PyPI `pdfspine==0.8.0`; model data is
`ocrspine-models==0.0.3`, with maturin 1.15.0 and pytest 9.1.1.

The unchanged self-reference PDFs cover minimal paragraphs, two-section
geometry, Letter/A4 sections, breaks/SDT content, merged tables, EMF fallback,
anchored images and paragraph boxes. No baseline was regenerated.

LibreOffice 26.8.0.3 was also run as the existing local advisory oracle at
96 dpi. All seven fixtures had matching page counts:

| Fixture | SSIM vs LibreOffice |
| --- | ---: |
| minimal_paragraph | 0.9703 |
| two_section_geometry | 0.9820 |
| sections_letter_a4 | 0.9976 |
| tracked_revisions | 0.9934 |
| content_loss_br_sdt | 0.9959 |
| merged_cell_table | 0.9979 |
| emf_placeholder | 0.9904 |

These are candidate-only advisory measurements, not evidence of improvement
over the previous engine. The SSIM helper was read from the pinned pdfspine
git checkout (`conformance/gt/render_diff.py`), without modifying that repo.

## Reproduction and limits

From this repository, with Python 3.12 and the pinned Rust toolchain:

```bash
uv venv --python 3.12 /tmp/docspine-validation-env
uv pip install --python /tmp/docspine-validation-env/bin/python \
  maturin==1.15.0 pytest==9.1.1 pdfspine==0.8.0 ocrspine-models==0.0.3
export CARGO_TARGET_DIR=/tmp/docspine-validation-target
export PYO3_PYTHON=/tmp/docspine-validation-env/bin/python
/tmp/docspine-validation-env/bin/maturin build --release --locked \
  --out /tmp/docspine-validation-wheels
uv pip install --python /tmp/docspine-validation-env/bin/python \
  /tmp/docspine-validation-wheels/*.whl
/tmp/docspine-validation-env/bin/python -m pytest python/tests -q -ra
DOCSPINE_DETERMINISTIC_FONTS=1 /tmp/docspine-validation-env/bin/python \
  scripts/ssim_selfref.py --min-ssim 0.97
```

Installation/build preparation may fetch official git/package sources; document
processing is local. The existing OCR tests still read an optional sibling
`ocrspine/tests/fixtures/ocr_sample.png` fixture, which was present for this run.
They skip the corresponding cases if it is absent; this fixture is not a
production/build dependency. README's separate editable-development recipe
was checked for maturin `--uv` installer support, not executed in this run.

This validates one host/Python combination, not the full Linux/macOS/Windows
and Python 3.12–3.14 CI matrix, a package upload, or arbitrary Word-layout
equivalence. Known export degradations remain as documented in PRD-PDF-EXPORT.

Local evidence is outside git under `/Volumes/ExternalSSD/tmp/`:
`docspine-v080-{rust-gate,wheel-build,python-tests,ssim,lo}.log`,
`docspine-v080-dependency-sources.json`, `docspine-v080-wheel-evidence.json`,
`docspine-v080-oracle-helper.json` and `docspine-v080-lo/`.
The shared family status is handed off separately; no family-document copy was
edited as part of this migration.
