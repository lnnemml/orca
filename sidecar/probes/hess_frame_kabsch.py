#!/usr/bin/env python3
"""Unit-3.12 GATE — does the `.hess $atoms` reframe contain a ROTATION?

`.hess $atoms` is the Freq geometry rigidly reframed vs the `.property.txt` final
geometry (the reference the reader already accepts). The reader's post-condition
compares pairwise DISTANCES, which are rotation-invariant BY CONSTRUCTION — so it
can never say whether the reframe is a pure translation or also a rotation. That
matters because `$normal_modes` are displacement vectors in the `.hess` frame: if
there is a rotation and we add those vectors to the reference-frame scene geometry,
the animation is smooth, symmetric and WRONG (same class as mass-weighted modes).

This probe answers it with Kabsch superposition (NO correspondence search — the
correspondence is already given by index), in index order, on the real jobs:

    R           the rotation mapping .hess -> reference (all 9 elements)
    max|R-I|    element-wise max deviation from identity
    t           translation vector (Angstrom)
    RMSD        after superposition

Run:  sidecar/.venv/bin/python sidecar/probes/hess_frame_kabsch.py
(paths below are the author's real job dirs — terminal run, not app code.)
"""
from __future__ import annotations

import re
from pathlib import Path

import numpy as np

BOHR_PER_ANGSTROM = 1.8897259886

JOBS = {
    "ethane-min (8)": "d7992449-10e3-47c9-9a16-8e22d60b955d",
    "saddle (19)": "99e805f5-1892-4ebb-9cb8-181cf7fc5fee",
    "dexketoprofen (33)": "b0d1db94-8012-47aa-9d2a-bb5924abca13",
}
JOBS_ROOT = Path.home() / ".local/share/orcastudio/jobs"


def hess_atoms_angstrom(path: Path):
    """(symbols, coords[N,3] in Angstrom) from `.hess $atoms`: 'SYM mass x y z' (Bohr)."""
    lines = path.read_text().splitlines()
    for i, ln in enumerate(lines):
        if ln.strip() == "$atoms":
            n = int(lines[i + 1].strip())
            syms, coords = [], []
            for row in lines[i + 2 : i + 2 + n]:
                p = row.split()
                syms.append(p[0])
                coords.append([float(p[2]), float(p[3]), float(p[4])])
            return syms, np.array(coords) / BOHR_PER_ANGSTROM
    raise ValueError(f"no $atoms in {path}")


def property_final_geometry_angstrom(path: Path):
    """(symbols, coords[N,3] in Angstrom) of the LAST $Geometry block (Bohr in file)."""
    text = path.read_text()
    last = None
    for m in re.finditer(r"\$Geometry\b(.*?)\$End", text, re.DOTALL):
        cm = re.search(r"&CartesianCoordinates[^\n]*\n(.*)", m.group(1), re.DOTALL)
        if not cm:
            continue
        syms, coords = [], []
        for row in cm.group(1).splitlines():
            p = row.split()
            if len(p) < 4:
                continue
            sym = re.sub(r"\(\d+\)$", "", p[0])
            try:
                xyz = [float(p[1]), float(p[2]), float(p[3])]
            except ValueError:
                continue
            syms.append(sym)
            coords.append(xyz)
        if syms:
            last = (syms, np.array(coords) / BOHR_PER_ANGSTROM)
    if last is None:
        raise ValueError(f"no $Geometry with coords in {path}")
    return last


def hess_normal_modes(path: Path) -> np.ndarray:
    """The 3N×3N `$normal_modes` matrix (column k = mode k's Cartesian displacement)."""
    L = path.read_text().splitlines()
    for i, ln in enumerate(L):
        if ln.strip() == "$normal_modes":
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
            return M
    raise ValueError(f"no $normal_modes in {path}")


def min_pair_distance(C: np.ndarray) -> float:
    """Smallest interatomic distance (Angstrom) — plain O(N²), N is small."""
    n = len(C)
    best = np.inf
    for a in range(n):
        for b in range(a + 1, n):
            best = min(best, float(np.linalg.norm(C[a] - C[b])))
    return best


def amplitude_floor_scan() -> None:
    """Part-B calibration: at the pltvib multiplier A=2.0, how close do the animated
    atoms get? x(t)=x_eq + A·sin·v; the extreme is sin=±1. Reports, per job, how many
    animatable modes drop below 0.3/0.5/0.7 Å — the input for the collapse-guard floor."""
    print("\n" + "=" * 74)
    print("PART-B CALIBRATION — min interatomic distance at A=2.0 (collapse guard floor)")
    print("frame = hess $atoms + 2.0·(±1)·mode-column; distances are frame-invariant")
    print("=" * 74)
    for name, jid in JOBS.items():
        d = JOBS_ROOT / jid
        _, P = hess_atoms_angstrom(d / "input.hess")
        n = 3 * len(P)
        freqs = []
        L = (d / "input.hess").read_text().splitlines()
        for i, ln in enumerate(L):
            if ln.strip() == "$vibrational_frequencies":
                cnt = int(L[i + 1])
                freqs = [float(L[i + 2 + k].split()[-1]) for k in range(cnt)]
                break
        M = hess_normal_modes(d / "input.hess")
        anim = [k for k in range(n) if abs(freqs[k]) > 0]  # real + imaginary (not exact-0)
        mins = []
        for k in anim:
            v = M[:, k].reshape(len(P), 3)
            mins.append(min(min_pair_distance(P + 2.0 * s * v) for s in (+1, -1)))
        mins = np.array(mins)
        eq = min_pair_distance(P)
        row = f"  {name:22s} eq_min={eq:.2f}  median={np.median(mins):.2f}  "
        for fl in (0.3, 0.5, 0.7):
            row += f"<{fl}:{int((mins < fl).sum())}/{len(mins)}  "
        print(row)
    print("-" * 74)
    print("  => 2.0 is safe for bends (median ≈1.0) but overshoots localized stretches;")
    print("     floor 0.5 Å separates collapse from ordinary compression (guard, not block).")


def kabsch(P: np.ndarray, Q: np.ndarray):
    """R (proper rotation), t, rmsd such that Q ≈ R @ P + t, per row. R maps P->Q.

    No correspondence search: row i of P pairs with row i of Q (given by index).
    """
    cP, cQ = P.mean(0), Q.mean(0)
    Pc, Qc = P - cP, Q - cQ
    H = Pc.T @ Qc  # covariance
    U, _, Vt = np.linalg.svd(H)
    d = np.sign(np.linalg.det(Vt.T @ U.T))
    D = np.diag([1.0, 1.0, d])  # reflection guard → proper rotation
    R = Vt.T @ D @ U.T
    t = cQ - R @ cP
    rmsd = float(np.sqrt(np.mean(np.sum((Qc @ R.T - Pc) ** 2, axis=1))))
    return R, t, rmsd


def fmt_mat(R: np.ndarray) -> str:
    return "\n".join("    [" + "  ".join(f"{v: .8f}" for v in row) + "]" for row in R)


def main() -> None:
    print("=" * 74)
    print("UNIT-3.12 GATE — is the .hess $atoms reframe a rotation? (Kabsch, index order)")
    print("R maps .hess -> reference (.property.txt final geometry); both Bohr->Angstrom")
    print("=" * 74)
    verdicts = []
    for name, jid in JOBS.items():
        d = JOBS_ROOT / jid
        hs, P = hess_atoms_angstrom(d / "input.hess")
        rs, Q = property_final_geometry_angstrom(d / "input.property.txt")
        order_ok = hs == rs
        R, t, rmsd = kabsch(P, Q)
        max_dev = float(np.max(np.abs(R - np.eye(3))))
        # per-atom |.hess - reference| BEFORE any transform (the raw reframe shift)
        raw_shift = np.linalg.norm(P - Q, axis=1)
        verdicts.append((name, max_dev, rmsd))
        print(f"\n### {name}   job {jid[:8]}   elements-in-order={order_ok}")
        print(f"  raw per-atom |hess-ref| Angstrom: min {raw_shift.min():.6f} "
              f"max {raw_shift.max():.6f} mean {raw_shift.mean():.6f}")
        print("  R (.hess -> reference):")
        print(fmt_mat(R))
        print(f"  det(R) = {np.linalg.det(R): .8f}")
        print(f"  max|R - I| (element-wise) = {max_dev:.3e}")
        print(f"  translation t (Angstrom) = [{t[0]: .6f} {t[1]: .6f} {t[2]: .6f}]  |t|={np.linalg.norm(t):.6f}")
        print(f"  RMSD after superposition = {rmsd:.3e} Angstrom")

    print("\n" + "=" * 74)
    print("VERDICT")
    print("=" * 74)
    TOL = 1e-6
    for name, max_dev, rmsd in verdicts:
        kind = "PURE TRANSLATION (R≈I)" if max_dev < TOL else "ROTATION PRESENT"
        print(f"  {name:22s}  max|R-I|={max_dev:.3e}  RMSD={rmsd:.3e}  -> {kind}")
    all_identity = all(md < TOL for _, md, _ in verdicts)
    any_identity = any(md < TOL for _, md, _ in verdicts)
    print("-" * 74)
    if all_identity:
        print(f"  ALL three < {TOL:.0e}: reframe is a PURE TRANSLATION on every job.")
        print("  => normal modes may be taken AS-IS (no rotation into the reference frame).")
    elif any_identity:
        print("  CONTRADICTORY (rotation on some jobs, not others) => STOP, do not animate.")
    else:
        print("  ROTATION on every job => reader must rotate modes into the reference frame.")
    amplitude_floor_scan()


if __name__ == "__main__":
    main()
