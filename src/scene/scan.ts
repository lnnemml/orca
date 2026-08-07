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

import { toOrcaIndex, fromOrcaIndex } from "./constraints";
import { scanTokens, locateGeom, leadingIndent } from "./geomBlock";

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

/**
 * The standalone block for an input that has no `%geom` yet:
 *
 *     %geom
 *       Scan
 *         B 0 1 = 1.4, 2.4, 6
 *       end
 *     end
 *
 * No trailing newline (callers add separators). **Byte-identical to Rust
 * `emit_scan_block`** — the golden pair pins this exact string.
 */
export function scanBlock(s: ScanCoordinate): string {
  return `%geom\n  Scan\n    ${scanLine(s)}\n  end\nend`;
}

/** The `Scan … end` sub-block at a given indent (first line un-indented so a
 * replace can keep the existing leading whitespace). */
function scanSubBlock(s: ScanCoordinate, indent: string): string {
  return `Scan\n${indent}  ${scanLine(s)}\n${indent}end`;
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
 *  - `scan === null` → remove an existing `Scan`, leaving any `Constraints` intact
 *    (a no-op when there is none).
 */
export function injectScan(input: string, scan: ScanCoordinate | null): string {
  const geom = locateGeom(input);

  if (!geom) {
    if (scan === null) return input;
    const block = scanBlock(scan);
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
    if (scan === null) {
      // Remove the sub-block and the blank line it leaves behind.
      const lineStart = input.lastIndexOf("\n", start - 1) + 1;
      let after = end;
      if (input[after] === "\n") after += 1;
      return input.slice(0, lineStart) + input.slice(after);
    }
    const indent = leadingIndent(input, start);
    return input.slice(0, start) + scanSubBlock(scan, indent) + input.slice(end);
  }

  // %geom exists, no Scan → insert right after the %geom line (sibling of Constraints).
  if (scan === null) return input;
  const nl = input.indexOf("\n", geom.geomOpen.end);
  const at = nl < 0 ? input.length : nl + 1;
  const block = "  " + scanSubBlock(scan, "  ") + "\n";
  return input.slice(0, at) + block + input.slice(at);
}

// ── `! Opt` guard ─────────────────────────────────────────────────────────────

/** Does any `!` keyword line carry `Opt` (or `OptTS`)? Comment-masked so a
 * commented-out keyword doesn't count. */
function hasOptKeyword(input: string): boolean {
  for (const raw of maskComments(input).split("\n")) {
    const line = raw.trim();
    if (!line.startsWith("!")) continue;
    for (const tok of line.slice(1).split(/\s+/).filter(Boolean)) {
      const w = tok.toLowerCase();
      if (w === "opt" || w === "optts") return true;
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
