/**
 * ORCA `%geom Scan` — pure generate / parse / inject / guard (Phase 4.5 Stage A1).
 * No React, no fetch, node-testable. The **text of the ORCA input is the source of
 * truth** for the scan coordinate, exactly as for constraints (ADR-008): the A2 UI
 * panel will be a *view over the text*, not a parallel store.
 *
 * A `Scan` and a `Constraints` block are BOTH `%geom` sub-blocks. Scan injection
 * therefore **composes into the one `%geom`** (Scan and Constraints as siblings
 * under a single `end … end`) — it must never emit a second `%geom`, which ORCA
 * would silently reduce to one, dropping a block (the central correctness property
 * of this unit; `injectScan` shares `geomBlock.ts`'s locator with constraints).
 *
 * The emit is order-bearing and 0-based — scan atoms are the SAME app-global index
 * space as `Constraint` (`wiki/orca/constraints.md`, settled 0-based by a real run)
 * — and rides the same `toOrcaIndex` seam (ADR-016). The Rust twin `emit_scan_block`
 * is byte-identical; the two goldens are the byte-identity gate.
 *
 * See `wiki/orca/scan.md` for the block syntax, the 0-based index base, and the
 * `! Opt` requirement (a relaxed scan without `Opt` is silently a single point).
 */

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import { globalIndexOfAtom } from "./scene";
import { toOrcaIndex, fromOrcaIndex } from "./constraints";
import { scanTokens, locateGeom, leadingIndent } from "./geomBlock";
// A2 view over this core: `ScanPanel.tsx` (edit) + `AtomInspector` "Scan this
// coordinate" → `scanFromSelection` (add). See wiki/modules/scene.md, editor-ui.md.

/** Scan coordinate kind → ORCA letter: distance (B), angle (A), dihedral (D).
 * No cartesian scan. */
export type ScanKind = "B" | "A" | "D";

/** The scan range, carried as the value_text analogue of `Constraint`: `start`/
 * `end` are numbers, but `startText`/`endText` preserve the user's EXACT entered
 * text through a parse→inject round-trip when it isn't the canonical rendering of
 * the number (e.g. `1.40`, which `String(1.4)` would flatten to `1.4`). Set by the
 * parser only for non-canonical text; unset for freshly-built coordinates. This is
 * why scan endpoints never hit the JS↔Rust float-parity question. `npoints` is a
 * plain integer ≥ 2. */
type ScanRange = {
  start: number;
  end: number;
  startText?: string;
  endText?: string;
  npoints: number;
};

/** A `%geom Scan` coordinate, atoms in OrcaStudio's own 0-based global index space
 * (the merged-xyz / ASE-mask space, ADR-008 — the same space as `Constraint`). */
export type ScanCoordinate =
  | ({ kind: "B"; atoms: [number, number] } & ScanRange)
  | ({ kind: "A"; atoms: [number, number, number] } & ScanRange)
  | ({ kind: "D"; atoms: [number, number, number, number] } & ScanRange);

const ATOM_COUNT: Record<ScanKind, number> = { B: 2, A: 3, D: 4 };

/** `Number(String(v)) === v` for finite v, so this round-trips through parse. The
 * byte-identical twin of `fmt_value` (Rust) — see `float-formatting-parity.md`. */
function formatValue(v: number): string {
  return String(v);
}

/** An endpoint's text: the user's exact text if preserved, else the canonical
 * render. Mirrors the constraint value rule (valueText ?? formatValue(value)). */
function endpoint(value: number, text: string | undefined): string {
  return text ?? formatValue(value);
}

/** One scan line: `LETTER a1 a2[ a3[ a4]] = start, end, N` — indices 0-based via
 * `toOrcaIndex`, space-joined; the letter is the kind directly. */
function scanLine(s: ScanCoordinate): string {
  const idx = s.atoms.map(toOrcaIndex).join(" ");
  const start = endpoint(s.start, s.startText);
  const end = endpoint(s.end, s.endText);
  return `${s.kind} ${idx} = ${start}, ${end}, ${s.npoints}`;
}

/** Normalize the injectScan/emit argument to an ordered coordinate list. A bare
 * `ScanCoordinate` → `[coord]` (so the 1-coordinate path is byte-identical to before);
 * an array is passed through (its ORDER is the grid's outer→inner scan order). */
function coerceCoords(scan: ScanCoordinate | ScanCoordinate[]): ScanCoordinate[] {
  return Array.isArray(scan) ? scan : [scan];
}

/**
 * The standalone block for an input that has no `%geom` yet. A ONE-coordinate scan is
 * byte-identical to before:
 *
 *     %geom
 *       Scan
 *         B 0 1 = 1.4, 2.4, 6
 *       end
 *     end
 *
 * A TWO-coordinate scan is a NATIVE nested N₁×N₂ relaxed surface grid — the same one
 * `Scan` block holds both lines, then one `end` closes `Scan` and one closes `%geom`
 * (the measured probe shape, `wiki/orca/scan.md`):
 *
 *     %geom
 *       Scan
 *         B 11 3 = 3.446, 1.5, 4
 *         B 10 0 = 3.4, 1.5, 4
 *       end
 *     end
 *
 * No trailing newline (callers add separators). The 1-coordinate rendering is
 * **byte-identical to Rust `emit_scan_block`** — the golden pair pins that exact string.
 */
export function scanBlock(scan: ScanCoordinate | ScanCoordinate[]): string {
  const body = coerceCoords(scan)
    .map((c) => `    ${scanLine(c)}`)
    .join("\n");
  return `%geom\n  Scan\n${body}\n  end\nend`;
}

/** The `Scan … end` sub-block at a given indent (first line un-indented so a replace can
 * keep the existing leading whitespace). One line per coordinate, in order — a
 * single-coordinate call renders byte-identically to before. */
function scanSubBlock(scan: ScanCoordinate | ScanCoordinate[], indent: string): string {
  const body = coerceCoords(scan)
    .map((c) => `${indent}  ${scanLine(c)}`)
    .join("\n");
  return `Scan\n${body}\n${indent}end`;
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/** Replace every `#`-to-EOL comment with the same number of spaces, keeping token
 * offsets identical to the original text (so a comment inside the block is seen). */
function maskComments(s: string): string {
  return s.replace(/#[^\n]*/g, (m) => " ".repeat(m.length));
}

/** The first non-empty, trimmed line — a compact sample for A2's notice. */
function firstSample(s: string): string {
  return s.split("\n").map((l) => l.trim()).find((l) => l.length > 0) ?? s.trim();
}

/** Parse one scan line (`B 0 1 = 1.4, 2.4, 6`). `null` if malformed. `=` and `,`
 * may be written with or without surrounding spaces. */
function parseScanLine(line: string): ScanCoordinate | null {
  // Isolate `=` and `,` as their own tokens regardless of surrounding spaces.
  const toks = line.replace(/([=,])/g, " $1 ").split(/\s+/).filter(Boolean);
  const letter = (toks[0] ?? "").toUpperCase();
  if (letter !== "B" && letter !== "A" && letter !== "D") return null;
  const nAtoms = ATOM_COUNT[letter];

  let p = 1;
  const atoms: number[] = [];
  for (let k = 0; k < nAtoms; k++) {
    const t = toks[p++];
    if (t === undefined || !/^\d+$/.test(t)) return null;
    atoms.push(fromOrcaIndex(Number(t)));
  }
  if (toks[p++] !== "=") return null;
  const startTok = toks[p++];
  if (toks[p++] !== ",") return null;
  const endTok = toks[p++];
  if (toks[p++] !== ",") return null;
  const nTok = toks[p++];
  if (startTok === undefined || endTok === undefined || nTok === undefined) return null;
  if (p !== toks.length) return null; // trailing garbage

  const start = Number(startTok);
  const end = Number(endTok);
  if (!Number.isFinite(start) || !Number.isFinite(end)) return null;
  if (!/^\d+$/.test(nTok)) return null;
  const npoints = Number(nTok);
  if (npoints < 2) return null;

  // Preserve the exact text only when it isn't the canonical rendering.
  const range: ScanRange = {
    start,
    end,
    npoints,
    ...(formatValue(start) !== startTok && { startText: startTok }),
    ...(formatValue(end) !== endTok && { endText: endTok }),
  };
  switch (letter) {
    case "B":
      return { kind: "B", atoms: [atoms[0], atoms[1]], ...range };
    case "A":
      return { kind: "A", atoms: [atoms[0], atoms[1], atoms[2]], ...range };
    case "D":
      return { kind: "D", atoms: [atoms[0], atoms[1], atoms[2], atoms[3]], ...range };
  }
}

/**
 * What a `Scan … end` sub-block *is*, so a caller can tell **"no block"** from **"a
 * block I don't fully understand"** (the same non-destructive discipline as
 * `inspectConstraintsBlock`). A1 owns a **single** scan coordinate, so more than one
 * scan line, a comment inside the block, or unmodelled syntax → `unrecognised`, and
 * `injectScan` must not rewrite it.
 */
export type ScanInspection =
  | { kind: "absent" }
  | { kind: "parsed"; scan: ScanCoordinate }
  | { kind: "unrecognised"; sample: string };

export function inspectScanBlock(input: string): ScanInspection {
  // Find the live block on comment-masked text (a commented-out block is absent),
  // but read inner content from the ORIGINAL text (offsets align — mask keeps
  // length) so a comment inside the block is visible and counts as unrecognised.
  const masked = maskComments(input);
  const toks = scanTokens(masked);
  const gi = toks.findIndex((t) => t.t.toLowerCase() === "scan");
  if (gi < 0) return { kind: "absent" };
  const endTok = toks.slice(gi + 1).find((t) => t.t.toLowerCase() === "end");
  if (!endTok) return { kind: "absent" }; // never closes → not a block we own
  const innerRaw = input.slice(toks[gi].end, endTok.start);

  if (innerRaw.includes("#")) return { kind: "unrecognised", sample: firstSample(innerRaw) };

  const lines = innerRaw.split("\n").map((l) => l.trim()).filter((l) => l.length > 0);
  if (lines.length !== 1) {
    // Zero lines (empty block) or >1 (multi-coordinate scan) — not the A1 model.
    return { kind: "unrecognised", sample: lines[0] ?? "" };
  }
  const scan = parseScanLine(lines[0]);
  if (!scan) return { kind: "unrecognised", sample: lines[0] };
  return { kind: "parsed", scan };
}

/**
 * Parse the `Scan … end` sub-block: the single coordinate for a fully-recognised
 * one, `null` for **both** absent and unrecognised — so a caller that only wants
 * "the scan, if I can trust it" gets `null` the moment the block holds anything we
 * couldn't safely rewrite.
 */
export function parseScanBlock(input: string): ScanCoordinate | null {
  const ins = inspectScanBlock(input);
  return ins.kind === "parsed" ? ins.scan : null;
}

// ── Injection ────────────────────────────────────────────────────────────────

/**
 * Insert / replace / remove the `Scan` sub-block, **composing into the single
 * `%geom`** and disturbing nothing else (any `Constraints`, `maxiter`, …). Four
 * shapes, mirroring `injectConstraints`:
 *  - no `%geom` at all → a full `%geom Scan … end end` before the geometry;
 *  - `%geom` present, no `Scan` → insert the sub-block just inside it (sibling of
 *    any `Constraints`) — NEVER a second `%geom`;
 *  - `%geom` with a `Scan` → replace that sub-block in place (no duplicate);
 *  - empty / `null` → remove an existing `Scan`, leaving any `Constraints` intact
 *    (a no-op when there is none).
 *
 * `scan` is **one coordinate OR a list** (1..N): a list emits every coordinate inside the
 * ONE `Scan` block (a native N₁×…×N_k relaxed grid), still one `%geom`. A single coordinate
 * (or a 1-element list) renders byte-identically to the pre-2D emit. `null` or an empty list
 * removes. The list order is the grid's scan order (outer→inner).
 */
export function injectScan(input: string, scan: ScanCoordinate | ScanCoordinate[] | null): string {
  const coords = scan === null ? [] : coerceCoords(scan);
  const isEmpty = coords.length === 0;
  const geom = locateGeom(input);

  if (!geom) {
    if (isEmpty) return input;
    const block = scanBlock(coords);
    const coordIdx = input.search(/^[ \t]*\*/m);
    if (coordIdx >= 0) {
      const before = input.slice(0, coordIdx);
      const sep = before.endsWith("\n") || before === "" ? "" : "\n";
      return before + sep + block + "\n" + input.slice(coordIdx);
    }
    const sep = input.endsWith("\n") || input === "" ? "" : "\n";
    return input + sep + block + "\n";
  }

  const existing = geom.subBlocks.get("scan");
  if (existing) {
    const { start, end } = existing;
    if (isEmpty) {
      // Remove the sub-block and the blank line it leaves behind.
      const lineStart = input.lastIndexOf("\n", start - 1) + 1;
      let after = end;
      if (input[after] === "\n") after += 1;
      return input.slice(0, lineStart) + input.slice(after);
    }
    const indent = leadingIndent(input, start);
    return input.slice(0, start) + scanSubBlock(coords, indent) + input.slice(end);
  }

  // %geom exists, no Scan → insert right after the %geom line (sibling of Constraints).
  if (isEmpty) return input;
  const nl = input.indexOf("\n", geom.geomOpen.end);
  const at = nl < 0 ? input.length : nl + 1;
  const block = "  " + scanSubBlock(coords, "  ") + "\n";
  return input.slice(0, at) + block + input.slice(at);
}

/**
 * Parse **every** coordinate line of the `Scan … end` sub-block into an ordered list — the
 * N-aware superset of {@link parseScanBlock} (which owns only the 1-coordinate case). `null`
 * for absent, an empty block, an inner `#` comment, or **any** line that fails to parse
 * (all-or-nothing — the same non-destructive safety: we never half-understand a scan). The
 * order is the grid's outer→inner scan order (row-major, matching the ORCA `.dat` layout —
 * `wiki/orca/scan.md`). A 1-coordinate block returns `[coord]`.
 *
 * The closing token is the FIRST `end` after `Scan` — coordinate lines (`B i j = …`) hold no
 * `end`, so this correctly bounds the block whether it has one line or many (the single-`%geom`
 * footgun lives in {@link locateGeom}, not here).
 */
export function parseScanCoordinates(input: string): ScanCoordinate[] | null {
  const masked = maskComments(input);
  const toks = scanTokens(masked);
  const gi = toks.findIndex((t) => t.t.toLowerCase() === "scan");
  if (gi < 0) return null;
  const endTok = toks.slice(gi + 1).find((t) => t.t.toLowerCase() === "end");
  if (!endTok) return null;
  const innerRaw = input.slice(toks[gi].end, endTok.start);
  if (innerRaw.includes("#")) return null;

  const lines = innerRaw.split("\n").map((l) => l.trim()).filter((l) => l.length > 0);
  if (lines.length === 0) return null;
  const coords: ScanCoordinate[] = [];
  for (const line of lines) {
    const c = parseScanLine(line);
    if (!c) return null;
    coords.push(c);
  }
  return coords;
}

// ── `! Opt` guard ─────────────────────────────────────────────────────────────

/**
 * The optimization keywords **measured** to drive a relaxed scan (rule #10 — a
 * real ORCA 6.1 ethane C–C scan, each producing a 6-row `.relaxscanact.dat`;
 * recorded in `wiki/orca/scan.md`). NOT widened from memory/docs: a keyword only
 * enters this set once a run confirms it triggers the scan, precisely so a
 * legitimate `! TightOpt` scan is not false-blocked.
 *   - `opt`, `optts`  — Stage A1
 *   - `tightopt`, `verytightopt`, `looseopt` — Stage A2 probe
 */
const RELAXED_SCAN_OPT_KEYWORDS = new Set([
  "opt",
  "optts",
  "tightopt",
  "verytightopt",
  "looseopt",
]);

/** Does any `!` keyword line carry a measured relaxed-scan optimization keyword?
 * Comment-masked so a commented-out keyword doesn't count. */
function hasOptKeyword(input: string): boolean {
  for (const raw of maskComments(input).split("\n")) {
    const line = raw.trim();
    if (!line.startsWith("!")) continue;
    for (const tok of line.slice(1).split(/\s+/).filter(Boolean)) {
      if (RELAXED_SCAN_OPT_KEYWORDS.has(tok.toLowerCase())) return true;
    }
  }
  return false;
}

/**
 * The loud guard: a scan is present but the `!` line has no `Opt`/`OptTS`. Without
 * `Opt` ORCA runs a **single point** and silently ignores the `Scan` block
 * (measured — `wiki/orca/parse-sources.md`), producing a plausible-but-empty
 * "scan". Returns a diagnostic string, or `null` when there is no scan or `Opt` is
 * present. Pure and testable here; A2 wires it to block Run.
 */
export function scanOptIssue(input: string): string | null {
  const geom = locateGeom(input);
  if (!geom || !geom.subBlocks.has("scan")) return null;
  if (hasOptKeyword(input)) return null;
  return "relaxed scan needs `! Opt` (else ORCA does a single point and silently ignores the Scan block — measured, wiki/orca/parse-sources.md)";
}

// ── Build from a selection (A2 — the ORCA-index emit seam) ────────────────────

/**
 * Build a `ScanCoordinate` from an ordered {@link AtomId} selection — the same
 * length→kind rule as `constraintFromSelection` (2 → B, 3 → A, 4 → D). The AtomId →
 * 0-based global index resolution happens **HERE, once, at build time** (the emit
 * seam, ADR-010): the scan is positional/textual by design (it lives in the `%geom`
 * text), so the id is resolved to a concrete index now and frozen in — never stored
 * as an id, and deliberately NOT re-tracked across later edits (the composition-
 * change warning surfaces that, as for constraints). `range` supplies the (editable)
 * start/end/npoints defaults. Returns `null` for a selection that isn't 2/3/4 atoms,
 * an id no longer in the scene, or an invalid range.
 */
export function scanFromSelection(
  scene: Scene,
  selection: AtomId[],
  range: { start: number; end: number; npoints: number },
): ScanCoordinate | null {
  const { start, end, npoints } = range;
  if (!Number.isFinite(start) || !Number.isFinite(end)) return null;
  if (!Number.isInteger(npoints) || npoints < 2) return null;
  const gi = selection.map((id) => globalIndexOfAtom(scene, id));
  if (gi.some((i) => i === null)) return null; // an atom left the scene
  const idx = gi as number[];
  const r = { start, end, npoints };
  if (idx.length === 2) return { kind: "B", atoms: [idx[0], idx[1]], ...r };
  if (idx.length === 3) return { kind: "A", atoms: [idx[0], idx[1], idx[2]], ...r };
  if (idx.length === 4) return { kind: "D", atoms: [idx[0], idx[1], idx[2], idx[3]], ...r };
  return null;
}
