# ADR-026: The New Job draft survives a tab switch

**Status:** Accepted · 2026-11-19 (forks decided by Anton 2026-11-19) · DESIGN round 1: FAIL
(1 HIGH + 6 MED + 8 LOW, all applied; one new fork, item 5, decided by Anton) · DESIGN round 2: PASS
WITH FINDINGS (1 MED + 4 LOW), applied · Amended 2026-11-19 after Part A's CODE review (items 5, 8,
9; the item 8 refusal decided by Anton), reviewed with Part B · Amended after Part B's CODE review
(item 9's honest display decided by Anton; items 7, 10 and Consequences reworded to the code)

Reverses the trade-off recorded in [modules/frontend.md](../modules/frontend.md) §App shell ("an
in-progress New Job draft is discarded on a tab switch (accepted)"). Rides on ADR-008 #10 / ADR-017
(the scene store is the single source of truth for geometry, a fold over its operation log) and on
the CLAUDE.md convention "state via Zustand".

## Context

Anton builds an input on New Job (geometry in the editor dock, method in the Input Builder, title,
editor text, Run On), switches to Settings / Jobs / Molecules, returns with the **New Job tab
button**, and his **title, editor text and Input Builder settings are gone while the coordinates come
back** (Anton's observation, 2026-11-19). Cause (line numbers at `d1551db`):

- `App.tsx` renders **only the active screen** (l.173–215). A tab switch unmounts `NewJobScreen`;
  its 71 `useState`s (title l.168, content l.172, run target l.363, group l.216–217, SMILES l.232,
  formula l.238, NEB payload l.231, the iteration frame picker l.206–207 …) are lost.
- The mount `useLayoutEffect` (l.881–908), with no navigation payload, **clears the scene store**
  (`seedScene(null, "text-adopt")`, l.906).
- **Why the coordinates come back anyway — an accidental resurrection, not a feature.** The first
  render after the remount still reads the *old* scene from the singleton store; the layout effect
  clears the store, but the passive scene→content effect (l.989–996) runs with that render's stale
  `scene` closure and injects the old coordinates into the empty editor (`injectSceneIntoInput` on
  `""` → a bare `* xyz c m … *` block). 500 ms later the content→scene effect (l.1012–1030) sees no
  scene and a block in the text and re-seeds it (`text-adopt`). So the atoms and the total
  charge/multiplicity return, but as a bare block without the `!` line or `%` blocks, re-seeded from
  the merged text (so, by the code, the fragment layout collapses into one fragment and the edit
  history is a fresh log). The stale-closure step is **reproduced** in a minimal React 19.2.8 +
  Zustand harness under jsdom (DESIGN round 2; StrictMode does not change it); the fragment collapse
  is inferred from the code and confirmed in the live gate. This whole path
  disappears with item 1: the scene is no longer cleared.
- `InputBuilderForm` keeps its `BuilderState` in local state (l.52–60) and is rendered only while its
  accordion is open (`NewJobScreen.tsx` l.2382); `handleGenerate` closes the accordion, so the
  method/basis/solvation are lost **even without a tab switch**.

## Decision

1. **Mechanism (Anton): a Zustand draft store**, `useNewJobDraft` (the implementer anchors the module,
   e.g. `src/screens/new-job-draft.ts`). `NewJobScreen` still unmounts on a tab switch exactly as
   today (the 3Dmol viewer and its WebGL context are destroyed on unmount, `MoleculeViewer.tsx:860`,
   as `frontend.md` requires); on mount it **reads** the draft instead of resetting it. The geometry
   stays in `useSceneStore` (ADR-017: one source of truth); the draft store holds **no scene and no
   log**. The draft is the pair (draft store, scene store). The mount no longer touches the scene
   store unless a seed is applied (item 6).
2. **Lifetime (Anton): the session only.** Not persisted; an app restart starts an empty draft. No
   DB / settings change, no migration.
3. **What the draft holds** — two slices, because they are cleared differently (item 5):
   - **work slice** (what this job is): `title`, `content`, `selectedTemplateId`, `pendingNeb`,
     `smiles`, `formula`; the NEB endpoint picks of `NebBuilderSection` (`NebBuilderSection.tsx:45–46`);
     the iteration context (source job, `iterFrames`, `selectedFrame`, `iterFrameSeed` — item 8 —,
     `carryBanner`, `carryRefusal`, `snapshotRejected`, `logRejected`); the Replace-input state
     (`replaceMode` + the pre-unlock snapshot, so a pasted, not-yet-adopted calculation survives and
     is not reverted by the content→scene effect, l.1014–1030); the constraint composition warning
     (`constraintCompWarn` + the last composition signature, so ЗАХИСТ 2 is not silently re-armed on
     remount, l.650–664); the AtomId-keyed annotations `formalCharges`, `hiddenBonds`,
     `showCationBonds` (still valid across a remount — the scene and its ids are kept).
   - **preferences slice** (how Anton likes to run): the builder (`BuilderState`, `maxcoreTouched`,
     `nprocsText` — ADR-025 item 2's raw text), `runTarget` + `runTargetTouched`, `pickedGroupId` +
     `groupTouched`.
   - **Not in the draft** (transient, reset on every mount as today, each named): atom `selection`,
     `showNumbers`, move/rotate/edit/guided modes and their preview/ephemeral scenes
     (`previewScene`, `rotateEphemeral`, `rotateAxis`, `rotateOverlay`, `preferAlternative`,
     `moveMode`, `moveGranularity`, `guidedMode`, `guidedReagent`), resolved component/split lookups,
     `clashK`, fullscreen, open dock sections / accordion, the Import-xyz text and dialog
     (`importXyzOpen`, `importXyz`), `replaceConfirm`, the reagent-save dialog fields, busy flags
     (`creating`, `generating`, `saving`, `xtbBusy`, …), `saved`, error and notice banners
     (`coordNotice`, `dragFallbackNotice`, xtb notes), the viewer theme (already a setting), the
     viewer camera (3Dmol remounts with its default view). Monaco's own undo stack is **expected** to
     be lost on remount (no `path` prop on `<Editor>`; unmeasured — checked in the live gate); the
     scene's history is not (it is the log).
4. **The builder remembers after Generate** (Anton). The builder slice lives in the draft, so closing
   the accordion or switching tabs keeps method/basis/solvation/SCF/`%maxcore`/nprocs.
   **Exception, chemistry (rule #9):** `charge` and `multiplicity` are **re-seeded from the current
   geometry header every time the builder opens**, exactly as today's `useState` initializer does.
   Reason: `InputBuilderForm.tsx:86–89` applies `state.multiplicity` to the scene it generates from
   (`setMultiplicity(parsed, state.multiplicity)`), so a remembered multiplicity would silently
   override a geometry that changed since (a radical imported after the builder was last used would
   be generated as a singlet). (`state.charge` is used only when there is no scene, l.164–186; it is
   re-seeded for the same uniformity.) A multiplicity typed into the builder but not generated is
   therefore not kept — the same as today. No other `BuilderState` field is derived from the geometry
   (`build-input.ts:26–47`). The `%maxcore` rule of ADR-024 o26.13.1 is unchanged: the default follows
   Run On while `maxcoreTouched` is false, now across remounts.
5. **When the draft is cleared** (Anton: the clearing points; Anton, round-1 fork: **only the work
   slice**):
   - `resetDraft` = the work slice to its initial value **and** `seedScene(null, …)` on the scene
     store, in one function, the only clearing path. **The preferences slice is kept with its touched
     flags** — the next job starts with the same method, Run On and group. Untouched preferences keep
     following their live defaults (`last_run_target` via `loadRunTarget`, `activeGroupId`, the
     `%maxcore` default), so no derived value is ever reset to a constant.
   - **after a successful create** — once `create_job` / `create_neb_job` returned a job, for both
     Create and Create & Run. **The create-time values are captured before the clear**: `create()`
     reads `title`, `content`, `pendingNeb`, the scene/log, `runTarget`, `groupTouched` and
     `pickedGroupId` into locals at its start, and `assignPickedGroup`, `set_setting
     last_run_target` and `onOpenDetail(job.id, true, runTarget)` use those locals, never the store
     after `resetDraft`. The draft is cleared right after the create call returns, **only if the
     draft generation is unchanged** (item 10) **and the title, the content and the scene are still the
     ones `create()` started with** (reference identity): an edit made while `create_job` was awaited is
     not in the created job, so the draft is kept and a notice says the job was created from the
     earlier state. A failed create keeps the draft.
   - A failed `move_job` after a created job still clears (the job exists; keeping the draft invites
     a duplicate); its error names the created job and, for Create & Run, says it was **not run**
     (today no navigation happens on that path, so the job stays an unrun draft row).
   - "Find conformers" is **not** a create of the draft and does not clear it (its GOAT job is built
     from one fragment, and the draft's fragment must survive for the conformer `replace` branch and
     the D3 DFT carry).
   - **"New draft"** — an explicit button on New Job; it asks first (inline prompt, the
     `replaceConfirm` pattern — no `window.confirm`, its WebKitGTK behaviour is unmeasured) when the
     draft is non-empty, and clears at once when it is empty.
6. **An incoming seed meets a non-empty draft** (Anton: ask). The navigation payloads become one
   union, `DraftSeed = {kind:"molecule", molecule} | {kind:"iteration", job} |
   {kind:"conformer-scene", scene, label}`, carried in `Screen` as `{kind:"new-job", seed?, seedId?}`.
   - **Empty draft** → `resetDraft` (so no stale iteration context, `carryRefusal`, reject flags or
     `pendingNeb` from a previous seed survives) and the seed is applied at once (today's behaviour).
   - **Non-empty draft** → an inline prompt "Replace the current draft with ‹seed›?" with *Replace*
     (`resetDraft` then apply) and *Keep draft* (the seed is dropped, the draft untouched). Nothing in
     either store is touched until *Replace*. Leaving New Job while the prompt is open counts as
     *Keep draft*.
   - **Non-empty** = a pure predicate `isDraftEmpty(draft, sceneStore)`: false when the trimmed title
     or trimmed content is non-empty, or the scene is non-null, or the log is non-empty (a fully
     undone scene with redo history is work). The preferences slice never makes a draft non-empty.
   - **"Use this conformer" / the D3 re-opt carry** (`JobDetailScreen.tsx` `applyConformer`): the
     `replace` branch of `planConformerApply` stays as today — a logged `replaceFragmentAtoms` op on
     the draft's own fragment (fragment ids are `crypto.randomUUID()`, `scene.ts:78`, so it cannot hit
     an unrelated draft), undoable, no prompt; it then navigates to New Job with **no seed**. The
     `new` branch no longer calls `seedScene` from `JobDetailScreen`; it passes the scene as a
     `conformer-scene` seed, so a non-empty draft gets the prompt instead of being silently replaced
     (before this ADR the draft was already gone on that path, so the silent seed was harmless).
   - **`keepScene` is removed**: no mount resets the scene any more, so the flag has no meaning.
7. **Seed consumption is idempotent.** The draft-store module mints a fresh, monotonic `seedId` for
   each seeding navigation (not `App`, so a dev HMR reset of `App` cannot mint an already-consumed
   id); the draft store records `consumedSeedId`, which survives `resetDraft` and is set on Apply,
   *Replace* and *Keep draft* alike. (Leaving New Job with the prompt open does not consume the id:
   the seed is discarded with the `screen` object, which is equivalent to *Keep draft*; consuming it
   in the effect cleanup would break the StrictMode remount.) A seed whose id is already consumed is ignored. Reason: `main.tsx`
   renders under `React.StrictMode`, which runs mount effects twice in dev — observed in this project
   (`frontend.md:114`; the `didSubmit` ref, `JobDetailScreen.tsx:440`). Without the id the second run
   would see the just-seeded (now non-empty) draft and raise a spurious prompt, or re-seed. It also
   makes a tab return (no seed) and a re-render with the same `screen` object harmless.
8. **The iteration frame seed is a tracked state, not a once-per-mount event.** The draft carries
   `iterFrameSeed: "none" | "pending" | "landed" | "refused"`. Applying an `iteration` seed sets
   `pending`. The async effect that reads the parent's results and seeds the default trajectory
   frame (`NewJobScreen.tsx` l.950–983) runs **on every mount while the state is `pending`** (and is
   cancelled on unmount, as today), and sets `landed` (frame seeded, banner set) or `refused`
   (`carryRefusal` set) only when its result is applied; a **genuine single point** (the
   `no-trajectory` branch with `isSinglePoint`, l.959–966, which keeps the seed silently) moves it to
   `landed` with no banner, so it never re-reads `read_job_results` on every mount. **A frame that
   cannot be built into a geometry** (`finalGeometryXyz` throws on an elements/rows mismatch, or
   `sceneFromXyz` returns null) is `refused` with a `carryRefusal` naming the frame and saying the
   geometry shown is the INPUT SEED (Anton, 2026-11-19); `iterFrames`/`selectedFrame` are written only
   after the frame was built, and a user re-pick that cannot be built leaves `selectedFrame` unchanged
   and says so. Never a silent input seed (debugging/022). A `landed`/`refused` state never re-runs, so
   a remount never re-seeds over the user's edits. Reason: with a once-per-seed rule, leaving the tab
   while `read_job_results` is pending (or the StrictMode mount–cleanup–mount) would leave the
   parent's **input seed** geometry with no banner and no refusal — the silent bug of
   [debugging/022](../debugging/022-verdict-null-misclassified-single-point-frame-picker.md).
   The user's frame re-pick (the picker's `onChange`) updates `selectedFrame` and stays `landed`.
9. **Run On re-validated on every mount.** `loadRunTarget` keeps its rule (a touched choice wins over
   `last_run_target`; it never writes the setting, o20.1). In addition, once the profile list loads
   **successfully** (a failed `list_server_profiles` changes nothing), a draft `runTarget` that is no
   longer an enabled run target (`runTargetOptions`; a profile deleted or disabled in Settings while
   away) is replaced by `resolveRunTarget` (`last_run_target`, else Local), `runTargetTouched`
   resets, and a notice says so. Never a silent switch; never a write of `last_run_target`. (An
   **untouched** target is a default, not a choice: it keeps following `loadRunTarget`'s preselect on
   every mount, as at base, with no notice.) **A kept target is always shown honestly** (Anton):
   while the profile list is loading or after it failed, a non-null `runTarget` absent from the
   options is rendered as an extra option "‹id› — profile list not loaded", so the select never shows
   Local or blank for a job that will go to a server; nothing is blocked. The group picker does the
   same for a kept `pickedGroupId` absent from a failed or loading `list_groups`.
   **The group likewise:** once `list_groups` loads successfully, a touched `pickedGroupId` that is no
   longer in the list (a group deleted in the Jobs sidebar while away; `move_job` refuses it,
   `groups.rs:173–175`) resets `groupTouched` — the picker follows `activeGroupId` again — and a
   notice says so; a failed list changes nothing.
10. **Async completions are bound to a draft generation.** The draft carries a `generation` counter,
    bumped by `resetDraft` and by applying a seed. Every async path that writes the draft or the
    scene store after an `await` — `generateFromSmiles` (l.1274–1316), the structure-file import
    (l.1251), the iteration frame seed (item 8), and `create()`'s clear (item 5) — captures the
    generation at its start and, on a mismatch, drops its result — with a notice for a user-started
    action (SMILES, import, create; the `xtbSceneRef` pattern, l.733); the frame seed drops silently,
    because a reset or a seed has already moved `iterFrameSeed` and the user replaced that iteration
    on purpose. Reason: before this ADR the next mount wiped a late write; now a SMILES fragment
    arriving after "New draft" or a *Replace* would be appended to the new draft's geometry, and a
    late create return would clear a newer draft.
11. **Unchanged / known:** the memory preflight (`useMemoryEstimate(content, runTarget)`) recomputes
    from the restored draft on mount — its sequence ref is per instance and a dead closure's reply is
    inert (o26.13.6). The scene↔content effects: on a plain return the content already reflects the
    scene; after the conformer `replace` branch (scene changed while New Job was unmounted) the
    scene→content effect re-projects the block on mount, as designed. An xtb optimisation in flight
    when the user leaves is dropped on unmount as today (Open). `activeGroupId` keeps driving the group
    default while `groupTouched` is false.
12. **Gates (conventions; each with a negative control that must go red).**
    - **Pure (Part A)**, on the draft store module and pure helpers:
      - `resetDraft` empties the work slice and the scene store and keeps the preferences slice with
        its touched flags (NC: leave the scene untouched → red; NC: reset preferences to constants →
        red);
      - `isDraftEmpty` truth table incl. the null-scene-non-empty-log row (NC: count preferences as
        work → red; NC: ignore the log → red);
      - `applySeed` with a consumed `seedId` is a no-op (NC: drop the id check → red); on a non-empty
        draft it returns "ask" and mutates nothing (NC: apply first → red); on an empty draft it
        clears stale iteration/NEB state first (NC: skip the reset → red); *Keep draft* marks the id
        consumed (NC: don't → a second apply re-asks → red);
      - `iterFrameSeed` transitions: `pending` survives an unmount before resolution; only a
        resolved result moves it to `landed`/`refused`, and a single-point result leaves `pending`
        (NC: set `landed` on start → red; NC: leave a single point pending → red);
      - the generation guard drops a stale result (NC: drop the check → a late SMILES lands in a
        replaced draft → red);
      - Run On re-validation: a gone/disabled profile → `resolveRunTarget` + notice; a failed list
        changes nothing (NC: keep the gone profile → red; NC: treat a failed list as empty → red);
      - group re-validation: a gone group → untouched + notice; a failed list changes nothing (NC:
        keep the gone id → red; NC: treat a failed list as empty → red);
      - the create clear: a generation, content or scene change during the create keeps the draft
        (NC: clear unconditionally → red);
      - builder charge/multiplicity re-seed from the header on open while other fields are kept
        (NC: keep the remembered multiplicity → red).
    - **Component (Part B)** — no `NewJobScreen` render test exists today; Part B **budgets the
      harness**: `vi.mock` of `InputEditor` (Monaco), `MoleculeViewer` (3Dmol), `@tauri-apps/api/core`
      `invoke`, `@tauri-apps/api/event` `listen`, and the sidecar client. Gates, each with its NC:
      - unmount → remount restores title, content, scene (fragment count + log length), the builder
        method, Run On, the group pick with its touched flag, `constraintCompWarn` (rule #9,
        ЗАХИСТ 2) and the Replace-input buffer — one assertion per field, each NC = that field
        moved back to local `useState` → red; plus the old `seedScene(null)` on mount → red;
      - the builder keeps the method after Generate closes the accordion (NC: builder state local →
        red);
      - a successful Create & Run clears the work slice, keeps preferences, and still calls
        `onOpenDetail` with the chosen target and `move_job` with the chosen group (NC: read them from
        the store after the reset → red); a failed create keeps the draft (NC: clear before the invoke
        resolves → red);
      - "New draft" while mounted: Run On = the resolved `last_run_target` when untouched, group =
        `activeGroupId` when untouched, the `%maxcore` default when untouched (NC: a constant reset →
        red);
      - a molecule seed on a non-empty draft shows the prompt; *Keep draft* leaves both stores intact
        (NC: apply without asking → red);
      - the conformer `new` branch on a non-empty draft leaves the scene untouched and raises the
        prompt (NC: `JobDetailScreen` calls `seedScene` directly → red); the `replace` branch lands
        with no prompt (NC: route it through the seed → a prompt appears → red);
      - inside an explicit `<React.StrictMode>` wrapper: a seed raises no prompt and an iteration seed
        lands the last frame + banner (NC: drop the `seedId` check → red; NC: gate the frame effect on
        "applied this mount" → red); unmount before `read_job_results` resolves, remount → the frame
        lands (same NC).
    - **Live WebKitGTK gate (Anton):** geometry with ≥2 fragments + builder + title + editor `!` line
      + Run On → Settings → back: all restored, fragment layout and history intact, the viewer renders;
      Monaco undo after return (record what happens); Create & Run → work cleared, method/Run On kept;
      Molecules "Use" on a non-empty draft → the prompt; "New draft".
13. **Wiki in the same change as the implementation:** `modules/frontend.md` — §App shell (l.25–31:
    the `Screen` union and the "discarded … (accepted)" sentence), the `NewJobScreen` props (l.57–58,
    `keepScene`), "Use this conformer" (l.425–426), Job iteration (l.431–433, `initialJob`, "before
    paint"), the store-wiring "**Store init is a `useLayoutEffect`**" (l.276–278), the gotcha "`useLayoutEffect` for store init — remounts on every nav" (l.960), the "only
    the scene store is Zustand" line (~l.976); `modules/scene.md` l.788–797 (`keepScene`, "if the scene
    was cleared (other session)"); `ROADMAP.md` (an item for this unit); `wiki/log.md`. This ADR
    change itself carries the `wiki/index.md` line and the `decision` entry in `log.md`.

## Alternatives rejected

- **Keep `NewJobScreen` mounted and hide it with CSS.** Keeps the camera, but a live 3Dmol/WebGL
  context, the Monaco instance and every effect of the screen (the 500 ms content→scene debounce, the
  memory estimate, sidecar calls) run while another tab is shown; `frontend.md` requires the viewer to
  be destroyed on unmount. It would also leave the builder-accordion loss unfixed.
- **Persist the draft across restarts** (Anton: not now). Adds a storage format and its versioning for
  little gain; it can be layered on the store later.
- **Replace on an incoming seed without asking** (today's behaviour). It is exactly the silent loss
  this ADR removes.
- **Reset everything on create / "New draft" and remount via a `draftEpoch` key** (round-1 option b;
  Anton chose the work-only reset). It would also throw away the method and Run On after every job.

## Consequences

- A tab switch keeps the work; it goes only by a successful create, "New draft", *Replace* in the
  seed prompt, or a seed applied to an empty draft — and none of them touches the builder, Run On or
  group preferences.
- The viewer camera (and, expected, Monaco's undo stack) still reset on return.
- `App`'s `Screen` union changes shape: `seed`/`seedId` replace `initialMolecule`/`initialJob`, and
  `keepScene` is removed.
- The accidental "coordinates come back as a bare block" path (Context) no longer exists.

## Open

- An xtb run in flight when leaving New Job is dropped (pre-existing). The orphaned run keeps Rust's
  xtb slot: after the remount `xtbBusy` is false and there is no Cancel, so a new pre-opt is rejected
  as busy until the orphan ends. A later unit could keep the run in a store.
- Persisting the draft across restarts (item 2).
- **Create inside the 500 ms content→scene debounce** (Part B review): a block pasted into an empty
  editor and created before the debounce fires is created with `sceneJson: null`; the debounce then
  seeds the scene during the await, so `clearAfterCreate` keeps the draft with a misleading "edits
  since" notice. Avoidable by running the debounce's no-scene adoption synchronously before
  `captureCreateStart()`; not done in this unit. (A hand-edited block created before its revert ships
  the edited text with the old snapshot — pre-existing, not covered by that fix.)
