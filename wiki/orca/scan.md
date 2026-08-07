# ORCA `%geom Scan` — relaxed surface scan input

The `Scan` sub-block of `%geom` drives ORCA's **relaxed surface scan**: it steps one
internal coordinate from a start to an end value over N points, re-optimising the rest of
the geometry at each point. OrcaStudio's Stage-A1 emit (`src/scene/scan.ts` / Rust
`orcastudio-core::emit::emit_scan_block`) generates this block; the artifacts it produces are
parsed per `parse-sources.md` §"Relaxed scan artifacts". See also `constraints.md` (the sibling
sub-block and the shared index base) and `adr-016-emit-input-ownership.md`.

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

A relaxed scan needs `! Opt` on the keyword line. **Without `Opt`, ORCA runs a single point and
silently ignores the `Scan` block** — 1 energy, 1 geometry, no `.relaxscanact.dat` (measured,
unit 3.3, `parse-sources.md`). This is a plausible-but-empty failure: the job finishes
`NORMALLY`, so nothing flags it. OrcaStudio guards it with `scanOptIssue` (`scan.ts`): a scan
present with no `Opt`/`OptTS` on the `!` line returns a loud diagnostic (A2 blocks Run on it).

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

## Not yet covered (later stages)

- **Multi-coordinate scans** (several coordinates in one `Scan` block — an N-D grid): A1 models a
  single coordinate; a multi-line `Scan` block reads as `unrecognised` (won't be rewritten).
- **Scan output parsing** into an energy profile: Stage B (`relaxscan.rs`).
- **Define-coordinate-from-selection UI** (pick 2/3/4 atoms → panel): Stage A2.
