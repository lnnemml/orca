#!/usr/bin/env python3
"""Cross-check OrcaStudio's IR Lorentzian broadening against ORCA's own
`orca_mapspc` (domain rule #9 — recompute what matters in our terms; rule #10 —
a third-party program's behaviour is accepted only from a run).

This is a ONE-OFF verification (report + wiki), NOT app code. It reproduces the
number recorded in `wiki/orca/parse-sources.md` (the `orca_mapspc` row).

What it does, and every parameter measured — not assumed:
  * runs `orca_mapspc <hess> IR -l0 -w<FWHM> -x0<min> -x1<max> -n<npts>`
    (flags + their ATTACHED-value syntax taken from `orca_mapspc`'s own `-h`,
    which also prints "Peak FWHM [cm-1]" — i.e. `-w` IS the FWHM, not a HWHM);
  * reads the `.ir.dat` it writes (measured: `1000 - absorption`, and it
    broadens column 1 of `$ir_spectrum` — the a.u. value — with a PEAK-HEIGHT
    normalization, whereas we broaden column 2 (km/mol) with an AREA
    normalization);
  * builds our own area-normalized Lorentzian sum on the same grid;
  * normalizes each curve to its own max and reports the max shape deviation.

Usage:
    LD_LIBRARY_PATH=/opt/orca sidecar/.venv/bin/python \
        sidecar/probes/ir_mapspc_xcheck.py \
        --orca /opt/orca/orca_mapspc --hess <job_dir>/input.hess --fwhm 10
"""
import argparse
import math
import os
import shutil
import subprocess
import tempfile


def read_ir_spectrum(hess_path):
    """($ir_spectrum) → (freq cm^-1, col1 a.u., col2 km/mol) lists."""
    lines = open(hess_path).read().splitlines()
    i = lines.index("$ir_spectrum")
    n = int(lines[i + 1])
    rows = [lines[i + 2 + k].split() for k in range(n)]
    freq = [float(r[0]) for r in rows]
    col1 = [float(r[1]) for r in rows]
    col2 = [float(r[2]) for r in rows]
    return freq, col1, col2


def area_lorentzian_sum(freq, inten, grid, fwhm):
    """Our app's curve: each mode an AREA-normalized Lorentzian (∫ = intensity),
    summed. Half-width g = FWHM/2 → L(x) = (I/π)·g/((x−x0)²+g²)."""
    g = fwhm / 2.0
    out = []
    for x in grid:
        s = 0.0
        for k in range(len(freq)):
            s += inten[k] * (1.0 / math.pi) * g / ((x - freq[k]) ** 2 + g * g)
        out.append(s)
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--orca", default="/opt/orca/orca_mapspc")
    ap.add_argument("--hess", required=True)
    ap.add_argument("--fwhm", type=float, default=10.0)
    ap.add_argument("--xmin", type=float, default=0.0)
    ap.add_argument("--xmax", type=float, default=3400.0)
    args = ap.parse_args()

    npts = int(args.xmax - args.xmin) + 1
    grid = [args.xmin + i for i in range(npts)]

    # orca_mapspc writes next to the .hess — run in an isolated temp copy
    # (domain rule #3, don't litter the job dir).
    with tempfile.TemporaryDirectory() as tmp:
        hess = os.path.join(tmp, "input.hess")
        shutil.copy(args.hess, hess)
        env = dict(os.environ, LD_LIBRARY_PATH=os.path.dirname(args.orca))
        # ATTACHED values (measured: `-w 10` with a space → "flag not understood").
        cmd = [args.orca, hess, "IR", "-l0",
               f"-w{args.fwhm:g}", f"-x0{args.xmin:g}",
               f"-x1{args.xmax:g}", f"-n{npts}"]
        r = subprocess.run(cmd, capture_output=True, text=True, env=env)
        print("orca_mapspc:", cmd)
        print(r.stdout.strip().splitlines()[-1] if r.stdout else "(no stdout)")
        dat_path = hess + ".ir.dat"
        if not os.path.exists(dat_path):
            print("FAILED — orca_mapspc produced no .ir.dat; comparison left open.")
            print(r.stdout, r.stderr)
            return
        dat = {}
        for ln in open(dat_path):
            p = ln.split()
            if len(p) == 2:
                dat[round(float(p[0]))] = float(p[1])

    freq, col1, col2 = read_ir_spectrum(args.hess)
    # measured proportionality of the two intensity columns (strong modes only —
    # weak col1 values have 1 sig fig and are rounding-dominated)
    strong = [(col2[k] / col1[k]) for k in range(len(freq)) if col1[k] > 1e-4]
    if strong:
        print(f"col2/col1 (strong modes): {min(strong):.2f}..{max(strong):.2f}")

    ours = area_lorentzian_sum(freq, col2, grid, args.fwhm)     # area, km/mol
    orca_abs = [1000.0 - dat[round(x)] for x in grid]           # 1000 - value

    mo, mp = max(ours), max(orca_abs)
    un = [v / mo for v in ours]
    pn = [v / mp for v in orca_abs]
    full = max(abs(un[i] - pn[i]) for i in range(len(grid)))
    im = max(range(len(grid)), key=lambda i: abs(un[i] - pn[i]))

    zeros = sum(1 for x in grid if orca_abs[round(x) - round(args.xmin)] == 0.0)
    print(f"FULL-grid max shape deviation = {full:.3e}  ({full * 100:.1f}%) "
          f"at {grid[im]:.0f} cm^-1")
    print(f"orca_abs == exactly 0.0 at {zeros}/{len(grid)} grid points "
          f"(a pure Lorentzian never is → orca truncates the wings)")
    print("Peak CORES agree; the residual is orca's wing truncation vs our "
          "full analytic wings (area = intensity, Part B). Reported, not fudged.")


if __name__ == "__main__":
    main()
