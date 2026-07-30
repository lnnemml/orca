# 007-phase1-decisions-phase3-outgrew.md — Half the results screen unreachable, and a blank header energy, on the first real molecule

**Date:** 2026-07-30 (unit 3.9) · **Area:** frontend + rust-core
**Symptom:** the author's first real run — dexketoprofen, 33 atoms, `! r2SCAN-3c CPCM(ethanol) Opt
Freq TightSCF`, 21m29s — surfaced two defects at once: (1) the Job-detail screen could not scroll
below thermochemistry, so the IR spectrum, imaginary-mode list, and minimum/TS verdict were rendered
but unreachable; (2) the job header showed `energy — Eh` while the results card right below it showed
`−843.690396 Eh`.

**The unifying root cause:** both are **Phase 1 decisions that Phase 3 outgrew.** Neither was a
mistake when made; both were invisible on ethane (8 atoms) and only appeared on the first real
molecule. So both fixes target the **cause**, not the symptom — a bigger constant or a status-
conditional layout would just be the same bug deferred.

---

## Defect 1 — `overflow: hidden` on a screen that grew content

**Root cause:** `.screen.detail` set `overflow: hidden` with a `flex: 1` log console filling the
viewport. In Phase 1 the console was the *only* thing on this screen, so filling the height was
correct. Phase 3 added the results card (trajectory player, IR spectrum, charges, thermochemistry)
**above** the console; `hidden` clipped everything past the fold.

**Fix:** the screen scrolls as a normal column (`overflow-y: auto`) and the console (and the Browse-
mode Monaco viewer) get a usable **fixed** height instead of `flex: 1` — a flex child of an auto-
height column has no space to fill and would collapse. This is the **identical** fix already applied
to `.screen.new-job` two rules above (its comment: "with the builder and templates expanded there was
no way to reach the editor"). **One layout for every job status** — deliberately *not* a status-
conditional split (running → full-height console, parsed → scroll), which would be two layouts to
test by hand and only ever tested by hand.

**Audit for the same disease:** every other `overflow: hidden` in `app.css` is on a *fixed-size* box
(the Monaco `editor-wrap` / `output-viewer-editor`, the 3D `viewer-panel`, the `mode-toggle`, the
`conv-progress-bar`), not a content-growing scroll container. `.screen.detail` was the only one left.

---

## Defect 2 — the header energy read from a fixed-size output tail

**Root cause (measured):** the job header/list energy came from `jobs.energy`, written by a regex over
a **64 KB tail** of `output.out` (`RESULT_TAIL_BYTES`). On dexketoprofen the last
`FINAL SINGLE POINT ENERGY` is **164 186 bytes** from EOF (file 1 039 023 B) — the whole 99-mode
normal-mode block, IR table, and thermochemistry sit between it and the end. The 64 KB window misses
it by 2.5×, so `jobs.energy` stayed NULL while `results.final_energy_eh` (parsed from
`.property.txt`) held −843.690396.

**Why not bump the constant:** the next molecule pushes the energy further back — a moving target —
and it contradicts **[ADR-012](../architecture/adr-012-output-parsing-ownership.md)**, which already
ruled `output.out` is **not** an authoritative source. The tail regex's real job is a **live estimate
during a run**, when `.property.txt` does not yet exist.

**Fix (three parts):**
- **Header energy from the authoritative tier.** After a successful parse, `jobs.energy` is
  overwritten from `results.final_energy_eh` (`local_backend::parse_results_after_completion` →
  `results::stored_final_energy` → `set_job_energy_conn`). The regex stays the run-time estimate; the
  two roles are now explicit.
- **Old jobs backfilled.** Migration v7→v8 fills `jobs.energy` from `results` where it is NULL and a
  parsed result exists — chosen over a read-time JOIN so the fix is a one-time data correction with no
  standing query change, and old jobs don't stay blank forever.
- **A post-condition that would have caught it (rule #9), the real prize.** The optimization-cycle
  energies now have **two independent sources** — the streaming `.out` convergence parser (a regex
  over a format ORCA can shift in 6.2) and the `_trj.xyz` frame comments (`E <energy>`, unit 3.7).
  `results::cycle_energy_cross_check` compares them after a run: counts (`n_traj ∈ {n_opt, n_opt+1}` —
  the trajectory carries one optional trailing converged-geometry frame, measured) and values (< 1e-6
  Eh). A divergence is a recorded `ParseFailed` diagnostic, not silence or a panic. **Gated to
  non-GOAT** — a GOAT trajectory is conformers, not one optimization's cycles (17 inner-opt blocks vs
  18 conformer frames, measured), so the two are unrelated by construction. Measured agreement on
  dexketoprofen / ethane / saddle: bit-for-bit on the shared prefix.

**Lesson / rule:** a fragile text-format parser needs a *second source* to check it, or it breaks
silently on the next version. Here the second source already existed (`_trj.xyz`) — the cross-check
just made the two look at each other. The 33-atom output-tail gap is now a committed regression
(`final_energy_sits_past_the_estimate_window_on_a_real_big_molecule`).

**Fix commit:** see the unit-3.9 `log.md` entry.
