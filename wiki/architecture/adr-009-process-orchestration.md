# ADR-009: Running external binaries belongs to Rust, not the sidecar

**Status:** accepted · 2026-07-29

Complements (does not supersede) [ADR-002](adr-002-python-sidecar.md) (the Python sidecar
is for chemistry logic) and [ADR-003](adr-003-execution-backend.md) (the ExecutionBackend
trait). ADR-002 said the sidecar owns "chemistry intelligence"; it did not say who *spawns
external processes*. This ADR draws that line, prompted by the 2.5.5 xTB decision.

## Context
Phase 2.5.5 needed to run the standalone `xtb` binary to pre-optimize a geometry. The
ROADMAP had pencilled this in as a sidecar endpoint (`/xtb-optimize`) — the sidecar already
"does chemistry". But process spawning is not chemistry; it is orchestration, and the
project already has a hardened orchestration layer in Rust (the ORCA `LocalBackend`).

## Decision
**Spawning an external binary is Rust's responsibility. The sidecar never spawns a
process.** xTB pre-optimization lives in `src-tauri/src/xtb.rs`, next to the ORCA backend it
mirrors, not in the sidecar.

## Rationale (from practice, not principle)
Every reason is something the ORCA path already had to solve, and the sidecar would have had
to re-solve:

- **The binary path is a setting**, and settings live in SQLite owned by Rust (ADR-002's
  "keep the sidecar stateless"). `xtb_path` is a `settings` row like `orca_path`.
- **One isolated directory per run** (domain rule #3) — Rust already prepares and cleans job
  dirs.
- **Kill the whole process group** when cancelling or timing out (`debugging/004`: MPI ranks
  and stray children escape the parent's group). `xtb.rs` reuses ORCA's `terminate_job`
  (killpg + cwd sweep), made `pub(crate)` — one copy, not two.
- **Cancel and timeout off the UI thread** were already built and proven for ORCA; the
  sidecar would need its own equivalent.

## Consequences (stated plainly, so the boundary is not misread)
- **A library call inside Python is NOT a violation — spawning a process is.** `ase.io`
  (`convert.py`), RDKit (`smiles.py`), and the ASE geometry kernel (`geometry.py`) all stay
  in the sidecar: they are in-process library calls on file *content*, no `subprocess`.
- **Open Babel, if ever needed** (ROADMAP Phase 2 conversion fallback for formats ASE
  lacks), means the **`pybel` library** inside the sidecar — that is fine. Shelling out to
  the **`obabel` binary** would fall under this ADR and belong in Rust. The ROADMAP item
  carries this caveat.
- **The future `SlurmBackend`** (ADR-003) and any other external tool are Rust, by the same
  rule. The sidecar understands the *chemistry of files*; it does not *run programs*.

## Precedent
The 2.5.5 xTB implementation is the first application — see the `decision` entry
`[2026-07-29] xTB pre-optimization lives in Rust, not the sidecar` in `wiki/log.md`, and
`wiki/orca/xtb.md` / `wiki/modules/tauri-core.md` for the mechanics.
