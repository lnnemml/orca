# `orca_plot` — orbital cubes (measured, unit-3.15 gate)

Everything here comes from **runs** against `/opt/orca/orca_plot` (ORCA 6.1.0) on the
dexketoprofen job (`b0d1db94`, 33 atoms, 446 MOs, closed shell), not from the manual.
Re-run on the next ORCA version — the menu-number contract below is version-pinned.

## Non-interactive invocation (from its own output)

`orca_plot` prints its usage when run with `-h` / no plot file:

```
usage orca_plot gbw-file plot-inputfile
   or orca_plot gbw-file -i (for interactive)
   or orca_plot gbw-file -i (for interactive) -m (for memory specification in MB)
   or orca_plot density-file (for a listing of all available densities)
```

**The advertised batch mode `orca_plot gbw-file plot-inputfile` was NOT usable.** Its
file is read field-by-field (`PlotType`, then `Format`, then `MO/OP`, …), but after
`MO/OP` it demands a field it names **"state density"** and then **"infile"** whose
format is undocumented and which I could not satisfy from any run — every attempt
(`1`/`7`/`66 0`, with a resolution line in every position) exited `rc=64`
`FATAL ERROR … I/O OPERATION FAILED` and wrote **no cube**. (An early "it worked" was a
**stale cube** from a prior run — corrected here.) Reported, not papered over.

**What works: drive the interactive menu over stdin.** Piping the menu's own answers is
a deterministic non-interactive call. The measured menu (ORCA 6.1.0):

```
2  - Enter no of orbital to plot
4  - Enter number of grid intervals
11 - Generate the plot
12 - exit this program
```

so to generate MO `N` at grid `G` (closed shell → operator 0, the default):

```
printf "2\n%s\n4\n%s\n11\n12\n" "$N" "$G" | LD_LIBRARY_PATH=/opt/orca /opt/orca/orca_plot input.gbw -i
```

- Invocation follows `orca_json.rs` (ADR-009): the binary path is `dirname(settings.orca_path)/orca_plot`
  (**not** hard-coded `/opt/orca`, rule #7), `LD_LIBRARY_PATH` = that dir, `cwd` = the job
  dir (rule #3). ORCA writes `input.xyz` only if absent, so an existing final-geometry
  `input.xyz` is not clobbered (measured).
- **Output filename is deterministic:** `input.mo{N}a.cube` (operator 0 → `a`). The app
  renames it to a grid-keyed cache name `orbital.mo{N}.g{G}.cube` so different grids of
  the same MO don't overwrite each other.
- The default volumetric format is **Cube** ("Format … Grid3d/Cube"); the boundaries
  auto-fit the molecule (measured box ≈ 32.5 × 27.2 × 22.9 Bohr for dexketoprofen).

## Size & time by grid (HOMO of dexketoprofen, measured)

| grid | points `(N+1)³` | bytes | MB | gen time |
|---|---|---|---|---|
| 40³ | 68 921 | 909 225 | 0.87 | 0.06 s |
| 60³ | 226 981 | 3 062 025 | 2.92 | 0.16 s |
| 80³ | 531 441 | 7 259 625 | **6.92** | 0.36 s |
| 100³ | 1 030 301 | 14 172 025 | 13.52 | 0.67 s |

The `.cube` is **ASCII**, ≈ **13.75 bytes/point**, so size = `(N+1)³ · 13.75` — it scales
with the **grid intervals**, and at a *fixed* interval count is **independent of atom
count** (the box is always split into `N` intervals). Generation is sub-second here.

**Extrapolation to ~60 atoms (arithmetic shown).** At a fixed grid the cube size is the
same; but a ~60-atom molecule has a ~1.5× larger box, so to keep the same resolution
(~0.2 Å/point) you raise `N` ~1.5× (80 → ~120): `121³ = 1.77M` points × 13.75 ≈ **24 MB**.
Time ∝ points × basis functions: `(1.77M/531k) × (≈1000 BF/446 BF) ≈ 7.4×` the 80³ time
≈ **~2.7 s**. So even a 60-atom orbital at good resolution is ~24 MB / ~3 s — far from the
"hundreds of MB" of domain rule #5.

**Rule #5 default, verified by number:** 80³ = **6.9 MB** is the chosen default — a
moderate grid with ~0.2 Å resolution, single-digit MB, sub-second. 100³ (13.5 MB) is fine
too; the app caps the cube it will read at **32 MB** (a 60-atom @120³ still fits), refusing
larger with a "lower the grid" message rather than reading an unbounded file.

**3Dmol needs the whole cube text (stated, not hidden).** `new VolumeData(cube, "cube")`
parses a full string, so the cube **is** loaded whole into memory once (rule #5's
stream/tail is impossible here — the isosurface needs the whole grid). At 80³ that string
is 6.9 MB; the 32 MB cap bounds the worst case. Measured, recorded, not worked around.

## WebKitGTK volumetric render — the real unknown, PASSED

The open risk (debugging/002: WebKitGTK's incomplete WebGL) was whether 3Dmol's
**isosurface** path renders under `webkit2gtk-4.1` at all. Tested with the debugging/002
**MiniBrowser** technique — a standalone probe (`window.OffscreenCanvas = undefined` fix +
`addModel(cube,"cube")` + two `addVolumetricData` for the +/− lobes + `render()`) in the
identical engine Tauri uses. Result: the window title reached **`ISO_OK`** (no exception)
and a `gnome-screenshot` shows the HOMO rendered — **blue (+phase) / red (−phase) lobes**
around the ball-and-stick molecule. So the volumetric path works in the real engine with
the existing direct-canvas fix. (The author should still confirm inside the actual Tauri
app, where theme/layout differ; the engine-level capability is confirmed.)

## Gate verdict: PASS
Batch generation works (stdin-menu), sizes/times are modest and bounded, the isosurface
renders in WebKitGTK. Part B (lazy cached generation + orbital picker + isovalue) proceeds.
