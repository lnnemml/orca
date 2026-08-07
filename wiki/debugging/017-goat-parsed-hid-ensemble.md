# 017 — A GOAT job reached `parsed` and hid its conformer ensemble (showed a trajectory instead)

**Phase 4.5 (regression caught on a real butanone conformer search, 2026-08-07).**

## Symptom

Two GOAT jobs, side by side:

| Job | Created | Status | What the detail screen showed |
|---|---|---|---|
| Ibuprofen (old) | 2026-07-28 | **`completed`** | a **Conformers (29)** ensemble panel (ΔE kcal/mol + "Use this conformer") |
| Butanone C₄H₈O (new) | 2026-08-07 | **`parsed`** | the standard **Results dashboard** — "trajectory 17 frames (optimization cycles)" — and **no** ensemble panel |

The author (correctly) expected the new GOAT job to show the conformer ensemble like the old one.

## Step 1 — measure first (rule #10)

On the two real job dirs (`~/.local/share/orcastudio/jobs/<id>/`) + the SQLite rows:

| | old ibuprofen | new butanone |
|---|---|---|
| `input.inp` `!` line | `! XTB GOAT` | `! XTB GOAT` |
| `input.property.txt` | **PRESENT** | **PRESENT** |
| `input.finalensemble.xyz` | PRESENT (29 conformers) | **PRESENT (8 conformers)** |
| `input_trj.xyz` | present | present |
| `jobs.status` | `completed` | `parsed` |
| `results` row | **none** | **present, `parser_version = 4`** |

**This corrected the initial hypothesis.** The first guess was "the old job had no `property.txt`, the
new one does." **False** — *both* GOAT jobs have a `property.txt`. The `finalensemble.xyz` is present
for both (so this is a display bug, not a missing-ensemble run — the fix will surface it, not a STOP).

The real difference: the **new** job has a stored `results` row at the **current** `parser_version 4`,
so the current `parse_and_store` **ran the single-structure readers on the GOAT `property.txt` and they
SUCCEEDED** → status driven to `parsed`. The **old** job predates that (no results row; it stayed
`completed` under earlier logic). Measured structure of the GOAT `property.txt`: **17 `$Geometry`
blocks** (butanone) — the internal optimization *cycles* of one candidate, with the first `$Geometry`
≈ the input. The single-structure `property.rs`/`_trj.xyz` readers therefore accept it as if it were a
normal Opt with a 17-cycle trajectory — the exact "17 optimization cycles" the author saw.

## Root cause

Two independent pieces conspired:

1. **A GOAT `property.txt` is accepted by the single-structure parse.** Its first `$Geometry` ≈ the
   input, so the geometry post-condition passes; the readers store a results row and the job reaches
   `parsed`. But a GOAT trajectory is **conformer-search internals, not a meaningful single structure**
   — there is no "final" geometry, and the `_trj.xyz` frames are one candidate's opt cycles, not
   conformers.
2. **The ensemble panel's guard keyed on exactly `completed`** (`JobDetailScreen.tsx`):
   ```ts
   if (job?.status !== "completed" || ensembleTried.current) return;   // too narrow
   ```
   A `parsed` GOAT job returned early → `read_job_ensemble` never fired → the ensemble panel was hidden
   and the (meaningless-for-GOAT) `ResultsCard` trajectory rendered.

## Fix (mirrors the scan B1 fix — a GOAT job is a special job type)

A GOAT conformer search is a **special job type whose authoritative result is the ENSEMBLE**, exactly
as a relaxed scan's result is the profile (`debugging/015`). Three parts:

1. **Route GOAT past the single-structure parse** (`results.rs::parse_and_store`). Right after the scan
   branch, an `input_is_goat(input_content)` check returns `ParseOutcome::NoArtifact` — the readers do
   not run, no results row is stored, and the caller leaves the job **`completed`**. The ensemble is
   read separately by `read_job_ensemble`. (`NoArtifact`'s doc was generalized: "no single-structure
   artifact to parse" now also covers this deliberate skip.)
2. **Broaden the ensemble guard** to any **terminal success** (`isTerminalSuccessStatus` = `completed`
   **or** `parsed`), so even a GOAT job that already reached `parsed` (like the existing butanone job)
   still reads and shows its ensemble. Defense in depth — the Rust routing fixes *future* jobs; this
   fixes the *already-parsed* one.
3. **A GOAT job shows the ensemble, not the trajectory.** `showsSingleStructureResults(input)` =
   `!isGoatInput(input)` suppresses `ResultsCard` for GOAT jobs; a GOAT job that somehow has no readable
   ensemble shows a plain note, never the misleading trajectory. Non-GOAT jobs are untouched.

## Controls (RED→GREEN, the load-bearing one bite-verified)

- **C-goat-not-parsed** (Rust, `results::tests::goat_dir_is_routed_past_the_single_structure_readers`):
  a GOAT dir with a present `property.txt` (over the same ethane geometry as the fixture) → `NoArtifact`,
  no results row. **Bite-verified**: removing the GOAT branch makes the outcome `Parsed` (the exact
  regression) and the test goes red.
- **C-goat-parsed-shows-ensemble** (vitest): `isTerminalSuccessStatus("parsed") === true` (the narrow
  `=== "completed"` guard returned false — the bite), and `showsSingleStructureResults(goat) === false`.
- **C-nongoat-unaffected**: `showsSingleStructureResults(opt) === true`; the existing
  `non_scan_dir_still_runs_the_single_structure_readers` (Opt → `Parsed`) and the scan-routing tests
  stay green — the GOAT branch didn't disturb the Opt/SP/Freq or scan paths.

## Lesson

**GOAT is a special job type, like a scan** — its authoritative result is the **ensemble**, not a
single-structure Results dashboard. Any job type whose `property.txt` is not a genuine single structure
must be **routed past the single-structure parse** at the `parse_and_store` seam, and terminal-success
UI must never key on one narrow status. Also: **measure before believing the stated mechanism** — the
"property.txt present vs absent" hypothesis was wrong; both jobs had one, and the real trigger was the
current parser succeeding where the old one hadn't run.

Related: `debugging/015` (the scan analogue), `orca/goat.md`, `modules/results-ui.md`,
`modules/tauri-core.md` (the `parse_and_store` routing).
