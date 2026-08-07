# Module: reactions-ui (Reactions screen)

*Phase 4.5 Stage C2a. The management UI over the C1 reaction/pathway commands: create a
reaction, attach scan jobs as labelled pathways, detach, delete. The comparative overlay +
ΔΔE‡ is C2b.*

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

## Manual gate (author, real window) — PENDING (code complete; the unit stays open until it passes)

- **m1** create "Ketone + BH₄ (si vs re)" → appears, opens.
- **m2** attach two completed scan jobs as pathways "si face" / "re face" → both listed with their
  job titles; a non-scan job in the picker is marked/warned.
- **m3** detach one pathway's job → un-groups; the scan job is **still in Jobs**, openable with its
  profile intact.
- **m4** delete the reaction → gone from the list; **both scan jobs still exist in Jobs** (the
  jobs-survive invariant, visible).

## Related

- `wiki/architecture/adr-007-reaction-modeling.md` (amendment) — the ratified normalized schema.
- `wiki/modules/tauri-core.md` (v13) — the C1 commands + `jobs.pathway_id`, and the C2a `Job` column.
- C2b (next) — promote reads coordinate/method/profile from the attached job, overlays Pathway A vs B,
  highlights ΔΔE‡, and adds the comparability guards.
