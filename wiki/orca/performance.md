# ORCA performance on the dev machine

## Hardware (Laptop-main)

ASUS TUF Gaming F15 FX507ZC4 · Linux Mint · kernel 6.14

**CPU:** 12th Gen Intel(R) Core(TM) i5-12500H — hybrid, 12 physical cores / 16 threads:

| Logical CPUs | Physical cores | Type | Max clock | HT |
|---|---|---|---|---|
| 0–7 | 0–3 | P-cores (Golden Cove) | 4500 MHz | yes (pairs 0/1, 2/3, 4/5, 6/7) |
| 8–15 | 4–11 | E-cores (Gracemont) | 3300 MHz | no |

**RAM:** 15 GB usable, **only 1 GB swap** — an overrun locks the machine rather
than slowing it. Budget `%maxcore` conservatively.

**Power:** `platform_profile=performance` is required for reproducible numbers.
On `balanced` the sustained power limit throttles long AVX loads.

## Measured scaling

Benchmark: **(-)-17-like scaffold, 39 atoms, 640 basis functions**
(`CN1C(=O)C(O)C(c2ccccc2)C1C(=O)c1ccccc1` via RDKit ETKDG — an arbitrary
stereoisomer; chemical correctness is irrelevant here, geometric *identity*
across runs is what matters).

```
! B3LYP def2-TZVP def2/J RIJCOSX D4 CPCM(dmso) TightSCF
%maxcore 768
```

30 runs (3 repeats × 10 configurations), thermal cooldown to within +8 °C of the
idle baseline before every run, `%maxcore` held constant, `taskset` pinning with
`OMPI_MCA_hwloc_base_binding_policy=none`.

**Validity:** `SCF CONVERGED AFTER 12 CYCLES` on all 30 runs — identical work
everywhere. Repeat-to-repeat spread <1%. No thermal throttling (median clocks
held at or near the boost ceiling throughout).

| Config | Mask | nprocs | Median (s) | vs best | Peak °C | Median clock |
|---|---|---|---|---|---|---|
| **ALL12** | `0,2,4,6,8-15` | 12 | **114.8** | — | 96–99 | 4199 MHz |
| HT8 | `0-7` | 8 | 134.8 | +17% | 97–98 | 4200 MHz |
| P4 | `0,2,4,6` | 4 | 146.7 | +28% | 93–98 | 4200 MHz |
| **E8** | `8-15` | 8 | **156.7** | +37% | **74–77** | 3099 MHz |
| E4 | `8-11` | 4 | 274.2 | +139% | 62 | 3099 MHz |
| P3 | `0,2,4` | 3 | 190.0 | +66% | 90–94 | 4200 MHz |
| P2 | `0,2` | 2 | 256.6 | +124% | 93 | 4499 MHz |
| E2 | `8,9` | 2 | 487.0 | +324% | 57–60 | 3263 MHz |
| P1 | `0` | 1 | 487.4 | +325% | 85 | 4499 MHz |
| E1 | `8` | 1 | 920.3 | +702% | 53–72 | 3299 MHz |

Same-architecture efficiency (P against P1, E against E1):

| Config | Speedup | Efficiency |
|---|---|---|
| P2 | 1.90 | 95% |
| P3 | 2.57 | 86% |
| P4 | 3.32 | 83% |
| E2 | 1.89 | 94% |
| E4 | 3.36 | 84% |
| E8 | 5.87 | 73% |

**One E-core delivers 53% of one P-core** (487.4 / 920.3) — a larger gap than
clock speed alone (3.3 / 4.5 = 73%), reflecting Gracemont's narrower AVX pipeline.

## Two assumptions the measurement refuted

Both of these were written down as rules before being measured. Neither survived.

1. **"Avoid mixed P+E core sets — the slowest rank sets the pace."**
   FALSE on this workload. ALL12 is the fastest configuration, 28% ahead of
   P4. The hybrid penalty is real (cross-architecture efficiency ≈52% after
   correcting for the 0.53 E/P core-equivalence) but eight extra cores more
   than pay for it.
2. **"Hyperthreading gives negative speedup in ORCA."**
   FALSE here. HT8 (8 ranks on 4 physical P-cores) beats P4 by 8%. The common
   advice does not hold for RIJCOSX hybrid DFT on Alder Lake.

Note on the ">70% efficiency" heuristic: it flags ALL12 and HT8 as poor, but that
is an artefact of comparing heterogeneous core sets against a homogeneous P1
baseline. **For choosing a default, compare absolute wall time, not efficiency.**

## Recommended presets

| Preset | Mask | nprocs | When |
|---|---|---|---|
| **Interactive** (default) | `8-15` | 8 | Machine stays usable — all P-cores free, 74 °C instead of 97 °C. Costs only 7% over P4. |
| **Max throughput** | `0,2,4,6,8-15` | 12 | Fastest. Machine is busy and hot (99 °C). Use when nothing else needs the laptop. |

**P4 belongs in no preset:** 28% slower than ALL12, 20 °C hotter than E8, and it
occupies the P-cores anyway. It loses to both.

Invocation (see domain rule 8 in CLAUDE.md):

```bash
OMPI_MCA_hwloc_base_binding_policy=none taskset -c 8-15 /opt/orca/orca job.inp > job.out 2>&1
```

## Memory ceiling on nprocs

`%maxcore` is **per MPI process**, not total. With 15 GB RAM and 1 GB swap:

| nprocs | Max safe `%maxcore` |
|---|---|
| 12 | ~850 MB |
| 8 | ~1400 MB |
| 4 | ~2800 MB |

768 MB/rank sufficed for 640 basis functions. Larger systems or basis sets need
more per rank, so **nprocs is not a static default** — on a big job ALL12 may not
fit and the run must drop to 8 or 4 ranks. This is why settings need presets plus
a manual override, not a single number.

## Scope of these results

Measured for **one job type**: RIJCOSX hybrid DFT single point, 39 atoms,
640 basis functions. Do not over-generalise:

- `NumFreq` — independent displacements, near-linear scaling; ALL12 should win
  by more.
- `Opt` — a sequence of SCF steps; expected to behave like SP.
- Small systems (<20 atoms) — MPI startup (~2–3 s) grows as a fraction of
  runtime and the optimum shifts down. An 8-atom test measured *negative*
  speedup from 1→2 ranks: pure startup overhead.

## Benchmark methodology (why the first attempt was invalid)

The first run used an 8-atom ethane geometry left over from Phase 1 testing:
98 basis functions, 4.8 s per run. At that duration OpenMPI startup dominated
and P2 measured *slower* than P1. Rules for a valid ORCA scaling benchmark:

1. **Serial run ≥ 3 minutes.** Otherwise MPI startup is a large fraction of the
   measurement.
2. **Identical work every run.** Verify by checking the SCF cycle count is the
   same across all runs; if it varies, the comparison is void.
3. **Constant `%maxcore`.** Scaling it with nprocs makes the memory regime a
   second variable.
4. **Thermal cooldown to a baseline before every run**, not a fixed sleep. A run
   that starts hot throttles immediately.
5. **≥3 repeats, report the median.** Laptop variance is real.
6. **Median clock over the steady-state window**, not the absolute minimum —
   the minimum always catches the idle ramp-up sample.
7. `LC_NUMERIC=C` when scripting: a comma decimal separator silently truncates
   times in awk.

Benchmark geometry is deliberately a **single arbitrary conformer**. That is
correct here — reproducibility across runs is the requirement, not chemical
realism. Do not "fix" it with a conformer search; that would break comparability
with these numbers. (Conformer sampling *is* mandatory for science — see below.)

## Implication for research workloads

A full mechanism study (NEB-TS + OptTS + Freq + IRC, across solvents and
counterions) is on the order of **300–800 jobs of this size**. At ~2 min each on
ALL12 that is 10–27 hours of pure compute, and real jobs (Opt+Freq, larger
systems) are 5–20× heavier — days to weeks.

**This laptop is a development machine, not a research compute node.** Remote
execution (Phase 5) or an HPC allocation is a requirement, not a convenience.

Locally it stays useful for: coding and UI work, xTB pre-optimisation, conformer
searches, parser testing against existing outputs, and single reference
calculations.
