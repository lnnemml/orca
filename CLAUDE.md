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
├── sidecar/             ← Python FastAPI service: RDKit (SMILES→3D), ASE (geometry kernel + conversion), manual indexing
└── resources/manual/    ← RAW SOURCES: indexed ORCA documentation. IMMUTABLE — never edit.
```

## Tech stack & key decisions

Full rationale lives in `wiki/architecture/` (ADRs). Summary:

- **Tauri 2 + React 18 + TypeScript (strict)** — desktop shell and UI. ADR-001.
- **Python sidecar (FastAPI on localhost:8765)** — in-process chemistry over file *content*:
  RDKit (SMILES→3D), ASE (geometry kernel + format conversion). ADR-002. **Result parsing is NOT
  here** — see the next bullet.
- **Authoritative result parsing = own Rust parsers over ORCA's structured artifacts**
  (`.property.txt`/`.hess`/`_trj.xyz`/`orca_2json`), **not cclib** (crashes on ORCA 6.1.0) and not the
  sidecar. External-binary spawns (`orca_2json`, `orca_plot`) are Rust's too. ADR-012 / ADR-009.
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
   otherwise OpenMPI parallelization silently fails. Current version **6.1.1**; `/opt/orca`
   is a symlink to `/opt/orca-<version>` (6.1.0 retained for reproducing old results), and
   6.1.1 is bit-identical to 6.1.0 across the parser cross-version regression + laptop/server
   `sha256`. See `wiki/orca/orca-basics.md`.
2. **OpenMPI version must exactly match** the version the ORCA build expects.
3. **One job directory per calculation**, always. ORCA litters scratch files;
   isolation + post-run cleanup is mandatory.
4. **Concurrency is per backend.** ORCA parallelizes itself via `%pal`. `LocalBackend` runs one job at a time; each server profile has a slot count (default 1 until measured, rule #10). Concurrent slots never share cores (rule #8). See ADR-024.
5. **Never load the unbounded `output.out` whole** — it reaches tens of MB; stream/tail it
   (streaming convergence parse, output search, the two tail regexes). The **small, bounded
   structured artifacts** ARE read whole and that is correct: `.property.txt` (≈344 KB max measured)
   and `.hess` (≈150 KB) fit in memory, each reader still **size-caps** (16 MB) and refuses a
   pathological file, and an isosurface needs the whole `.cube` (read capped at 32 MB). The rule is
   about the unbounded log, not every file. Cube generation still uses moderate grids (80–100).
6. Job completion = marker file (`.exit_code`) **and** `ORCA TERMINATED NORMALLY` in output.
7. ORCA binaries are **never bundled or redistributed**; the app points to a user-configured
   install path. Same for the manual: indexed locally for personal use only.
8. **Pin ORCA to an explicit core set** and disable OpenMPI's own binding so the
   two don't fight: `OMPI_MCA_hwloc_base_binding_policy=none taskset -c <mask> ...`.
   The optimal mask is **measured, not assumed** — on the dev machine's hybrid CPU
   both "avoid mixing P+E cores" and "hyperthreading always hurts" turned out to be
   false. Default preset is E-cores only (machine stays usable); max-throughput uses
   all physical cores. See `wiki/orca/performance.md`.
9. **Every process boundary has a post-condition that checks the result in OUR terms** —
   never trust a third party's "finished successfully". Recompute what matters and verify
   it: `measured` is re-derived from the returned geometry, `max_static_displacement` is
   checked, atom count *and* order are asserted invariant across a round-trip. A binary that
   exits 0 having done the wrong thing is the common case, not the edge case. (Empirical
   complement to the type invariants of ADR-010 — every phase-2.5 defect was caught by a
   post-condition or a probe, not by a type.)
10. **No fact about a third-party program's behaviour is accepted from memory or docs —
    only from a run, recorded in the wiki.** The manual is wrong often enough that a claim
    only counts once a real invocation confirms it. Settled this way, each with a wiki page:
    ORCA's `%geom` index base is 0-based while xtb's `$constrain` is 1-based (opposite, both
    verified — `wiki/orca/constraints.md`, `wiki/orca/xtb.md`); an empty `--input` hangs xtb
    (`debugging/006`); `mask` silently overrides `indices` in the geometry kernel.
11. **No physical quantity crosses a parser boundary as a bare number.** Each artifact reader
    converts to the app's **canonical units exactly once, at the boundary**: lengths → **Å**,
    energies → **Eh**, frequencies → **cm⁻¹**, IR intensities → **km/mol** (all measured, not
    assumed — `wiki/orca/parse-sources.md`). Units are established only by (1) a file literal,
    (2) a numeric cross-check with a stated ratio, or (3) a determiner run — never from
    convention or memory; what none settles is `UNDETERMINED`, not guessed. **Post-condition
    (rule #9, in our terms):** a reader whose artifact contains geometry we already know (the
    first `$Geometry` vs the input xyz) recomputes it after conversion and a **missed Bohr→Å
    conversion fails loudly** (≈1.889× off) instead of animating plausible-but-wrong physics.
    The threshold-only readers (`property`/`hess`/`xyz`/`relaxscan`) assert max distance Δ below
    their tolerance; the `orca_2json` (`mo`) reader — whose `.gbw` geometry legitimately lags the
    property-final by a small **same-unit** amount on a plain `Opt` (measured 0.027 Å, no Freq) —
    instead classifies the interatomic-distance **ratio**: a **~1.889× / 0.529× signature** is the
    loud Bohr↔Å failure (either direction), a ratio ≈ 1 with small Δ is benign staleness that
    passes, and a ratio ≈ 1 with large Δ is a different-structure mismatch (not a unit error). So
    the unit guard is preserved by the ratio signature, not by a bare 1e-4 threshold
    (`wiki/debugging/019`). Named seam to preserve: `$SCF_Nuc_Gradient &grad` is a bare
    positional array; its order comes from the co-located `$Geometry` block. Why this is a rule:
    the authoritative tier spans **two unit systems** — `.property.txt`/`.hess` geometry is
    **Bohr**, `orca_2json`/`.xyz`/`_trj.xyz` is **Å** (measured) — and a stray 1.889 on a normal-
    mode displacement does not crash; it renders a believable, wrong animation (the IR-peak
    click of Phase 3).

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
| Module page | `wiki/modules/<module>.md` | One per module: responsibilities, interfaces, status, quirks. Written in the **present tense, describing the CURRENT state** — see the rule below. Update when the module changes meaningfully. |
| ORCA knowledge | `wiki/orca/*.md` | Any fact learned about ORCA behavior, formats, tools, gotchas. |
| Chemistry notes | `wiki/chemistry/*.md` | Quantum chemistry concepts the author is learning. **Write these in Ukrainian** — they are personal study notes. |
| Debugging log | `wiki/debugging/*.md` | One page per non-trivial solved bug: symptom → root cause → fix. |
| Measurement page (rule #10 record) | by **what it serves**, not "is it about the host": a measurement supporting a specific **decision** → `wiki/architecture/` (e.g. `keyring-availability.md` under ADR-015); a measurement about **ORCA's behavior or how to run it** → `wiki/orca/` (`performance.md` under domain rule #8, `parse-sources.md`, `manual-sources.md`, `input-syntax.md`). | Whenever rule #10 requires a third-party fact be recorded from a run. Pick the home by the criterion above so a third such page doesn't drift to a third location. |

Language convention: technical pages (architecture, modules, orca) in English;
`chemistry/` in Ukrainian; conversation with the author in Ukrainian.

**Module pages describe the present, not the history.** A module page states the CURRENT
state in the present tense. Do **not** grow per-unit `As built (<unit>)` sections — the
chronicle lives in `log.md`, which is the append-only record. Where a decision changed during
development, the page names the **final rule** and points to the log entry in one line, rather
than narrating each intermediate state. (Reason, recorded so this isn't relitigated: the phase-2.5
lint found five stale claims, all of them the tail of ~42 accumulated `As built` sections in the
module pages — a module page that doubles as a second chronicle drifts from the code by
construction.)

### index.md and log.md

- `wiki/index.md` — catalog of every wiki page: link + one-line summary, grouped by category.
  **Update it whenever a page is created or renamed.**
- `wiki/log.md` — append-only chronicle. Every entry starts with a parseable prefix:

  ```
  ## [YYYY-MM-DD] type | Short title
  ```

  where `type ∈ {session, decision, ingest, lint, milestone, feat, fix, probe}`.
  (`feat`/`fix` carry a real signal — a landed feature vs a bug fix — and are used
  consistently; `probe` = a measurement of a third-party program's or a host's behaviour under
  Rule #10; the vocabulary is these eight. One historical `chore` entry, 2026-08-12,
  predates this rule and reads as a `lint` pass — it is NOT re-titled, the log is
  append-only, and `chore` is not a blessed type going forward.)
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
- **A gate whose ability to fail is not demonstrated is green for an unknown reason.** For a test that
  guards an invariant (a post-condition, a preservation/coverage gate), show it *bites* — a negative
  control that deliberately breaks the invariant and confirms the test goes red (e.g. `d9a6492`:
  a render dropping inline-code content unbalances the corpus sum). Without that, a passing gate does
  not distinguish "invariant holds" from "test checks nothing."
- **Commits**: conventional commits (`feat:`, `fix:`, `docs:`, `refactor:`).
  Wiki updates ride in the same commit as the change they document.
- Small, reviewable increments. The author reads the diffs — write code to be read.

## Agentic development workflow (in Claude Code)

The whole loop — strategy, architecture, implementation, verification — runs inside Claude Code as
the main session plus subagents (`.claude/agents/`, ADR-022). Claude Web is not part of the loop.
Roles:

- **orchestrator-architect** = the main session (not a subagent, not `--agent`). It is also the
  **cross-session strategic architect**:
  - keeps `ROADMAP.md` and `wiki/log.md` as the memory between sessions;
  - decomposes phases into units;
  - writes delegation prompts in a fixed shape: **main risk first** → steps with references to the
    rules → checks with a **negative control** → wiki updates → commit → **STOP-AND-REPORT**;
  - brings every design fork to Anton with its own lean;
  - runs **every ADR (or ADR change) through the verifier in DESIGN mode before acceptance**.

  It does **not** write code and does **not** review its own plan.
- **prober** (`sonnet`) — settles third-party facts from real runs only (domain rule #10).
- **explorer** (`haiku`) — read-only codebase/wiki archaeology; returns `file:line` anchors + reuse
  candidates; separates ADR intent from code reality.
- **implementer** (`sonnet`; the orchestrator invokes it with **`model: opus`** for high-risk units:
  state machine, concurrency, reconciliation, DB migrations, parsers with chemical consequences) — one
  unit, STOP-AND-REPORT (Part A pure+tested → STOP → Part B wiring), reuse over rebuild, wiki in the
  same change. **Never commits.**
- **verifier** (`opus`, worktree-isolated), two modes:
  - **CODE** — fresh-context push ritual. It applies the orchestrator's patch of the uncommitted
    change in its own worktree, runs `tsc`/`vitest`/`cargo`/`pytest` for real, and proves each
    negative control bites. It marks render/chemistry units REQUIRES LIVE GATE and hands them to
    Anton.
  - **DESIGN** — a checklist review of an ADR before acceptance; report only.

Loop: probe → decompose → *(fork? Anton decides, ADR same session → verifier DESIGN)* → anchors →
implementer Part A → **STOP** → verifier CODE → *(Anton greenlight)* → implementer Part B → verifier
CODE → *(Anton live gate if render; chemistry gate if science)* → commit on Anton's approval.
**Two verifier FAILs in a row on the same unit or ADR → stop and escalate to Anton**; there is no
third automatic round.

**Mandatory commit binding (tree hash).**
1. The orchestrator builds the verifier's patch and the expected `git write-tree` hash in a throwaway
   index (`GIT_INDEX_FILE=<tmp>`: `read-tree HEAD` → `add -A` → `diff --cached --binary HEAD` +
   `write-tree`).
2. The verifier reports the tree hash it actually verified.
3. **Immediately before `git commit`**, the orchestrator runs `git add -A` + `git write-tree` in the
   main checkout and compares the result with the verified hash. Any difference → verify again;
   never commit a tree the verifier did not see. A **second** mismatch on the same unit is escalated
   to Anton, like two FAILs.
4. The commit is a plain `git commit` of that index, never `-a` and never with pathspecs. **Right after
   it**, `git rev-parse HEAD^{tree}` must equal the verified hash; if not, stop and report to Anton
   before anything else.
5. Before every commit Anton sees `git diff --stat`, the verifier's report and the matching tree
   hash.

**The human gates are structural, not optional.** Anton is:
- the merge approver — the usual invocations of commit-creating or publishing git commands
  (`commit`, `push`, `merge`, `cherry-pick`, `revert`, `rebase`, `pull`, `am`, inline aliases) hit an
  **`ask`** rule;
- the live WebKitGTK gate;
- the chemistry sanity gate;
- the sole resolver of design forks.

The `ask` rules prompt even in auto mode and even when an `allow` rule also matches. The verifier
cannot certify render or chemistry correctness.

**Auto mode is allowed**, because the gates do not rely on anyone remembering them:
1. **Managed settings** (`/etc/claude-code/managed-settings.json`, root-owned, installed by Anton) hold
   the gate rules:
   - `ask` on the git commands above;
   - `deny` on `sudo`/`su`/`pkexec`/`doas`, on anything naming the admin ssh alias, and on any ssh-style
     login as the `yats` account.

   No project, local or user rule can override a managed rule. The **OS** boundary is that the file
   is root-owned and the agent has no sudo password. The `sudo` deny rule is only a text guardrail on
   top of that; whether a sudo ticket could be reused from the agent's Bash is an open question
   (ADR-022 §6). The same rules are mirrored in `.claude/settings.json`, which also denies `Edit` on
   the settings files themselves. Deny and ask rules resolve before auto mode's classifier and apply
   to subagents.
2. **OS isolation on the server**: agent work on the server runs as the dedicated no-sudo user `anton`
   (ADR-024 k). The admin alias uses the **same ssh key**, so the `deny` rules are what keep agents off
   it on the laptop side.
3. **The verifier's own worktree**: Claude Code blocks tool edits, commands run from the main checkout,
   and git redirects into it. It does not block a plain Bash write to an absolute path, and the
   implementer works in the main checkout.

**Limits, stated honestly.** The Bash rules match **command text**, and the docs say such a rule
*"isn't a security boundary around the program"*.
- Ordinary one-liners slip past them: split quoting (`git com''mit`), variables (`S=sudo; $S`),
  plumbing (`git send-pack`, `git fetch . topic:main`), script files.
- Auto mode's classifier **allows pushing to the current repository by default**, so a push the
  patterns miss gets no second check.
- An installed **mod that handles `tool.check` can approve a call an `ask` rule would prompt for**,
  even a managed one (managed `deny` still holds). **No mod is installed without Anton re-checking
  this gate** (ADR-022 §6).
- The threat model is therefore a **cooperative agent making a mistake, not an adversary**.

The boundaries that hold against more than that are OS-level: root-owned managed settings, the
no-sudo `anton` account, and the verifier's worktree. The full list of residual risks is in
ADR-022 §6–§8.

Two more points:
- In auto mode a subagent's `permissionMode` frontmatter is ignored (documented), so no gate may
  depend on it.
- Subagents take CLAUDE.md as it was **when the main session started** (documented; measured), so
  every delegation prompt restates the rules and protocol it relies on.

## Division of labor

- **Claude Code** (you): the whole loop — strategy, architecture (ADRs), decomposition,
  implementation, verification, wiki maintenance, lint. Ask before destructive operations.
- **Claude Web/desktop**: **not in the loop**. Optionally used for an external review at a **phase
  boundary**; any outcome is ingested into the wiki by Claude Code on the author's request.
