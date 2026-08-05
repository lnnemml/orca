# ADR-014: AI integration boundary

**Status:** accepted · 2026-08-01
**Narrows:** [ADR-007](adr-007-reaction-modeling.md) §"AI integration (Phase 6+ / long-term)" — its
Level 1–4 ladder describes *what* AI does; this ADR fixes *what AI is allowed to touch*. Same
precedent as [ADR-012](adr-012-output-parsing-ownership.md) narrowing ADR-002 and
[ADR-013](adr-013-manual-indexing-ownership.md) narrowing ADR-006: ADR-007 is **not edited** — history
is narrowed by a new ADR, never rewritten.

## Context

ADR-007 sketched an AI ladder (L1 reaction-setup assistant, L2 literature-informed defaults, L3
result interpretation, L4 multi-step mechanism exploration) as a *capability* wishlist. The agentic-AI
field has since made the question concrete rather than hypothetical: by mid-2026 several systems drive
real quantum-chemistry engines from natural language — El Agente over ORCA, ChemGraph over ORCA /
NWChem / Psi4, Aitomia over Gaussian / ORCA / xtb, and Schrödinger's Bunsen over its validated
physics stack (full register in [ai-landscape.md](ai-landscape.md)). "AI over ORCA" is no longer a
future idea; it exists. That makes it urgent to fix, *before* Phase 4's "Explain with Claude" ships,
**where the boundary of the AI's authority runs** — so Phase 6 does not re-decide it ad hoc, possibly
more permissively.

ADR-007's ladder answers a different axis (*what AI does*) than the one that governs trust (*what AI
may touch*). Two numbered AI ladders in the wiki with no stated relation is a guaranteed lint finding;
this ADR makes the mapping explicit and adds the authority axis ADR-007 never had.

## Decision

### (1) AI is never inside the numerical pipeline

Energies, geometries, thermochemistry, parsing, and unit conversion are produced **only** by
deterministic code: the ADR-012 Rust readers, the ASE geometry kernel, `ir.ts`. The AI **reads what
they emit**; it never produces a number that lands in a plot, a table, or the coordinates of an input
file.

**Why.** Rule #11 (no physical quantity crosses a parser boundary as a bare number) already forbids
even *our own* code from moving a value without conversion at the boundary; an LLM emitting a physical
quantity is that violation in its worst form — a number with no boundary and no provenance. And the
reputational stakes are asymmetric: one fabricated figure in a paper ends trust in the whole tool.

**(1a) Geometric constants are RETRIEVED, never RECALLED.** AI may **point at** a value that exists in
the app's own curated data — [wiki/chemistry/burgi-dunitz.md](../chemistry/burgi-dunitz.md), the
reagent library (`src/scene/fragment-library.ts`), the manual FTS index — and it **must name the
source**. It may **not** emit a bond length, angle, or dihedral from training data. ADR-007's **Level
2** ("literature-informed defaults", which as written admits values "from literature *or training
data*") is admitted **only** in this retrieval form. This does not cancel L2 — it makes it
implementable: the curated Bürgi–Dunitz page already exists and is the correct source for a
~107° attack angle.

**Scan windows, step sizes, and point counts are geometric constants in the sense of this rule.** The
number that decides whether a profile crosses the barrier at all is not a coordinate — it is the scan
range, e.g. `%geom Scan B 3 17 = 1.5, 3.0, 12 end end`. A range like 1.5–3.0 Å is formally a bond
length under this rule, but a reader scanning for "typical TS geometries" would miss it, so it is
named explicitly here. The AI may propose a scan range only **(a)** by pointing at a curated entry, or
**(b)** by **deriving** it from the current *measured* geometry — a computed number carries
provenance, a recalled one does not. **"Typically 1.5 to 3.0 Å" is forbidden.**

The empirical reason a recalled geometric constant is dangerous is the same one that grounds decision
(3) — see (3): a confident model is systematically wrong about third-party behaviour. The claims
differ (there, third-party software behaviour; here, recalled physical constants), so this is a
pointer, not a copy.

### (2) Three tiers of authority, each opened separately

| tier | what it may touch | constraint / gate |
|---|---|---|
| **T1 explain / diagnose** | read-only | grounded on the FTS manual (Phase 4) and parsed results (ADR-012) |
| **T2 draft** | proposes input text / methodology as **text the author reads before Run** | never submits by itself |
| **T3 orchestrate** | an MCP server over the command layer — **invokes what spends compute** | depends on Phase 4.5 |

- **T1** is already in the ROADMAP as "Explain with Claude".
- **T2** proposes an `.inp` draft or a methodology; the artifact **stays text the author reads
  *before* Run**. It never submits.
- **T3** calls commands that consume computation. It waits on Phase 4.5: while the central object is
  the **Job**, not the **Reaction**, there is nothing to orchestrate (it would only be "run three
  identical calculations"). The value appears when a single action means *"build the si and re
  approaches, scan both, compare ΔΔE‡"*.

**Mapping to ADR-007's ladder (the two axes are orthogonal — L = what AI does, T = what AI may
touch):**

| ADR-007 capability | ADR-014 authority |
|---|---|
| **L3** result interpretation | **T1** (read-only) |
| **L1** reaction-setup assistant | **T2** (draft; author reads before Run) |
| **L2** literature-informed defaults | **T2**, restricted by **(1a)** — retrieved, never recalled |
| **L4** multi-step mechanism exploration | **T3** (MCP, after Phase 4.5) |

### (3) Methodology is an executable guard, not a prompt

Method rules are **tool refusals**, not advice in a system prompt: `! XTB GOAT` before building a
pathway, `TightOpt` before `Freq`, a warning when ΔΔE‡ is requested without thermochemistry. These are
enforced by the command layer, so they hold regardless of what the model was told.

**None of the three is implemented today** — this describes the intended invariant, not a current one.
All three land with the **Phase 4.5** scan/pathway generator: the `! XTB GOAT` and `TightOpt`-before-
`Freq` refusals are guards on that generator's outputs, and the ΔΔE‡-without-thermochemistry warning
sits in the same place (it guards the ΔΔE‡ comparison Phase 4.5 introduces). Named here as debt, in
the manner of decision (4); no code in this unit.

**Design criterion:** the MCP surface must be **safe with a mediocre model**; a strong model is
upside, not a precondition. **Why empirical:** rule #10 exists precisely because a confident model is
systematically wrong about third-party software (0-based `%geom` vs 1-based `$constrain`; empty
`--input` hangs xtb). A boundary that relies on the model being smart is a boundary that fails on the
model's worst day.

### (4) Commands are designed as an API, not as UI handlers

The existing `*_conn(&Connection)` layer already provides half of this. **The concrete consequence,
and a named hole:** the threading rule (`tauri-core.md`) makes long operations **event-driven**, but
an MCP client does **not** listen to Tauri events — so **every compute-spending operation must expose
a *pollable* path to its status/result, not only an event**. `submit_job` has one (`get_job`);
`xtb_optimize` does **not** (it emits only `xtb:done`). This is a rule for **new** commands;
retrofitting `xtb_optimize` is a **named debt**, not a task of this unit — and no code changes here.

### (5) The model is a rented asset

No in-house model is trained for orchestration. The only place training is even on the table is
MLIP / delta-correction, which is **out of scope for this ADR**.

## What does not change

The phase order stays **4 → 4.2 → 4.5 → 5**. A competitor's release ([ai-landscape.md](ai-landscape.md))
is **not** grounds to reorder phases — recorded here so the question is not reopened. T3 remains gated
on Phase 4.5 by decision (2); nothing in this ADR pulls AI orchestration earlier.

## Consequences

- `ROADMAP.md` — Phase 6 gains an explicit item: *MCP server over the Tauri command layer (T3 of
  ADR-014) — depends on Phase 4.5*; and the existing "AI-assisted reaction setup" item points to this
  ADR. No other ROADMAP change, no marker change.
- `wiki/modules/tauri-core.md` — the "Commands" section records the **pollable-path rule for new
  commands** (decision (4)), naming `xtb_optimize` as the outstanding event-only case. No code change.
- [ai-landscape.md](ai-landscape.md) — the standing register of agentic-AI systems this ADR reacts
  to; its *What it confirms* section is the same boundary as decision (1).
- ADR-007 is unchanged (narrowed, not edited). ADR-006 / ADR-013 untouched.
- No new dependency; no code in this unit.

## Amendment (2026-08-05) — [ADR-015](adr-015-api-key-storage.md) makes T1 structural

The decision text above **stands unchanged**; this records a narrowing made by a later ADR (same
precedent as ADR-013's own amendments). Decision (2) described the T1 tier's authority as a *property*
— "read-only, grounded on the FTS manual and parsed results". A boundary stated as a property, not
built as one, holds only as long as everyone respects it. **ADR-015 (3) + its "Consequence for ADR-014"
close that for T1 by construction:** the Phase-4 explain command accepts **exactly** the selected word,
its surrounding line, and the section text — it has **no parameter** for the input file or the
coordinates, so the "read-only, bounded" boundary is the command's *type*, enforced by the compiler,
not by intent. ADR-015 also fixes the two questions ADR-014 left implicit for T1: **where the secret
lives** (system keyring, not the copied `.db`) and **where the network call is made** (Rust, so the key
never enters webview scope). This refines, does not cancel, decision (2).
