# 012-xtb-completion-gate-both-ways.md — the xTB completion gate was wrong in BOTH directions

**Date:** 2026-08-06 · **Area:** rust-core (`xtb.rs`), external tool (xtb 6.6.1 `builduser@buildhost`)
**Symptom:** the author's **xTB pre-optimize** on dexketoprofen+BH₄⁻ (34 atoms, no constraints)
reported failure — "xtb did not terminate normally" — even though xtb had finished and written a
**correct optimized geometry** (`xtbopt.xyz` + `.xtboptok`). The scene was not updated.
**Root cause:** the completion gate was `tail(xtb.out, 30).contains("normal termination")`. Measured:
`normal termination of xtb` is printed to **stderr** and then **~41 lines of stdout** (the post-opt
Wiberg bond-order table) follow it, so the marker is **buried** past a 30-line tail → false negative.
And the same gate is a **latent false positive**: a non-converged run (`--cycles 2`) exits 0, prints
`normal termination`, and writes a **non-optimized** `xtbopt.xyz`, which the gate would silently
accept.
**Fix:** anchor completion on **results in our terms** — an optimized geometry present + parseable,
and **no `FAILED TO CONVERGE GEOMETRY OPTIMIZATION`** line (scanned over the whole size-capped log).
`normal termination` and the exit code become **named diagnostics**, not gates.

---

## The two failure modes, both measured on the real binary

xtb **6.6.1 (`builduser@buildhost`, 2023-08-07)** — the exact build on the author's machine.

**1. False negative (the reported bug).** On the author's real kept dir, `xtb.out` is 1533 lines;
`normal termination of xtb` is at **line 1492 — 41 lines from the end**, under the bond-order table.
`tail(30)` never sees it. Yet `xtbopt.xyz` + `.xtboptok` are present and the geometry is good. So a
finished, correct optimization was rejected.

**2. Latent false positive (found while fixing #1).** Re-running the same input with `--cycles 2`:

```
$ xtb input.xyz --opt --gfn 2 --chrg -1 --uhf 0 --cycles 2 ; echo "exit=$?"
   *** FAILED TO CONVERGE GEOMETRY OPTIMIZATION IN 2 ITERATIONS ***
exit=0
$ ls .xtboptok xtbopt.xyz          # BOTH exist
$ grep -c "normal termination" xtb.out   # → 1  (printed anyway)
```

So on non-convergence xtb still exits 0, writes `.xtboptok` + a **non-optimized** `xtbopt.xyz`, and
prints `normal termination`. A gate on `normal termination` or the exit code would apply that
non-optimized geometry to the scene — a **silent substitution**, the worst kind (the geometry then
seeds a multi-hour ORCA run).

**Conclusion:** `normal termination` and the exit code **lie in both directions**. Neither can gate.

## The fix — a results-anchored, pure classifier

`xtb::classify_completion(out_text, geometry_ok) -> XtbCompletion` (`Ok` | `NonConvergence{line,
iterations}` | `NoGeometry`), checked in this order:

1. **`FAILED TO CONVERGE GEOMETRY OPTIMIZATION` present?** → `NonConvergence` (quote the measured
   line + parse `N`). Checked **first**, because a non-converged run *has* a geometry file.
2. else **no parseable `xtbopt.xyz`?** → `NoGeometry`.
3. else → `Ok`.

Then the existing count / element-order / `check_held` post-conditions run. On `NonConvergence` /
`NoGeometry` the error message appends the **named diagnostics** (exit code, `normal termination`
seen — both flagged as non-gates) and the scratch dir is **kept** (rule #3 keeps evidence on
failure), so the user can inspect the non-optimized geometry; it is **not** applied.

- The FAILED marker is scanned over the **whole size-capped `xtb.out`** (16 MB cap, rule #5), not a
  tail: measured, the marker sits **436 lines from the end** in the `--cycles 2` log (the full
  property analysis runs even after giving up), so a tail cannot catch it.
- The exit code is captured for diagnostics but **never gated** — it is 0 on non-convergence and can
  be non-zero on a clean run whose teardown trips an `IEEE_UNDERFLOW`/`DENORMAL` FP exception.

## Fixtures (real, not synthesized)

- `src-tauri/tests/fixtures/xtb_success_dexketoprofen_bh4.out` — the author's real clean run (buried
  `normal termination`, good geometry — the false-negative case);
- `src-tauri/tests/fixtures/xtb_fail_cycles2.out` — the same input re-run `--cycles 2` (real
  non-convergence).

The classifier is tested on both. **Negative controls:** (a) the clean fixture through the *old* gate
reproduces the false negative (`old_gate_false_negatives_on_the_real_clean_run`); (b) removing the
FAILED scan makes the classifier accept the non-converged fixture — shown red, then restored
(`a_gate_without_the_failed_scan_would_accept_the_non_converged_run`).

## Not touched

- **Spawn / killpg / isolation / cancel / timeout** (`debugging/004`) — unchanged; cancel and timeout
  remain gates (they are not "xtb finished").
- **GOAT** — a different path (`! XTB GOAT` via the ORCA binary, parsed by the ADR-012 readers); it
  does **not** share this gate, so it was not touched.

## See also

- `wiki/orca/xtb.md` — the completion-signals table (this measurement, as a domain fact).
- `wiki/debugging/006-xtb-empty-input-hang.md` — the earlier xtb 6.6.1 quirk; same "keep the dir on
  failure, the evidence is the log" discipline that made this findable.
