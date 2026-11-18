# ADR-025: `%pal` is the run target's by default

**Status:** Accepted · 2026-11-17 (decided by Anton in the uni run) · DESIGN round 1: PASS WITH
FINDINGS (4 MED + 7 LOW), applied; forks of items 8–9 decided by Anton 2026-11-17 · DESIGN round 2:
PASS WITH FINDINGS (3 MED + 6 LOW), applied

Amends [ADR-024](adr-024-remote-execution-intermittent-connectivity.md) o item 14.2 in its
*inputs*, not its rule: the remote alignment stays downward-only. Rides on domain rule #8 and
[orca/performance.md](../orca/performance.md) ("Memory ceiling on nprocs").

## Context

The B4 uni run (2026-11-17, [remote-live-run-b4.md](remote-live-run-b4.md) §Step 1) ran two
r²SCAN-3c Opt Freq jobs with **4 MPI ranks on a 24-CPU mask** — 20 CPUs idle. Cause (measured; line
numbers below are **at a4dcfde**, before this ADR's change):

- The input builder writes `%pal nprocs ${state.nprocs} end` with `DEFAULT_BUILDER_STATE.nprocs = 4`
  (`src/input-builder/build-input.ts:104,273`), and the templates write a literal `%pal nprocs 4 end`
  (`src/templates/orca-templates.ts:38`). No other **app path** emits `%pal` (a probe script in
  `sidecar/probes/` does; it is not an app path).
- The builder state reaches inputs through these producers:
  - the form (`InputBuilderForm.tsx`) — and **NEB**, which spreads the form's own `state`
    (`src/scene/neb.ts:142`, via `reactions/NebBuilderSection.tsx`), so NEB follows the form's value;
  - `DEFAULT_BUILDER_STATE` spreads: OptTS (`scene/optts.ts`, both branches), the GOAT-conformer
    re-opt and the IRC endpoints (`scene/reopt.ts` `buildReoptInput`, used by `scene/connectivity.ts`),
    and the CREST re-opt (`crest/reopt.ts`).
- Locally the 4 is invisible: `LocalBackend` rewrites `%pal` to the preset's rank count in both
  directions (`align_pal_nprocs`).
- Remotely ADR-024 o14.2 aligns **downward only** (`min(input nprocs, mask CPUs)`) because "a
  deliberately small `%pal` is never raised" — per-rank `%maxcore` multiplies with it. It cannot tell
  a deliberate 4 from the builder's placeholder 4, so the placeholder is honoured as a choice.

Both backends already treat **an input with no `%pal`** as "use the run target's count": local
inserts the preset's ranks right after the `!` line (`local_backend.rs` `align_pal_nprocs`, test
`align_pal_inserts_after_bang_line_when_absent`); remote inserts the mask's distinct-CPU count on
every attempt — first submit, Retry and the 5.4c pending re-check all pass `write_local_dir` →
`align_remote_pal`, whose post-condition demands exactly one `%pal` (`remote/backend_e2e_tests.rs`
covers the absent case). The GOAT path (`NewJobScreen` "Find conformers") already relies on this. No
consumer of an input requires a `%pal` line, and no path reaches ORCA without one inserted.

## Decision

1. **Auto is the default** (Anton). `BuilderState.nprocs` is typed **`number | null`** (a required
   key, not `nprocs?:`); `null` = **auto** and is the default. `buildOrcaInput` emits `%pal nprocs N
   end` **only when `nprocs !== null`** (never a truthiness test); auto emits **no `%pal` line**, and
   the backend that runs the job supplies it.
2. **The form field.** The form keeps the field's **raw text** in local state (`nprocsText`); the
   input is `type="text" inputMode="numeric"` (not `type="number"`, whose value sanitisation reports
   half-typed text such as "-" or "1e" as `""` — the empty-means-auto case — and whose WebKitGTK
   behaviour is unmeasured, rule #10). Placeholder "auto" and a one-line hint (the run target sets it).
   Parse: trimmed `""` → `null` (auto); `/^\d+$/` with value > 0 → that number (explicit); anything
   else (0, a negative, a non-integer, any other text) is **invalid** (item 9). `BuilderState.nprocs` is
   set **only from a valid parse**; it never holds NaN or an invalid value.
3. **The templates drop their `%pal nprocs 4 end`** (auto). `%maxcore` stays as it is.
4. **The backends are unchanged.** Local: aligned to the preset's ranks (both directions, as today;
   for **builder output** the local result of an auto input is byte-identical to today's, since the
   insert lands on the line the builder used; for a template only the blank line after `!` moves below
   the inserted `%pal`, harmless). Remote: o14.2 unchanged — no `%pal` → the mask's distinct-CPU count; an explicit
   `%pal` → `min` (downward only). An explicit value is still never raised on a server.
5. **Where the count is visible.** The lasting record is ORCA's own banner in `output.out` ("Program
   running with N parallel MPI-processes", measured in
   `src-tauri/tests/fixtures/xver/opt_freq_water/v611/mayer_tail.out:51`). Besides it: the local `[OrcaStudio] %pal nprocs aligned to
   N (cpu preset: …)` line is a transient live-log emit; the remote `%pal` notice reaches the view only
   for a user-initiated submit or Retry, not for a 5.4c submit the poller sends. No new surfacing is
   added here.
6. **Stored inputs are not migrated.** `jobs.input_content` is the user's text (o14.2) and no path
   edits a stored input or rebuilds it from builder state, so an existing draft or queued row that holds
   `%pal nprocs 4 end` keeps it and a Retry re-derives it as 4. **Recreating the job is the only route**
   to auto for such a row. No DB migration.
7. **Memory stays 5.5's.** Auto raises the rank count on a server, so `%maxcore × N` grows with it.
   Measured for uni (2026-11-17, [remote-live-run-b4.md](remote-live-run-b4.md) §Step 1, `/proc/meminfo`):
   62 GiB total, 59 GiB available. *Inferred* (from ADR-024 h's model "peak ≤ nprocs × `%maxcore`",
   not measured by any project run): the builder's `%maxcore 2000` × 24 ≈ 45–47 GiB fits (ORCA's "MB" read as 10⁶ B or MiB — unmeasured; it fits either way). A RAM check
   against `%maxcore × N` belongs to the 5.5 preflight and is **not** added here; until then a large
   `%maxcore` with auto on a big mask is the user's to size. **Pre-existing exposure, unchanged by this
   ADR:** locally the Interactive preset (8 ranks) × `%maxcore 2000` ≈ 16 GB already exceeds the
   laptop's recorded ceiling (~1400 MB at 8 ranks, `orca/performance.md`), and 5.5's preflight is
   scoped to server profiles — no unit covers local memory today.
8. **Derived jobs** (OptTS, GOAT/IRC re-opt, CREST re-opt) build from `DEFAULT_BUILDER_STATE`, so they
   become auto even when the source job had an explicit `%pal` (before, they got 4 only by accident).
   **Intended** (Anton): derived jobs are auto; they do not inherit a source's `%pal`.
9. **Invalid field input** (item 2; Anton): a field error under `nprocs` (a new `role="alert"` line —
   not the parity warning's `role="status"`, which by design never blocks) and **both Generate controls
   are disabled** until the text is a positive integer or the field is cleared (= auto): the form's
   "Generate Input" (`InputBuilderForm.tsx` ~l.209) and, for `NEB-TS`, `NebBuilderSection`'s Generate
   (its `canGenerate` gains the form's validity, passed in as a prop). An invalid value is never emitted
   and never silently turned into auto; `onGenerate` is not called while invalid.
10. **Post-condition and gates (rule #9, conventions).**
    - `buildOrcaInput` with `nprocs: null` yields **no** line starting `%pal`; with an explicit N
      exactly **one** `%pal nprocs N end` (the existing explicit-8 pin `build-input.test.ts:312-317`
      stays).
    - Each `DEFAULT_BUILDER_STATE` producer (OptTS both branches, `scene/reopt.ts`, `crest/reopt.ts`)
      yields no `%pal`; NEB is tested both ways (form auto → none; form explicit N → N).
      `crest/reopt` takes a **cluster**, not a source input, so its gate asserts "no `%pal`" without a
      source `%pal` to be carried.
    - Form: clearing the field yields no `%pal`. For "0", "2.5", "-1" and a non-numeric string: the
      alert is visible, **both** Generate controls are disabled (the NEB one with `jobType = NEB-TS`),
      and `onGenerate` is never called; typing a valid integer afterwards re-enables them and emits it.
    - Derived jobs (item 8): each producer test (`scene/optts.ts` — `optts.test.ts:65,146` already —,
      `scene/reopt.ts`, `crest/reopt.ts`) uses a source holding `%pal nprocs 4 end` and asserts the
      child has **no** `%pal`.
    - Templates: every `ORCA_TEMPLATES[i].inputContent` has zero lines starting `%pal` (new test —
      the templates have none today).
    - Expected snapshot updates: `InputBuilderForm.test.tsx` golden (l.40-50) and the `optts.test.ts`
      inline baseline (l.157-167); the **only** allowed diff is the removed `%pal` line — no blind `-u`.
    - Negative controls: default back to `4` → the auto tests red; `%pal` emitted for `null` → red;
      the form keeping `Number(value)` (empty → 0) → the clear-field test red; the `disabled` predicate
      dropped, or invalid mapped to `null` → the invalid-input tests red; a child copying the source's
      `%pal` → the derived-job tests red; the template line
      restored → the template test red.
11. **Wiki in the same change:** a pointer in ADR-024 o14.2 ("ADR-025: the builder emits no `%pal` by
    default"); `wiki/modules/frontend.md` l.198 and l.226 (present-tense "`%pal nprocs 4 end`" /
    "nprocs 4"); `wiki/index.md` (ADR-025 entry); `wiki/log.md`; `ROADMAP.md` if an item exists.

## Alternatives rejected

- **B — raise remotely when `%maxcore × N` fits the server's RAM.** Needs the server's RAM and a
  memory model; that is 5.5's preflight. It would also override a user's deliberate small value.
- **C — keep the 4; the user edits `nprocs` by hand.** The default would stay wrong on every server
  and on any future larger mask; the barrier this app exists to lower.
- **Treat a stored `nprocs 4` as auto.** It cannot be told from a deliberate 4; guessing breaks o14.2.

## Consequences

- New builder-made jobs use the whole mask on a server (24 ranks on uni) and the preset locally (no
  local change).
- Old rows keep their 4 until recreated (item 6).
- The local path still raises an explicit value up to the preset, unlike the remote path; that
  asymmetry predates this ADR and is not changed here.

## Open

- The memory exposure of item 7 until 5.5 (and locally, with no owning unit).
- **`PALn` precedence** (carried from ADR-024 o14.2's Open, unmeasured): an editor input that sets its
  count with `! … PAL8` reads as "no `%pal`" to `read_pal_nprocs`, so a backend inserts the full count
  beside it; which of the two ORCA honours is not measured.
