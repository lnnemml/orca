#!/usr/bin/env python3
"""Probe: from which sources can we read ORCA 6.1.0 results?

This is a PROBE, not a feature. It measures — it does not decide. Every number and
every artifact name in the report below is produced by a run against REAL job
directories, never asserted from documentation or memory. It lives in the repo so it
survives us and can be re-run against the next ORCA version:

    sidecar/.venv/bin/python sidecar/probes/parse_sources.py \
        --sp        <job_dir> \
        --optfreq   <job_dir> \
        --saddle    <job_dir> \
        --goat      <job_dir>

Any role may be omitted; the probe reports it as a gap rather than inventing a run.

What it checks (maps 1:1 to the unit's task list):
  A. atom ORDER in EACH structured artifact (.hess $atoms, orca_2json, every _trj/.xyz
     frame, every .property.txt $Geometry block) vs the launching input.inp — exact
     element-sequence equality, first mismatch reported. Plus whether the &AtomicCharges
     / &grad / &FREQ arrays are element-labelled or BARE positional (the seam). This is
     the GATE: any reorder → stop.
  1. cclib version + which attributes are present/absent per output (SP vs Opt+Freq).
  2. Atom order & count: cclib atomnos/atomcoords[0] vs the input .inp xyz block —
     exact element-sequence equality, coord match within input's own precision.
  3. Imaginary frequencies: does cclib keep the negative sign; the measured values of
     the near-zero (trans/rot) modes, so a cutoff threshold is measured not guessed.
  4. Rule #5: largest output.out size, cclib parse peak RSS + wall time.
  5. Structured-artifact inventory: .hess sections, _trj.xyz frames, .property.txt
     blocks, and whatever /opt/orca converter we find (run, not assumed).
"""
from __future__ import annotations

import argparse
import json
import logging
import os
import re
import resource
import subprocess
import sys
import time
from pathlib import Path

# Quiet cclib's own logger — its "[ORCA … ERROR]" lines are captured as findings
# in the report, not left to interleave with it on stderr.
logging.getLogger("cclib").setLevel(logging.CRITICAL)

ORCA_DIR = Path("/opt/orca")

# Attributes the Results screen (ROADMAP Phase 3) will want. Probed per output.
CCLIB_ATTRS = [
    "scfenergies", "atomcoords", "atomnos", "natom",
    "vibfreqs", "vibirs", "vibdisps", "vibsyms",
    "atomcharges", "moenergies", "homos", "mocoeffs",
    "temperature", "enthalpy", "entropy", "freeenergy", "zpve",
    "etenergies", "etoscs", "metadata",
]


# ----------------------------------------------------------------------------- #
# subprocess mode: parse ONE file, report time + peak RSS in isolation           #
# ----------------------------------------------------------------------------- #
def _measure_ccread(path: str) -> None:
    """Run in a fresh process so ru_maxrss reflects this parse alone."""
    import cclib  # noqa: F401
    from cclib.io import ccread

    t0 = time.perf_counter()
    crashed = None
    ok = False
    try:
        data = ccread(path)
        ok = data is not None
    except Exception as e:  # noqa: BLE001 — still want the cost even on crash
        crashed = f"{type(e).__name__}: {e}"
    dt = time.perf_counter() - t0
    maxrss_kb = resource.getrusage(resource.RUSAGE_SELF).ru_maxrss  # Linux: KiB
    print(json.dumps({"ok": ok, "crashed": crashed, "seconds": dt, "maxrss_kb": maxrss_kb}))


def _measure_baseline() -> None:
    """Import cclib+numpy but parse nothing — the floor to subtract."""
    import cclib  # noqa: F401
    import numpy  # noqa: F401
    print(json.dumps({"maxrss_kb": resource.getrusage(resource.RUSAGE_SELF).ru_maxrss}))


def _spawn(*args: str) -> dict:
    out = subprocess.run(
        [sys.executable, __file__, *args],
        capture_output=True, text=True,
    )
    line = out.stdout.strip().splitlines()[-1] if out.stdout.strip() else "{}"
    try:
        d = json.loads(line)
    except json.JSONDecodeError:
        d = {"error": out.stderr.strip()[-500:] or "no output"}
    if out.returncode != 0 and "error" not in d:
        d["error"] = out.stderr.strip()[-500:]
    return d


# ----------------------------------------------------------------------------- #
# helpers                                                                        #
# ----------------------------------------------------------------------------- #
Z_BY_SYMBOL = {
    "H": 1, "B": 5, "C": 6, "N": 7, "O": 8, "F": 9, "P": 15, "S": 16, "Cl": 17,
    "Br": 35, "I": 53, "Pd": 46, "Na": 11, "B r": 35,
}


def parse_inp_xyz(inp: Path):
    """Return [(symbol, x, y, z), ...] from the * xyz ... * block of an .inp."""
    text = inp.read_text()
    m = re.search(r"\*\s*xyz\s+-?\d+\s+\d+\s*\n(.*?)\n\s*\*", text, re.DOTALL | re.IGNORECASE)
    if not m:
        return None
    rows = []
    for ln in m.group(1).splitlines():
        parts = ln.split()
        if len(parts) < 4:
            continue
        sym = parts[0]
        rows.append((sym, float(parts[1]), float(parts[2]), float(parts[3])))
    return rows


def count_xyz_frames(path: Path) -> tuple[int, int]:
    """(#frames, #atoms-in-first-frame) for a (multi-frame) xyz file."""
    lines = path.read_text().splitlines()
    frames, i, natom0 = 0, 0, 0
    while i < len(lines):
        try:
            n = int(lines[i].strip())
        except (ValueError, IndexError):
            break
        if frames == 0:
            natom0 = n
        frames += 1
        i += 2 + n
    return frames, natom0


def hess_sections(path: Path) -> list[str]:
    return re.findall(r"^\$(\S+)", path.read_text(), re.MULTILINE)


def property_blocks(path: Path) -> list[str]:
    # top-level $Block ... (not the $End terminators, not &props)
    names = re.findall(r"^\$([A-Za-z]\w+)", path.read_text(), re.MULTILINE)
    return [n for n in names if n != "End"]


# ----------------------------------------------------------------------------- #
# per-job cclib probe                                                            #
# ----------------------------------------------------------------------------- #
def _harvest_partial(out_path: Path):
    """cclib populates self.<attr> incrementally during parse and only assembles
    ccData at the end. If parse() raises mid-file we can still read whatever the
    parser accumulated by catching the exception and inspecting the parser object.
    Returns (data_or_None, exception_str_or_None, harvested_dict)."""
    from cclib.io import ccopen
    import numpy as np

    parser = ccopen(str(out_path))
    exc = None
    try:
        data = parser.parse()
        return data, None, {}
    except Exception as e:  # noqa: BLE001 — the probe wants the exception text
        exc = f"{type(e).__name__}: {e}"
    # harvest partial attributes left on the parser instance
    harvested = {}
    for attr in CCLIB_ATTRS:
        if hasattr(parser, attr):
            v = getattr(parser, attr)
            try:
                if isinstance(v, np.ndarray):
                    harvested[attr] = {"shape": list(v.shape), "dtype": str(v.dtype)}
                elif isinstance(v, (list, tuple)):
                    harvested[attr] = {"len": len(v), "sample": _short(v)}
                elif isinstance(v, dict):
                    harvested[attr] = {"keys": list(v.keys())}
                else:
                    harvested[attr] = v
            except Exception:  # noqa: BLE001
                harvested[attr] = "<unreadable>"
    return None, exc, harvested


def probe_cclib(out_path: Path) -> dict:
    from cclib.io import ccread
    import numpy as np

    result = {"file": str(out_path), "bytes": out_path.stat().st_size}
    try:
        data = ccread(str(out_path))
    except Exception as e:  # noqa: BLE001 — the probe wants the exception text
        result["ccread_exception"] = f"{type(e).__name__}: {e}"
        # measured finding needs the partial harvest too
        _, _, harvested = _harvest_partial(out_path)
        result["partial_attrs_before_crash"] = harvested
        return result
    result["ccread_ok"] = data is not None
    if data is None:
        return result

    present, absent = {}, []
    for attr in CCLIB_ATTRS:
        if hasattr(data, attr):
            val = getattr(data, attr)
            if attr == "metadata":
                present[attr] = {
                    k: val.get(k) for k in ("package", "package_version", "methods", "functional")
                    if k in val
                }
            elif isinstance(val, np.ndarray):
                present[attr] = {"shape": list(val.shape), "dtype": str(val.dtype)}
            elif isinstance(val, (list, tuple)):
                present[attr] = {"len": len(val), "sample": _short(val)}
            elif isinstance(val, dict):
                present[attr] = {"keys": list(val.keys())}
            else:
                present[attr] = val
        else:
            absent.append(attr)
    result["present"] = present
    result["absent"] = absent

    # imaginary / near-zero frequency detail
    if hasattr(data, "vibfreqs"):
        f = np.asarray(data.vibfreqs, dtype=float)
        result["vibfreqs"] = {
            "count": int(f.size),
            "n_negative": int((f < 0).sum()),
            "most_negative": float(f.min()) if f.size else None,
            "near_zero_abs_lt_50": sorted(float(x) for x in f if abs(x) < 50.0),
            "first_six": [float(x) for x in f[:6]],
        }
    return result


def _short(val):
    try:
        flat = np.ravel(np.asarray(val, dtype=float))
        return [round(float(x), 6) for x in flat[:4]]
    except Exception:  # noqa: BLE001
        return str(val)[:80]


import numpy as np  # noqa: E402  (used by _short/probe_cclib)


# ----------------------------------------------------------------------------- #
# atom-order seam check                                                          #
# ----------------------------------------------------------------------------- #
def probe_atom_order(job: Path) -> dict:
    from cclib.io import ccopen

    inp_rows = parse_inp_xyz(job / "input.inp")
    if inp_rows is None:
        return {"status": "no-inp-xyz-block"}
    # tolerate a mid-file crash: harvest atomnos/atomcoords off the parser instance
    parser = ccopen(str(job / "output.out"))
    crashed = None
    try:
        data = parser.parse()
    except Exception as e:  # noqa: BLE001
        crashed = f"{type(e).__name__}: {e}"
        data = parser  # partial attributes live on the parser
    if data is None or not hasattr(data, "atomnos"):
        return {"status": "cclib-missing-atomnos", "ccread_crashed": crashed}

    z_inp = [Z_BY_SYMBOL.get(sym) for sym, *_ in inp_rows]
    z_ccl = [int(z) for z in np.asarray(data.atomnos)]

    r = {
        "ccread_crashed": crashed,
        "n_atoms_inp": len(inp_rows),
        "n_atoms_cclib": len(z_ccl),
        "count_match": len(inp_rows) == len(z_ccl),
    }
    # element-sequence exact equality
    first_z_mismatch = next(
        (i for i, (a, b) in enumerate(zip(z_inp, z_ccl)) if a != b), None
    )
    r["element_order_exact"] = (first_z_mismatch is None) and r["count_match"]
    r["first_element_mismatch_index"] = first_z_mismatch

    # coordinate match: cclib atomcoords[0] (initial geometry) vs input, Angstrom.
    if hasattr(data, "atomcoords") and len(data.atomcoords):
        c0 = np.asarray(data.atomcoords[0], dtype=float)
        inp_c = np.asarray([[x, y, z] for _, x, y, z in inp_rows], dtype=float)
        if c0.shape == inp_c.shape:
            d = np.abs(c0 - inp_c)
            r["coord_max_abs_diff_angstrom"] = float(d.max())
            r["coord_match_1e-4"] = bool(d.max() < 1e-4)
            bad = np.argwhere(d.max(axis=1) >= 1e-4)
            r["first_coord_mismatch_index"] = int(bad[0][0]) if bad.size else None
        else:
            r["coord_shape_cclib"] = list(c0.shape)
            r["coord_shape_inp"] = list(inp_c.shape)
    return r


# ----------------------------------------------------------------------------- #
# PART A — atom ORDER in each structured artifact (the real Phase-3 seam)         #
#                                                                                 #
# The previous unit checked order for cclib only; cclib is dead, so the seam      #
# moved to the structured artifacts and the check must move with it. Here we      #
# compare the ELEMENT SEQUENCE of every artifact against the launching input.inp, #
# frame by frame and block by block — exact equality, first mismatch reported.    #
# ----------------------------------------------------------------------------- #
BOHR_PER_ANGSTROM = 1.8897259886


def _cmp_elements(z_ref: list[int], z_test: list[int]) -> dict:
    """Exact element-sequence equality; report first divergent index."""
    first = next((i for i, (a, b) in enumerate(zip(z_ref, z_test)) if a != b), None)
    return {
        "n_ref": len(z_ref),
        "n_test": len(z_test),
        "count_match": len(z_ref) == len(z_test),
        "order_exact": (first is None) and (len(z_ref) == len(z_test)),
        "first_mismatch_index": first,
    }


def _z_from_symbols(symbols: list[str]) -> list[int]:
    return [Z_BY_SYMBOL.get(s) for s in symbols]


def _hess_atoms(path: Path):
    """Return (symbols, coords) from the .hess $atoms section: 'SYM mass x y z'."""
    lines = path.read_text().splitlines()
    for i, ln in enumerate(lines):
        if ln.strip() == "$atoms":
            n = int(lines[i + 1].strip())
            syms, coords = [], []
            for row in lines[i + 2: i + 2 + n]:
                p = row.split()
                syms.append(p[0])
                coords.append([float(p[2]), float(p[3]), float(p[4])])
            return syms, coords
    return None, None


def _hess_dims(path: Path) -> dict:
    """Dimension lines of $normal_modes and $vibrational_frequencies."""
    lines = path.read_text().splitlines()
    out = {}
    for i, ln in enumerate(lines):
        s = ln.strip()
        if s == "$normal_modes":
            out["normal_modes_dim"] = lines[i + 1].strip()
        elif s == "$vibrational_frequencies":
            out["vibfreqs_count"] = int(lines[i + 1].strip())
    return out


def _xyz_frames_symbols(path: Path) -> list[list[str]]:
    """Element column of EVERY frame in an (xmol) xyz file."""
    lines = path.read_text().splitlines()
    frames, i = [], 0
    while i < len(lines):
        try:
            n = int(lines[i].strip())
        except (ValueError, IndexError):
            break
        syms = [lines[i + 2 + k].split()[0] for k in range(n)]
        frames.append(syms)
        i += 2 + n
    return frames


def _property_geometry_blocks(path: Path) -> list[list[str]]:
    """Element column of EVERY $Geometry block's &CartesianCoordinates."""
    text = path.read_text()
    blocks = []
    for m in re.finditer(r"\$Geometry\b(.*?)\$End", text, re.DOTALL):
        body = m.group(1)
        cm = re.search(r"&CartesianCoordinates[^\n]*\n(.*)", body, re.DOTALL)
        if not cm:
            continue
        syms = []
        for row in cm.group(1).splitlines():
            p = row.split()
            if len(p) < 4:
                continue
            # GOAT/xTB tags elements with a fragment suffix, e.g. 'C(1)' — strip it
            sym = re.sub(r"\(\d+\)$", "", p[0])
            if sym in Z_BY_SYMBOL:
                syms.append(sym)
        if syms:
            blocks.append(syms)
    return blocks


def _parse_atno_array(body: str) -> list[int]:
    """Extract the integer values of the first &ATNO array in a block body."""
    m = re.search(r"&ATNO[^\n]*\n(.*)", body, re.DOTALL)
    if not m:
        return []
    vals = []
    for row in m.group(1).splitlines():
        p = row.split()
        # rows are 'idx value'; the array ends at the next '&' directive
        if not p:
            continue
        if p[0].startswith("&"):
            break
        if len(p) >= 2 and p[0].isdigit():
            try:
                vals.append(int(float(p[1])))
            except ValueError:
                break
    return vals


def _property_array_labeling(path: Path, z_inp: list[int]) -> dict:
    """For each population scheme + gradient + FREQ: is the array element-labelled
    (carries a co-located &ATNO whose order we VERIFY == input) or a BARE positional
    array whose order must be ASSUMED from the $Geometry block? Reports it plainly —
    this is the seam."""
    text = path.read_text()
    out = {}
    for scheme in ("Mulliken", "Loewdin", "Mayer"):
        m = re.search(
            rf"\$SCF_{scheme}_Population_Analysis\b(.*?)\$End", text, re.DOTALL,
        )
        if not m:
            out[scheme] = "block-absent"
            continue
        body = m.group(1)
        has_charges = "&AtomicCharges" in body or "&QA" in body
        atno = _parse_atno_array(body)
        if atno:
            cmp = _cmp_elements(z_inp, atno)
            out[scheme] = {
                "has_atomic_charges": has_charges,
                "kind": "element-labelled (co-located &ATNO in same block)",
                "ATNO_order_matches_input": cmp["order_exact"],
                "ATNO_first_mismatch_index": cmp["first_mismatch_index"],
            }
        else:
            out[scheme] = {
                "has_atomic_charges": has_charges,
                "kind": "BARE positional (order assumed from $Geometry)",
            }
    grad = re.search(r"\$SCF_Nuc_Gradient\b(.*?)\$End", text, re.DOTALL)
    if grad:
        out["SCF_Nuc_Gradient.grad"] = (
            "BARE positional, &Dim (3N,1) flattened xyz "
            "(order assumed from $Geometry)"
        )
    freq = re.search(r"&FREQ\b[^\n]*\n", text)
    if freq:
        out["THERMOCHEMISTRY.FREQ"] = (
            "BARE positional, &Dim (3N,1) (mode index, not atom-indexed)"
        )
    return out


def probe_artifact_order(job: Path) -> dict:
    """PART A: element order in each artifact vs the launching input.inp."""
    inp_rows = parse_inp_xyz(job / "input.inp")
    if inp_rows is None:
        return {"status": "no-inp-xyz-block"}
    z_inp = _z_from_symbols([s for s, *_ in inp_rows])
    inp_max = max(abs(v) for _, *xyz in inp_rows for v in xyz)
    n_inp = len(z_inp)
    r = {"n_inp": n_inp}

    # 1. .hess $atoms + dims
    hess = job / "input.hess"
    if hess.exists():
        syms, coords = _hess_atoms(hess)
        if syms is not None:
            cmp = _cmp_elements(z_inp, _z_from_symbols(syms))
            hmax = max(abs(v) for row in coords for v in row)
            ratio = hmax / inp_max if inp_max else 0.0
            cmp["coord_ratio_vs_input"] = round(ratio, 4)
            cmp["units_measured"] = (
                "Bohr" if ratio > 1.5 else "Angstrom" if 0.7 < ratio < 1.3 else "?"
            )
            dims = _hess_dims(hess)
            cmp["normal_modes_dim"] = dims.get("normal_modes_dim")
            cmp["vibfreqs_count"] = dims.get("vibfreqs_count")
            cmp["expected_3N"] = 3 * n_inp
            cmp["vibfreqs_is_3N"] = dims.get("vibfreqs_count") == 3 * n_inp
            cmp["normal_modes_is_3Nx3N"] = (
                dims.get("normal_modes_dim") == f"{3 * n_inp} {3 * n_inp}"
            )
            r["hess_$atoms"] = cmp

    # 2. orca_2json Molecule.Atoms
    r["orca_2json_Atoms"] = _probe_json_atom_order(job, z_inp)

    # 3. _trj.xyz + .xyz — every frame
    for name, key in (("input_trj.xyz", "trj"), ("input.xyz", "xyz")):
        p = job / name
        if p.exists():
            frames = _xyz_frames_symbols(p)
            per = [_cmp_elements(z_inp, _z_from_symbols(f)) for f in frames]
            bad = [i for i, c in enumerate(per) if not c["order_exact"]]
            r[key] = {
                "n_frames": len(frames),
                "all_frames_order_exact": len(bad) == 0,
                "bad_frame_indices": bad,
                "per_frame_first_mismatch": [c["first_mismatch_index"] for c in per],
            }

    # 4. .property.txt — every $Geometry block + array labeling
    prop = job / "input.property.txt"
    if prop.exists():
        blocks = _property_geometry_blocks(prop)
        per = [_cmp_elements(z_inp, _z_from_symbols(b)) for b in blocks]
        bad = [i for i, c in enumerate(per) if not c["order_exact"]]
        r["property_$Geometry"] = {
            "n_blocks": len(blocks),
            "all_blocks_order_exact": len(bad) == 0,
            "bad_block_indices": bad,
        }
        r["property_array_labeling"] = _property_array_labeling(prop, z_inp)
    return r


def _probe_json_atom_order(job: Path, z_inp: list[int]) -> dict:
    conv = ORCA_DIR / "orca_2json"
    gbw = job / "input.gbw"
    if not conv.exists() or not gbw.exists():
        return {"status": "orca_2json-or-gbw-absent"}
    import shutil
    import tempfile
    with tempfile.TemporaryDirectory() as td:
        shutil.copy(gbw, Path(td) / "input.gbw")
        env = {**os.environ, "LD_LIBRARY_PATH": str(ORCA_DIR)}
        subprocess.run([str(conv), "input.gbw"], cwd=td, capture_output=True,
                       text=True, timeout=120, env=env)
        jf = Path(td) / "input.json"
        if not jf.exists():
            return {"status": "no-json-emitted"}
        atoms = json.loads(jf.read_text())["Molecule"]["Atoms"]
    z_json = [a["ElementNumber"] for a in atoms]
    idx = [a["Idx"] for a in atoms]
    cmp = _cmp_elements(z_inp, z_json)
    cmp["Idx_is_monotonic_0..N-1"] = idx == list(range(len(idx)))
    cmp["coords_note"] = "coords are FINAL geometry (Angs); compared by ELEMENT only"
    return cmp


# ----------------------------------------------------------------------------- #
# structured-artifact inventory                                                 #
# ----------------------------------------------------------------------------- #
def probe_artifacts(job: Path) -> dict:
    r = {}
    hess = job / "input.hess"
    if hess.exists():
        secs = hess_sections(hess)
        r["hess"] = {
            "bytes": hess.stat().st_size,
            "sections": secs,
            "has_freqs": "vibrational_frequencies" in secs,
            "has_normal_modes": "normal_modes" in secs,
            "has_ir_spectrum": "ir_spectrum" in secs,
        }
    trj = job / "input_trj.xyz"
    if trj.exists():
        frames, natom = count_xyz_frames(trj)
        r["trj"] = {"bytes": trj.stat().st_size, "frames": frames, "natom": natom}
    xyz = job / "input.xyz"
    if xyz.exists():
        frames, natom = count_xyz_frames(xyz)
        r["xyz"] = {"bytes": xyz.stat().st_size, "frames": frames, "natom": natom}
    prop = job / "input.property.txt"
    if prop.exists():
        blocks = property_blocks(prop)
        r["property_txt"] = {
            "bytes": prop.stat().st_size,
            "unique_blocks": sorted(set(blocks)),
            "has_thermochemistry": "THERMOCHEMISTRY_Energies" in blocks,
            "has_dipole": "SCF_Dipole_Moment" in blocks,
            "has_hessian": "Hessian" in blocks,
            "n_geometry_blocks": blocks.count("Geometry"),
        }
    return r


def probe_orca_2json(job: Path) -> dict:
    """Run /opt/orca/orca_2json on the job's gbw and report what it emits."""
    conv = ORCA_DIR / "orca_2json"
    if not conv.exists():
        return {"status": "orca_2json-not-found"}
    gbw = job / "input.gbw"
    if not gbw.exists():
        return {"status": "no-gbw"}
    # Copy the gbw to a temp dir so the real job dir is never polluted.
    import shutil
    import tempfile
    with tempfile.TemporaryDirectory() as td:
        tmp = Path(td)
        shutil.copy(gbw, tmp / "input.gbw")
        # orca_2json wants the filename WITH extension (measured: "input" alone →
        # "Cannot open GBW file"). Needs ORCA's shared libs on LD_LIBRARY_PATH.
        env = {**os.environ, "LD_LIBRARY_PATH": str(ORCA_DIR)}
        proc = subprocess.run(
            [str(conv), "input.gbw"], cwd=td, capture_output=True, text=True,
            timeout=120, env=env,
        )
        out_json = tmp / "input.json"
        res = {
            "returncode": proc.returncode,
            "stdout_tail": proc.stdout.strip()[-300:],
            "stderr_tail": proc.stderr.strip()[-300:],
        }
        if out_json.exists():
            res["json_bytes"] = out_json.stat().st_size
            try:
                j = json.loads(out_json.read_text())
                res["top_level_keys"] = list(j.keys())
                mol = j.get("Molecule", {})
                if isinstance(mol, dict):
                    res["molecule_keys"] = list(mol.keys())
            except Exception as e:  # noqa: BLE001
                res["json_parse_error"] = str(e)[:200]
    return res


# ----------------------------------------------------------------------------- #
# driver                                                                         #
# ----------------------------------------------------------------------------- #
def run(jobs: dict[str, Path]) -> None:
    print("=" * 78)
    print("ORCA 6.1.0 PARSE-SOURCE PROBE")
    print("=" * 78)

    import cclib
    print(f"\ncclib version: {cclib.__version__}")
    print(f"python: {sys.version.split()[0]}")
    print(f"orca binary: {ORCA_DIR / 'orca'} "
          f"({'present' if (ORCA_DIR / 'orca').exists() else 'MISSING'})")

    print("\nselected jobs:")
    for role, job in jobs.items():
        print(f"  {role:10s} {job}")

    # ---- PART A: atom ORDER in each artifact (the gate) ----------------------
    print("\n" + "=" * 78)
    print("PART A — atom ORDER in each structured artifact vs input.inp (GATE)")
    print("=" * 78)
    gate_fail = []
    for role, job in jobs.items():
        print(f"\n### {role}  {job.name}")
        rep = probe_artifact_order(job)
        print(json.dumps(rep, indent=2))
        # collect any order failure for the gate verdict
        for k, v in rep.items():
            if not isinstance(v, dict):
                continue
            if v.get("order_exact") is False:
                gate_fail.append(f"{role}:{k}")
            if v.get("all_frames_order_exact") is False:
                gate_fail.append(f"{role}:{k}")
            if v.get("all_blocks_order_exact") is False:
                gate_fail.append(f"{role}:{k}")
    print("\n" + "-" * 78)
    print(f"GATE VERDICT: {'FAIL — ' + ', '.join(gate_fail) if gate_fail else 'PASS (all artifact element orders == input)'}")
    print("-" * 78)

    # ---- 1. cclib attributes -------------------------------------------------
    print("\n" + "-" * 78)
    print("1. cclib attributes (present / absent), per output")
    print("-" * 78)
    for role, job in jobs.items():
        out = job / "output.out"
        print(f"\n### {role}  {out}")
        rep = probe_cclib(out)
        if "ccread_exception" in rep:
            print(f"  ccread RAISED: {rep['ccread_exception']}")
            print("  PARTIAL attributes harvested off the parser before it crashed:")
            for k, v in rep.get("partial_attrs_before_crash", {}).items():
                print(f"    {k:16s} {v}")
            print(f"  (attrs NOT reached before crash: "
                  f"{[a for a in CCLIB_ATTRS if a not in rep.get('partial_attrs_before_crash', {})]})")
            continue
        print(f"  ccread_ok={rep.get('ccread_ok')}  bytes={rep['bytes']}")
        print("  PRESENT:")
        for k, v in rep.get("present", {}).items():
            print(f"    {k:16s} {v}")
        print(f"  ABSENT: {rep.get('absent')}")
        if "vibfreqs" in rep:
            print(f"  vibfreqs detail: {rep['vibfreqs']}")

    # ---- 2. atom order -------------------------------------------------------
    print("\n" + "-" * 78)
    print("2. atom order & count — cclib vs input .inp (the Phase-3 seam)")
    print("-" * 78)
    for role, job in jobs.items():
        print(f"\n### {role}")
        print(f"  {json.dumps(probe_atom_order(job), indent=None)}")

    # ---- 3. imaginary handled inside section 1 (vibfreqs detail) --------------
    print("\n" + "-" * 78)
    print("3. imaginary / near-zero frequencies — see 'vibfreqs detail' above.")
    print("   (negative-sign preservation is n_negative + most_negative there.)")
    print("-" * 78)

    # ---- 4. rule #5: size, peak RSS, wall time -------------------------------
    print("\n" + "-" * 78)
    print("4. domain rule #5 — cclib parse cost (peak RSS, wall time)")
    print("-" * 78)
    base = _spawn("--measure-baseline")
    base_kb = base.get("maxrss_kb", 0)
    print(f"  baseline (import cclib+numpy, no parse): maxrss={base_kb} KiB")
    for role, job in jobs.items():
        out = job / "output.out"
        m = _spawn("--measure-ccread", str(out))
        if "error" in m:
            print(f"  {role:10s} {out.stat().st_size:>9d} B  -> ERROR: {m['error'][:120]}")
            continue
        peak = m.get("maxrss_kb", 0)
        crashed = f"  CRASHED({m['crashed']})" if m.get("crashed") else ""
        print(f"  {role:10s} {out.stat().st_size:>9d} B  "
              f"wall={m['seconds']*1000:7.1f} ms  "
              f"peak_rss={peak} KiB  (Δ over baseline={peak - base_kb} KiB){crashed}")

    # ---- 5. structured artifacts --------------------------------------------
    print("\n" + "-" * 78)
    print("5. structured-artifact inventory")
    print("-" * 78)
    for role, job in jobs.items():
        print(f"\n### {role}  {job.name}")
        arts = probe_artifacts(job)
        print(json.dumps(arts, indent=2))

    print("\n--- orca_2json converter (run on one job's gbw) ---")
    # run on the Opt+Freq if available, else the first job
    target = jobs.get("optfreq") or next(iter(jobs.values()))
    print(f"target: {target.name}")
    print(json.dumps(probe_orca_2json(target), indent=2))

    print("\n" + "=" * 78)
    print("END PROBE")
    print("=" * 78)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--sp", type=Path)
    ap.add_argument("--optfreq", type=Path)
    ap.add_argument("--saddle", type=Path)
    ap.add_argument("--goat", type=Path)
    ap.add_argument("--scan", type=Path)
    ap.add_argument("--measure-ccread", metavar="FILE")
    ap.add_argument("--measure-baseline", action="store_true")
    args = ap.parse_args()

    if args.measure_baseline:
        _measure_baseline()
        return
    if args.measure_ccread:
        _measure_ccread(args.measure_ccread)
        return

    jobs = {
        role: getattr(args, role)
        for role in ("sp", "optfreq", "saddle", "goat", "scan")
        if getattr(args, role) is not None
    }
    if not jobs:
        ap.error("provide at least one of --sp/--optfreq/--saddle/--goat/--scan")
    run(jobs)


if __name__ == "__main__":
    main()
