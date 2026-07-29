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

/** A geometry constraint, atoms in OrcaStudio's own 0-based global index space
 * (the merged-xyz / ASE-mask space, ADR-008). `value` optional: present → freeze
 * at that explicit value; absent → freeze at the current geometry. */
export type Constraint =
  | { kind: "distance"; atoms: [number, number]; value?: number }
  | { kind: "angle"; atoms: [number, number, number]; value?: number }
  | { kind: "dihedral"; atoms: [number, number, number, number]; value?: number }
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
  const value = c.value !== undefined ? ` ${formatValue(c.value)}` : "";
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

/** Strip `#`-to-EOL comments so a commented-out `Constraints`/`{…}` never parses
 * as live (ORCA's comment char is `#`). */
function stripComments(s: string): string {
  return s
    .split("\n")
    .map((l) => l.replace(/#.*/, ""))
    .join("\n");
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
  const valueToks = rest.slice(nAtoms);
  if (valueToks.length === 1) {
    if (type === "C") return null; // cartesian carries no value
    value = Number(valueToks[0]);
    if (!Number.isFinite(value)) return null;
  }

  const kind = LETTER_KIND[type];
  switch (kind) {
    case "distance":
      return { kind, atoms: [atoms[0], atoms[1]], ...(value !== undefined && { value }) };
    case "angle":
      return {
        kind,
        atoms: [atoms[0], atoms[1], atoms[2]],
        ...(value !== undefined && { value }),
      };
    case "dihedral":
      return {
        kind,
        atoms: [atoms[0], atoms[1], atoms[2], atoms[3]],
        ...(value !== undefined && { value }),
      };
    case "cartesian":
      return { kind, atoms: [atoms[0]] };
  }
}

/**
 * Parse the `Constraints … end` sub-block out of an ORCA input. Tolerant of
 * whitespace and case. Returns:
 *  - `null` when there is **no** Constraints block (or the input is empty), and
 *  - `null` when a `{ … }` line is malformed (don't silently drop a constraint);
 *  - `[]` for a present-but-empty block.
 */
export function parseConstraintsBlock(input: string): Constraint[] | null {
  const clean = stripComments(input);
  // First `end` after `Constraints` closes it — constraint lines carry no `end`,
  // so the non-greedy match can't overshoot into the enclosing `%geom` end.
  const m = /constraints\b([\s\S]*?)\bend\b/i.exec(clean);
  if (!m) return null;
  const tokens = m[1].match(/\{[^}]*\}/g);
  if (!tokens) return [];
  const out: Constraint[] = [];
  for (const tk of tokens) {
    const c = parseConstraintToken(tk.slice(1, -1));
    if (!c) return null;
    out.push(c);
  }
  return out;
}

// ── Injection ────────────────────────────────────────────────────────────────

interface Tok {
  t: string;
  start: number;
  end: number;
}
function scanTokens(s: string): Tok[] {
  const re = /\S+/g;
  const out: Tok[] = [];
  let m: RegExpExecArray | null;
  while ((m = re.exec(s))) out.push({ t: m[0], start: m.index, end: m.index + m[0].length });
  return out;
}

/**
 * Locate the `%geom` block by tracking block depth (`Constraints` opens a
 * sub-block; every `end` closes one). Returns the char span of the `%geom`
 * closing `end` and, if present, the char span of the inner `Constraints … end`.
 * Handles both the inline (`%geom Constraints`) and separate-line forms. `null`
 * if there is no `%geom` or it never closes.
 */
function locateGeom(
  text: string,
): { geomOpen: Tok; constraints: { start: number; end: number } | null } | null {
  const toks = scanTokens(text);
  const gi = toks.findIndex((t) => t.t.toLowerCase() === "%geom");
  if (gi < 0) return null;

  let depth = 1; // %geom consumed
  let cStart: number | null = null;
  let cEnd: number | null = null;
  let cDepth = -1;
  for (let i = gi + 1; i < toks.length; i++) {
    const w = toks[i].t.toLowerCase();
    if (w === "constraints") {
      if (cStart === null) {
        cStart = toks[i].start;
        cDepth = depth;
      }
      depth++;
    } else if (w === "end") {
      depth--;
      if (cStart !== null && cEnd === null && depth === cDepth) cEnd = toks[i].end;
      if (depth === 0) {
        return {
          geomOpen: toks[gi],
          constraints: cStart !== null && cEnd !== null ? { start: cStart, end: cEnd } : null,
        };
      }
    }
  }
  return null;
}

/** The `Constraints … end` sub-block at a given indent (first line un-indented so
 * a replace can keep the existing leading whitespace). */
function constraintsSubBlock(cs: Constraint[], indent: string): string {
  const inner = cs.map((c) => indent + "  " + constraintLine(c)).join("\n");
  return `Constraints\n${inner}\n${indent}end`;
}

/** The whitespace before `pos` on its line (the line's indent when `pos` is the
 * first non-space char). */
function leadingIndent(text: string, pos: number): string {
  const lineStart = text.lastIndexOf("\n", pos - 1) + 1;
  return text.slice(lineStart, pos).match(/^\s*/)![0];
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

  if (geom.constraints) {
    const { start, end } = geom.constraints;
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
 * Build a constraint from an ordered atom selection — the same length→kind rule
 * as `measureSelection` (2 → distance, 3 → angle, 4 → dihedral). `value` omitted
 * → freeze the coordinate as-is (the common TS-guess case). Returns `null` for a
 * selection that isn't 2/3/4 atoms. Atoms are in OrcaStudio's 0-based global
 * space, kept in click order (so the constraint reads the way the chemist picked).
 */
export function constraintFromSelection(
  selection: number[],
  value?: number,
): Constraint | null {
  const withValue = value !== undefined && Number.isFinite(value) ? { value } : {};
  if (selection.length === 2) {
    return { kind: "distance", atoms: [selection[0], selection[1]], ...withValue };
  }
  if (selection.length === 3) {
    return { kind: "angle", atoms: [selection[0], selection[1], selection[2]], ...withValue };
  }
  if (selection.length === 4) {
    return {
      kind: "dihedral",
      atoms: [selection[0], selection[1], selection[2], selection[3]],
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
