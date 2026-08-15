# `<MethodPicker>` — the reusable method sub-UI (`src/input-builder/`)

The method-family selector (composite / DFT / wave-function / xTB) and its per-family controls,
extracted from `InputBuilderForm` so **two** callers share one control: the New Job form (where it
always yields a concrete method) and the **OptTS-derived refine sites** (where it gains an "Inherit
from source" default so a refine can run at a chosen level). Phase 4.5 / 4.7.

## Files

- **`MethodPicker.tsx`** — pure presentational. Props: `value: MethodSlice`, `onChange: (patch:
  Partial<MethodSlice>) => void`, an optional `leading` slot (New Job renders its Job-type control
  there so the extraction preserves that row's layout), and an optional `inherit` object
  (`{active, onChange, note?}`). When `inherit` is present it prepends an **"Inherit from source"**
  radio; while active the per-family controls are hidden and `note` is shown instead. Absent →
  New Job behaviour, no inherit option. Renders the exact markup the form used, so consuming it
  there is a **no-behaviour-change refactor**.
- **`OptionSelect.tsx`** — the labelled `<select>` over `OrcaOption[]`, extracted from
  `InputBuilderForm` so the form and `MethodPicker` render the identical control (no DOM drift).
- **`OptTSMethodPicker.tsx`** — the OptTS-refine wrapper shared by the three OptTS-derived spawn
  sites (1D scan, 2D scan, NEB band). Owns the `inherit` boolean + a seeded `MethodSlice`, renders
  `<MethodPicker>` with the XTB-source note, and hands the parent an
  `OptTSMethodOverride = { methodState?: MethodSlice }` to pass straight into `buildOptTSInput`.
  Also exports `useSourceIsXtb(jobId)` — reads the source job once on mount and reports whether it
  ran semi-empirical (drives the note). **Inherit = pass nothing:** the default is inherit →
  `onChange({})` → the caller passes `{}` to `buildOptTSInput`, which re-uses the source's method
  verbatim (the byte-identical path of `src/scene/optts.ts`). There is **no reconstruction** of the
  source method into a slice — that would be a back-door reverse-parse of the source `!` line;
  inherit literally sends no override, so the default is unchanged for anyone who does not touch it.

## `MethodSlice` (in `build-input.ts`)

`Pick<BuilderState, "methodFamily" | "composite" | "functional" | "basis" | "dispersion" | "ri" |
"xtbMethod" | "wavefunction">` — the method-relevant fields only, spreadable into a full
`BuilderState`. `methodSliceOf(state)` projects a state (or another slice) down to it — the single
place the slice's fields are enumerated. The point of the type: a slice spread into `DEFAULT_BUILDER_
STATE` lets **`buildOrcaInput` apply the family logic** — so a DFT override still carries its
functional + basis + **paired** RI aux + dispersion (the pairing lives in `buildKeywordLine`'s `dft`
branch, **never flattened into a string** — the MAIN RISK this design avoids). See
[optts.md](../orca/optts.md) for the OptTS override seam.

## The no-behaviour-change guarantee (tested)

`InputBuilderForm.test.tsx` (the project's first component test — jsdom + `@testing-library/react`)
pins it:
- **`new_job_default_input_is_byte_identical`** — renders the form, clicks Generate, and the emitted
  input matches an inline snapshot byte-for-byte. If the extraction changed New Job's input by so
  much as a space, it goes red.
- the method-family radio, driven through the extracted picker, flows to the emitted `!` line
  (`B3LYP def2-TZVP def2/J RIJCOSX D4 …` — the aux pairing proves the family logic still runs);
- New Job never shows the "Inherit from source" option (no `inherit` prop).

## Mount sites

The three **`buildOptTSInput`** callers — `src/scan/ScanProfilePanel.tsx` (1D),
`src/scan/ScanSurface2dPanel.tsx` (2D), `src/reactions/NebBandPanel.tsx`. **`ConnectivityPanel` is
deliberately NOT a site**: it uses `buildConnectivityChildren` (plain-Opt children that must stay at
the TS's method to locate the two basins), which has no `methodState` — overriding the method there
would be chemically wrong. Each site holds `methodOverride` (default `{}` = inherit) + `sourceIsXtb`,
and the existing 2b group picker rides alongside unchanged.
