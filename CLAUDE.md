# CLAUDE.md — OrcaStudio

## What this project is

**OrcaStudio** is a desktop GUI application (Linux Mint / Linux first) that wraps the ORCA
quantum chemistry package into one integrated environment: build molecules, generate inputs,
run calculations (locally or on a remote server over SSH), monitor them live, parse results,
and visualize everything — orbitals, spectra, trajectories, normal modes.

**Mission.** ORCA is extremely powerful but atomized: dozens of standalone binaries
(`orca`, `orca_plot`, `orca_mapspc`, ...), a 1300-page manual, terminal-only workflow.
OrcaStudio removes that barrier — first for the author (a chemist learning quantum chemistry
and doing research), potentially for others later. Beyond lowering the barrier to ORCA,
OrcaStudio is a **reaction mechanism workstation**: an environment where the researcher
constructs reaction geometries with precise control (distances, angles of attack, dihedral
angles), explores competing pathways, and compares activation energies — expressing
computational experiments as directly as the scientific question demands (see ADR-007).
Every design decision should be tested against two questions: *does it lower the barrier?*
and *does it give the researcher the geometric control they need?*

The app is also a **learning instrument**: live convergence plots, context-sensitive manual
help, and explanations are first-class features, not extras.

## Repository layout

```
orca-studio/
├── CLAUDE.md            ← this file: the schema. Read it at the start of every session.
├── README.md            ← human-facing project description
├── ROADMAP.md           ← phased development plan; keep status markers current
├── wiki/                ← the knowledge base (see "Wiki system" below)
├── src/                 ← React + TypeScript frontend (Vite)
├── src-tauri/           ← Rust core: process spawn, file watching, SSH, SQLite access
├── sidecar/             ← Python FastAPI service: cclib parsing, RDKit, manual indexing
└── resources/manual/    ← RAW SOURCES: indexed ORCA documentation. IMMUTABLE — never edit.
```

## Tech stack & key decisions

Full rationale lives in `wiki/architecture/` (ADRs). Summary:

- **Tauri 2 + React 18 + TypeScript (strict)** — desktop shell and UI. ADR-001.
- **Python sidecar (FastAPI on localhost:8765)** — chemistry logic: cclib, RDKit, ASE. ADR-002.
- **ExecutionBackend abstraction** — every calculation runs through a backend trait
  (`LocalBackend`, `SshBackend`, later `SlurmBackend`). UI code never knows where a job runs. ADR-003.
- **SQLite** (via Rust, one DB file per user data dir) — projects, molecules, jobs, results,
  plus FTS5 for manual search. ADR-004.
- **System `ssh` / `rsync`** for remote execution — shell out, don't reimplement SSH. ADR-005.
- **3Dmol.js** for molecular visualization, **Monaco** for the input editor,
  **recharts** for plots.

## Commands

```bash
# frontend + tauri dev
npm run tauri dev
# sidecar (from sidecar/, in venv)
uvicorn app.main:app --port 8765 --reload
# tests
npm test                    # frontend (vitest)
cargo test                  # src-tauri/
pytest                      # sidecar/
# build release
npm run tauri build
```

(Adjust this section as tooling solidifies — keeping it accurate is part of wiki maintenance.)

## Domain rules (hard-won ORCA knowledge — do not violate)

1. **Always invoke ORCA with its full absolute path** (`/opt/orca/orca input.inp`),
   otherwise OpenMPI parallelization silently fails. See `wiki/orca/orca-basics.md`.
2. **OpenMPI version must exactly match** the version the ORCA build expects.
3. **One job directory per calculation**, always. ORCA litters scratch files;
   isolation + post-run cleanup is mandatory.
4. **Default concurrency = 1.** ORCA parallelizes itself via `%pal`; the queue runs
   jobs sequentially unless the user explicitly overrides.
5. **Never load whole output files into memory.** Outputs reach tens of MB; stream/tail.
   Cube files reach hundreds of MB; generate with moderate grids (80–100) by default.
6. Job completion = marker file (`.exit_code`) **and** `ORCA TERMINATED NORMALLY` in output.
7. ORCA binaries are **never bundled or redistributed**; the app points to a user-configured
   install path. Same for the manual: indexed locally for personal use only.
8. **Pin ORCA to an explicit core set** and disable OpenMPI's own binding so the
   two don't fight: `OMPI_MCA_hwloc_base_binding_policy=none taskset -c <mask> ...`.
   The optimal mask is **measured, not assumed** — on the dev machine's hybrid CPU
   both "avoid mixing P+E cores" and "hyperthreading always hurts" turned out to be
   false. Default preset is E-cores only (machine stays usable); max-throughput uses
   all physical cores. See `wiki/orca/performance.md`.

## Wiki system

The wiki follows the LLM-wiki pattern (Karpathy): **you, Claude, write and maintain it**;
the human reads, directs, and asks questions. It is the project's compounding memory —
architecture, decisions, ORCA domain knowledge, chemistry learning notes, solved bugs.

### Layers

- **Raw sources** — `resources/manual/` (ORCA docs). Read-only. Never modified.
- **The wiki** — `wiki/**`. You own this layer: create pages, update cross-references,
  keep it consistent.
- **The schema** — this file. Co-evolves with the human; propose changes when workflows drift.

### Page types & where things go

| Type | Location | When to create/update |
|---|---|---|
| ADR (decision) | `wiki/architecture/adr-NNN-*.md` | Any significant tech/design choice. Never rewrite history: supersede with a new ADR. |
| Architecture overview | `wiki/architecture/overview.md` | Whenever component boundaries change. |
| Module page | `wiki/modules/<module>.md` | One per module: responsibilities, interfaces, status, quirks. Update when the module changes meaningfully. |
| ORCA knowledge | `wiki/orca/*.md` | Any fact learned about ORCA behavior, formats, tools, gotchas. |
| Chemistry notes | `wiki/chemistry/*.md` | Quantum chemistry concepts the author is learning. **Write these in Ukrainian** — they are personal study notes. |
| Debugging log | `wiki/debugging/*.md` | One page per non-trivial solved bug: symptom → root cause → fix. |

Language convention: technical pages (architecture, modules, orca) in English;
`chemistry/` in Ukrainian; conversation with the author in Ukrainian.

### index.md and log.md

- `wiki/index.md` — catalog of every wiki page: link + one-line summary, grouped by category.
  **Update it whenever a page is created or renamed.**
- `wiki/log.md` — append-only chronicle. Every entry starts with a parseable prefix:

  ```
  ## [YYYY-MM-DD] type | Short title
  ```

  where `type ∈ {session, decision, ingest, lint, milestone}`.
  `grep "^## \[" wiki/log.md | tail -5` must always show the 5 latest events.

### Session workflow (follow this every session)

**Start of session:**
1. Read `CLAUDE.md` (this file), `wiki/index.md`, and the last ~5 entries of `wiki/log.md`.
2. If the task touches a specific module, read its page in `wiki/modules/` first.

**End of any significant session:**
1. Append a `session` entry to `wiki/log.md`: what was done, what was decided, what's next.
2. Update every wiki page the session's changes touched (module pages, ADRs, gotchas).
3. Update `wiki/index.md` if pages were added/renamed.
4. Update `ROADMAP.md` status markers if a phase item was completed.

**When a bug is solved** (more than ~30 min of work): create a `wiki/debugging/` page immediately,
while the context is fresh.

**When an architectural decision is made** (including in conversation with the author):
write the ADR in the same session. Decisions that live only in chat history are lost decisions.

### Lint (run when asked, or suggest it every ~2 weeks)

Health-check the wiki:
- contradictions between pages; stale claims superseded by newer decisions;
- orphan pages not referenced from `index.md`;
- module pages that no longer match the code;
- missing cross-references; `ROADMAP.md` drift vs reality.
Report findings, propose fixes, apply approved ones, log a `lint` entry.

## Coding conventions

- **TypeScript**: strict mode; functional components + hooks; state via Zustand;
  no `any` without a comment explaining why.
- **Rust**: `thiserror` for errors; every Tauri command returns `Result<T, AppError>`;
  no `.unwrap()` outside tests.
- **Python**: type hints everywhere; Pydantic models for all API schemas; `ruff` for lint.
- **Commits**: conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`).
  Wiki updates ride in the same commit as the change they document.
- Small, reviewable increments. The author reads the diffs — write code to be read.

## Division of labor

- **Claude web/desktop** (chat): architecture discussions, research, planning.
  Outcomes get ingested into the wiki as ADRs/notes — by Claude Code, on the author's request
  ("ingest this decision: ...").
- **Claude Code** (you): implementation, wiki maintenance, lint. Use plan mode for
  multi-file features. Ask before destructive operations.
