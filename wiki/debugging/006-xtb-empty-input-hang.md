# 006-xtb-empty-input-hang.md — xTB pre-optimize hangs 300 s on a no-constraint run

**Date:** 2026-07-29 · **Area:** rust-core (`xtb.rs`), external tool (xtb 6.6.1)
**Symptom:** the author ran **xTB pre-optimize** on dexketoprofen (C16H14O3, 33 atoms,
one fragment, **no constraints**) and it sat silent for 300 s, then failed on the
timeout. No geometry, no diagnostic.
**Root cause:** the app wrote an **empty `xcontrol`** file and still passed
`--input xcontrol`. xtb 6.6.1 given an **empty `--input` file hangs at 99 % CPU
before the first optimization cycle** — regardless of molecule, `OMP_*`, or opt level.
**Fix:** `build_xcontrol` returns `Option<String>` (`None` = nothing to write); one
value decides both whether the file is written and whether `--input` is passed. Pure
`xtb_args(has_xcontrol, …)` builds the argv, and a test asserts `--input` is present
with constraints and absent without.

---

## The symptom, precisely

`xtb.out` reached ~840 bytes (the banner + setup) and then **nothing** for the whole
run, while xtb burned one core at 99 %. Zero `CYCLE` markers. So this was a hang at
**startup**, not slow convergence — an important distinction the live-progress work
(2.5.5-fix-2) surfaced: a stalled clock with cycle 0 means "never started", not
"still optimizing".

The app made it worse before this could even be read: `remove_dir_all` was
unconditional, so `xtb.out` was deleted on the timeout — the evidence gone exactly
when needed. That was fixed first (2.5.5-fix-2: keep the dir on failure, tail the log,
stream progress); only then was the cause diagnosable.

## Hypotheses we had to reject (in order)

Guessing the fix here would have been easy and wrong. The measured table (xtb 6.6.1,
timeout 45 s, `124` = killed):

| hypothesis | test | result |
|---|---|---|
| the molecule is pathological | ibuprofen (33 atoms), same invocation | **also hangs** — not the molecule |
| our `OMP_*` env is wrong | same run, no `OMP_NUM_THREADS`/`OMP_STACKSIZE` | **still hangs** — not OMP |
| the default opt level is too tight | `--opt loose`, `--opt crude` | **still hangs** — not the opt level |
| **the empty `--input` file** | drop `--input xcontrol` | **converges in 0.3 s, 16 cycles** ✅ |

The **ibuprofen control was the key**: the same molecule that pre-optimized fine in
2.5.5 (with a constraint) hangs identically with an empty `xcontrol`. That moved the
fault off the molecule and onto the **invocation**.

## Why it wasn't caught earlier

**Every xtb run in 2.5.5 had a non-empty `xcontrol`** — they all had constraints
(that was the feature under test: hold a reaction coordinate). `build_xcontrol`
returning `""` for zero constraints, the file being written anyway, and `--input`
being passed anyway, only combined into a hang on the **first real no-constraint
run** — which dexketoprofen was. Three decisions (what content, whether to write the
file, whether to pass the flag) had drifted apart, and nothing exercised the empty
path until a user did.

## The fix (one source, not an extra `if`)

- `build_xcontrol(...) -> Option<String>` — `None` when there is nothing to write.
- `None` → the `xcontrol` file is **not created** AND `--input` is **not passed**;
  `Some` → both. Both reads come from the one `Option`, so they cannot disagree.
- `xtb_args(has_xcontrol, charge, uhf) -> Vec<String>` builds the argv purely; the
  unit test `argv_includes_input_only_with_an_xcontrol` asserts `--input` is present
  with constraints and absent without — **the test that would have caught this in
  milliseconds instead of five minutes.**

Verified with real xtb 6.6.1 on dexketoprofen: no-constraint (fixed argv, no
`--input`) → **1 s, 16 cycles, converged**; with-constraint (`--input xcontrol`) →
**<1 s, 17 cycles, converged, constraint applied** (so the flag is not lost — the
likely regression, checked).

## Lesson / rule

- **When a value is used to make several downstream decisions, compute it once and
  let them all read it.** The bug lived in the gap between "the content is empty" and
  "but the file and the flag don't know that."
- **A pure argv builder is worth a test.** Shelling out is hard to test; the *command
  we build* is trivial to test, and that's where this bug was.
- **`gotchas.md` / `wiki/orca/xtb.md`:** xtb 6.6.1 hangs on an empty `--input` file —
  a foreign-binary quirk we don't own but must not step on. Candidate for an upstream
  xtb issue (the author decides whether to file it; not sent from here).
