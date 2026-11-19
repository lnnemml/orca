# ORCA memory behaviour and host memory/disk facts (rule #10 record)

**Status:** measured 2026-11-18 by the prober for unit 5.5 (preflight), ORCA **6.1.1** at
`/opt/orca/orca` on the laptop, pinned (`OMPI_MCA_hwloc_base_binding_policy=none taskset -c <mask>`);
uni read only, as `anton`. M = measured, I = inferred. Serves [ADR-024](../architecture/adr-024-remote-execution-intermittent-connectivity.md)
Decision h / o26 and [ADR-025](../architecture/adr-025-pal-auto-by-default.md) item 7.

## Default and unit of `%maxcore`

- **Default on 6.1.1 is 4096**, not 4000 (M): water r²SCAN-3c, `%pal nprocs 2`, no `%maxcore` →
  `output.out` prints `:Max Core in MB = 4096.00` and `Max core memory ... 4096 MB`. With an explicit
  `%maxcore 2000` it prints 2000 and the note "MaxCore=2000 MB was set to SCF,MP2,MDCI,…". (The older
  wiki figure "4000 MB" had no source; the 6.1 changelog says "Default MaxCore increased to 4GB".)
- **ORCA's "MB" is MiB (2²⁰ B)** in its own memory lines (M): the fit check prints
  `Checking whether 4 symmetric matrices of dimension 385 fit in memory`, `MB needed = 2.27`;
  4 × 385·386/2 doubles × 8 B = 2 377 760 B = 2.2676 MiB (2.378 MB would print 2.38). The same block
  prints `Max Core in MB = 2000.00`, so `%maxcore N` is N MiB (I: no line prints the cap in bytes).
- So the default is **4096 (M), in MiB (I)**. The 6.1 changelog line "Default MaxCore increased to
  4GB" (local manual copy, `resources/manual/6.1/contents/appendix/detailedchangelog.md.txt:448`) is
  sourced, not measured; the run above is the measurement.

## Per-rank memory vs `%maxcore`

Ibuprofen (33 atoms, 385 basis functions), `! r2SCAN-3c Freq` (analytic), `%pal nprocs 4`, P-cores
`0,2,4,6`; RSS sampled every 1 s per process (matched by cwd), plus `VmHWM` from `/proc/<pid>/status`.
MiB throughout.

| `%maxcore` | peak per-rank RSS (sampled / VmHWM) | module at peak | max Σ over 4 ranks | 4 × cap | ORCA "Maximum memory used" |
|---|---|---|---|---|---|
| 2000 | 458.0 / 458.9 | `orca_propint_mpi` | 1798.5 | 8000 | 377.5 |
| 500 | 458.2 / 459.2 | `orca_propint_mpi` | 1801.6 | 2000 | 377.5 |
| 100 (cut in the XC-Hessian) | 193.4 / 193.4 | `orca_propint_mpi` | 752.5 | 400 | 107.5 |

- **The cap is a cap, not a use** (M): at 2000 and 500 the peaks are identical — the job needs less.
- **When the cap binds, ORCA overshoots it** (M, at 100): per-rank RSS 1.93 × cap, Σ 1.88 × 4 × cap;
  ORCA's own accounting also exceeded it (107.5 vs 100); **no warning, no abort** — memory-limited
  modules adapt.
- **Fixed overhead per rank** (I, from the table): RSS ≈ ORCA's internal "Maximum memory used" +
  ~80–100 MiB of runtime baseline; the startup module holds ~114 MiB in all (its own ~50 MiB use
  included, so its baseline is also ~65–85 MiB); the master `orca`
  process ~118 MiB and `mpirun` ~24 MiB, flat.
- **Working rule (I):** a job uses about `nprocs × (min(need, %maxcore) + ~100 MiB) + ~140 MiB`.
  The **cap-based estimate** is therefore `nprocs × (%maxcore + 100) + 140` MiB — not an upper bound:
  real use is usually far below it, and a cap set too small is exceeded rather than enforced.
- **Not measured:** the overshoot of a much heavier module, and per-rank peaks at 24 ranks.

## Host commands and their output formats (M)

| What | Command | Output |
|---|---|---|
| RAM | `head -5 /proc/meminfo` | `Key:<spaces>value kB`; line 1 `MemTotal`, line 3 `MemAvailable`. Laptop `MemTotal: 15988408 kB`; uni `MemTotal: 65795232 kB`. **kB = KiB** (M, determiner by the orchestrator: on the laptop `MemTotal × 1024` = `getconf _PHYS_PAGES × PAGESIZE` = `free -b` total = 16 372 129 792) |
| CPUs | `nproc` | laptop `16`, uni `48` (logical) |
| Free disk | `df --output=avail,target -B1 <path>` | header `      Avail Mounted on`, then `<bytes> <mount>` (right-aligned). Laptop data dir `101075353600 /`; uni `/home/anton/.orcastudio` → `78360924160 /home` |
| Free disk (POSIX) | `df -P -B1 <path>` | header + 6 columns, **Available = column 4** |
| **Gotcha** | `df -P --output=… -B1` | **fails**: "options -P and --output are mutually exclusive" (coreutils 9.4, both hosts) |
| FS | `findmnt -no FSTYPE,TARGET -T <path>` | one line, e.g. `ext4   /home` |
| Job dir size | `du -s -B1 <dir>/*` | `<bytes>\t<path>` per line |

## Scratch footprint (M)

- uni job dirs (2026-11-18): 4 KB … **182.6 MB** (`e900980b`, a cancelled Opt+Freq that kept its
  scratch); the whole `jobs/` 252 MB.
- Laptop ibuprofen Freq: **6.3 MB** after a clean exit, **180 MB** when cut mid-XC-Hessian (largest
  `in.cpscfdata.tmp.*` 17.6 MB, grid files 13.3 MB each). The final size is a floor, not the peak.
- No peak-scratch measurement exists; heavier methods (DLPNO, large bases) are unmeasured.
