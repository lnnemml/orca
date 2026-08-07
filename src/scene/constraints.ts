/**
 * ORCA `%geom Constraints` — pure generate / parse / inject (2.5.4a). No React,
 * no fetch, node-testable. The **text of the ORCA input is the source of truth**
 * for constraints, exactly as it is for the `!` keyword line and the geometry
 * (ADR-008): the 2.5.4b UI panel will be a *view over the text*, not a parallel
 * store. A second home for constraints would drift from the input the same way a
 * parallel Scene would drift from the coordinate block if we hadn't introduced
 * `xyzMatchesScene`. So every operation here round-trips through the text.
 *
 * See `wiki/orca/constraints.md` for the block syntax and the empirically-settled
 * index base.
 */

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import { globalIndexOfAtom } from "./scene";
import { scanTokens, locateGeom, leadingIndent } from "./geomBlock";

/** A geometry constraint, atoms in OrcaStudio's own 0-based global index space
 * (the merged-xyz / ASE-mask space, ADR-008). `value` optional: present → freeze
 * at that explicit value; absent → freeze at the current geometry.
 *
 * `valueText` (2.5.5) preserves the user's EXACT numeric text through a
 * parse→inject round-trip when it isn't the canonical rendering of the number
 * (e.g. `90.0`, which `String(90)` would flatten to `90`). Set by the parser only
 * for non-canonical text; unset for freshly-built constraints. It exists so a
 * rewrite of a recognised block does not silently reformat what the user typed. */
export type Constraint =
  | { kind: "distance"; atoms: [number, number]; value?: number; valueText?: string }
  | { kind: "angle"; atoms: [number, number, number]; value?: number; valueText?: string }
  | {
      kind: "dihedral";
      atoms: [number, number, number, number];
      value?: number;
      valueText?: string;
    }
  | { kind: "cartesian"; atoms: [number] };

/** OrcaStudio's own atom indices are **0-based** (ADR-008). */
const APP_INDEX_BASE = 0;

/**
 * ORCA's `%geom Constraints` atom indices are **0-based** — SETTLED BY A REAL
 * ORCA 6.1.0 RUN on 2026-07-29, not by memory (`gotchas.md` forbids trusting
 * memory about ORCA behaviour, and an off-by-one here freezes the wrong bond on a
 * calculation that finishes "successfully"). The run: chloromethane with atom
 * order `Cl, C, H, H, H` and the constraint `{B 1 2 1.234 C}`. After the opt, the
 * **C–H** bond (atoms 1,2 under 0-based = C,H) sat at exactly 1.234 Å while C–Cl
 * relaxed to 1.80 Å — and ORCA's own internal-coordinate table printed
 * `B(H 2, C 1)`, i.e. carbon = atom **1**, chlorine = atom **0**. An out-of-range
 * probe `{C 5 C}` on the 5-atom molecule **segfaulted** (index 5 reads past the
 * 0..4 array) — ORCA does NOT bounds-check the index. See wiki/orca/constraints.md.
 */
export const ORCA_INDEX_BASE: 0 | 1 = 0;

/** OrcaStudio global index → the index to write in a constraint line. */
export function toOrcaIndex(globalIndex: number): number {
  return globalIndex - APP_INDEX_BASE + ORCA_INDEX_BASE;
}

/** A constraint-line index → OrcaStudio global index. Inverse of `toOrcaIndex`. */
export function fromOrcaIndex(orcaIndex: number): number {
  return orcaIndex - ORCA_INDEX_BASE + APP_INDEX_BASE;
}

const TYPE_LETTER: Record<Constraint["kind"], string> = {
  distance: "B",
  angle: "A",
  dihedral: "D",
  cartesian: "C",
};
const ATOM_COUNT: Record<string, number> = { B: 2, A: 3, D: 4, C: 1 };
const LETTER_KIND: Record<string, Constraint["kind"]> = {
  B: "distance",
  A: "angle",
  D: "dihedral",
  C: "cartesian",
};

/** `Number(String(v)) === v` for finite v, so this round-trips through parse. */
function formatValue(v: number): string {
  return String(v);
}

/** One `{ … }` constraint line (no indentation). */
function constraintLine(c: Constraint): string {
  const idx = c.atoms.map(toOrcaIndex).join(" ");
  if (c.kind === "cartesian") return `{C ${idx} C}`;
  // Prefer the user's exact text (valueText) over a re-rendered number, so a
  // rewrite never turns `90.0` into `90`.
  const vt = c.valueText ?? (c.value !== undefined ? formatValue(c.value) : undefined);
  const value = vt !== undefined ? ` ${vt}` : "";
  // Trailing `C` is ORCA's "constrain" flag — required for the coordinate to be
  // held (verified in the 6.1.0 run).
  return `{${TYPE_LETTER[c.kind]} ${idx}${value} C}`;
}

/**
 * The standalone block for an input that has no `%geom` yet:
 *
 *     %geom
 *       Constraints
 *         {B 0 1 1.234 C}
 *       end
 *     end
 *
 * No trailing newline (callers add separators).
 */
export function constraintsBlock(cs: Constraint[]): string {
  const lines = cs.map((c) => "    " + constraintLine(c)).join("\n");
  return `%geom\n  Constraints\n${lines}\n  end\nend`;
}

// ── Parsing ──────────────────────────────────────────────────────────────────

/** Replace every `#`-to-EOL comment with the **same number of spaces**, so token
 * offsets stay identical to the original text (unlike a line-join strip). Used to
 * find the live block while ignoring commented-out `Constraints`/`end`/`{…}`. */
function maskComments(s: string): string {
  return s.replace(/#[^\n]*/g, (m) => " ".repeat(m.length));
}

/** Parse the inside of one `{ … }` (braces already removed). `null` if malformed. */
function parseConstraintToken(inner: string): Constraint | null {
  const toks = inner.trim().split(/\s+/).filter(Boolean);
  if (toks.length < 2) return null;
  const type = toks[0].toUpperCase();
  const nAtoms = ATOM_COUNT[type];
  if (nAtoms === undefined) return null;

  let rest = toks.slice(1);
  // Drop the trailing "constrain" flag `C` if present (tolerant: some inputs omit it).
  if (rest.length && rest[rest.length - 1].toUpperCase() === "C") {
    rest = rest.slice(0, -1);
  }
  if (rest.length < nAtoms || rest.length > nAtoms + 1) return null;

  const atoms: number[] = [];
  for (const t of rest.slice(0, nAtoms)) {
    if (!/^\d+$/.test(t)) return null; // atom indices are non-negative integers
    atoms.push(fromOrcaIndex(Number(t)));
  }

  let value: number | undefined;
  let valueText: string | undefined;
  const valueToks = rest.slice(nAtoms);
  if (valueToks.length === 1) {
    if (type === "C") return null; // cartesian carries no value
    value = Number(valueToks[0]);
    if (!Number.isFinite(value)) return null;
    // Keep the exact text only when it isn't the canonical rendering (e.g. `90.0`,
    // `1.2340`) — canonical numbers round-trip via `formatValue` with no baggage.
    if (String(value) !== valueToks[0]) valueText = valueToks[0];
  }
  const valueFields = {
    ...(value !== undefined && { value }),
    ...(valueText !== undefined && { valueText }),
  };

  const kind = LETTER_KIND[type];
  switch (kind) {
    case "distance":
      return { kind, atoms: [atoms[0], atoms[1]], ...valueFields };
    case "angle":
      return { kind, atoms: [atoms[0], atoms[1], atoms[2]], ...valueFields };
    case "dihedral":
      return { kind, atoms: [atoms[0], atoms[1], atoms[2], atoms[3]], ...valueFields };
    case "cartesian":
      return { kind, atoms: [atoms[0]] };
  }
}

/**
 * What a `Constraints … end` sub-block *is*, so a caller can tell **"no block"**
 * from **"a block I don't fully understand"** (2.5.5). The distinction is
 * load-bearing: `injectConstraints` rewrites the whole block, so it must run ONLY
 * on `parsed` — otherwise a comment or an unknown token the panel couldn't show
 * gets silently destroyed on the next add/delete (the 2.5.4b data-loss bug).
 *
 * **We rewrite only what we fully recognised.** `unrecognised` covers a `{ … }`
 * token we can't parse AND a `#` comment inside the block (we can't preserve a
 * comment through a rewrite) AND any non-`{…}` syntax we don't model.
 */
export type ConstraintsInspection =
  | { kind: "absent" }
  | { kind: "parsed"; cs: Constraint[] }
  | { kind: "unrecognised"; sample: string };

export function inspectConstraintsBlock(input: string): ConstraintsInspection {
  // Find the LIVE block on comment-masked text (a commented-out block is absent),
  // but read its inner content from the ORIGINAL text (offsets align — mask keeps
  // length) so a comment INSIDE the block is visible and counts as unrecognised.
  const masked = maskComments(input);
  const toks = scanTokens(masked);
  const gi = toks.findIndex((t) => t.t.toLowerCase() === "constraints");
  if (gi < 0) return { kind: "absent" };
  const endTok = toks.slice(gi + 1).find((t) => t.t.toLowerCase() === "end");
  if (!endTok) return { kind: "absent" }; // never closes → not a block we own
  const innerRaw = input.slice(toks[gi].end, endTok.start);

  // A comment inside the block — we can't round-trip it, so hands off.
  if (innerRaw.includes("#")) return { kind: "unrecognised", sample: firstSample(innerRaw) };

  const braceTokens = innerRaw.match(/\{[^}]*\}/g) ?? [];
  const cs: Constraint[] = [];
  for (const tk of braceTokens) {
    const c = parseConstraintToken(tk.slice(1, -1));
    if (!c) return { kind: "unrecognised", sample: tk.trim() };
    cs.push(c);
  }
  // Anything that isn't a `{ … }` or whitespace is syntax we don't model.
  const leftover = innerRaw.replace(/\{[^}]*\}/g, "").trim();
  if (leftover) return { kind: "unrecognised", sample: firstSample(leftover) };
  return { kind: "parsed", cs };
}

/** The first non-empty, trimmed line — a compact sample for the panel's notice. */
function firstSample(s: string): string {
  return s.split("\n").map((l) => l.trim()).find((l) => l.length > 0) ?? s.trim();
}

/**
 * Parse the `Constraints … end` sub-block. A convenience over
 * `inspectConstraintsBlock`: `[]` for a present-but-empty block, the constraints
 * for a fully-recognised one, and `null` for **both** absent and unrecognised —
 * so a caller that only wants "the constraints, if I can trust them" gets `null`
 * the moment the block holds anything we couldn't safely rewrite.
 */
export function parseConstraintsBlock(input: string): Constraint[] | null {
  const ins = inspectConstraintsBlock(input);
  return ins.kind === "parsed" ? ins.cs : null;
}

// ── Injection ────────────────────────────────────────────────────────────────
// The `%geom` locator (`locateGeom`) and `leadingIndent` live in `geomBlock.ts`,
// shared with `injectScan` so both compose into the single `%geom` — see that
// module's note on why Scan and Constraints must share one depth-tracking parser.

/** The `Constraints … end` sub-block at a given indent (first line un-indented so
 * a replace can keep the existing leading whitespace). */
function constraintsSubBlock(cs: Constraint[], indent: string): string {
  const inner = cs.map((c) => indent + "  " + constraintLine(c)).join("\n");
  return `Constraints\n${inner}\n${indent}end`;
}

/**
 * Replace the existing `Constraints` block or insert a new one, **without
 * disturbing any other `%geom` setting** (maxiter, etc.) or the rest of the
 * input. Three shapes:
 *  - no `%geom` at all → a full `%geom Constraints … end end` before the geometry;
 *  - `%geom` present, no `Constraints` → insert the sub-block just inside it;
 *  - `%geom` with a `Constraints` → replace that sub-block in place (no duplicate).
 * `cs === []` removes an existing block (and is a no-op when there is none).
 */
export function injectConstraints(input: string, cs: Constraint[]): string {
  const geom = locateGeom(input);

  if (!geom) {
    if (cs.length === 0) return input;
    const block = constraintsBlock(cs);
    const coordIdx = input.search(/^[ \t]*\*/m);
    if (coordIdx >= 0) {
      const before = input.slice(0, coordIdx);
      const sep = before.endsWith("\n") || before === "" ? "" : "\n";
      return before + sep + block + "\n" + input.slice(coordIdx);
    }
    const sep = input.endsWith("\n") || input === "" ? "" : "\n";
    return input + sep + block + "\n";
  }

  const existing = geom.subBlocks.get("constraints");
  if (existing) {
    const { start, end } = existing;
    if (cs.length === 0) {
      // Remove the sub-block and the blank line it leaves behind.
      const lineStart = input.lastIndexOf("\n", start - 1) + 1;
      let after = end;
      if (input[after] === "\n") after += 1;
      return input.slice(0, lineStart) + input.slice(after);
    }
    const indent = leadingIndent(input, start);
    return input.slice(0, start) + constraintsSubBlock(cs, indent) + input.slice(end);
  }

  // %geom exists, no Constraints → insert right after the %geom line.
  if (cs.length === 0) return input;
  const nl = input.indexOf("\n", geom.geomOpen.end);
  const at = nl < 0 ? input.length : nl + 1;
  const block = "  " + constraintsSubBlock(cs, "  ") + "\n";
  return input.slice(0, at) + block + input.slice(at);
}

// ── UI helpers (2.5.4b) ──────────────────────────────────────────────────────

/**
 * Range-check every constraint's atom indices against the atom count of the
 * geometry the constraint will run against. **This is the guard that stops a
 * segfault:** ORCA does NOT validate constraint indices — an out-of-range index
 * makes it read past the atom array and crash with no diagnostic (verified,
 * `wiki/orca/constraints.md`). Returns one entry per offending constraint with
 * the bad indices (in OrcaStudio's 0-based space); an empty array means safe.
 *
 * The dangerous path is not only hand-typing `{B 0 99 C}` — it's editing the
 * scene: a constraint written at 38 atoms, then a fragment removed → 33 atoms,
 * leaves the `%geom` block untouched (Scene→Monaco rewrites only the coordinate
 * block), so index 37 is now out of range. Valid range is `[0, atomCount)`.
 */
export function constraintIndexIssues(
  cs: Constraint[],
  atomCount: number,
): { constraint: Constraint; badIndices: number[] }[] {
  const out: { constraint: Constraint; badIndices: number[] }[] = [];
  for (const c of cs) {
    const badIndices = c.atoms.filter((i) => i < 0 || i >= atomCount);
    if (badIndices.length > 0) out.push({ constraint: c, badIndices });
  }
  return out;
}

/**
 * Build a constraint from an ordered {@link AtomId} selection (unit 2c2) — the
 * same length→kind rule as `measureSelection` (2 → distance, 3 → angle, 4 →
 * dihedral). `value` omitted → freeze the coordinate as-is (the common TS-guess
 * case). Returns `null` for a selection that isn't 2/3/4 atoms, or if any id is no
 * longer in the scene.
 *
 * **The AtomId → index conversion happens HERE, once, at build time** — this is
 * the ORCA-index emit seam (ADR-010: order matters in exactly one place). A
 * `Constraint` is **positional/textual by design** (it lives in the `%geom` block,
 * and the input text is the source of truth — 2.5.4a): its atoms are 0-based
 * global indices *as of when the constraint was made*. They deliberately do NOT
 * track later edits (that is the no-remap rule the composition-change warning
 * surfaces), so the id is resolved to a concrete index now and frozen into the
 * text, never stored as an id. Kept in click order (so the line reads the way the
 * chemist picked).
 */
export function constraintFromSelection(
  scene: Scene,
  selection: AtomId[],
  value?: number,
): Constraint | null {
  const withValue = value !== undefined && Number.isFinite(value) ? { value } : {};
  const gi = selection.map((id) => globalIndexOfAtom(scene, id));
  if (gi.some((i) => i === null)) return null; // an atom left the scene
  const idx = gi as number[];
  if (idx.length === 2) {
    return { kind: "distance", atoms: [idx[0], idx[1]], ...withValue };
  }
  if (idx.length === 3) {
    return { kind: "angle", atoms: [idx[0], idx[1], idx[2]], ...withValue };
  }
  if (idx.length === 4) {
    return {
      kind: "dihedral",
      atoms: [idx[0], idx[1], idx[2], idx[3]],
      ...withValue,
    };
  }
  return null;
}

/** Structural equality (kind + atoms in the same order) — used to avoid adding
 * the exact same constraint twice from a repeated "Constrain selection". */
export function sameConstraint(a: Constraint, b: Constraint): boolean {
  return (
    a.kind === b.kind &&
    a.atoms.length === b.atoms.length &&
    a.atoms.every((x, i) => x === b.atoms[i])
  );
}
