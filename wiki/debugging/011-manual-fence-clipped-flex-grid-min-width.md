# 011-manual-fence-clipped-flex-grid-min-width.md — `​```orca` fences clip right, no scrollbar (drawer AND page)

**Date:** 2026-08-04 · **Area:** frontend (CSS layout)
**Symptom:** In a rendered manual page, a long line inside a ` ```orca ` fence is cut off on the right — "default for geometry optimiz…", "shift the diagon…" — with **no horizontal scrollbar**, so the rest of the line is unreachable. Seen in the editor's `ManualDrawer`; the same clip is on the full `ManualScreen` view.

## The contradiction that names the bug

`.manual-fence` already has `white-space: pre` **and** `overflow-x: auto` — a horizontal scrollbar *should* appear. It does not. So the fence is not the problem; something upstream lets the fence grow to its intrinsic (content) width instead of being bounded to the viewport, and then `overflow-x` has nothing to scroll (the box is as wide as its content).

## Root cause — the flexbox/grid **automatic minimum size** (`min-width: auto`)

A **flex item** (main axis) and a **grid item** both get an automatic minimum size of `min-content` when `min-width` is `auto` (the initial value). That minimum **overrides** the item's shrink-to-fit / track size: the item refuses to become narrower than its widest indivisible content — here the long ` ```orca ` line. So the item grows to fit the fence, the fence is never bounded, and its `overflow-x: auto` never engages. The line simply spills to the item's edge and is clipped there.

This bit in **two layouts at once — one defect, two manifestations:**

- **`ManualScreen`** — `.manual-view-col` is the **`1fr` GRID track** of `.manual-screen` (`overflow: hidden`). Without `min-width: 0` the track's min-content (the fence) pushes it past its `1fr` share; `.manual-screen`'s `overflow: hidden` clips the overflow at the viewport edge. No scroll.
- **`ManualDrawer`** — `.manual-drawer-body` is a **FLEX ITEM** of the column-flex `.manual-drawer`. The same auto-minimum keeps it from shrinking below the fence's content width, so again `overflow-x` on the fence never triggers. (`min-height: 0` was already present for the *vertical* twin of this trap — the horizontal one was missed.)

## Fix

`min-width: 0` on **both** container items — the grid track and the flex item:

```css
.manual-view-col  { min-width: 0; }   /* grid 1fr track */
.manual-drawer-body { min-width: 0; } /* column-flex item */
```

That releases the automatic minimum, the item shrinks to its allotted width, and the fence's own `overflow-x: auto` finally scrolls. **Not** `white-space: pre-wrap` / line-wrapping — ORCA input indentation and column alignment are significant (`render.ts` keeps `white-space: pre` for exactly that; a wrapped fence would misalign an input).

**Post-condition (in our terms):** a long line in a ` ```orca ` fence is **reachable by horizontal scroll** in the drawer at ANY width (down to its 320 px minimum) and on the page — never clipped, never wrapped.

## Lesson / rule

**`min-width: auto` (and `min-height: auto`) on a flex/grid item silently prevents a child's `overflow` from scrolling** — the child looks like it "should" scroll (it has `overflow: auto`), but an ancestor item grew to content width so there is nothing to scroll. When a scroll container refuses to scroll, look UP the flex/grid chain for a missing `min-width: 0` / `min-height: 0`, not at the scroll container itself. Same family as `010` (a widget clipped by an ancestor's `overflow`) and `007` (`overflow: hidden` ate a child) — in a WebView, an ancestor's box model silently eats what a descendant paints past it. Here it was the ancestor's *minimum size*, not its overflow.
