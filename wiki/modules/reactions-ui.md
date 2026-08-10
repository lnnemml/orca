# Module: reactions-ui (Reactions screen)

*Phase 4.5 Stage C2 (C2a + C2b). The management UI over the reaction/pathway commands (create a
reaction, attach scan jobs as labelled pathways, detach, delete — C2a) plus the comparative overlay:
ΔΔE‡ + intrinsic barriers (C2b-1, reference-free) and the reactant reference + absolute barriers vs
separated reactants (C2b-2b).*

## What it is

`src/screens/ReactionsScreen.tsx` — a "Reactions" tab alongside Jobs/Molecules (registered in
`App.tsx`). A **thin view over the C1 commands** (`commands/reactions.rs`): it calls
`create_reaction` / `list_reactions` / `rename_reaction` / `delete_reaction` / `create_pathway` /
`list_pathways` / `delete_pathway` / `attach_job_to_pathway` / `detach_job_from_pathway` and
`list_jobs` / `read_job_results`. It adds **no data logic** — no energy/coordinate reading, no
ΔΔE‡/overlay (that is C2b).

The only extracted, unit-tested logic lives in `src/reactions/pathway.ts`:
- `isScanJob(results)` — a completed job's results carry a relaxed-scan profile
  (`results.scan` non-null with ≥1 point). Drives the attach picker's **mark/warn**.
- `isValidPathwayLabel(label)` / `normalizePathwayLabel(label)` — non-empty-after-trim validation.

Tested in `src/reactions/pathway.test.ts` (two negative controls): **C-scan-detection** (true with
a scan profile, false without / null / zero-points — a `return true` version fails the FALSE cases)
and **C-empty-label** (empty/whitespace rejected). The scan-detection bite was demonstrated RED.

## Screens

- **List** — reactions (name, description, created). "New reaction" form: name required, description
  optional → `create_reaction(name, description?)`. Selecting a row opens the detail below it.
- **Reaction detail** — the reaction's pathways (`list_pathways`), each row showing its **label** and
  the **attached job's title + status**. Rename (inline edit → `rename_reaction`), delete reaction
  (`delete_reaction`), detach a pathway's job (`detach_job_from_pathway`), delete a pathway
  (`delete_pathway`).
- **Attach a scan job as a pathway** — a label field + a job picker over **unattached completed/parsed
  jobs**. Each option is marked `✓ scan` or `(not a scan)` via `isScanJob`; picking a non-scan job
  shows an **advisory warning** ("C2b compares scan profiles — this job has none") but **does not
  block** (C1's `attach_job_to_pathway` is permissive; the comparability guard is C2b). Attach =
  `create_pathway(reaction_id, label)` then `attach_job_to_pathway(job_id, pathway.id)`.

## Pathway → job mapping (one source of truth)

A pathway row does not store its job; the **job carries the FK** (`jobs.pathway_id`). C2a exposes
`pathway_id` on the `Job` model (its 14th column) so the UI finds a pathway's job by
`jobs.find(j => j.pathway_id === pathway.id)`. This survives reload (it is not session state) and
keeps the single source of truth on the job side. Attach candidates are completed/parsed jobs with
`pathway_id === null` (not already grouped).

## The load-bearing property: jobs survive, visibly

Deleting a reaction or a pathway, or detaching, **only un-groups** — it never removes a scan job
from the Jobs list (C1's `delete_*` nulls `pathway_id` and never deletes a job). The UI must not
paper over or contradict this:
- every destructive action carries a **Tauri-dialog `confirm`** (from `@tauri-apps/plugin-dialog` —
  **not** `window.confirm`/`prompt`, which are unreliable under WebKitGTK; a silently-false native
  `confirm` would make delete a no-op) whose copy says the scan job stays in the Jobs list and only
  the grouping is removed;
- a pathway's job title is a link that **opens the job** in the Jobs detail — proving a grouped job
  is still a fully-openable standalone job with its Stage-A/B results.

Rename uses an **inline edit** (input + Save/Cancel), because the dialog plugin has `confirm`/`ask`/
`message` but no text prompt.

## Manual gate (author, real window) — PASSED (m1–m4 in the real window; Stage C2a complete)

- **m1** create "Ketone + BH₄ (si vs re)" → appears, opens.
- **m2** attach two completed scan jobs as pathways "si face" / "re face" → both listed with their
  job titles; a non-scan job in the picker is marked/warned.
- **m3** detach one pathway's job → un-groups; the scan job is **still in Jobs**, openable with its
  profile intact.
- **m4** delete the reaction → gone from the list; **both scan jobs still exist in Jobs** (the
  jobs-survive invariant, visible).

## Compare view — barriers + ΔΔE‡ overlay (C2b-1)

Shown in the reaction detail when **≥ 1 pathway carries a scan profile** (`0` → a clear empty state,
no crash). The gate is **≥ 1, not ≥ 2** (corrected by the C2b-2b manual gate): the per-pathway
intrinsic and absolute barriers are **per-pathway** (need one pathway); only ΔΔE‡ (a difference of two
maxima) needs two — so a single-pathway reaction (e.g. an SN2 with no si/re face) still shows its
barriers. `CompareView` (`src/reactions/CompareView.tsx`) overlays the pathways' scan profiles on one
recharts chart (`src/reactions/compare.ts` for the numbers, unit-tested; the chart only wires them):

- **Overlay on a shared zero** — every curve is ΔE (kcal/mol) relative to the **global minimum**
  absolute energy across all overlaid pathways, so they sit on one comparable scale. A single pathway
  renders as one curve. One colour + legend label (the pathway label) each; explicit width via
  `useContainerWidth` (**no** `ResponsiveContainer` — the WebKitGTK 0×0 class). Each maximum is marked
  in its pathway's colour.
- **Intrinsic barrier per pathway** = E(max) − E(**reactant-side** minimum), self-contained (needs no
  reference), always listed with the pathway label. The reactant-side min is the minimum over
  `points[0..argmax]` (the pre-barrier branch, scan reactant→product convention) — **not the global
  minimum**: on an exothermic scan run past the barrier into a lower product the global min IS the
  product, which would give the *reverse* barrier (corrected by the gate; see
  `chemistry/reaction-barriers.md`).
- **ΔΔE‡** = E(max_A) − E(max_B), **reference-free** (ADR-018 — the shared reactant reference cancels).
  **Shown only at ≥ 2 pathways**; at one pathway a note ("Attach a second pathway … to compute ΔΔE‡")
  replaces the number — never a NaN. For > 2 pathways a **baseline selector** shows each other
  pathway's ΔΔE‡ vs the baseline.
- **Comparability guard** (`pathwaysComparable`) — parses each job input's **method signature** (the
  identity-bearing `!`-line keywords, dropping run-type/SCF-conv/print/PAL, + SMD from `%cpcm smd true`)
  and the **scan coordinate** (kind + atoms + unit). On a method **or** coordinate mismatch it **shows
  the curves but replaces the ΔΔE‡ number with the specific reason** ("methods differ …" / "different
  scan coordinate …") — never a faked number (the C2b correctness gate; ADR-018,
  `chemistry/reaction-barriers.md`). The `NON_METHOD` drop-set covers the **full
  {Loose,Normal,Tight,VeryTight}Opt** geometry-opt convergence-preset family — these change convergence
  tightness (energy at a small level), **not** the electronic-structure method, so a **`LooseOpt` scan
  is comparable to an `Opt`/`TightOpt` reference** on the same functional/basis (r1-gate fix,
  2026-08-09; `LOOSEOPT`/`NORMALOPT` had been omitted beside the already-present `TIGHTOPT`/
  `VERYTIGHTOPT`). A genuine functional/basis/dispersion/solvation difference (B3LYP vs r²SCAN-3c, …)
  still yields different signatures and still refuses — the presets are neutralized, the method is not.
  **Screening-level caveat:** a `LooseOpt` maximum is slightly less converged than a tighter reference,
  so the screening barrier carries a little extra uncertainty — acceptable at this level; tighten both
  for a publication ΔG‡ (Stage E).
- **Honest + reference-free notes** — maxima are *approximate TS (scan maximum)*, ΔΔE‡ is a *screening*
  value, and a note says absolute (vs separated reactants) barriers need a reactant reference (added in
  **C2b-2b**, below).

The reaction detail reads each finished job's `results` once (`read_job_results`) into a `resultsById`
map, reused for both the attach picker's scan mark/warn and the compare view's profiles — no re-parse
(ADR-012); the profile is B1's `results.scan`.

## Reactant reference + absolute barriers (C2b-2b)

The UI half of C2b-2 (ADR-018): manage the summed reactant reference (the C2b-2a data model /
commands) and show the **absolute barrier vs separated reactants** = E(max) − Σ E(reactant jobs)
alongside the intrinsic barriers. **Additive** — ΔΔE‡ and intrinsic barriers are unchanged; the
reference is optional (no reference → the overlay is exactly C2b-1).

- **Reference management** (`ReferenceJobsSection` in `ReactionsScreen.tsx`) — reads
  `reaction_reference_energy(reaction_id)` (C2b-2a) into `refEnergy`. Lists each reference job with its
  `final_energy_eh` (or "no parsed energy") and a **Remove** (`remove_reference_job`); an add picker
  over completed/parsed jobs **not already referenced** (marked `✓ optimized` / `(scan — usually not a
  reference)`) → `add_reference_job`. Copy states the semantics (**the app sums + labels, the user
  chooses**): one job = a pre-reaction complex; two+ = separated reactants. Candidates are **not**
  filtered on `pathway_id` (a reference is an independent optimized reactant, unlike a pathway job).
- **Honest-or-absent, surfaced** — when `reaction_reference_energy.energy_eh` is `null` (any reference
  job unparsed), the section shows **"Reference incomplete — … Missing: job X"** and **no E(ref)
  number**; the overlay likewise withholds the absolute barrier (never a partial sum). No reference at
  all → the C2b-1 "needs a reactant reference" note.
- **"Separated reactants" zero** — the overlay's **ΔE relative to** selector gains a third option
  (`shared minimum` | `separated reactants`), **enabled only** when E(ref) is complete AND every
  overlaid pathway is method-consistent with the reference. Selecting it re-zeros the curves on E(ref),
  so each curve's max height reads as the absolute barrier; the chart title names the active zero.
- **Absolute-barrier column** — the barriers table gains an "Absolute barrier (vs separated reactants)"
  column (only when ≥ 1 reference job is set). Each cell shows the number **only** where the reference
  is complete + method-matching; otherwise the **reason** (incomplete / method mismatch), never a
  faked number.
- **Guard chain (three refusals, honest-or-absent)** — the absolute-barrier cell shows a number only
  after ALL pass, else the specific reason:
  1. **complete** — E(ref) non-null (every reference job parsed; else "reference incomplete — a
     reference job has no parsed energy");
  2. **method-consistent** — `referenceComparable(referenceInputs, pathwayMethodSig)` (reusing
     `methodSignature`) refuses when a reference job's method ≠ the pathway's (a B3LYP reactant under an
     r²SCAN-3c scan max is nonsense — ADR-018);
  3. **mass- and charge-balanced** — `referenceStoichiometryOk(complexInput, referenceInputs)` reuses
     **`sceneFromOrcaInput`** (the app's one `* xyz` reader, **not** a hand-rolled regex) to read each
     `* xyz <c> <m> … *` block, sums the reference **element multisets + charges**, and refuses unless
     they equal the reacting complex's atoms and charge. **Both valid shapes pass:** two references
     summing to the complex (8 + 7 = 15 atoms) OR a single reference that IS the whole complex; only a
     reference whose atoms/charge ≠ the complex's is refused. Why it exists: on the real SN2, dropping
     one of the two references left EtI alone (8 atoms) and the app subtracted it from the 15-atom
     E(max) → a confident **−60127 kcal/mol** (≈ −E(methylamine)). A composition/charge mismatch is now
     an honest refusal, never a number (the r2-gate fix). Order: complete → method → stoichiometry
     (the composition check assumes parseable, method-consistent inputs).

  Coordinate comparability among pathways is unchanged (C2b-1's `pathwaysComparable`).
- **Honest labelling** — the absolute barrier is a **screening ΔE‡** on the relaxed surface vs
  separated reactants (barrier 3), **not ΔG‡** (that is OptTS + thermochemistry, Stage E).

**Pure logic (all in `compare.ts`, unit-tested — the chart carries no correctness weight, the B2
lesson):** `absoluteBarrierKcal(maxEh, refEh)` = (max − ref)·627.509; `referenceComparable(...)`;
`referenceStoichiometryOk(complexInput, refInputs)` (composition + charge balance, reuses
`sceneFromOrcaInput`); `absoluteBarrierCell(maxEh, refEh|null, refInputs, pathwaySig, refJobCount,
complexInput)` — the honest-or-absent decision (`{ kcal } | { reason }`) that the UI renders verbatim,
so a `null` reference cannot be treated as `0` and a mismatched reference cannot become a number. Controls in `compare.test.ts`: **C-absolute-barrier** (the factor),
**C-ref-method-mismatch** (guard refuses — bite-verified: a compute-anyway guard turns three tests
red), **C-incomplete-no-number** (`absoluteBarrierCell` with `refEh = null` → a reason, not a number —
bite-verified: treating null as 0 turns it red).

**Symmetry sanity (why it's a control, not a comment):** two mirror-image TSs (si/re of an achiral
hydride on an achiral ketone) are enantiomeric → **ΔΔE‡ = 0 by symmetry**. So `deltaDeltaEKcal` on two
identical/mirror profiles is ~0; a sign or wrong-reference bug makes it non-zero — `C-symmetry-zero`
bites it. `C-guard-refuses` bites a compute-anyway guard; `C-intrinsic` pins the (max−min)·627.509 factor.

## ΔG‡ / ΔΔG‡ from a located TS — the scan-max estimate superseded (E1b)

Stage E1b **generalizes the C2b barriers from the scan maximum to a LOCATED TS** (an OptTS child on
the pathway, parsed with Freq → G) and **retires the "approximate TS" label** where one exists.
Additive: a pathway with no located TS shows exactly the C2b behaviour above.

- **Per-pathway located TS** (`ReactionDetail`, `ReactionsScreen.tsx`) — a pathway can now hold **two**
  jobs (the scan AND its OptTS refinement, both attached in E1a). The compare builder picks the **scan**
  job by its scan profile (not merely the first attached job — the fix that keeps the curve when a TS is
  also attached) and the **located TS** by `isLocatedTsInput(input)` (an `OptTS` token, matched
  **specifically**, never a bare `Opt` substring) among parsed jobs. From the TS's parsed results it
  reads `E = final_energy_eh` and `G = thermochemistry.free_energy_g_eh` (`null` unless Freq parsed).
- **Reference ΣG** — `reaction_reference_energy` now also returns `free_energy_g_eh` = Σ over the
  reference jobs' `free_energy_g_eh`, on the **same honest-or-absent discipline as Σ E**: non-null
  **only** when the list is non-empty AND **every** reference job ran Freq. A 2-of-3-have-Freq partial
  ΣG is `null`, never summed (it would look like a valid ΔG‡ denominator while being wrong).
- **What the compare view shows** (`CompareView`, a new "Located TS — ΔE‡ · ΔG‡" column, present only
  when some pathway has a located TS):
  - **Located ΔE‡** = `locatedBarrierEKcal(E(TS), ΣE(ref))` — the electronic barrier from the actual
    saddle (more accurate than the scan-max estimate). **Real ΔG‡** = `deltaGDoubleDaggerKcal(G(TS),
    ΣG(ref))` — **raw ORCA G** (ideal-gas RRHO, 1 atm), shown with a caveat (association entropy already
    in G → ΔG‡ ≫ ΔE‡ expected; the 1 atm→1 M standard-state correction is **named, not auto-applied** —
    Fork A). Both gated on the **same reference usability** as the scan-max absolute barrier (reuse
    `absoluteBarrierCell`'s verdict — complete + method-consistent + balanced), so a located barrier is
    never shown against an unusable reference.
  - **Honest-or-absent for G:** a located TS with a usable reference but ΔG‡ still `null` (the TS or a
    reference lacks Freq) shows **"ΔG‡ — re-run w/ Freq"**, never a fabricated/partial number.
  - **Label retirement:** where a located TS exists the footer says **"located TS"** shows real ΔE‡/ΔG‡;
    a pathway still on the scan maximum keeps **"approximate TS (scan maximum)"**.
  - **ΔΔG‡ headline** — for two pathways that **both** have a located TS with G:
    `deltaDeltaGKcal(G(TS_A), G(TS_B))`, **reference-free** (shared reactants cancel) and labelled
    **"standard-state cancels"** (same molecularity both faces — the correction drops out, so the raw
    ΔΔG‡ is directly comparable to experiment). This is the mission's selectivity number in free energy.

**Pure logic (all in `compare.ts`, unit-tested):** `isLocatedTsInput` (OptTS-specific),
`deltaGDoubleDaggerKcal` / `locatedBarrierEKcal` / `deltaDeltaGKcal` — all reuse the generic
`absoluteBarrierKcal` converter and all **null-propagate a missing G** (never treat `null` as 0).
Controls: **G-isLocatedTs** (OptTS true, plain Opt/scan false — bite vs a substring match),
**G-honest-absent** (any null → null across ΔG‡, located ΔE‡, ΔΔG‡ — bite vs null-as-0), **G-ddg-
reference-free** (ΔΔG‡ reads no reference — the function has no reference parameter). Rust:
`reference_gibbs_sum_incomplete_is_none_but_energy_sums` (a 2-of-3-Freq ΣG is `None` while Σ E sums).

## Connectivity check on a located TS (Stage E2)

The **Results screen** (`ResultsCard`) grows a **Connectivity check** panel
(`spectrum/ConnectivityPanel.tsx`) for a **located TS** — gated on the parsed **result**
`frequencies.imaginary_count === 1`, NOT on `isLocatedTsInput(input)`. That is deliberate: the
precondition for the check is *having one imaginary mode to displace along* (a genuine first-order
saddle), which is the same signal the IR panel's "transition state" verdict uses and which a future
**NEB-TS** result satisfies with no `OptTS` token — so the panel is E3-forward-compatible by
construction. A scan/SP/minimum (no `.hess`, or `imaginary_count ≠ 1`) never shows it.

- **The app builds BOTH displaced geometries** (`buildConnectivityChildren` → `displaceAlongImaginary
  Mode(±δ)` → `buildReoptInput({freq:false})`): the imaginary mode (the single negative frequency's
  `$normal_modes` column, reused not re-parsed) is normalized so the busiest atom moves δ (default
  **0.5 Å**, user-adjustable), then `x_TS ± δ·v̂`. Method/solvation/charge are inherited from the TS
  input verbatim and asserted (comparability + the charge footgun; same discipline as `reopt.ts`).
- **Two plain-Opt children** are created via the **generic `create_optts_job`** path (source = the TS
  job, so both join its pathway) — no scan/TS-specific branch, reused unchanged by E3's NEB. Their
  ids are persisted in `localStorage` keyed by the TS job (no schema change), so the verdict survives
  a reload.
- **Verdict** once both parse — `connectivityVerdict`: `distinctBasins` ⟺ each endpoint left the TS
  AND the two endpoints differ from each other (rotation/translation-invariant **max interatomic-
  distance** metric, no Kabsch). `reactionCoordinateChanges` tables each endpoint's key bond
  distances (forward / TS / backward) so the chemist reads reactant vs product. **Honest-pending:** no
  verdict until both children parse; a failed child is reported, never smoothed over.
- **Pure logic** (`spectrum/mode.ts`, `scene/connectivity.ts`, unit-tested): `displaceAlongImaginary
  Mode`, `connectivityVerdict`, `reactionCoordinateChanges`, `buildConnectivityChildren`. Method:
  `wiki/orca/connectivity.md`. Validated on the real MeNH₂+EtI TS (forward → product, backward →
  reactant). Manual gate m1–m3 pending.

## NEB-TS creation lives on New Job (Stage E3a-1) — the IA principle

`NebSetupPanel` (pick a reactant job + a product job → `buildNebInput` → `create_neb_job`) lives on
the **New Job screen** as a labelled, **expanded** section (not a collapsed `<details>`), NOT on the
Jobs list where it first landed. The information-architecture rule (Anton's, recorded so it isn't
relitigated): **New Job is where jobs are CREATED**, so a create path belongs there — including one
that combines two *existing* jobs into a new one. By contrast **OptTS-refine / connectivity / (later)
IRC are result-derived actions**: each depends on a specific *computed* job and its parsed results, so
each stays on that job's Results (`ResultsCard` / `ScanProfilePanel`). The dividing line is "does this
action need a specific job's results?" — if yes it lives on the results; if it only *creates* a job it
lives on New Job. The same-order refusal (`buildNebInput` throws on an atom-order mismatch) surfaces as
an honest banner in the panel, so no mismatched NEB job is ever created.

## NEB band viewer + Refine-with-OptTS — closing NEB → located TS → ΔG‡ (Stage E3a-2)

The **Results screen** (`ResultsCard`) grows a **NEB band** panel (`reactions/NebBandPanel.tsx`)
rendered iff `results.neb != null` (absent-is-normal — a non-NEB job shows no band panel). It is
presentation over the already-parsed `ParsedResults.neb` (E3a-1) plus one reuse action — **no new
parser, no Rust**.

- **Pure transforms** (`reactions/nebBand.ts`, unit-tested): `iterationSeries(it)` (each image's energy
  **minus that iteration's own image-0**, ×`HARTREE_TO_KCAL` → kcal/mol), `mepSeries(neb)` (the
  already-relative `.final.interp` MEP, ×`HARTREE_TO_KCAL`), `barrierSeries(neb)` (each iteration's
  `barrier_eh` → kcal/mol — the convergence curve). Imports the converter symbol from `scan/scanProfile`
  (same source as `compare.ts`; the value is still duplicated across four modules — a pending dedup
  tidy-up, tracked separately).
- **The absolute/relative firewall (rule #11, presentation side).** The `.NEB.log` per-iteration
  energies are **ABSOLUTE** (Eh); the `.final.interp` MEP is **RELATIVE** (image 0 = 0). They are never
  plotted as one absolute quantity: `iterationSeries` relativizes each iteration to its own reactant end
  (so iterations overlay AND share the MEP's relative footing), and the MEP is its own labelled curve.
  Both plotted series are ΔE-in-kcal/mol, each honestly relative to its own reactant; the y-axis says so.
- **The viewer** reuses the `TrajectoryPlayer` transport idiom exactly (play/step/slider, the current
  **iteration** is application state, not the chart's): each frame draws `iterationSeries(iteration)` as a
  recharts line (arc length Å vs ΔE kcal/mol) with the converged smooth MEP (`mepSeries`) overlaid as a
  distinct dashed labelled series, and the climbing image marked with a `ReferenceDot` once
  `climbing_image` is `Some`. A small barrier-convergence chart (`barrierSeries`, barrier vs iteration,
  the current iteration a vertical marker) sits below. Explicit chart width via `ResizeObserver` (the
  WebKitGTK 0×0 class); two series with different x-domains, so each `<Line>` carries its own `data`.
- **Refine TS with OptTS** MIRRORS `ScanProfilePanel`'s Stage-E1a action **exactly** (this is why E1a
  was built source-agnostic, ADR-020): read THIS NEB job's own input via `get_job` (never reconstructed),
  `buildOptTSInput(source.input_content, results.neb.ts_geometry)` (charge/method/solvation
  inherited + asserted — never a 0-default), then the **generic** `create_optts_job` (source = the NEB
  job) + `submit_job` + navigate. No NEB-specific refine path. A charge/post-condition failure surfaces
  as an honest banner, no job created. The located TS it produces flows into the **existing E1b ΔG‡
  machinery** (role from `! OptTS`, pathway inherited) with **no new code** — closing NEB → located TS →
  ΔG‡.

### Manual gate — NEB band viewer + Refine (author, real window) — PENDING

- **m1** open the Menshutkin NEB job → the band viewer shows the energy profile per iteration (slide
  0→23, the band relaxes onto the MEP), the barrier-convergence line, the climbing image marked; the
  smooth MEP overlaid.
- **m2** "Refine TS with OptTS" → an OptTS child seeded from the NEB-TS (N···C ≈ 2.35), inheriting
  r2SCAN-3c/SMD(dmf)/charge → run → located TS (one imaginary ≈ −385) → its ΔG‡ appears in the reaction
  compare (E1b), closing NEB → located TS → ΔG‡.
- **m3** (negative) a non-NEB job shows no band panel (`results.neb` null).

### Manual gate (author, real window) — PENDING (code complete; the unit stays open until it passes)

- **c1** a reaction with two scan pathways → both profiles overlaid, legend-labelled, shared zero; each
  intrinsic barrier listed; each max marked approximate-TS.
- **c2** ΔΔE‡ shown when the two scans share method/coordinate; **on two identical/mirror scans ΔΔE‡ ≈ 0**
  (the symmetry sanity — test with two clones of one scan, or the butanone si/re pair once built).
- **c3** two scans with **different methods** → curves shown, **ΔΔE‡ replaced by the reason**.
- **c4** a reaction with < 2 scan pathways → the clear empty state, no crash.

### Manual gate — C2b-2b (author, real window) — **PASSED 2026-08-09** (Menshutkin SN2: methylamine + ethyl iodide, DMF/SMD)

Passed live after the gate surfaced four hidden-assumption defects (all invisible to the prior
endothermic/symmetric fixtures), each fixed first: intrinsic-from-reactant-side-min + per-pathway
barriers at ≥ 1 (`9ea3ace`), `NON_METHOD` LooseOpt/NormalOpt (`2ed1473`), and the composition+charge
**stoichiometry guard** (`39b39e7`) — r2 caught a confident wrong number (−60127 kcal/mol), the kind
any green CI would pass. Screening result (approximate-TS ΔE‡, **not** ΔG‡): intrinsic ≈ +7.87,
absolute vs separated reactants ≈ +6.23 kcal/mol.

- **r1** add an optimized-substrate job + an optimized-BH₄ job as references → E(ref) shows as their
  sum; the "separated reactants" zero option enables; absolute barriers appear per pathway.
- **r2** remove one reference so a needed job is missing / add an unparsed job → **"incomplete — job X"**,
  no absolute number (and the "separated reactants" zero stays disabled).
- **r3** add a reference job computed with a **different method** than the pathways → the reference
  shows but the absolute barrier is **refused with the reason**; ΔΔE‡ (reference-free) still shows.
- **r4** no reference → the overlay is exactly C2b-1 (ΔΔE‡ + intrinsic + the "needs a reference" note).

## Related

- `wiki/architecture/adr-007-reaction-modeling.md` (amendment) — the ratified normalized schema.
- `wiki/architecture/adr-018-reaction-energy-reference.md` — reference-free ΔΔE‡; the comparability guard.
- `wiki/chemistry/reaction-barriers.md` — the three barriers (ΔΔE‡ reference-free, intrinsic, absolute).
- `wiki/modules/tauri-core.md` (v13 + v14) — the C1 commands + `jobs.pathway_id` + the C2a `Job` column,
  and the C2b-2a `reaction_reference_jobs` table + `reaction_reference_energy` (honest-or-absent).
- Next — CREST probe → Stage D (conformer→reaction-center rigor) → Stage E/F (ΔG‡: OptTS + Freq +
  thermochemistry), per the ratified reorder. The Phase 4.5 ΔΔE‡ story (screening ΔΔE‡ + intrinsic +
  absolute-vs-separated-reactants barriers) is **code-complete; C2b-2b's r1–r4 gate PASSED**
  (2026-08-09). Only C2b-1's **c1–c4** (ΔΔE‡) gate remains — it needs a two-pathway (si/re)
  stereochemical case the single-pathway SN2 does not provide.
