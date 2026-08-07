# debugging/016 — a recharts chart click selects nothing (scan + trajectory)

**Date:** 2026-08-07 · **Area:** frontend (recharts v3)
**Symptom.** In the live window (B2 manual gate h2), clicking a scan point in the energy-profile
chart did **nothing**: the readout stayed "point 1 / 6" and the molecule stayed point 1. Hover
tooltips worked. Pure vitest tests were all green.

## Localized, not guessed (the evidence chain)

- The **readout was unchanged** on a click → `setSelected` never fired → the bug is in the
  **click → `setSelected`** layer, not the viewer feed / `read_scan_geometries` / profile math
  (all separately verified correct).
- Hover worked but click didn't → the chart receives events; only the click **handler** is wrong.
- The project is on **recharts 3.10.1** (`package.json`). Measured in the installed types:
  `node_modules/recharts/types/state/tooltipSlice.d.ts` → **`type TooltipIndex = string | null`**.
  recharts **v3 changed the chart-`onClick` payload**: `activeTooltipIndex` arrives as a **string**,
  where v2 gave a number.
- The handler guarded `if (typeof i === "number" …)` → on a v3 **string** it is `false` → **every
  click silently dropped**. The type annotation already read `number | string | null` — the author
  anticipated a string; only the runtime guard didn't handle it.

## Why the tests didn't catch it — the wiring gap

The handler was **inline JSX**, never unit-tested (jsdom cannot fire a real recharts click that
produces `activeTooltipIndex`). So the pure `scanProfile.ts` / `frame.ts` tests were green while the
inline glue was broken — exactly the gap the manual gate exists to catch.

## Cross-cutting — the trajectory chart too

`src/trajectory/TrajectoryPlayer.tsx` (energy-per-cycle, click-to-jump, unit 3.8) used the
**identical** `activeTooltipIndex` + `typeof number` pattern, so it had been **silently broken since
the recharts→v3 upgrade** as well. This is a **class** bug, not one instance.
(IR-spectrum peak select and the per-cycle table use element/row-level `onClick` with the datum's own
`m.index`, **not** the chart `activeTooltipIndex` — grep-confirmed unaffected, left untouched.)

## Fix — one pure, tested resolver both charts route through

`src/charts/clickIndex.ts` — `resolveClickedIndex(state, series, getX?) → number | null` (the array
position; the caller maps to `series[pos].index`):

1. `activeTooltipIndex` as **number OR string** → `Number(i)`, accepted only as an integer in
   `[0, series.length)`;
2. else fall back to `activeLabel` (the clicked x value) → the series element whose x (`getX`:
   `coordinate` for scan, `cycle` for trajectory) matches within `1e-6`;
3. `null` when nothing resolves; tolerant of an unknown-shaped `state`, never throws.

Both `ScanProfilePanel` and `TrajectoryPlayer` route their chart `onClick` through it. Each handler
also emits `console.warn("[chart click] … unresolved", state)` under `import.meta.env.DEV`, so a
residual miss shows the **real payload** immediately instead of a second blind round-trip.

### Redundant on-dot path — the measured caveat (rule #10)

The plan called for also wiring `<Line activeDot={{ onClick }}>` as a belt-and-suspenders on-dot
select. **Measured in the recharts source** (`es6/component/ActivePoints.js` + `es6/util/types.js`
`adaptEventHandlers`): the **object** form wraps handlers as `onClick(activeDotObject, event)` — it
passes the *activeDot props object you wrote*, **not** the datum, so it cannot select. The **function**
form is the one recharts calls with the real datum props (`{ index, payload, cx, cy }` —
`ActiveDotProps`). So the redundant path uses the **function form**, returning a `<circle>` whose
`onClick` resolves `props.index` through the same helper. (This corrects the plan's "the object form
delivers the datum" premise — it does not.)

**Commit:** `fix(results): chart-click selects a point under recharts v3 …` (Phase 4.5 B2 fix).

## Lesson

A chart library's event payload is third-party behaviour that **changes across majors** (rule #10:
verified from the installed types + source, not memory). Inline chart-click handlers are untestable
glue — **extract the resolution into a pure, tested function**, cover the shapes that actually arrive
(v3 string, v2 number, label fallback, garbage), and add a DEV-warn so the next payload change is
visible, not silent. See `wiki/modules/results-ui.md` (the shared click resolver).
