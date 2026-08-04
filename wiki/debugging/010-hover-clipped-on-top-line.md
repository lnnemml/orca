# 010-hover-clipped-on-top-line.md — Manual hover shows nothing on the `!` line, only there

**Date:** 2026-08-01 · **Area:** frontend
**Symptom:** Hovering a keyword on the first line of an ORCA input — `! r2SCAN-3c Opt Freq TightSCF` — shows **no** hover popup. Grammar highlighting works, the manual drawer works, and every hover unit test is green. The keyword resolves; the popup simply never appears.
**Root cause:** An ORCA input opens with the `!`-line as **line 1**, and Monaco lays a hover *above* the hovered line. Without `fixedOverflowWidgets`, the hover content widget is rendered **inside the editor's `.overflow-guard` (`overflow:hidden`)**. A popup positioned above line 1 lands above the editor's top edge and is **clipped away** — present in the DOM, `visibility:visible`, but not on screen. Nothing wrong with the provider, the language id, the word, or the lookup.
**Fix:** `fixedOverflowWidgets: true` in the editor options (`src/editor/editor-options.ts`) — Monaco then renders overflow widgets in a body-level fixed overlay that escapes the editor's clip. Plus registration hardening in `registerOrcaHover` (`src/editor/orca-hover.ts`): the hover provider is registered **first** (mandatory), the "open in drawer" command **second** and wrapped in `try/catch` (optional — its failure must not vanish the hover), and `registered = true` is set only **after** the mandatory registration succeeds. Commit: see `fix: hover popup clipped on top line …`.

## How it was measured (not guessed)

The architect's exclusions ruled out the pure functions, so the defect had to be in the wiring — where there were no tests. Measurement, not a real mouse (no input injection available in WebKitGTK), was done by seeding a buffer in `onMount` and writing facts into **visible DOM banners** (the window title does not propagate in Tauri; `document.title` was a dead end). Four probes, each overturning a candidate:

1. `model.getLanguageId()` = `orca-inp` = the registered id → **outcome (i) ruled out** (language matches).
2. `getWordAtPosition` at the `!` line = `r2SCAN-3c`, full token → **outcome (ii) ruled out** (wordPattern fine).
3. `resolveHover(...)` on the live model = MATCH; `buildHoverMarkdown` = 673 chars, no throw → the pure pipeline is healthy on the real model.
4. A DOM-banner side-effect inside the **registered** `provideHover` fired (`PROV FIRED`) → the provider **is** registered and invoked. Yet no popup. Querying the DOM: `.monaco-hover` exists, `visibility:visible`, but its rect sits at **y≈255 — above the editor's top edge (~y≈310)**. → clipped by the overflow guard.

Enabling `fixedOverflowWidgets` then made the real manual popup render un-clipped above the top line (verified in-window).

## Lesson / rule

**The pure functions were unit-tested; the wiring was not — and the wiring is where it broke.** Registration order, editor options, and widget rendering have no tests by default, so a defect there passes an all-green suite. The fix ships a **wiring test** (`orca-hover-wiring.test.ts`) with a fake monaco that would have caught it: the hover is registered for the same language id `<Editor>` uses, it survives the optional command throwing, and `fixedOverflowWidgets` is pinned so removing it re-breaks a red test instead of a silent top-line hover.

Same clipping family as `007-phase1-decisions-phase3-outgrew.md` (an `overflow:hidden` clipped the results screen): in a WebView, `overflow:hidden` on a container silently eats whatever a child tries to paint outside it — the element is "there" and "visible" by CSS, just cut off.

**Update (4.13):** the hover was replaced by a **selection panel** (`src/editor/selection-panel.ts`), and `orca-hover-wiring.test.ts` by `selection-panel.test.ts`. The clipping lesson carries over unchanged: the panel is a Monaco **content widget with `allowEditorOverflow: true`**, and the SAME `fixedOverflowWidgets: true` un-clips it on the top line — the wiring test still pins that option, and still exercises the wire (selection → resolve → panel → the Open channel) with a fake editor, no jsdom.
