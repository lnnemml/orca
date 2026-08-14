# ORCA `%geom Scan` — relaxed surface scan input

The `Scan` sub-block of `%geom` drives ORCA's **relaxed surface scan**: it steps one (or several —
see the 2D grid below) internal coordinate(s) from a start to an end value over N points, re-optimising
the rest of the geometry at each point. OrcaStudio's **frontend `src/scene/scan.ts` generates this block**
(`injectScan`, 1..N coordinates — the input text is the source of truth, ADR-008); the Rust
`orcastudio-core::emit::emit_scan_block` is a **1-coordinate golden-only twin** (see the 2D section). The
artifacts are parsed per `parse-sources.md` §"Relaxed scan artifacts". See also `constraints.md` (the
sibling sub-block and the shared index base) and `adr-016-emit-input-ownership.md`.

## Block syntax

```
%geom
  Scan
    B 0 1 = 1.4, 2.4, 6
  end
end
```

- **Letter** = coordinate kind: `B` distance (2 atoms), `A` angle (3), `D` dihedral (4).
  (There is no cartesian scan — the `C` of constraints has no scan analogue.)
- **Indices** are space-joined and **0-based** — the SAME index base as `%geom Constraints`
  (settled by a real run, `constraints.md`). OrcaStudio's app-global 0-based atom index is
  written verbatim (`to_orca_index` is the identity map).
- **`= start, end, N`** — start and end are the coordinate values (distance in **Å**, angles in
  **degrees**); `N` is the integer number of points (≥ 2), inclusive of both endpoints. Step =
  `(end − start) / (N − 1)`.
- OrcaStudio emits start/end as the **user's exact entered text** (the `startText`/`endText`
  carrier, the `value_text` analogue of a constraint value) so a scan endpoint never depends on
  JS↔Rust float formatting; a programmatic value too precise to round-trip byte-identically is
  refused by the shared 17-digit guard (`float-formatting-parity.md`).

## `! Opt` is REQUIRED (silent single-point otherwise) — measured

A relaxed scan needs an optimization keyword on the `!` line. **Without one, ORCA runs a single
point and silently ignores the `Scan` block** — 1 energy, 1 geometry, no `.relaxscanact.dat`
(measured, unit 3.3, `parse-sources.md`). This is a plausible-but-empty failure: the job finishes
`NORMALLY`, so nothing flags it. OrcaStudio guards it with `scanOptIssue` (`scan.ts`): a scan
present with no measured opt keyword on the `!` line returns a loud diagnostic; Stage A2 blocks
Run on it (`NewJobScreen`, the same gate the out-of-range constraint uses).

### Which opt keywords trigger a relaxed scan (measured — rule #10)

The guard must recognise **exactly** the keywords that actually drive a relaxed scan — too narrow
and a legitimate `! TightOpt` scan is silently blocked. So the set is *measured*, never taken from
docs. Each keyword below was run as a real ORCA 6.1 ethane C–C scan (`! r2SCAN-3c <kw> TightSCF` +
`%geom Scan B 0 1 = 1.4, 2.4, 6 end end`, full-path, isolated dir) and checked for a **6-row**
`.relaxscanact.dat`:

| Keyword | Stage | 6-row `.relaxscanact.dat`? | Relaxed scan? |
|---|---|---|---|
| `Opt` | A1 | ✅ 6 rows | yes |
| `OptTS` | A1 | ✅ 6 rows | yes |
| `TightOpt` | A2 | ✅ 6 rows (coords 1.4→2.4) | yes |
| `VeryTightOpt` | A2 | ✅ 6 rows (coords 1.4→2.4) | yes |
| `LooseOpt` | A2 | ✅ 6 rows (coords 1.4→2.4) | yes |
| *(none — SP)* | 3.3 | ✗ no file (single point) | no |

`RELAXED_SCAN_OPT_KEYWORDS` (`scan.ts`) = `{opt, optts, tightopt, verytightopt, looseopt}`,
matched case-insensitively as a whole token on a `!` line (comment-masked). A keyword **not** in
this table is **not** recognised until a run confirms it (e.g. a Cartesian-opt or geometry-specific
variant would be added only after its own probe) — the measurement decides, not convention.

## One `%geom` — Scan and Constraints compose, never duplicate

`Scan` and `Constraints` are **both `%geom` sub-blocks**. An input may carry both at once (e.g.
scan one bond while freezing another). They must live under **one** `%geom` as sibling
sub-blocks:

```
%geom
  Scan
    B 0 1 = 1.4, 2.4, 6
  end
  Constraints
    {B 2 3 C}
  end
end
```

If a scan were injected as its own second `%geom`, **ORCA silently takes one `%geom` and drops
the other** — losing either the scan or the constraint with no error. OrcaStudio therefore shares
a single `%geom` locator (`src/scene/geomBlock.ts`) between `injectScan` and `injectConstraints`;
`injectScan` inserts/replaces only the `Scan` sub-block inside the existing `%geom`. The locator
tracks block depth over the full recognised sub-block set (`constraints` + `scan`) — a locator
that knew only `Constraints` would mis-read a `Scan` block's `end` as closing `%geom`.

## Two-coordinate (2D) relaxed surface scan — a native nested grid (Stage 4a)

A `Scan` block may hold **more than one coordinate**: ORCA then runs a **native nested N₁×N₂ relaxed
surface scan** (every combination of coordinate-1 × coordinate-2 values, re-optimising the rest at
each grid point). This is how a **concerted** reaction (Diels-Alder: two forming bonds) is mapped as a
2D potential-energy surface. **Probe-measured** (rule #10, a real XTB run, 2026 — fact, not memory):

```
%geom Scan
  B 11 3 = 3.446, 1.5, 4
  B 10 0 = 3.4, 1.5, 4
end
end
```

- **4 × 4 = 16 grid points.** `output.out` reports **`There are 2 parameter to be scanned`**.
- **The two `end`s:** the FIRST closes `Scan`, the SECOND closes `%geom` — one `Scan` block, one
  `%geom`. Indices are 0-based verbatim (`B 11 3`, `B 10 0`), same base as the 1D scan.
- **Emit form.** OrcaStudio emits the **separate-line** form (`%geom` / `Scan` on their own lines,
  byte-identical in style to the gated 1D emit), NOT the probe's **inline** `%geom Scan`. The two are
  **ORCA-equivalent**; that ORCA accepts our *separate-line two-coordinate* form specifically is
  confirmed by a real app-**generated** run (the Stage-4a manual gate m2), not by the inline probe —
  "our form ≠ the measured-good form" until a run on OUR bytes says so.
- **Rust twin is 1-coordinate only.** `orcastudio-core::emit::emit_scan_block` is a **golden-only**
  `pub fn` (its only callers are its own `#[cfg(test)]` byte-identity tests — it is NOT composed into
  `emit_input` or any run/re-emit path). The **frontend `injectScan` owns the emitted scan text**
  (ADR-008: the input text is the source of truth). So the 2-coordinate emit lives in the frontend
  only; the Rust golden was deliberately left 1-coordinate. **If a future re-emit path** (a server-side
  input regeneration in Phase 5/6 SSH/SLURM) is ever added, the Rust twin must be widened to match — a
  divergence recorded here so it is not a silent trap.

### The `.dat`/point-file layout for the 2D parser (measured — pinned for Stage 4b)

Recorded now, from the probe, so Stage 4b's surface parser reads the grid correctly:

- **`input.relaxscanact.dat` is row-major with coordinate 1 as the OUTER loop and coordinate 2 as the
  INNER loop** — i.e. coordinate 2 varies fastest; the first N₂ rows are coordinate-1 point 0 × all
  coordinate-2 points, and so on. (The list order of the coordinates in the `Scan` block IS this
  outer→inner axis order — `injectScan` preserves it, which is why the emit order is load-bearing.)
- **`input.relaxscanscf.dat` is all zeros for an XTB scan** (the SCF-energy column has no meaning under
  the semiempirical driver) — the parser must read the **act** curve, not scf, for XTB.
- **Point geometry files `input.NNN.xyz` map NNN ↔ row NNN** of the act table (the same 1:1 the 1D
  reader uses), so a grid point's geometry is `input.<rowIndex>.xyz`.

(Stage 4a is **input only** — this layout is not parsed yet; the heatmap + OptTS-from-a-grid-saddle
handoff is Stage 4b.)

## Real run that confirmed the app-generated input (Stage A1, 2026-08-07)

Domain rules #1/#3/#10: the emit is only trusted once a real invocation confirms it. The
Stage-A1 `scan.ts` emit generated an ethane C–C scan (indices 0,1; 1.4 → 2.4 Å; 6 points —
mirrors the terminal-run scan of unit 3.3), written with `! r2SCAN-3c Opt TightSCF` and run via
`/opt/orca/orca` (full path) in an isolated dir:

- `input.relaxscanact.dat` / `input.relaxscanscf.dat` — **6 rows** each (2 cols coordinate Å +
  energy Eh), one per scan point. ✅ (post-condition: rows == npoints)
- `output.out` ends **`ORCA TERMINATED NORMALLY`**. ✅

This closes the loop unit 3.3 opened: our **generator** provably produces the measured artifacts.
Run recorded in `wiki/log.md` (Stage A1 session entry).

## The Scan panel + Scan-from-selection (Stage A2)

`ScanPanel` (`src/scene/ScanPanel.tsx`) is a **view over the input text** — its source is
`parseScanCoordinates(content)` (the N-aware read), every edit is an `injectScan(content, …)` transform;
there is no React state that *is* the scan (the number fields keep a transient keystroke draft only). It
renders the states (absent → add-path hint; parsed → coordinate 1 + editable start/end/npoints + remove,
**plus an optional second coordinate**; unrecognised, i.e. a `#` comment or an unparsable line → a
hands-off notice), surfaces `scanOptIssue` inline, and shows the **point count** (N₁×N₂) so the run cost
is visible before submitting. **Scan-from-selection** lives in `AtomInspector` ("Scan this
{distance/angle/dihedral}"): a 2/3/4-atom pick → `scanFromSelection` (kind from count; atoms resolved
from `AtomId` to the current 0-based global index at build time, so the coordinate survives a fragment
index shift) with an editable default range (start = current measured value; +1 Å / +30° / +60° span;
N = 10) — this creates/replaces the **first** coordinate.

**The second coordinate (Stage 4a)** is an atom-**PAIR** (a bond, `B`) entered in the panel: "+ Add 2nd
coordinate" seeds a valid default (its range from coordinate 1), then its two 0-based atom indices +
range are editable in place; removing it returns to a 1D scan. Because `inspectScanBlock` still flags a
>1-line block as `unrecognised`, the selection add-path is guarded off while a 2D scan is present
(`scanIsMultiCoord` in `NewJobScreen` gives the honest "a 2D scan is already set — edit it in the Scan
panel" reason, not a false "unrecognised") — so a single-coordinate "Scan this" can never clobber the
grid. The panel sits in the editor dock next to Constraints.

## Reading a 2D scan back (Stage 4b — landed)

A 2D scan does **not** parse into `results.scan`: the B1 1D reader is 2-column and its coordinate column
is the OUTER loop, which repeats (`3.446, 3.446, …`), so its **monotone post-condition** rejects the
3-column `.dat`. Measured: the real 10×10 job was `completed` with **no results row** (`error_message`:
*"relaxscan: malformed scan coordinate column: not strictly monotone at point 1"*). Two consequences,
both handled in Stage 4b (`modules/scan-surface-2d.md`):

- **The 1D reader stands down on a 3-column `.dat`** (`RelaxScan::from_path` — a column discriminator: 3+
  columns → `Ok(None)`, NOT a `Malformed` error), so a **successful 2D scan finishes cleanly** without a
  spurious monotone failure. Its "result" is the surface, read separately. A 2-column (1D) `.dat` still
  gets the full monotone + Å cross-check guard — unchanged.
- **The surface is read by a file-gated sibling** `read_scan_surface` (gated on `input.relaxscanact.dat`
  existing, NOT `results.scan`): it returns the `.dat` text + the point geometries `input.NNN.xyz` in row
  order. The frontend `parseScanSurface2d` shapes the grid; `ScanSurface2dPanel` draws the contour and
  hands a clicked node's geometry to OptTS (`geometries[nodeRow-1]`, a count-asserted identity seam).

## Not yet covered (later stages)
- **3-D (or higher) grids** — the pure builder/parser already loop over 1..N coordinates; the UI caps at
  2 (the driving Diels-Alder case). A 3rd coordinate is a UI-only extension.
- **A/D second coordinates** — the second-coordinate UI is an atom-**pair** (`B`) only; the model
  supports `A`/`D`, so this is a UI extension. A hand-written `A`/`D` second coordinate round-trips
  (range-editable, atoms shown read-only).
