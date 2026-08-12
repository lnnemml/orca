# CREST/QCG microsolvation (Stage F)

Explicit-solvent microsolvation for the reaction workstation: grow a solvent shell around a solute
with CREST's **QCG** (Quantum Cluster Growth), then hand the cluster to ORCA. Built on the measured
probe of record — **`wiki/orca/crest.md`** (CREST 3.0.2, xtb 6.6.1 linkage, the flag list, the two
real `grow/` runs, the ionic footgun). This page describes the CODE; the probe page is the evidence.

## The honest path — grow-as-seed, re-opt-in-ORCA

QCG grow is cheap and correctly linked (~10 s for a 3-solvent shell, xtb 6.6.1 + ALPB), but it is
**not** a production solvated-result generator for the mission's ionic reactant:

- **QCG grows an ion's cluster NEUTRAL** (four evidences, `crest.md`): `-chrg -1` reaches only the
  solute monomer preopt; the grow/cluster optimization runs at total charge 0. So the grown cluster's
  energy is the **wrong species' energy** for an ion.
- **`-ensemble` crashes** on the ionic system (CREGEN segfault / MTD non-convergence) at v3.0.2.

So the path OrcaStudio takes is: **take QCG's grown cluster as a GEOMETRY SEED only, and re-optimize it
in ORCA at the correct charge with SMD** — where the real ionic solvation energy should come from
anyway (SMD-not-ALPB for ions, ADR-018). The CREST cluster energy is an **xtb-level seed energy**,
never the solvated answer. This framing is why the reader names the field `seed_energy_eh`, not
`solvated_energy`.

## Staging

| unit | what | status |
|---|---|---|
| **F1a** | **grow PARSE + COMPLETION** — `crest.rs`: classify a run, read the grown cluster (geometry + seed energy + intended charge) from a real `grow/` dir | **done (2026-08-12)** |
| **F1b** | the **ephemeral runner** — spawn CREST/QCG grow off-thread, parse, emit events (mirrors `XtbRunner`); the arg vector + `qcg_energy.dat` growth-table parse (display) | **done (2026-08-12)** |
| F1c | the persistent CREST **job record + migration + setup form** | pending |
| F2 | the **ORCA re-opt handoff** — the seed cluster → an ORCA `Opt` at the correct charge + SMD | pending |
| — | the **ensemble** path (`-ensemble`) and QCG **quasi-RRHO thermo** | **deferred** (ensemble segfaults at v3.0.2; thermo is soft on the floppy shell) |

## F1a — the seed reader (`src-tauri/src/crest.rs`)

Pure data, no process spawning (that is F1b). Mirrors the external-tool discipline of `crate::xtb`:
completion is **classified from the log**, never trusted from an exit code.

- **Completion sentinel is `CREST terminated normally.`** (CREST's own stdout line in `crest.out`) —
  **NOT** `normal termination of xtb`. The probe measured that xtb's `normal termination` is written to
  **stderr**, which CREST does not keep, so it is not grep-able in the run dir (`crest.md`,
  `wiki/orca/xtb.md`). `classify_crest_completion(crest_out, cluster_xyz_present)` →
  `Ok` (sentinel AND cluster present) / `NoCluster` (sentinel but no cluster — a real, refused
  "terminated normally, nothing to seed" state) / `Failed` (no sentinel).
- **The cluster's `energy:` comment is CREST-format, parsed by its own home.**
  `cluster_optimized.xyz`'s line-2 comment is `energy: <Eh> gnorm: <val> xtb: 6.6.1 (unknown)` —
  a **different** convention from ORCA's `E <Eh>`. `parse_crest_energy_comment` matches the `energy:`
  token and deliberately does **not** match the ORCA form (`XyzFile::first_frame_energy` owns that):
  one format, one parser, never cross-matched. The raw comment is exposed via the new
  `XyzFile::first_frame_comment()` so the foreign-format reader parses its own energy — a clean seam,
  not a second xyz parser.
- **The intended charge is the solute's, from `crest.out` — never the cluster's.**
  `parse_crest_grow` reads `Molecular charge : <n>` from `crest.out` into `intended_charge`. Because
  QCG grows the cluster neutral, **a nonzero `intended_charge` means the grown cluster is the neutral
  species** — a **wrong-charge seed** the F1b/F2 step must warn on (and the ORCA re-opt must set the
  real charge). The parser derives **no** charge from the cluster geometry.
- **`seed_energy_eh`, never a solvated/final energy.** The struct
  `CrestGrowResult { cluster: FinalGeometry, seed_energy_eh: Option<f64>, intended_charge: Option<i32>,
  n_atoms }` names the energy a seed on purpose. F1a computes **no** solvation energy.
- **Honest absence.** `parse_crest_grow` returns `Ok(None)` when `cluster_optimized.xyz` is absent —
  a grow that produced no optimized cluster is not a fabricated geometry.

**Fixtures + bites** (`crest.rs` tests, real `grow/` dirs — `tests/fixtures/crest_grow_neutral`
[benzoic + 3 H₂O, charge 0, 24 atoms, −41.452349] and `crest_grow_anion` [BH₄⁻ + 3 MeOH, charge −1, 23
atoms, −27.915061]): `crest_completion_ok_needs_sentinel_and_cluster` (the sentinel contract, both
edges), `crest_energy_comment_parses_energy_colon_form` (CREST `energy:` yes, ORCA `E` no),
`parse_neutral_grow_rung0` (charge-clean), `parse_anion_grow_rung1` (the nonzero intended charge
surfaced), `parse_crest_grow_none_when_no_cluster` (honest absence).

## F1b — the ephemeral runner (`crest.rs`, mirrors `crate::xtb`)

**K3: nothing here persists.** A CREST grow is a helper (seconds), exactly like an xtb pre-opt — the
grown cluster is returned as an **event**, not a jobs row; the persisted artifact is the F2 ORCA re-opt.
The runner is a byte-for-byte mirror of `XtbRunner` (the whole 2.5.5 lesson: a synchronous long command
freezes the GTK/WebKit window AND blocks cancel).

- **The arg vector** — `build_crest_args(opts)` returns what follows `crest solute.xyz`:
  `-qcg solvent.xyz -grow -nsolv <n> -alpb <solvent> [-chrg <c>] [-uhf <u>] (-fixsolute|-nofix) -T <n>`.
  **ALWAYS `-grow`, NEVER `-ensemble`** (reproducibly segfaults on the ionic system, `crest.md`) and
  **no `-keepdir`** in production (probe-only). `-chrg`/`-uhf` are emitted **only when nonzero** — the
  two probed invocations exactly (neutral omits `-chrg`; the anion passes `-chrg -1`). ⚠️ **`-uhf`: only
  `uhf = 0` (singlets) was probed** — a nonzero `-uhf` is emitted by the same pattern but is **unverified**.
- **`crest_grow(app, db, runner, solute_xyz, solvent_xyz, opts)`** — a *starter* mirroring
  `xtb_optimize`: validate synchronously (`nsolv ≥ 1`, `uhf ≥ 0`, a solvent name), **reserve the single
  slot** (reject a concurrent run) before returning, then `std::thread::spawn` → `run_crest_in_dir`. The
  thread's cleanup: **SUCCESS → remove the dir AFTER parsing** (rule #3 scratch-litter), **CANCEL →
  remove**, **any other FAILURE → KEEP** (crest.out is the only evidence of where it failed, tailed into
  the error); the slot is freed unconditionally. Emits `crest:done` (a `CrestGrowDone { result, growth }`
  — the F1a `CrestGrowResult` seed + the display growth table) or `crest:error` (`{ message, dir? }`).
- **`run_crest_in_dir`** — isolated `data_dir/orcastudio/crest/<uuid>` → write `solute.xyz` +
  `solvent.xyz` → spawn `crest solute.xyz <args>` (cwd = the dir, own process group, stdout+stderr →
  `crest.out`) → poll for exit/cancel/timeout (`terminate_job` killpg + cwd sweep on cancel, mirroring
  xtb; `CREST_TIMEOUT_SECS = 600`) → **classify completion from `crest.out` + `grow/cluster.xyz`
  presence** (F1a, never the exit code) → **`parse_crest_grow` + `parse_qcg_energy` BEFORE returning**,
  so the cluster is read before the caller's `remove_dir_all` on success. `seed_energy_eh` flows straight
  through — never relabelled solvated.
- **`crest_cancel(runner)`** — flips the `AtomicBool` (must not block the main thread); the worker's poll
  loop does the killpg. Mirrors `xtb_cancel`.
- **`crest_path` is a user setting** (`settings` key `crest_path`, default `/opt/crest/crest`) — full
  path, **never bundled** (rule #7); reuses `xtb`'s `resolve_binary` `$PATH` resolver (one home).

Events: **`crest:done`** / **`crest:error`** on the frontend event bus, beside `xtb:*` (`tauri-core.md`).
No frontend consumes them yet — that is F1c (the setup form + solvent-monomer library + result panel).

## See also

- `wiki/orca/crest.md` — the QCG probe of record (the four-evidence neutral-grow footgun, the flag
  list, the artifact inventory, the E-before-F ordering recommendation).
- `wiki/orca/xtb.md` — the sibling external-tool completion discipline (the stderr `normal termination`
  fact reused here; exit codes lie).
- `wiki/architecture/adr-018-reaction-energy-reference.md` — SMD-not-ALPB for ions (the ORCA re-opt F2
  feeds).
