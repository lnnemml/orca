# 003-webkitgtk-select-styling.md — `<select>` text is dark-on-dark in the Tauri (WebKitGTK) webview

**Date:** 2026-07-27 · **Area:** frontend
**Symptom:** In the Input Builder form the dropdown values (`Opt + Freq`, `B3LYP`, `def2-TZVP`,
`D4`, `RIJCOSX`, `Gas phase`, `Tight`) render as **dark text on a dark background** — practically
unreadable — but only in the real Tauri window on Linux. In Chromium (`vite dev`) the same selects
look fine. A **disabled** select (the gas-phase `water` solvent) does the opposite: a light-grey
native background that clashes with the dark theme.

**Root cause:** WebKitGTK (2.52 / `libwebkit2gtk-4.1`) renders `<select>` as a **native GTK
widget**. A native widget does **not** inherit the CSS `color` from the `.input` class the way
Chromium's styled control does — it paints itself with GTK's default text colour (near-black),
which lands dark-on-dark against our `--panel` background. The old `.select { cursor: pointer; }`
rule set nothing that mattered, so nothing fixed it. The disabled case is the same story from the
other side: with no explicit rule, GTK paints its own light "insensitive" background.

**Fix (`src/styles/app.css`):** strip the native appearance and set **every** visual property
explicitly, so WebKit paints a normal styled box instead of a GTK widget:

```css
.select {
  cursor: pointer;
  -webkit-appearance: none;   /* stop WebKit rendering the native GTK widget */
  appearance: none;
  color: var(--text);         /* must be explicit — inheritance is ignored */
  background-color: var(--panel);
  background-image: url("data:image/svg+xml;… chevron …");  /* appearance:none removes the arrow */
  background-repeat: no-repeat;
  background-position: right 10px center;
  background-size: 12px;
  padding-right: 30px;
}
.select:disabled { color: var(--muted-2); background-color: var(--panel-2); opacity: 0.6; }
.input[type="number"] { color: var(--text); background-color: var(--panel); }  /* same story for spinners */
```

Because `appearance: none` also removes the native dropdown arrow, a chevron is re-added as an
inline-SVG `background-image` (fill `#8b949e`, ≈ `--muted`) pinned to the right with `padding-right`
reserving room for it.

**The `option` popup — partially out of our hands.** The open dropdown is a **separate native GTK
menu**, not part of the document, so `.select option { background/color }` is best-effort:
WebKitGTK may or may not honour it depending on the GTK theme. The rule is kept (it costs nothing
and helps where supported), but the **closed control** — the part always on screen — is the part
that actually mattered and is fully fixed. If a future GTK theme shows an unreadable open popup,
that is the known limitation; the closed-select contrast is not affected.

**How it was verified (reusable technique — same as `debugging/002`):** the Tauri GUI can't be
driven headlessly here, but Tauri's webview **is** `libwebkit2gtk-4.1`, and the distro ships
`MiniBrowser` at `/usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/MiniBrowser`. Loading a standalone probe
HTML that inlines the real `--*` theme variables and the exact `.select` / `.input[type=number]`
rules, then `gnome-screenshot -w`, reproduces the engine faithfully. The screenshot confirmed the
select values render as **light text on dark** and the disabled select is muted-dark (not
light-grey) — the fix works in the identical engine. The **open** popup could not be captured
headlessly (opening it needs a click and `xdotool` isn't installed), so the `option` rule is
documented as best-effort rather than screenshot-verified.

**Lesson / rule:** in WebKitGTK, form controls (`<select>`, number spinners, and friends) are
native GTK widgets that ignore inherited `color`/`background`. To theme them you must
`appearance: none` **and** set colour + background (+ re-supply the arrow) explicitly — styling the
wrapping `.input` class is not enough. Anything rendered as a native popup (the `option` list) is
only best-effort styleable; don't rely on it for contrast. This is the **second** WebKit-incomplete
gotcha in the project (see `debugging/002` — `OffscreenCanvas` present-but-null): when something
works in Chromium but not the Tauri webview, suspect a WebKit/GTK-specific behaviour and reproduce
in `webkit2gtk-4.1/MiniBrowser` before chasing the integration. Recorded in `wiki/modules/frontend.md`.

---

## Amendment — 2026-07-29 (2.5.2e-3a): the fix lives on the ELEMENT selector, not a class

The 2.5.2e-2 viewer theme `<select>` (`.viewer-theme-select`) was created as a **new class** and
did **not** carry this fix — no `-webkit-appearance: none`, no explicit colour — so WebKitGTK
rendered it as the native GTK widget with dark text again (visible on the light-theme screenshot).
The regression proves the real lesson: **a fix you must remember to apply to each new control will
be forgotten.**

So the rule moved from the `.select` class to the **element selector** `select` (with `.select`
kept as an alias): `select, .select { -webkit-appearance: none; … }`. Now every `<select>` in the
app is fixed by default; a new dropdown can't opt out by forgetting a class. `.viewer-theme-select`
was reduced to cosmetic-only tweaks (font-size, border, radius) — critically it must **not** set the
`background` shorthand, which would wipe the chevron `background-image` from the element rule.

Verified in `MiniBrowser`: the fixed select renders light text + chevron; a reference `<select>`
with the e-2 styling (no `appearance: none`) renders dark native text beside it. Grep confirms all
five `<select>` in `src/` are now covered by the element rule (four already carried `.input select`,
the fifth is `.viewer-theme-select`).

**Revised lesson:** for WebKitGTK form-control theming, put the fix on the **element selector** so
coverage is automatic — a class-scoped fix is a fix waiting to be forgotten (and it was).
