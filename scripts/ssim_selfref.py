#!/usr/bin/env python3
"""自渲染 SSIM 基线门 —— **CI 可阻断**(PRD-PDF-EXPORT C-10 第 (4) 层 follow-up)。

与 local-only 的 ``lo_oracle_ssim.py``(对 LibreOffice 比对,永不进 CI)不同,本门
**不依赖任何外部工具**,只自比对:提交一批**确定性**基线 PDF(``conformance/ssimref/``),
CI 里重渲染同一批 fixture,把「新渲染」与「提交基线」双双用 ``pdfspine`` 栅格化后逐页算
SSIM,低于 ``--min-ssim``(缺省 0.97)即失败。

为什么这样设计:

- **确定性渲染**:``DOCSPINE_DETERMINISTIC_FONTS=1`` 让 ``to_pdf`` 只用引擎内置
  Liberation/Noto 兜底字体(不扫系统字体),输出跨机器逐字节一致——引擎 rev 不变时
  新渲染与基线**字节相同**,SSIM=1.0。
- **栅格版本无关**:比对时**两侧都**由当前 runner 的 ``pdfspine`` 栅格化,所以 pdfspine
  版本差异不影响判定(两边同一套栅格器)。真正的版面回归会让新渲染偏离基线、SSIM 掉下去;
  引擎 rev 善意微调只带来小漂移、仍在 0.97 容差内。
- **与 pptspine 同构**:一份自包含脚本(纯 Python SSIM),ppt-render 落地后可原样复用。

用法(仓库根)::

    # 重新生成基线(改了渲染、或首次落地时;人工 review PDF 后提交)
    DOCSPINE_DETERMINISTIC_FONTS=1 .venv/bin/python scripts/ssim_selfref.py --update

    # 校验(CI 跑这个;缺省 --min-ssim 0.97)
    DOCSPINE_DETERMINISTIC_FONTS=1 .venv/bin/python scripts/ssim_selfref.py

基线目录 ``conformance/ssimref/`` 已提交进仓(小体积确定性 PDF)。
"""

from __future__ import annotations

import argparse
import sys
import warnings
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]
REF_DIR = REPO_ROOT / "conformance" / "ssimref"
DEFAULT_MIN_SSIM = 0.97
# 栅格 DPI 与降采样上限(比对分辨率;两侧同参,只影响灵敏度)。
DPI = 100.0
MAX_DIM = 256


def _fixture_matrix(conftest) -> dict[str, bytes]:
    """导出 fixture 矩阵 -> ``名字 -> docx 字节``(取每个 fixture 的主变体,含 C-8 收口的
    锚定图与段落边框/底纹)。"""

    def fx(name: str) -> bytes:
        return getattr(conftest, name).__wrapped__()

    return {
        "minimal_paragraph": fx("minimal_docx_bytes"),
        "two_section_geometry": fx("pdf_export_docx_bytes"),
        "sections_letter_a4": fx("sections_docx_bytes"),
        "content_loss_br_sdt": fx("content_loss_docx_bytes"),
        "merged_cell_table": fx("simple_table_docx_bytes"),
        "emf_placeholder": fx("emf_docx_bytes"),
        "anchored_image": fx("anchored_image_docx_bytes"),
        "para_box": fx("para_box_docx_bytes"),
    }


def _render(docspine, docx_bytes: bytes) -> bytes:
    """确定性渲染一份 docx -> PDF 字节(降级告警静默)。"""
    with warnings.catch_warnings():
        warnings.simplefilter("ignore")
        return docspine.open_bytes(docx_bytes).to_pdf()


def _gray_pages(pdfspine, pdf_bytes: bytes) -> list[tuple[int, int, bytes]]:
    """逐页栅格化 + 灰度降采样(到 ``MAX_DIM`` 上限,均值池化)-> ``(w, h, gray)``。"""
    doc = pdfspine.open(stream=pdf_bytes, filetype="pdf")
    zoom = DPI / 72.0
    out: list[tuple[int, int, bytes]] = []
    for page in doc:
        pm = page.get_pixmap(matrix=pdfspine.Matrix(zoom, zoom))
        out.append(_downsample_gray(pm.width, pm.height, pm.n, bytes(pm.samples), MAX_DIM))
    return out


def _downsample_gray(w: int, h: int, n: int, samples: bytes, max_dim: int) -> tuple[int, int, bytes]:
    """把 ``n`` 通道像素均值池化成灰度,并把长边降到 ``max_dim`` 以内。"""
    scale = max(1, (max(w, h) + max_dim - 1) // max_dim)
    ow, oh = (w + scale - 1) // scale, (h + scale - 1) // scale
    out = bytearray(ow * oh)
    for oy in range(oh):
        y0, y1 = oy * scale, min((oy + 1) * scale, h)
        for ox in range(ow):
            x0, x1 = ox * scale, min((ox + 1) * scale, w)
            acc = cnt = 0
            for yy in range(y0, y1):
                base = yy * w
                for xx in range(x0, x1):
                    p = (base + xx) * n
                    if n >= 3:
                        g = (samples[p] * 299 + samples[p + 1] * 587 + samples[p + 2] * 114) // 1000
                    else:
                        g = samples[p]
                    acc += g
                    cnt += 1
            out[oy * ow + ox] = acc // cnt if cnt else 0
    return ow, oh, bytes(out)


def _ssim(a: bytes, b: bytes, w: int, h: int, block: int = 16) -> float:
    """非重叠块 SSIM 均值(纯 Python,无 numpy)。两张同尺寸灰度图。"""
    c1 = (0.01 * 255) ** 2
    c2 = (0.03 * 255) ** 2
    total = 0.0
    nblocks = 0
    for by in range(0, h, block):
        for bx in range(0, w, block):
            xs: list[int] = []
            ys: list[int] = []
            for yy in range(by, min(by + block, h)):
                row = yy * w
                for xx in range(bx, min(bx + block, w)):
                    xs.append(a[row + xx])
                    ys.append(b[row + xx])
            m = len(xs)
            mx = sum(xs) / m
            my = sum(ys) / m
            vx = sum((v - mx) ** 2 for v in xs) / m
            vy = sum((v - my) ** 2 for v in ys) / m
            cov = sum((xs[i] - mx) * (ys[i] - my) for i in range(m)) / m
            s = ((2 * mx * my + c1) * (2 * cov + c2)) / ((mx * mx + my * my + c1) * (vx + vy + c2))
            total += s
            nblocks += 1
    return total / nblocks if nblocks else 1.0


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--update", action="store_true", help="重新生成并覆盖提交基线")
    parser.add_argument("--min-ssim", type=float, default=DEFAULT_MIN_SSIM)
    args = parser.parse_args(argv[1:])

    try:
        import pdfspine
    except ImportError:
        raise SystemExit("需要 pdfspine(读回栅格器):pip install 'pdfspine>=0.3'")
    import docspine

    conftest = _load_conftest()
    matrix = _fixture_matrix(conftest)

    if args.update:
        REF_DIR.mkdir(parents=True, exist_ok=True)
        for name, docx_bytes in matrix.items():
            (REF_DIR / f"{name}.pdf").write_bytes(_render(docspine, docx_bytes))
        print(f"wrote {len(matrix)} baseline PDF(s) to {REF_DIR.relative_to(REPO_ROOT)}/")
        return 0

    width = max(len(n) for n in matrix) + 2
    print(f"{'fixture':<{width}}{'min SSIM':>9}  status  (gate {args.min_ssim:.2f})")
    failures = 0
    for name, docx_bytes in matrix.items():
        ref_path = REF_DIR / f"{name}.pdf"
        if not ref_path.exists():
            print(f"{name:<{width}}{'--':>9}  MISSING baseline (run --update)")
            failures += 1
            continue
        fresh = _gray_pages(pdfspine, _render(docspine, docx_bytes))
        ref = _gray_pages(pdfspine, ref_path.read_bytes())
        if len(fresh) != len(ref):
            print(f"{name:<{width}}{'--':>9}  FAIL page count fresh={len(fresh)} ref={len(ref)}")
            failures += 1
            continue
        page_scores = []
        for (aw, ah, apx), (bw, bh, bpx) in zip(fresh, ref):
            if (aw, ah) != (bw, bh):
                page_scores.append(0.0)
            else:
                page_scores.append(_ssim(apx, bpx, aw, ah))
        worst = min(page_scores) if page_scores else 1.0
        ok = worst >= args.min_ssim
        failures += 0 if ok else 1
        print(f"{name:<{width}}{worst:9.4f}  {'ok' if ok else 'FAIL'}")

    if failures:
        print(f"\n{failures} fixture(s) below {args.min_ssim:.2f} — layout regression vs committed baseline.")
        return 1
    print(f"\nall fixtures ≥ {args.min_ssim:.2f} vs committed baseline.")
    return 0


def _load_conftest():
    import importlib.util

    path = REPO_ROOT / "python" / "tests" / "conftest.py"
    spec = importlib.util.spec_from_file_location("conftest", path)
    if spec is None or spec.loader is None:
        raise SystemExit(f"cannot import conftest from {path}")
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
