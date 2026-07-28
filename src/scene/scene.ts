/**
 * Pure core of the Scene / SceneFragment model (ADR-008, task 2.5.0a).
 *
 * Every function here is pure and immutable — none mutate their inputs. The one
 * exception is {@link makeFragmentId}, deliberately kept as the only impure
 * helper (it calls `crypto.randomUUID()`) so that every other function is
 * deterministic and tests can pass literal ids.
 *
 * No React / 3Dmol / Tauri imports: this module is node-testable on its own.
 *
 * OVERLAP (consolidation completes in 2.5.0d, not 2.5.0b): `src/viewer/
 * xyz-format.ts` and `src/viewer/parse-xyz-from-input.ts` parse coordinate lines
 * into *string* rows (`"O   0.0   0.0   0.1"`) for the Phase 2 viewer path. This
 * module parses into structured {@link SceneAtom} objects, and {@link
 * sceneFromOrcaInput} below extracts the `* xyz c m ... *` block straight into a
 * Scene. 2.5.0b migrated only `InputBuilderForm.tsx` onto `src/scene/`; the
 * viewer helpers stay in use by `NewJobScreen.tsx` / `MoleculesScreen.tsx`
 * (both rewritten in 2.5.0d), so `sceneFromOrcaInput` deliberately duplicates a
 * little of `extractXyzFromInput` for now. 2.5.0d removes the viewer copies once
 * every screen is migrated — that is where the ADR-008 "full consolidation"
 * lands. See `wiki/modules/scene.md`.
 */

import {
  FRAGMENT_SOURCES,
  type FragmentSource,
  type Scene,
  type SceneAtom,
  type SceneFragment,
} from "./types";

// ── Atomic numbers, H–Kr (enough for organic + first-row-TM chemistry) ───────

const ATOMIC_NUMBERS: Readonly<Record<string, number>> = {
  H: 1, He: 2,
  Li: 3, Be: 4, B: 5, C: 6, N: 7, O: 8, F: 9, Ne: 10,
  Na: 11, Mg: 12, Al: 13, Si: 14, P: 15, S: 16, Cl: 17, Ar: 18,
  K: 19, Ca: 20,
  Sc: 21, Ti: 22, V: 23, Cr: 24, Mn: 25, Fe: 26, Co: 27, Ni: 28, Cu: 29, Zn: 30,
  Ga: 31, Ge: 32, As: 33, Se: 34, Br: 35, Kr: 36,
};

/** Canonicalise an element symbol to `Xx` casing (`cl` / `CL` → `Cl`). */
function normalizeElement(symbol: string): string {
  if (symbol.length === 0) return symbol;
  return symbol[0].toUpperCase() + symbol.slice(1).toLowerCase();
}

/** Atomic number for an element symbol. Throws (naming the symbol) if unknown. */
export function atomicNumber(symbol: string): number {
  const z = ATOMIC_NUMBERS[normalizeElement(symbol)];
  if (z === undefined) {
    throw new Error(`unknown element symbol: "${symbol}" (supported H–Kr)`);
  }
  return z;
}

// ── The only impure helper ───────────────────────────────────────────────────

/** Fresh fragment id. The ONLY non-deterministic function in this module. */
export function makeFragmentId(): string {
  return crypto.randomUUID();
}

// ── Merge / canonical serialization (ADR-008 decision 4) ─────────────────────

/** Every atom of the scene, in fragment order then in-fragment order. */
function allAtoms(scene: Scene): SceneAtom[] {
  return scene.fragments.flatMap((f) => f.atoms);
}

/**
 * Format one coordinate: `toFixed(8)` right-aligned in a 14-char column. A
 * non-finite coordinate is a programming error — throw rather than emit `NaN`.
 */
function formatCoord(n: number): string {
  if (!Number.isFinite(n)) {
    throw new Error(`non-finite coordinate: ${n}`);
  }
  return n.toFixed(8).padStart(14);
}

/** Canonical coordinate row: element padded to 2 chars, then the three coords. */
function atomRow(a: SceneAtom): string {
  return (
    a.element.padEnd(2) + formatCoord(a.x) + formatCoord(a.y) + formatCoord(a.z)
  );
}

/** Merged flat coordinate block — one canonical row per atom, fragment order. */
export function mergeToAtomLines(scene: Scene): string[] {
  return allAtoms(scene).map(atomRow);
}

/** Merged canonical xyz: `count\ncomment\nrows\n`. */
export function mergeToXyz(scene: Scene, comment = ""): string {
  const rows = mergeToAtomLines(scene);
  return `${rows.length}\n${comment}\n${rows.join("\n")}\n`;
}

// ── Aggregates ───────────────────────────────────────────────────────────────

/** Sum of the per-fragment formal charges. */
export function totalCharge(scene: Scene): number {
  return scene.fragments.reduce((sum, f) => sum + f.charge, 0);
}

/** Total number of atoms across all fragments. */
export function atomCount(scene: Scene): number {
  return scene.fragments.reduce((sum, f) => sum + f.atoms.length, 0);
}

/**
 * Electron count: Σ Z − total charge. Its parity constrains the allowed
 * multiplicity parity (ADR-008 decision 8). Throws on an unknown element.
 */
export function electronCount(scene: Scene): number {
  const protons = allAtoms(scene).reduce((sum, a) => sum + atomicNumber(a.element), 0);
  return protons - totalCharge(scene);
}

// ── Index space (the ASE-mask / picking primitives) ──────────────────────────

function requireFragment(scene: Scene, fragmentId: string): SceneFragment {
  const f = scene.fragments.find((frag) => frag.id === fragmentId);
  if (f === undefined) {
    throw new Error(`no fragment with id "${fragmentId}"`);
  }
  return f;
}

/** Number of atoms in all fragments before `fragmentId` (its global offset). */
function fragmentStart(scene: Scene, fragmentId: string): number {
  let start = 0;
  for (const f of scene.fragments) {
    if (f.id === fragmentId) return start;
    start += f.atoms.length;
  }
  throw new Error(`no fragment with id "${fragmentId}"`);
}

/** Global (merged-xyz) index of a fragment-local atom index. */
export function globalIndex(
  scene: Scene,
  fragmentId: string,
  localIndex: number,
): number {
  const fragment = requireFragment(scene, fragmentId);
  if (localIndex < 0 || localIndex >= fragment.atoms.length) {
    throw new Error(
      `local index ${localIndex} out of range for fragment "${fragmentId}" ` +
        `(0..${fragment.atoms.length - 1})`,
    );
  }
  return fragmentStart(scene, fragmentId) + localIndex;
}

/** Contiguous global indices owned by a fragment — the future ASE mask. */
export function fragmentAtomIndices(scene: Scene, fragmentId: string): number[] {
  const fragment = requireFragment(scene, fragmentId);
  const start = fragmentStart(scene, fragmentId);
  return Array.from({ length: fragment.atoms.length }, (_, i) => start + i);
}

/** Which fragment (and local index within it) a global index refers to. */
export function locateAtom(
  scene: Scene,
  globalIdx: number,
): { fragment: SceneFragment; localIndex: number } | null {
  if (!Number.isInteger(globalIdx) || globalIdx < 0) return null;
  let start = 0;
  for (const fragment of scene.fragments) {
    if (globalIdx < start + fragment.atoms.length) {
      return { fragment, localIndex: globalIdx - start };
    }
    start += fragment.atoms.length;
  }
  return null;
}

/**
 * Per-fragment global index ranges, **start inclusive / end exclusive** (same
 * convention as `OutputMatch` col_start/col_end from Phase 2.7). For viewer
 * index-range styling.
 */
export function fragmentRanges(
  scene: Scene,
): { fragmentId: string; start: number; end: number }[] {
  const ranges: { fragmentId: string; start: number; end: number }[] = [];
  let start = 0;
  for (const fragment of scene.fragments) {
    const end = start + fragment.atoms.length;
    ranges.push({ fragmentId: fragment.id, start, end });
    start = end;
  }
  return ranges;
}

// ── Immutable mutators ───────────────────────────────────────────────────────

/** Append a fragment (kept last — placement decides its coordinates). */
export function addFragment(scene: Scene, fragment: SceneFragment): Scene {
  return { ...scene, fragments: [...scene.fragments, fragment] };
}

/** Drop a fragment by id (no-op if absent). */
export function removeFragment(scene: Scene, fragmentId: string): Scene {
  return {
    ...scene,
    fragments: scene.fragments.filter((f) => f.id !== fragmentId),
  };
}

/** Rename a fragment. */
export function renameFragment(
  scene: Scene,
  fragmentId: string,
  name: string,
): Scene {
  return {
    ...scene,
    fragments: scene.fragments.map((f) =>
      f.id === fragmentId ? { ...f, name } : f,
    ),
  };
}

/** Set a fragment's formal charge. */
export function setFragmentCharge(
  scene: Scene,
  fragmentId: string,
  charge: number,
): Scene {
  return {
    ...scene,
    fragments: scene.fragments.map((f) =>
      f.id === fragmentId ? { ...f, charge } : f,
    ),
  };
}

/** Set the system spin multiplicity. */
export function setMultiplicity(scene: Scene, multiplicity: number): Scene {
  return { ...scene, multiplicity };
}

/**
 * Replace a fragment's atoms with new coordinates. Enforces that the atom count
 * and the element sequence are unchanged — geometry operations move atoms, they
 * never alter composition. This invariant is what keeps atom indices stable
 * across an ASE call or an xTB round trip (ADR-008 "index space" rationale).
 * Throws on any composition change.
 */
export function replaceFragmentAtoms(
  scene: Scene,
  fragmentId: string,
  atoms: SceneAtom[],
): Scene {
  const fragment = requireFragment(scene, fragmentId);
  if (atoms.length !== fragment.atoms.length) {
    throw new Error(
      `replaceFragmentAtoms would change atom count for "${fragmentId}": ` +
        `${fragment.atoms.length} → ${atoms.length}`,
    );
  }
  for (let i = 0; i < atoms.length; i++) {
    const before = normalizeElement(fragment.atoms[i].element);
    const after = normalizeElement(atoms[i].element);
    if (before !== after) {
      throw new Error(
        `replaceFragmentAtoms would change element sequence for "${fragmentId}" ` +
          `at index ${i}: ${before} → ${after}`,
      );
    }
  }
  return {
    ...scene,
    fragments: scene.fragments.map((f) =>
      f.id === fragmentId ? { ...f, atoms: atoms.map((a) => ({ ...a })) } : f,
    ),
  };
}

// ── Parsing coordinate blocks into SceneAtoms ────────────────────────────────

/**
 * Parse ORCA coordinate lines (`element x y z`) into {@link SceneAtom}s. Blank
 * lines and `#` comments are skipped; a line without four whitespace-separated
 * fields (element + three finite numbers) is skipped. Returns `null` when no
 * valid atom line is found.
 */
export function parseAtomLines(lines: string[]): SceneAtom[] | null {
  const atoms: SceneAtom[] = [];
  for (const line of lines) {
    const t = line.trim();
    if (t.length === 0 || t.startsWith("#")) continue;
    const parts = t.split(/\s+/);
    if (parts.length < 4) continue;
    const [element, xs, ys, zs] = parts;
    const x = Number(xs);
    const y = Number(ys);
    const z = Number(zs);
    if (![x, y, z].every((n) => Number.isFinite(n))) continue;
    atoms.push({ element, x, y, z });
  }
  return atoms.length > 0 ? atoms : null;
}

/** Options for {@link sceneFromAtomLines}. */
export interface SceneFromAtomLinesOptions {
  /** Fragment id. Pass explicitly for determinism; defaults to a fresh uuid. */
  id?: string;
  name?: string;
  charge?: number;
  multiplicity?: number;
  source?: FragmentSource;
  sourceLabel?: string;
}

/**
 * Build a single-fragment scene from a flat ORCA coordinate block (the "editor"
 * path). Returns `null` if no atoms parse.
 */
export function sceneFromAtomLines(
  atomLines: string[],
  opts: SceneFromAtomLinesOptions = {},
): Scene | null {
  const atoms = parseAtomLines(atomLines);
  if (atoms === null) return null;
  const fragment: SceneFragment = {
    id: opts.id ?? makeFragmentId(),
    name: opts.name ?? "Molecule",
    atoms,
    charge: opts.charge ?? 0,
    source: opts.source ?? "editor",
    ...(opts.sourceLabel !== undefined ? { sourceLabel: opts.sourceLabel } : {}),
  };
  return { fragments: [fragment], multiplicity: opts.multiplicity ?? 1 };
}

/**
 * Build a single-fragment Scene from a full ORCA input by extracting its
 * `* xyz <charge> <mult> ... *` coordinate block: the fragment charge comes from
 * the header, `scene.multiplicity` from the header, and the atoms from the block
 * rows. Returns `null` when there is no inline coordinate block — including the
 * `* xyzfile ...` form, whose geometry lives in an external file we don't read.
 *
 * This is the ORCA-input → Scene adapter used by `InputBuilderForm.tsx` (2.5.0b)
 * so the form no longer needs the viewer parsers. See the OVERLAP note above.
 */
export function sceneFromOrcaInput(
  content: string,
  opts: Omit<SceneFromAtomLinesOptions, "charge" | "multiplicity"> = {},
): Scene | null {
  const lines = content.split(/\r?\n/);

  // Find the opening `* xyz <c> <m>` marker. `xyzfile` also starts with "xyz",
  // so reject it explicitly (external geometry, unreadable here).
  let start = -1;
  let charge = 0;
  let multiplicity = 1;
  for (let i = 0; i < lines.length; i++) {
    const t = lines[i].trim();
    if (!t.startsWith("*")) continue;
    const rest = t.slice(1).trim();
    const keyword = rest.toLowerCase();
    if (keyword.startsWith("xyzfile")) return null;
    if (keyword.startsWith("xyz")) {
      const parts = rest.split(/\s+/); // ["xyz", charge, mult, ...]
      const c = Number(parts[1]);
      const m = Number(parts[2]);
      if (Number.isInteger(c)) charge = c;
      if (Number.isInteger(m)) multiplicity = m;
      start = i;
      break;
    }
  }
  if (start === -1) return null;

  // Collect coordinate rows until the closing `*`.
  const block: string[] = [];
  for (let i = start + 1; i < lines.length; i++) {
    if (lines[i].trim().startsWith("*")) break;
    block.push(lines[i]);
  }

  return sceneFromAtomLines(block, { ...opts, charge, multiplicity });
}

// ── (De)serialization for the `scene_json` snapshot ──────────────────────────

const SCENE_JSON_VERSION = 1;

/** Serialize a scene to the versioned JSON snapshot written to `jobs.scene_json`. */
export function serializeScene(scene: Scene): string {
  return JSON.stringify({
    version: SCENE_JSON_VERSION,
    fragments: scene.fragments,
    multiplicity: scene.multiplicity,
  });
}

function isFiniteNumber(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}

function validAtom(v: unknown): v is SceneAtom {
  if (typeof v !== "object" || v === null) return false;
  const a = v as Record<string, unknown>;
  return (
    typeof a.element === "string" &&
    isFiniteNumber(a.x) &&
    isFiniteNumber(a.y) &&
    isFiniteNumber(a.z)
  );
}

function validFragment(v: unknown): v is SceneFragment {
  if (typeof v !== "object" || v === null) return false;
  const f = v as Record<string, unknown>;
  if (typeof f.id !== "string" || typeof f.name !== "string") return false;
  if (typeof f.charge !== "number" || !Number.isFinite(f.charge)) return false;
  if (!FRAGMENT_SOURCES.includes(f.source as FragmentSource)) return false;
  if (f.sourceLabel !== undefined && typeof f.sourceLabel !== "string") return false;
  if (!Array.isArray(f.atoms) || !f.atoms.every(validAtom)) return false;
  return true;
}

/**
 * Parse a `scene_json` snapshot back into a Scene. Validates shape and version
 * and returns `null` on anything unexpected — never throws on user/DB data.
 */
export function deserializeScene(json: string): Scene | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const obj = parsed as Record<string, unknown>;
  if (obj.version !== SCENE_JSON_VERSION) return null;
  if (typeof obj.multiplicity !== "number" || !Number.isFinite(obj.multiplicity)) {
    return null;
  }
  if (!Array.isArray(obj.fragments) || !obj.fragments.every(validFragment)) {
    return null;
  }
  return {
    fragments: obj.fragments as SceneFragment[],
    multiplicity: obj.multiplicity,
  };
}

// ── Reset-detection primitive (ADR-008 decision 6) ───────────────────────────

/**
 * Does a flat coordinate block match the scene's merged geometry? Parses BOTH
 * sides and compares element symbols (case-insensitive) plus coordinates within
 * `tol`. This is float comparison, **never** string comparison — number
 * formatting differs (`1.0` vs `1.00000000`). A different atom count or element
 * sequence, or a `null` input, is `false`.
 */
export function xyzMatchesScene(
  scene: Scene,
  atomLines: string[] | null,
  tol = 1e-6,
): boolean {
  if (atomLines === null) return false;
  const other = parseAtomLines(atomLines);
  if (other === null) return false;
  const mine = allAtoms(scene);
  if (mine.length !== other.length) return false;
  for (let i = 0; i < mine.length; i++) {
    if (normalizeElement(mine[i].element) !== normalizeElement(other[i].element)) {
      return false;
    }
    if (
      Math.abs(mine[i].x - other[i].x) > tol ||
      Math.abs(mine[i].y - other[i].y) > tol ||
      Math.abs(mine[i].z - other[i].z) > tol
    ) {
      return false;
    }
  }
  return true;
}
