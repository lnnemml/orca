#!/usr/bin/env python3
"""Probe: does ORCA 6.1.0 REORDER atoms in its output artifacts when `! UseSym`
is active?

This is a PROBE, not a feature. It measures — it does not decide. The verdict it
prints is produced by REAL `/opt/orca/orca` runs against isolated job directories,
never asserted from the manual or from memory (domain rule #10). The result gates
the IndexMap design of Phase 4.2 Stage 1 and stands under ADR-016.

    LD_LIBRARY_PATH=/opt/orca sidecar/.venv/bin/python \
        sidecar/probes/usesym_atom_order.py            # runs ORCA, writes a workdir
    ... --workdir /path        # reuse a fixed workdir (persist artifacts)
    ... --skip-run             # re-analyse an existing --workdir without re-running ORCA

WHY a naive coordinate comparison is WRONG here, twice (both silent):
  1. `! UseSym` legally REORIENTS the molecule into the symmetry frame and
     SYMMETRIZES the geometry (a small coordinate drift). A naive coord compare
     shows "everything moved" and gives a false verdict in BOTH directions. So the
     detector compares RIGID-MOTION-INVARIANT fingerprints (per atom: the sorted
     vector of distances to every other atom), with a tolerance for symmetrization,
     and reports the measured symmetrization drift as a number.
  2. A permutation of symmetry-EQUIVALENT atoms (the two H of water) is in principle
     UNOBSERVABLE — no fingerprint detector can see it. The detector therefore names
     the equivalence classes; a swap inside one is reported as "unobservable", never
     as "no reorder".

THE GATE: any OBSERVABLE reorder in any artifact of any run → the IndexMap must be a
mandatory permuted map at the parse_output boundary, not an identity post-condition.

NEGATIVE CONTROLS (CLAUDE.md convention — a gate whose ability to fail is not shown
is green for an unknown reason):
  * NC-elem: formaldehyde with C and O swapped in the xyz — the element-sequence
    check MUST bite.
  * NC-fp:  methanol with the in-plane methyl H swapped with an out-of-plane methyl
    H (SAME element, INEQUIVALENT) — the fingerprint check MUST bite where the
    element-sequence check cannot.
  * ID-ctrl: formaldehyde WITHOUT UseSym — the detector on a known identity MUST
    report "no observable reorder".
"""
from __future__ import annotations

import argparse
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ORCA_DIR = Path("/opt/orca")
ORCA = ORCA_DIR / "orca"

Z_BY_SYMBOL = {"H": 1, "C": 6, "N": 7, "O": 8, "F": 9}
FP_TOL_ANGSTROM = 5e-3  # symmetrization + reorientation slack for a fingerprint match

# --------------------------------------------------------------------------- #
# the three systems — input atom order is fixed EXACTLY as written             #
# --------------------------------------------------------------------------- #
# 1. Formaldehyde, order H, C, H, O — a mixed multi-element order: any reorder by
#    element would show up immediately in the element sequence. C2v; the two H are
#    equivalent (mirror through the molecular plane).
FORMALDEHYDE_XYZ = [
    ("H", 0.000000, 0.943000, -0.588000),
    ("C", 0.000000, 0.000000, 0.000000),
    ("H", 0.000000, -0.943000, -0.588000),
    ("O", 0.000000, 0.000000, 1.205000),
]

# 2. Methanol, Cs. The decisive case: the two mirror-image methyl H are EQUIVALENT,
#    the third (in-plane) methyl H is NOT — distinguishable atoms of one element are
#    the only way to see a reorder INSIDE one element. Mirror plane = xz (y -> -y);
#    C, O, hydroxyl-H and the in-plane methyl H all lie at y = 0. Order interleaves
#    the methyl H so an intra-H reorder would be visible: C, O, H(O), Hm(mirror+),
#    Hm(in-plane), Hm(mirror-).
METHANOL_XYZ = [
    ("C", 0.000000, 0.000000, 0.000000),
    ("O", 0.000000, 0.000000, 1.430000),
    ("H", 0.890000, 0.000000, 1.690000),   # hydroxyl H, in plane (distinguishable)
    ("H", 0.515000, 0.892000, -0.360000),  # methyl H, mirror (+y)
    ("H", -1.030000, 0.000000, -0.360000), # methyl H, IN-PLANE (inequivalent)
    ("H", 0.515000, -0.892000, -0.360000), # methyl H, mirror (-y)
]

# 3. Water, order H, O, H — Opt+Freq: covers the motion artifacts (_trj.xyz,
#    .hess $atoms, final .xyz). C2v; the two H are equivalent.
WATER_XYZ = [
    ("H", 0.757000, 0.000000, 0.586000),
    ("O", 0.000000, 0.000000, 0.000000),
    ("H", -0.757000, 0.000000, 0.586000),
]

RUNS = {
    "formaldehyde": {
        "xyz": FORMALDEHYDE_XYZ,
        "keywords": "! HF def2-SV(P) UseSym TightSCF",
        "charge_mult": (0, 1),
        "kind": "sp",
    },
    "methanol": {
        "xyz": METHANOL_XYZ,
        "keywords": "! HF def2-SV(P) UseSym TightSCF",
        "charge_mult": (0, 1),
        "kind": "sp",
    },
    "water": {
        "xyz": WATER_XYZ,
        "keywords": "! r2SCAN-3c UseSym TightOpt Freq",
        "charge_mult": (0, 1),
        "kind": "optfreq",
    },
    # The decisive gap closer: distinguishable atoms of one element (methanol's
    # in-plane vs mirror methyl H) crossing the MOTION artifacts (_trj.xyz, .hess
    # $atoms, final .xyz). Water covers motion but all its H are equivalent, so an
    # intra-element reorder on the Opt/Freq path was structurally unobservable until
    # this run.
    "methanol_optfreq": {
        "xyz": METHANOL_XYZ,
        "keywords": "! r2SCAN-3c UseSym TightOpt Freq",
        "charge_mult": (0, 1),
        "kind": "optfreq",
    },
    # ID control: same formaldehyde, symmetry OFF — detector on a known identity.
    "formaldehyde_nosym": {
        "xyz": FORMALDEHYDE_XYZ,
        "keywords": "! HF def2-SV(P) TightSCF",
        "charge_mult": (0, 1),
        "kind": "sp",
    },
}


# --------------------------------------------------------------------------- #
# input / run                                                                  #
# --------------------------------------------------------------------------- #
def write_input(job: Path, spec: dict) -> None:
    q, m = spec["charge_mult"]
    lines = [spec["keywords"], "%pal nprocs 1 end", "", f"* xyz {q} {m}"]
    for sym, x, y, z in spec["xyz"]:
        lines.append(f"  {sym:2s} {x:14.8f} {y:14.8f} {z:14.8f}")
    lines.append("*")
    lines.append("")
    (job / "input.inp").write_text("\n".join(lines))


def run_orca(job: Path) -> dict:
    """Run ORCA with the FULL absolute path in an isolated dir (rules #1, #3).
    Serial (nprocs 1) so MPI is not even a variable. stdout -> output.out."""
    env = {
        **os.environ,
        "LD_LIBRARY_PATH": str(ORCA_DIR),
        "OMPI_MCA_hwloc_base_binding_policy": "none",  # rule #8, harmless at nprocs 1
    }
    out = job / "output.out"
    with out.open("w") as fh:
        proc = subprocess.run(
            [str(ORCA), "input.inp"], cwd=job, stdout=fh,
            stderr=subprocess.PIPE, text=True, env=env, timeout=1800,
        )
    txt = out.read_text(errors="replace")
    return {
        "returncode": proc.returncode,
        "terminated_normally": "ORCA TERMINATED NORMALLY" in txt,
        "stderr_tail": (proc.stderr or "").strip()[-400:],
        "point_group": _detected_point_group(txt),
    }


def _detected_point_group(out_txt: str) -> str | None:
    # ORCA prints e.g. "Point Group ( C2v )" / "point group is C2v" in the sym module.
    m = re.search(r"[Pp]oint [Gg]roup[^\n]*?\b([CDS]\d?[a-z]?v?h?d?|Td|Oh|Ci|Cs|C1)\b", out_txt)
    return m.group(1) if m else None


# --------------------------------------------------------------------------- #
# artifact readers — return (symbols, coords) per artifact / per frame          #
# --------------------------------------------------------------------------- #
def parse_inp_xyz(job: Path):
    text = (job / "input.inp").read_text()
    m = re.search(r"\*\s*xyz\s+-?\d+\s+\d+\s*\n(.*?)\n\s*\*", text, re.DOTALL | re.IGNORECASE)
    rows = []
    for ln in m.group(1).splitlines():
        p = ln.split()
        if len(p) >= 4:
            rows.append((p[0], float(p[1]), float(p[2]), float(p[3])))
    return rows


def out_cartesian_blocks(job: Path):
    """Every 'CARTESIAN COORDINATES (ANGSTROEM)' echo in output.out."""
    text = (job / "output.out").read_text(errors="replace")
    blocks = []
    for m in re.finditer(
        r"CARTESIAN COORDINATES \(ANGSTROEM\)\s*\n-+\n(.*?)\n\s*\n", text, re.DOTALL
    ):
        rows = []
        for ln in m.group(1).splitlines():
            p = ln.split()
            if len(p) >= 4 and p[0] in Z_BY_SYMBOL:
                rows.append((p[0], float(p[1]), float(p[2]), float(p[3])))
        if rows:
            blocks.append(rows)
    return blocks


def property_geometry_blocks(job: Path):
    prop = job / "input.property.txt"
    if not prop.exists():
        return []
    text = prop.read_text()
    blocks = []
    for m in re.finditer(r"\$Geometry\b(.*?)\$End", text, re.DOTALL):
        cm = re.search(r"&CartesianCoordinates[^\n]*\n(.*)", m.group(1), re.DOTALL)
        if not cm:
            continue
        rows = []
        for ln in cm.group(1).splitlines():
            p = ln.split()
            if len(p) < 4:
                continue
            sym = re.sub(r"\(\d+\)$", "", p[0])
            if sym in Z_BY_SYMBOL:
                # property.txt geometry is Bohr; coords are returned AS-IS (Bohr) and
                # compared against a Bohr-scaled reference (inp_bohr) at the call site —
                # fingerprints only need both sides in the same scale, so no conversion here.
                rows.append((sym, float(p[-3]), float(p[-2]), float(p[-1])))
        if rows:
            blocks.append(rows)
    return blocks


BOHR_PER_ANGSTROM = 1.8897259886


def xyz_frames(path: Path):
    if not path.exists():
        return []
    lines = path.read_text().splitlines()
    frames, i = [], 0
    while i < len(lines):
        try:
            n = int(lines[i].strip())
        except (ValueError, IndexError):
            break
        rows = []
        for k in range(n):
            p = lines[i + 2 + k].split()
            rows.append((p[0], float(p[1]), float(p[2]), float(p[3])))
        frames.append(rows)
        i += 2 + n
    return frames


def hess_atoms(job: Path):
    hess = job / "input.hess"
    if not hess.exists():
        return None
    lines = hess.read_text().splitlines()
    for i, ln in enumerate(lines):
        if ln.strip() == "$atoms":
            n = int(lines[i + 1].strip())
            rows = []
            for row in lines[i + 2: i + 2 + n]:
                p = row.split()
                # .hess $atoms geometry is Bohr; fingerprints are scale-consistent
                # within one artifact, so match against a Bohr-scaled reference.
                rows.append((p[0], float(p[2]), float(p[3]), float(p[4])))
            return rows
    return None


# --------------------------------------------------------------------------- #
# fingerprint matching — the rigid-motion-invariant detector                    #
# --------------------------------------------------------------------------- #
def fingerprints(coords):
    """Per atom: sorted vector of distances to every other atom (rigid invariant)."""
    n = len(coords)
    fps = []
    for i in range(n):
        ds = []
        xi, yi, zi = coords[i]
        for j in range(n):
            if i == j:
                continue
            xj, yj, zj = coords[j]
            ds.append(math.sqrt((xi - xj) ** 2 + (yi - yj) ** 2 + (zi - zj) ** 2))
        fps.append(sorted(ds))
    return fps


def fp_distance(a, b):
    if len(a) != len(b):
        return math.inf
    return max(abs(x - y) for x, y in zip(a, b)) if a else 0.0


def equivalence_classes(syms, fps, tol):
    """Group atoms that are indistinguishable (same element, fp within tol)."""
    n = len(syms)
    parent = list(range(n))

    def find(x):
        while parent[x] != x:
            parent[x] = parent[parent[x]]
            x = parent[x]
        return x

    for i in range(n):
        for j in range(i + 1, n):
            if syms[i] == syms[j] and fp_distance(fps[i], fps[j]) <= tol:
                parent[find(i)] = find(j)
    classes = {}
    for i in range(n):
        classes.setdefault(find(i), []).append(i)
    return [sorted(c) for c in classes.values() if len(c) > 1]


def detect(ref, test, tol=FP_TOL_ANGSTROM):
    """Compare a TEST artifact (symbols, coords) against a REF (symbols, coords).
    ref/test are lists of (sym, x, y, z). Returns the verdict dict.

    'Observable reorder' means: some test position j whose ONLY same-element
    fingerprint matches are ref atoms != j — i.e. the identity mapping is
    inconsistent with the fingerprints. Swaps inside an equivalence class are
    reported as unobservable, never as 'no reorder'."""
    r_syms = [r[0] for r in ref]
    t_syms = [t[0] for t in test]
    r_xyz = [r[1:] for r in ref]
    t_xyz = [t[1:] for t in test]

    res = {
        "n_ref": len(ref),
        "n_test": len(test),
        "count_match": len(ref) == len(test),
        "element_sequence_exact": r_syms == t_syms,
        "element_first_mismatch": next(
            (i for i, (a, b) in enumerate(zip(r_syms, t_syms)) if a != b), None
        ),
    }
    if not res["count_match"]:
        res["verdict"] = "COUNT MISMATCH"
        return res

    fp_ref = fingerprints(r_xyz)
    fp_test = fingerprints(t_xyz)
    classes = equivalence_classes(r_syms, fp_ref, tol)
    res["equivalence_classes_ref"] = classes

    max_drift = 0.0
    observable_reorder = []
    no_match = []
    per_atom = []
    for j in range(len(test)):
        cands = []
        best_d = math.inf
        for i in range(len(ref)):
            if r_syms[i] != t_syms[j]:
                continue
            d = fp_distance(fp_ref[i], fp_test[j])
            best_d = min(best_d, d)
            if d <= tol:
                cands.append(i)
        per_atom.append({"test": j, "ref_candidates": cands, "best_fp_dist": round(best_d, 6)})
        if not cands:
            no_match.append(j)
            continue
        max_drift = max(max_drift, min(
            fp_distance(fp_ref[i], fp_test[j]) for i in cands
        ))
        if j not in cands:
            observable_reorder.append({"test": j, "matches_ref": cands})

    res["max_symmetrization_drift_angstrom"] = round(max_drift, 6)
    res["per_atom"] = per_atom
    if no_match:
        res["no_fingerprint_match"] = no_match
        res["verdict"] = "NO MATCH (geometry differs beyond tol — not comparable here)"
    elif observable_reorder:
        res["observable_reorder"] = observable_reorder
        res["verdict"] = "OBSERVABLE REORDER"
    else:
        res["verdict"] = (
            "no observable reorder (identity consistent within tol)"
            + (f"; order inside equivalence classes {classes} is unobservable" if classes else "")
        )
    return res


# --------------------------------------------------------------------------- #
# per-run artifact comparison                                                   #
# --------------------------------------------------------------------------- #
def compare_run(job: Path, kind: str) -> dict:
    inp = parse_inp_xyz(job)
    inp_bohr = [(s, x * BOHR_PER_ANGSTROM, y * BOHR_PER_ANGSTROM, z * BOHR_PER_ANGSTROM)
                for s, x, y, z in inp]
    out: dict = {"input_order": [s for s, *_ in inp]}

    if kind == "sp":
        # SP: input geometry is preserved (up to reorient+symmetrize) in every
        # artifact — fingerprint-match each against the input.
        blocks = out_cartesian_blocks(job)
        out["out_cartesian_block0"] = detect(inp, blocks[0]) if blocks else "absent"
        pblocks = property_geometry_blocks(job)
        # Bohr-space compare: FP_TOL_ANGSTROM is here effectively 1.889x STRICTER
        # (1 Å tol ≈ 0.529 Bohr) — conservative for an identity verdict, named not chance.
        out["property_$Geometry_block0"] = detect(inp_bohr, pblocks[0]) if pblocks else "absent"
        out["property_n_blocks"] = len(pblocks)
    else:  # optfreq — geometry CHANGES; input ≈ FIRST trajectory frame only
        trj = xyz_frames(job / "input_trj.xyz")
        out["trj_n_frames"] = len(trj)
        out["trj_frame0_vs_input"] = detect(inp, trj[0]) if trj else "absent"
        # every later artifact: ELEMENT SEQUENCE only (geometry has moved)
        out["trj_all_frames_element_sequence"] = _elem_seq_all(
            [s for s, *_ in inp], [[a[0] for a in f] for f in trj]
        )
        finalxyz = xyz_frames(job / "input.xyz")
        out["final_xyz_element_sequence"] = (
            _elem_seq_all([s for s, *_ in inp], [[a[0] for a in finalxyz[-1]]])
            if finalxyz else "absent"
        )
        hatoms = hess_atoms(job)
        out["hess_$atoms_element_sequence"] = (
            _elem_seq_all([s for s, *_ in inp], [[a[0] for a in hatoms]])
            if hatoms else "absent"
        )
        # .hess is computed at the FINAL optimized geometry, so it is the LAST trj
        # frame (not the first) that it must fingerprint-match — both at the same
        # geometry. This is the rigid, within-element check on the .hess ordering.
        if trj and hatoms:
            flast = trj[-1]
            flast_bohr = [(s, x * BOHR_PER_ANGSTROM, y * BOHR_PER_ANGSTROM, z * BOHR_PER_ANGSTROM)
                          for s, x, y, z in flast]
            # Bohr-space compare: FP_TOL_ANGSTROM is here 1.889x STRICTER (see note above)
            # — conservative for an identity verdict.
            out["hess_$atoms_vs_trj_final_frame_fingerprint"] = detect(flast_bohr, hatoms)
        pblocks = property_geometry_blocks(job)
        out["property_$Geometry_element_sequence"] = _elem_seq_all(
            [s for s, *_ in inp], [[a[0] for a in b] for b in pblocks]
        )
        out["property_n_blocks"] = len(pblocks)
    return out


def _elem_seq_all(ref_syms, list_of_test_syms):
    bad = []
    for k, t in enumerate(list_of_test_syms):
        if t != ref_syms:
            bad.append(k)
    return {
        "n": len(list_of_test_syms),
        "all_match_input_order": len(bad) == 0,
        "bad_indices": bad,
    }


def run_has_observable_reorder(cmp: dict) -> bool:
    for v in cmp.values():
        if isinstance(v, dict):
            if v.get("verdict") == "OBSERVABLE REORDER":
                return True
            if v.get("all_match_input_order") is False:
                return True
    return False


# --------------------------------------------------------------------------- #
# negative controls                                                             #
# --------------------------------------------------------------------------- #
def negative_controls() -> dict:
    """Prove the detector CAN fail. A gate that cannot go red is green for an
    unknown reason (CLAUDE.md)."""
    nc = {}

    # NC-elem: formaldehyde with C(idx1) and O(idx3) swapped — element seq must bite.
    ref = FORMALDEHYDE_XYZ
    perm = list(FORMALDEHYDE_XYZ)
    perm[1], perm[3] = perm[3], perm[1]
    d = detect(ref, perm)
    nc["NC_elem_swap_C_O"] = {
        "expected": "OBSERVABLE REORDER (different elements at 1 and 3)",
        "element_sequence_exact": d["element_sequence_exact"],
        "verdict": d["verdict"],
        "bites": d["verdict"] == "OBSERVABLE REORDER" or not d["element_sequence_exact"],
    }

    # NC-fp: methanol with the IN-PLANE methyl H (idx4) swapped with a MIRROR methyl
    # H (idx3) — SAME element, INEQUIVALENT. Element sequence is unchanged; only the
    # fingerprint check can catch it.
    ref = METHANOL_XYZ
    perm = list(METHANOL_XYZ)
    perm[3], perm[4] = perm[4], perm[3]
    d = detect(ref, perm)
    nc["NC_fp_swap_inplane_mirror_methyl_H"] = {
        "expected": "OBSERVABLE REORDER via fingerprint; element sequence UNCHANGED",
        "element_sequence_exact": d["element_sequence_exact"],
        "verdict": d["verdict"],
        "observable_reorder": d.get("observable_reorder"),
        "equivalence_classes_ref": d.get("equivalence_classes_ref"),
        "bites": d["verdict"] == "OBSERVABLE REORDER",
        "bites_where_element_check_cannot": d["element_sequence_exact"]
        and d["verdict"] == "OBSERVABLE REORDER",
    }

    # NC-null: methanol swap two EQUIVALENT mirror methyl H (idx3, idx5) — this MUST
    # be reported as unobservable, proving the detector does not cry wolf on an
    # in-principle-invisible permutation.
    perm = list(METHANOL_XYZ)
    perm[3], perm[5] = perm[5], perm[3]
    d = detect(METHANOL_XYZ, perm)
    nc["NC_null_swap_equivalent_mirror_H"] = {
        "expected": "NOT flagged as reorder — the two mirror H are an equivalence class",
        "verdict": d["verdict"],
        "correctly_unobservable": d["verdict"].startswith("no observable reorder"),
    }
    return nc


# --------------------------------------------------------------------------- #
# driver                                                                        #
# --------------------------------------------------------------------------- #
def run(workdir: Path, skip_run: bool) -> None:
    print("=" * 78)
    print("ORCA UseSym ATOM-ORDER PROBE")
    print("=" * 78)
    print(f"orca binary: {ORCA} ({'present' if ORCA.exists() else 'MISSING'})")
    print(f"workdir: {workdir}")
    print(f"fingerprint match tol: {FP_TOL_ANGSTROM} Angstrom")

    # ORCA version banner (from any run, not asserted)
    print("\n--- runs ---")
    run_meta = {}
    for name, spec in RUNS.items():
        job = workdir / name
        if not skip_run:
            job.mkdir(parents=True, exist_ok=True)
            # isolate: clear any prior artifacts (rule #3, one dir per calc)
            for f in job.iterdir():
                f.unlink()
            write_input(job, spec)
            meta = run_orca(job)
        else:
            meta = {"skipped": True}
        run_meta[name] = meta
        print(f"  {name:22s} {json.dumps(meta)}")

    banner = _orca_version(workdir)
    print(f"\nORCA version (from run banner): {banner}")

    print("\n" + "=" * 78)
    print("NEGATIVE CONTROLS — the detector MUST be able to go red")
    print("=" * 78)
    nc = negative_controls()
    print(json.dumps(nc, indent=2))
    nc_ok = (
        nc["NC_elem_swap_C_O"]["bites"]
        and nc["NC_fp_swap_inplane_mirror_methyl_H"]["bites_where_element_check_cannot"]
        and nc["NC_null_swap_equivalent_mirror_H"]["correctly_unobservable"]
    )
    print(f"\nNEGATIVE CONTROLS PASS (detector demonstrably bites & does not cry wolf): {nc_ok}")

    print("\n" + "=" * 78)
    print("PER-RUN ARTIFACT COMPARISON vs input order")
    print("=" * 78)
    gate = {}
    for name, spec in RUNS.items():
        job = workdir / name
        print(f"\n### {name}  [{spec['keywords']}]  point_group={run_meta[name].get('point_group')}")
        cmp = compare_run(job, spec["kind"])
        print(json.dumps(cmp, indent=2))
        gate[name] = run_has_observable_reorder(cmp)

    print("\n" + "-" * 78)
    any_reorder = any(gate.values())
    print(f"GATE: observable reorder per run: {gate}")
    print(f"GATE VERDICT: {'REORDER DETECTED — IndexMap must be a permuted map' if any_reorder else 'NO observable reorder in any measured artifact — identity map is a valid post-condition (within the measured scope)'}")
    print("-" * 78)
    print("\nSCOPE OF CLAIM: these molecules (formaldehyde C2v, methanol Cs, water C2v),")
    print("these job types (SP; Opt+Freq), these artifacts (.out echo, .property.txt")
    print("$Geometry, _trj.xyz, final .xyz, .hess $atoms), ORCA 6.1.0. Permutations of")
    print("symmetry-EQUIVALENT atoms are unobservable in principle. Larger systems and")
    print("other point groups are NOT measured here.")
    print("=" * 78)


def _orca_version(workdir: Path) -> str | None:
    for job in workdir.iterdir():
        out = job / "output.out"
        if out.exists():
            m = re.search(r"Program Version\s+([\d.]+)", out.read_text(errors="replace"))
            if m:
                return m.group(1)
    return None


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--workdir", type=Path, default=None,
                    help="dir for job subdirs (default: fresh mkdtemp)")
    ap.add_argument("--skip-run", action="store_true",
                    help="re-analyse an existing --workdir without re-running ORCA")
    args = ap.parse_args()

    workdir = args.workdir or Path(tempfile.mkdtemp(prefix="usesym_probe_"))
    workdir.mkdir(parents=True, exist_ok=True)
    run(workdir, args.skip_run)


if __name__ == "__main__":
    main()
