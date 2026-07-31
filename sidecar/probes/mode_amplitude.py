#!/usr/bin/env python3
"""Unit-3.13 — amplitude normalization + reduced mass, MEASURED on dexketoprofen.

Two things the animation got wrong and this probe grounds in numbers (rule #10):

1. The mode vector is unit-normalized over all 3N components, so the SAME scalar A
   gives wildly different physical motion for a delocalized mode (norm spread over
   ~100 components) vs a localized one (norm on 2 atoms). Fix: A = the MAXIMUM ATOMIC
   displacement (Å), i.e. divide the mode by max_j|v_j| where |v_j| is the norm of
   atom j's tri-vector (NOT the max single component — that is off by up to √3).

2. The real thermal (zero-point) amplitude is A0 = sqrt(hbar / (2 μ ω)), with the
   mode's effective reduced mass μ = 1 / Σ_i(|v_i|²/m_i). Masses are the 2nd column
   of `.hess $atoms` — VERIFY that column is masses (C≈12.011, H≈1.008, O≈15.999)
   before trusting it.

Run: sidecar/.venv/bin/python sidecar/probes/mode_amplitude.py
"""
from __future__ import annotations

import re
from pathlib import Path

import numpy as np

BOHR_PER_ANGSTROM = 1.8897259886
HBAR = 1.054_571_817e-34  # J·s
C_CM = 2.997_924_58e10    # cm/s
AMU = 1.660_539_066e-27   # kg
DEX = "b0d1db94-8012-47aa-9d2a-bb5924abca13"
ROOT = Path.home() / ".local/share/orcastudio/jobs"


def read_hess(path: Path):
    L = path.read_text().splitlines()
    syms, masses, coords, freqs, M = [], [], [], None, None
    for i, ln in enumerate(L):
        s = ln.strip()
        if s == "$atoms":
            n = int(L[i + 1])
            for k in range(n):
                p = L[i + 2 + k].split()
                syms.append(p[0])
                masses.append(float(p[1]))  # 2nd column — CLAIMED to be mass
                coords.append([float(p[2]), float(p[3]), float(p[4])])
        if s == "$vibrational_frequencies":
            n = int(L[i + 1])
            freqs = np.array([float(L[i + 2 + k].split()[-1]) for k in range(n)])
        if s == "$normal_modes":
            dim = int(L[i + 1].split()[0])
            M = np.zeros((dim, dim))
            j, cols = i + 2, []
            while j < len(L):
                t = L[j].split()
                if not t:
                    j += 1
                    continue
                if all(x.lstrip("-").isdigit() for x in t) and len(t) <= dim and "." not in L[j]:
                    cols = [int(x) for x in t]
                    j += 1
                    continue
                try:
                    r = int(t[0])
                except ValueError:
                    break
                for c, v in zip(cols, t[1:]):
                    M[r][c] = float(v)
                j += 1
    return (np.array(syms), np.array(masses), np.array(coords) / BOHR_PER_ANGSTROM, freqs, M)


def main() -> None:
    syms, masses, xyz, freqs, M = read_hess(ROOT / DEX / "input.hess")
    N = len(syms)

    print("=" * 74)
    print("1. VERIFY the mass column (.hess $atoms col 2) — rule #10")
    print("=" * 74)
    for el, exp in [("C", 12.011), ("H", 1.008), ("O", 15.999)]:
        vals = masses[syms == el]
        if len(vals):
            ok = abs(vals[0] - exp) < 0.02
            print(f"  {el}: file={vals[0]:.4f}  expected≈{exp}  {'OK' if ok else '*** MISMATCH ***'}")
    print(f"  (unique masses present: {sorted(set(np.round(masses, 3)))})")

    # locate the modes of interest by frequency
    def mode_near(cm):
        return int(np.argmin(np.abs(freqs - cm)))

    k84 = mode_near(1752.7)   # C=O acid stretch (localized)
    k7 = mode_near(21.4)      # low delocalized
    kCH = mode_near(3188.0)   # a C–H stretch (very localized)
    print(f"\n  mode indices: C=O≈{freqs[k84]:.1f} → #{k84}; "
          f"low≈{freqs[k7]:.1f} → #{k7}; C–H≈{freqs[kCH]:.1f} → #{kCH}")

    print("\n" + "=" * 74)
    print("2. NORMALIZATION: max atomic displacement vs max component")
    print("=" * 74)
    for k, name in [(k84, "C=O #%d" % k84), (k7, "low #%d" % k7), (kCH, "C–H #%d" % kCH)]:
        v = M[:, k].reshape(N, 3)
        atom_norms = np.linalg.norm(v, axis=1)
        max_atom = atom_norms.max()
        max_comp = np.abs(v).max()
        print(f"  {name:10s}: max|v_atom|={max_atom:.4f}  max|v_component|={max_comp:.4f}  "
              f"ratio={max_atom/max_comp:.4f} (1..√3); Σ|v|²={ (v**2).sum():.4f}")

    print("\n" + "=" * 74)
    print("3. NEW normalization d_i = A·v_i/max_j|v_j|: is max atomic move == A?")
    print("=" * 74)
    A = 0.25
    for k, name in [(k84, "C=O"), (k7, "low"), (kCH, "C–H")]:
        v = M[:, k].reshape(N, 3)
        vmax = np.linalg.norm(v, axis=1).max()
        d = v / vmax * A
        moved = np.linalg.norm(d, axis=1).max()
        print(f"  {name:5s} A={A}: max atomic move={moved:.6f} Å  (== A? {abs(moved-A)<1e-9})")

    print("\n" + "=" * 74)
    print("4. C=O #%d bond-length excursion + min interatomic distance vs default A" % k84)
    print("=" * 74)
    from itertools import combinations
    v = M[:, k84].reshape(N, 3)
    vmax = np.linalg.norm(v, axis=1).max()
    unit = v / vmax  # max atomic norm == 1 → displacement per unit A
    c16 = int(np.argmax(np.linalg.norm(v, axis=1)))  # the max-displacement atom = C16
    # its bonded partners: nearest O (~1.2, the C=O) and nearest C (~1.5, the C–C)
    def nearest(el, i):
        cand = [(np.linalg.norm(xyz[i] - xyz[j]), j) for j in range(N) if syms[j] == el and j != i]
        return min(cand)
    oO = nearest("O", c16)
    cC = nearest("C", c16)
    print(f"  max-displacement atom = index {c16} ({syms[c16]}); |v|={vmax:.4f} "
          f"(→ 0.360 Å at old A=0.5, matches the report)")

    def pair_len(scale, A_, i, j):
        return np.linalg.norm((xyz[i] + scale * A_ * unit[i]) - (xyz[j] + scale * A_ * unit[j]))

    def min_dist(scale, A_):
        P = xyz + scale * A_ * unit
        return min(np.linalg.norm(P[a] - P[b]) for a, b in combinations(range(N), 2))

    for A_ in (0.30, 0.25, 0.20, 0.15):
        scales = np.linspace(-1, 1, 41)
        co_lo = min(pair_len(s, A_, c16, oO[1]) for s in scales)
        cc_lo = min(pair_len(s, A_, c16, cC[1]) for s in scales)
        gmin = min(min_dist(s, A_) for s in scales)
        tag = "  <- default candidate" if A_ == 0.25 else ""
        print(f"  A={A_:.2f}: C=O eq={oO[0]:.3f}→min {co_lo:.3f}  C–C eq={cC[0]:.3f}→min {cc_lo:.3f}  "
              f"global min interatomic={gmin:.3f}{tag}")
    print("  (old A=0.5 unnormalized drove C=O to 0.63 Å; see the report.)")

    print("\n" + "=" * 74)
    print("5. PHYSICAL (zero-point) amplitude A0 = sqrt(hbar/(2 μ ω)), μ=1/Σ(|v_i|²/m_i)")
    print("=" * 74)
    for k, name in [(k84, "C=O"), (k7, "low"), (kCH, "C–H")]:
        v = M[:, k].reshape(N, 3)
        m_kg = masses * AMU
        mu = 1.0 / np.sum(np.sum(v**2, axis=1) / m_kg)  # kg
        omega = 2 * np.pi * C_CM * freqs[k]  # rad/s
        a0_m = np.sqrt(HBAR / (2 * mu * omega))
        a0_ang = a0_m * 1e10
        print(f"  {name:5s} ν={freqs[k]:7.1f} cm⁻¹  μ={mu/AMU:6.3f} amu  A0={a0_ang:.4f} Å")
    print("  (expect C=O #84 ≈ 0.04 Å — real vibrations are ~0.04 Å; we DRAW 0.25 for visibility.)")


if __name__ == "__main__":
    main()
