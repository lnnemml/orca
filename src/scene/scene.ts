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
 * ORCA-input ↔ Scene text I/O lives here: {@link sceneFromOrcaInput} (read the
 * `* xyz c m ... *` block into a Scene) and {@link injectSceneIntoInput} (write a
 * Scene back). 2.5.0d closed the ADR-008 consolidation — the duplicate viewer
 * parsers `parse-xyz-from-input.ts` and `inject-xyz-into-input.ts` were deleted
 * once `NewJobScreen` moved onto this module. `src/viewer/xyz-format.ts` remains,
 * but only for standard-xyz-string ↔ atom-line formatting (`xyzToAtomLines` /
 * `atomLinesToXyz`), whose live consumers are `import-file.ts` and
 * `MoleculesScreen` — those are not ORCA-input parsers, so no duplication with
 * this module remains. See `wiki/modules/scene.md`.
 */

import { carryIds, stampFreshIds } from "./ids";
import type { AtomId } from "./ids";
import {
  FRAGMENT_SOURCES,
  type FragmentSource,
  type RawAtom,
  type RawFragment,
  type Scene,
  type SceneAtom,
  type SceneFragment,
} from "./types";

// ── Atomic numbers, H–Rn (Z ≤ 86) ───────────────────────────────────────────
// Covers the cross-coupling metals ADR-007 names — Pd (46), Pt (78) — plus the
// lanthanides, so `electronCount` / `checkElectronParity` work for real
// organometallic scenes instead of silently declining on an unknown symbol.

const ATOMIC_NUMBERS: Readonly<Record<string, number>> = {
  H: 1, He: 2,
  Li: 3, Be: 4, B: 5, C: 6, N: 7, O: 8, F: 9, Ne: 10,
  Na: 11, Mg: 12, Al: 13, Si: 14, P: 15, S: 16, Cl: 17, Ar: 18,
  K: 19, Ca: 20,
  Sc: 21, Ti: 22, V: 23, Cr: 24, Mn: 25, Fe: 26, Co: 27, Ni: 28, Cu: 29, Zn: 30,
  Ga: 31, Ge: 32, As: 33, Se: 34, Br: 35, Kr: 36,
  Rb: 37, Sr: 38,
  Y: 39, Zr: 40, Nb: 41, Mo: 42, Tc: 43, Ru: 44, Rh: 45, Pd: 46, Ag: 47, Cd: 48,
  In: 49, Sn: 50, Sb: 51, Te: 52, I: 53, Xe: 54,
  Cs: 55, Ba: 56,
  La: 57, Ce: 58, Pr: 59, Nd: 60, Pm: 61, Sm: 62, Eu: 63, Gd: 64, Tb: 65,
  Dy: 66, Ho: 67, Er: 68, Tm: 69, Yb: 70, Lu: 71,
  Hf: 72, Ta: 73, W: 74, Re: 75, Os: 76, Ir: 77, Pt: 78, Au: 79, Hg: 80,
  Tl: 81, Pb: 82, Bi: 83, Po: 84, At: 85, Rn: 86,
};

/** Canonicalise an element symbol to `Xx` casing (`cl` / `CL` → `Cl`). Exported
 * so composition checks (replaceFragmentAtoms, xyzMatchesScene,
 * conformerMatchesFragment) share one normalisation. */
export function normalizeElement(symbol: string): string {
  if (symbol.length === 0) return symbol;
  return symbol[0].toUpperCase() + symbol.slice(1).toLowerCase();
}

/** Atomic number for an element symbol (H–Rn, Z ≤ 86). Throws (naming the
 * symbol) if unknown. */
export function atomicNumber(symbol: string): number {
  const z = ATOMIC_NUMBERS[normalizeElement(symbol)];
  if (z === undefined) {
    throw new Error(`unknown element symbol: "${symbol}" (supported H–Rn, Z ≤ 86)`);
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

// ── The viewer feed: geometry + AtomId↔viewer-index table, born together ──────
//
// 3Dmol is a dumb renderer (ADR-010 I1 / ADR-011): it is handed one geometry and
// an AtomId↔viewer-index table, and is never a source of truth. "Viewer index" is
// 3Dmol's `atom.index` on an xyz model — the merged-xyz LINE order, which is
// exactly `allAtoms(scene)` order (fragment order, then in-fragment). That is the
// ONLY coupling between our index space and 3Dmol's, and it is made explicit here
// rather than left as a coincidence a caller must re-derive (ADR-008 #3).

/**
 * The mapping between a Scene atom's stable {@link AtomId} and its position in the
 * geometry handed to 3Dmol (the "viewer index" == merged-xyz line == 3Dmol
 * `atom.index`). Both directions, total over the scene it was built from. It is a
 * pure snapshot: it holds no live reference to the scene and is rebuilt whenever
 * the geometry is (never mutated in place), so it cannot drift out from under the
 * geometry it names.
 */
export interface ViewerAtomTable {
  /** AtomId drawn at this viewer index, or `undefined` if out of range. */
  atomIdAt(viewerIndex: number): AtomId | undefined;
  /** Viewer index of this AtomId, or `undefined` if absent from the scene. */
  viewerIndexOf(id: AtomId): number | undefined;
  /** Number of atoms == number of drawn viewer indices (0..length-1). */
  readonly length: number;
}

/**
 * Geometry + its companion table, produced by ONE pass so they cannot disagree.
 * This is the whole point of unit 2c1: the table is not a second piece of state
 * that "also has to be updated" — it is a return value of the same function that
 * forms the geometry, from the same atom sequence.
 */
export interface ViewerFeed {
  /** The merged xyz string handed to 3Dmol's `addModel`. */
  xyz: string;
  /** Its AtomId↔viewer-index table, over the exact same atom order. */
  table: ViewerAtomTable;
}

/** Wrap a viewer-index→AtomId list as a two-way {@link ViewerAtomTable}. The
 * reverse map is a bijection because AtomIds are unique within a Scene (the
 * `nextAtomId` invariant); `buildViewerFeed`/`buildViewerAtomTable` are the only
 * builders, so no caller can construct an inconsistent one. */
function makeViewerAtomTable(idsByViewerIndex: readonly AtomId[]): ViewerAtomTable {
  const indexById = new Map<AtomId, number>();
  idsByViewerIndex.forEach((id, i) => indexById.set(id, i));
  return {
    atomIdAt: (i) => idsByViewerIndex[i],
    viewerIndexOf: (id) => indexById.get(id),
    length: idsByViewerIndex.length,
  };
}

/**
 * Form the geometry 3Dmol will draw AND the AtomId↔viewer-index table for it, in
 * a SINGLE pass over `allAtoms(scene)`. The viewer calls exactly this at the
 * drawing site, so the model it builds and the table it resolves picks through
 * come from one function, one atom sequence — they are the same object, not two
 * states kept in sync. The rows are byte-identical to {@link mergeToXyz} (same
 * `atomRow`), so nothing about the drawn geometry changes.
 */
export function buildViewerFeed(scene: Scene, comment = ""): ViewerFeed {
  const atoms = allAtoms(scene);
  const rows = atoms.map(atomRow);
  const table = makeViewerAtomTable(atoms.map((a) => a.id));
  return { xyz: `${rows.length}\n${comment}\n${rows.join("\n")}\n`, table };
}

/**
 * The table alone (no xyz), for a consumer that must map AtomId→viewer index
 * without rebuilding the geometry string — the 2c1→2c2 boundary adapter in
 * `NewJobScreen`. It is the SAME pure derivation `buildViewerFeed` uses (same
 * `allAtoms` order, same `makeViewerAtomTable`), so a table from here and the
 * feed's table are identical for a given scene — recomputed from the current
 * scene, never a stored copy that could lag.
 */
export function buildViewerAtomTable(scene: Scene): ViewerAtomTable {
  return makeViewerAtomTable(allAtoms(scene).map((a) => a.id));
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
 * The smallest valid `nextAtomId` for a set of fragments already carrying ids:
 * one past the largest id present (0 for no atoms). For wrapping an existing
 * fragment into a fresh single-fragment Scene (e.g. the GOAT snapshot) without
 * re-minting its atom ids — the result satisfies the v2 invariant (every id <
 * nextAtomId).
 */
export function nextAtomIdFor(fragments: SceneFragment[]): number {
  let max = -1;
  for (const f of fragments) {
    for (const a of f.atoms) {
      if (a.id > max) max = a.id;
    }
  }
  return max + 1;
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

/**
 * The **bijection AtomId ↔ global index** over `allAtoms(scene)` order — the same
 * order as the merged xyz and the viewer index (ADR-008 #3). This is the resolver
 * the selection/measure/constraint pipeline keys on in unit 2c2: a `selection`
 * holds stable {@link AtomId}s, and these two functions map an id to where the
 * atom currently sits (and back). It is **independent of** the viewer's
 * pre-compiled `ViewerAtomTable` (2c1) on purpose — that table names indices for
 * *3Dmol*; these name indices for the *core*, so nothing outside the viewer sits
 * on a structure called "viewer". Both return `null` for an absent id / an
 * out-of-range index (the non-throwing contract of {@link locateAtom}).
 */
export function globalIndexOfAtom(scene: Scene, id: AtomId): number | null {
  let i = 0;
  for (const fragment of scene.fragments) {
    for (const atom of fragment.atoms) {
      if (atom.id === id) return i;
      i++;
    }
  }
  return null;
}

/** Inverse of {@link globalIndexOfAtom}: the {@link AtomId} at a global index in
 * `allAtoms(scene)` order, or `null` if out of range. */
export function atomIdAtIndex(scene: Scene, globalIdx: number): AtomId | null {
  if (!Number.isInteger(globalIdx) || globalIdx < 0) return null;
  let i = 0;
  for (const fragment of scene.fragments) {
    for (const atom of fragment.atoms) {
      if (i === globalIdx) return atom.id;
      i++;
    }
  }
  return null;
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

/**
 * A "composition signature" for a scene: the ordered fragment `id:size` list,
 * joined — but **not** the coordinates. The one canonical way to say "the
 * scene's composition changed" (a fragment or atom was added/removed), the
 * sibling of `xyzMatchesScene` for "the coordinates changed"; there must not be
 * a second way to ask this in the code. Two scenes with the same signature
 * differ only in atom positions. Consumers:
 *  - `MoleculeViewer` re-`zoomTo`s only when the signature changes — a
 *    coordinate-only edit must not move the camera (the 2.5.2 geometry loop —
 *    type an angle, apply, look, adjust — is unusable if the view re-zooms on
 *    every apply);
 *  - the constraint composition-change warning (2.5.4b) fires exactly when the
 *    signature moves (the selection itself is pruned by AtomId now — 2c2 — not
 *    by this signature).
 */
export function compositionSignature(scene: Scene): string {
  return scene.fragments.map((f) => `${f.id}:${f.atoms.length}`).join("|");
}

// ── Immutable mutators ───────────────────────────────────────────────────────

/**
 * Append a fragment (kept last — placement decides its coordinates). Its atoms
 * are **new to this Scene**, so they get **fresh** `AtomId`s from the Scene's
 * counter and the counter advances — any ids the incoming fragment carried
 * (e.g. the provisional 0-based ids of a just-built or library fragment) are
 * discarded, because ids are only unique *within a Scene* and this fragment is
 * joining one now. (Contrast `replaceFragmentAtoms`, which is a geometry edit of
 * atoms that already belong to the Scene and therefore *keeps* their ids.)
 */
export function addFragment(scene: Scene, fragment: RawFragment): Scene {
  const { atoms, nextAtomId } = stampFreshIds(fragment.atoms, scene.nextAtomId);
  return {
    ...scene,
    fragments: [...scene.fragments, { ...fragment, atoms }],
    nextAtomId,
  };
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
  atoms: RawAtom[],
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
  // The incoming atoms are RAW (from ASE / xtb / GOAT / parsed xyz — no id). Atom
  // identity is preserved by carrying the OLD atoms' ids positionally; the
  // count + element-order invariant just checked is exactly what makes the
  // positional carry correct. Never mint fresh ids here — that would silently void
  // atom identity on every geometry edit (unit-1b rule). `nextAtomId` is untouched.
  return {
    ...scene,
    fragments: scene.fragments.map((f) =>
      f.id === fragmentId ? { ...f, atoms: carryIds(f.atoms, atoms) } : f,
    ),
  };
}

/**
 * Replace the coordinates of EVERY atom in the scene from one flat list, sliced
 * back to the fragments by their existing `[start, end)` windows (2.5.5). Used
 * when a whole-scene optimizer (xtb pre-opt) hands back a merged geometry: the
 * atom count and element order are an invariant (ADR-008 one-index-space), so
 * each fragment's slice goes through `replaceFragmentAtoms`, which enforces
 * count + element order per fragment. Throws (never silently mis-slices) if the
 * flat list's length doesn't equal `atomCount(scene)`.
 */
export function replaceAllAtoms(scene: Scene, atoms: RawAtom[]): Scene {
  if (atoms.length !== atomCount(scene)) {
    throw new Error(
      `replaceAllAtoms: got ${atoms.length} atoms but the scene has ${atomCount(scene)}`,
    );
  }
  let next = scene;
  for (const { fragmentId, start, end } of fragmentRanges(scene)) {
    next = replaceFragmentAtoms(next, fragmentId, atoms.slice(start, end));
  }
  return next;
}

/**
 * Guard for applying an off-thread result (an xtb pre-optimization, 2.5.5-fix)
 * back to the scene: **apply only to the exact scene reference it was launched
 * against.** The result is a whole-scene geometry computed from the scene as it
 * was at launch; if the user changed the scene while it ran (a fragment
 * added/removed, another edit), applying it would clobber that change or mismatch
 * the atom count. This is the same stale-response drop as the split-mask fetch
 * (2.5.3b) — a result for a superseded input is discarded, not forced on. Identity
 * (not composition) is the key: any `setScene` yields a new reference (store
 * contract), so a coordinate-only edit during the run also (correctly) drops the
 * now-stale result.
 */
export function xtbResultApplies(
  launchedScene: Scene | null,
  currentScene: Scene | null,
): boolean {
  return launchedScene !== null && launchedScene === currentScene;
}

/**
 * Rigid-body translate a fragment by (dx, dy, dz). Pure/immutable — returns a new
 * fragment with the same id, composition and internal geometry, only shifted.
 * Used by fragment placement (2.5.0d-2) and the geometry editor (2.5.3).
 */
export function translateFragment<F extends RawFragment>(
  fragment: F,
  dx: number,
  dy: number,
  dz: number,
): F {
  // Generic over Raw/Scene fragment: `...a` preserves the `id` when there is one
  // (a rigid move keeps atom identity), and its absence when there isn't.
  return {
    ...fragment,
    atoms: fragment.atoms.map((a) => ({
      ...a,
      x: a.x + dx,
      y: a.y + dy,
      z: a.z + dz,
    })),
  } as F;
}

/**
 * Rigid-body translate ONE fragment (by id) within a scene by (dx, dy, dz) Å,
 * leaving every other fragment untouched. Pure/immutable; the moved fragment
 * keeps its id, composition, internal geometry and atom order (`translateFragment`
 * above), so atom identity and count/order are invariant across the move — the
 * post-condition the rigid-drag op relies on (domain rule #9). No-op if the id is
 * absent. This is the scene-level mutator the store's `translateFragment` commits.
 */
export function translateFragmentInScene(
  scene: Scene,
  fragmentId: string,
  dx: number,
  dy: number,
  dz: number,
): Scene {
  return {
    ...scene,
    fragments: scene.fragments.map((f) =>
      f.id === fragmentId ? translateFragment(f, dx, dy, dz) : f,
    ),
  };
}

/**
 * Rigid-body translate an EXPLICIT SET of atoms (by stable {@link AtomId}) by
 * (dx, dy, dz) Å, leaving every other atom fixed. This is the scene-level mutator
 * a drag commits once the moving set is the dragged atom's PERCEIVED CONNECTED
 * COMPONENT (Stage 3.x; sidecar `/geometry/connected-component`) rather than the
 * whole fragment — so after a bond is broken the two pieces move independently.
 * Pure/immutable. Count + atom order are invariant by construction: every atom
 * keeps its exact array slot, only the selected ids have their coordinates shifted
 * (the `replaceFragmentAtoms` discipline of ADR-008, here inlined because nothing
 * about composition changes — same shape as `translateFragmentInScene`). When
 * `atomIds` covers a whole fragment the result equals `translateFragmentInScene`
 * for it — the backward-compatible whole-fragment drag. A no-op on an empty set or
 * a zero delta (returns the SAME reference); ids not in the scene are ignored.
 */
export function translateAtomsInScene(
  scene: Scene,
  atomIds: readonly AtomId[],
  dx: number,
  dy: number,
  dz: number,
): Scene {
  const moving = new Set(atomIds);
  if (moving.size === 0 || (dx === 0 && dy === 0 && dz === 0)) return scene;
  return {
    ...scene,
    fragments: scene.fragments.map((f) =>
      f.atoms.some((a) => moving.has(a.id))
        ? {
            ...f,
            atoms: f.atoms.map((a) =>
              moving.has(a.id)
                ? { ...a, x: a.x + dx, y: a.y + dy, z: a.z + dz }
                : a,
            ),
          }
        : f,
    ),
  };
}

/** Below which an axis vector (Q − P, Å) is treated as degenerate: the two
 * endpoints coincide, so no rotation direction is defined. Well under any real
 * inter-atomic separation (≥ ~0.5 Å), well over float noise. */
const AXIS_DEGENERATE_EPS = 1e-8;

type Vec3 = [number, number, number];

/**
 * Rigid-body rotate a fragment by `angleRad` about the line through `pivot` with
 * direction `axisDir` (Rodrigues' rotation formula). Pure/immutable — returns a
 * new fragment with the same id, composition and internal geometry, only turned.
 * Generic over Raw/Scene (`...a` preserves an `id` when present, its absence when
 * not — a rigid turn keeps atom identity), the sibling of {@link translateFragment}.
 *
 * `axisDir` is normalized here, so a caller may pass a raw `Q − P`; a zero-length
 * axis is a programming error (the degenerate case is caught upstream by
 * {@link rotationAxis} / the store guard, never reaching here) and throws rather
 * than emitting `NaN` — the same discipline as the non-finite-coordinate throw in
 * the canonical xyz writer.
 *
 * **Rigid by construction:** every atom is mapped `p ↦ pivot + R·(p − pivot)` with
 * the SAME rotation `R`, so all pairwise distances inside the fragment are
 * preserved, any point on the axis (`pivot`, and the second axis atom Q which lies
 * on the line) is a fixed point, and count/order are unchanged (domain rule #9 —
 * proven by the c1/c2/c4 negative controls, `scene.test.ts`).
 */
export function rotateFragment<F extends RawFragment>(
  fragment: F,
  axisDir: Vec3,
  angleRad: number,
  pivot: Vec3,
): F {
  const len = Math.hypot(axisDir[0], axisDir[1], axisDir[2]);
  if (!(len > 0)) {
    throw new Error("rotateFragment: degenerate (zero-length) axis");
  }
  const kx = axisDir[0] / len;
  const ky = axisDir[1] / len;
  const kz = axisDir[2] / len;
  const c = Math.cos(angleRad);
  const s = Math.sin(angleRad);
  const [px, py, pz] = pivot;
  return {
    ...fragment,
    atoms: fragment.atoms.map((a) => {
      // v = a − pivot, expressed relative to the axis.
      const vx = a.x - px;
      const vy = a.y - py;
      const vz = a.z - pz;
      // Rodrigues: v_rot = v·cosθ + (k×v)·sinθ + k·(k·v)·(1−cosθ).
      const kdotv = kx * vx + ky * vy + kz * vz;
      const crossx = ky * vz - kz * vy;
      const crossy = kz * vx - kx * vz;
      const crossz = kx * vy - ky * vx;
      const rx = vx * c + crossx * s + kx * kdotv * (1 - c);
      const ry = vy * c + crossy * s + ky * kdotv * (1 - c);
      const rz = vz * c + crossz * s + kz * kdotv * (1 - c);
      return { ...a, x: px + rx, y: py + ry, z: pz + rz };
    }),
  } as F;
}

/** Resolve the {@link SceneAtom} carrying an {@link AtomId}, or `null` if absent —
 * the coordinate lookup `rotationAxis` needs (no positional round-trip). */
function findAtomById(scene: Scene, id: AtomId): SceneAtom | null {
  for (const f of scene.fragments) {
    for (const a of f.atoms) if (a.id === id) return a;
  }
  return null;
}

/**
 * The rotation axis two picked atoms define: `dir = normalize(Q − P)` and
 * `pivot = P` (the attaching atom stays put). Returns `null` when either atom is
 * absent from the scene or the two coincide (no direction is defined) — the single
 * degeneracy test shared by {@link rotateFragmentInScene} (→ no-op) and the UI
 * (→ Apply disabled with a reason), so the two never disagree. The axis is BY
 * DEFINITION two atoms (the approach axis of the reaction, ADR-007), which is why
 * the op stores the atoms, not this derived vector.
 */
export function rotationAxis(
  scene: Scene,
  p: AtomId,
  q: AtomId,
): { dir: Vec3; pivot: Vec3 } | null {
  const pa = findAtomById(scene, p);
  const qa = findAtomById(scene, q);
  if (!pa || !qa) return null;
  const dir: Vec3 = [qa.x - pa.x, qa.y - pa.y, qa.z - pa.z];
  if (Math.hypot(dir[0], dir[1], dir[2]) < AXIS_DEGENERATE_EPS) return null;
  return { dir, pivot: [pa.x, pa.y, pa.z] };
}

/**
 * Rigid-body rotate ONE fragment (by id) within a scene by `angleRad` about the
 * axis the two picked atoms `[P, Q]` define — `dir = normalize(Q − P)`,
 * `pivot = P`. Pure/immutable; every other fragment is untouched and the moved
 * fragment keeps its id, composition and atom order (`rotateFragment`), so atom
 * identity and count/order are invariant (domain rule #9). The scene-level mutator
 * the store's `rotateFragment` commits; the op stores `[P, Q]` and this resolves
 * them **in the target scene** at commit time (they are present by construction —
 * ADR-017). Returns the scene **unchanged (same reference)** when the axis is
 * degenerate or the fragment/atoms are absent, so a no-op never appends a log entry.
 */
export function rotateFragmentInScene(
  scene: Scene,
  fragmentId: string,
  axisAtoms: [AtomId, AtomId],
  angleRad: number,
): Scene {
  const axis = rotationAxis(scene, axisAtoms[0], axisAtoms[1]);
  if (!axis) return scene;
  if (!scene.fragments.some((f) => f.id === fragmentId)) return scene;
  return {
    ...scene,
    fragments: scene.fragments.map((f) =>
      f.id === fragmentId ? rotateFragment(f, axis.dir, angleRad, axis.pivot) : f,
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
export function parseAtomLines(lines: string[]): RawAtom[] | null {
  const atoms: RawAtom[] = [];
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
  const raw = parseAtomLines(atomLines);
  if (raw === null) return null;
  // This is a Scene boundary: raw parser output becomes SceneAtoms here, ids
  // minted from 0 for a fresh single-fragment scene, counter left at the count.
  const { atoms, nextAtomId } = stampFreshIds(raw, 0);
  const fragment: SceneFragment = {
    id: opts.id ?? makeFragmentId(),
    name: opts.name ?? "Molecule",
    atoms,
    charge: opts.charge ?? 0,
    source: opts.source ?? "editor",
    ...(opts.sourceLabel !== undefined ? { sourceLabel: opts.sourceLabel } : {}),
  };
  return { fragments: [fragment], multiplicity: opts.multiplicity ?? 1, nextAtomId };
}

/**
 * Build a single-fragment Scene from a **standard xyz string** (atom count,
 * comment, then `element x y z` rows) — the shape returned by the SMILES sidecar
 * and stored on library molecules. Returns `null` if the first line isn't a
 * positive atom count or no atoms parse. `opts` is forwarded to
 * {@link sceneFromAtomLines} (id / name / charge / multiplicity / source / label).
 */
export function sceneFromXyz(
  xyz: string,
  opts: SceneFromAtomLinesOptions = {},
): Scene | null {
  const lines = xyz.split(/\r?\n/);
  if (lines.length < 3) return null;
  const count = Number(lines[0].trim());
  if (!Number.isInteger(count) || count <= 0) return null;
  return sceneFromAtomLines(lines.slice(2), opts);
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

/**
 * Replace (or insert) the `* xyz charge mult ... *` coordinate block in an ORCA
 * input with a Scene's merged geometry: canonical rows from
 * {@link mergeToAtomLines}, header charge from {@link totalCharge}, multiplicity
 * from `scene.multiplicity`. Everything outside the block (the `!` line, `%`
 * blocks, comments) is preserved. This is the Scene → Monaco write of ADR-008 #6
 * and the inverse of {@link sceneFromOrcaInput}; it absorbed the former
 * `src/viewer/inject-xyz-into-input.ts`.
 */
export function injectSceneIntoInput(content: string, scene: Scene): string {
  const block =
    `* xyz ${totalCharge(scene)} ${scene.multiplicity}\n` +
    `${mergeToAtomLines(scene).join("\n")}\n*`;
  const lines = content.split(/\r?\n/);

  // Locate an existing block: an opening `*` followed by an `xyz`/`xyzfile`
  // keyword — the same marker sceneFromOrcaInput scans for.
  let start = -1;
  for (let i = 0; i < lines.length; i++) {
    const rest = lines[i].trim();
    if (!rest.startsWith("*")) continue;
    if (rest.slice(1).trim().toLowerCase().startsWith("xyz")) {
      start = i;
      break;
    }
  }

  if (start === -1) {
    // No block yet — append at the end, separated by a blank line.
    const trimmed = content.replace(/\s*$/, "");
    return trimmed.length > 0 ? `${trimmed}\n\n${block}\n` : `${block}\n`;
  }

  // Closing `*` is the first `*` line after the opener; else the block runs to
  // the end of the content.
  let end = lines.length - 1;
  for (let i = start + 1; i < lines.length; i++) {
    if (lines[i].trim().startsWith("*")) {
      end = i;
      break;
    }
  }

  return [...lines.slice(0, start), block, ...lines.slice(end + 1)].join("\n");
}

// ── (De)serialization for the `scene_json` snapshot ──────────────────────────

const SCENE_JSON_VERSION = 2;

/**
 * Serialize a scene to the versioned JSON snapshot written to `jobs.scene_json`.
 * v2 adds a per-atom `id` (carried automatically — `SceneAtom` has the field) and
 * the scene-level `nextAtomId` counter, so identity survives save → reopen.
 */
export function serializeScene(scene: Scene): string {
  return JSON.stringify({
    version: SCENE_JSON_VERSION,
    fragments: scene.fragments,
    multiplicity: scene.multiplicity,
    nextAtomId: scene.nextAtomId,
  });
}

function isFiniteNumber(v: unknown): v is number {
  return typeof v === "number" && Number.isFinite(v);
}

/** A raw atom (v1 shape, and the geometry half of a v2 atom): element + coords. */
function validRawAtom(v: unknown): v is RawAtom {
  if (typeof v !== "object" || v === null) return false;
  const a = v as Record<string, unknown>;
  return (
    typeof a.element === "string" &&
    isFiniteNumber(a.x) &&
    isFiniteNumber(a.y) &&
    isFiniteNumber(a.z)
  );
}

/** A v2 atom: a raw atom plus a non-negative integer `id`. */
function validSceneAtom(v: unknown): v is SceneAtom {
  if (!validRawAtom(v)) return false;
  const id = (v as { id?: unknown }).id;
  return typeof id === "number" && Number.isInteger(id) && id >= 0;
}

/** Common fragment fields (everything except the atom-array element type). */
function validFragmentMeta(f: Record<string, unknown>): boolean {
  if (typeof f.id !== "string" || typeof f.name !== "string") return false;
  if (typeof f.charge !== "number" || !Number.isFinite(f.charge)) return false;
  if (!FRAGMENT_SOURCES.includes(f.source as FragmentSource)) return false;
  if (f.sourceLabel !== undefined && typeof f.sourceLabel !== "string") return false;
  return true;
}

function validFragmentWith(
  v: unknown,
  atomValidator: (a: unknown) => boolean,
): boolean {
  if (typeof v !== "object" || v === null) return false;
  const f = v as Record<string, unknown>;
  if (!validFragmentMeta(f)) return false;
  return Array.isArray(f.atoms) && f.atoms.every(atomValidator);
}

/**
 * Parse a `scene_json` snapshot back into a Scene. Validates shape and version
 * and returns `null` on anything unexpected — never throws on user/DB data.
 *
 * **v1 is MIGRATED, never rejected.** Every scene saved before unit 1b is a v1
 * snapshot (atoms without ids, no `nextAtomId`). Returning `null` for those would
 * make `restoreScene` treat every existing multi-fragment job as a malformed
 * snapshot and silently collapse its layout to one fragment. So a valid v1 is
 * migrated in place: ids are minted 0..N-1 across all fragments in order (the same
 * scene-wide allocation a fresh scene uses), `nextAtomId = N`.
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
  if (typeof obj.multiplicity !== "number" || !Number.isFinite(obj.multiplicity)) {
    return null;
  }
  if (!Array.isArray(obj.fragments)) return null;

  if (obj.version === 1) {
    if (!obj.fragments.every((f) => validFragmentWith(f, validRawAtom))) return null;
    return migrateV1ToV2(obj.fragments as V1Fragment[], obj.multiplicity);
  }

  if (obj.version === SCENE_JSON_VERSION) {
    if (!obj.fragments.every((f) => validFragmentWith(f, validSceneAtom))) return null;
    if (typeof obj.nextAtomId !== "number" || !Number.isInteger(obj.nextAtomId)) {
      return null;
    }
    const fragments = obj.fragments as SceneFragment[];
    // ids must be unique within the scene and all below the counter (else the
    // counter could re-issue a live id — the invariant `nextAtomId` guarantees).
    const seen = new Set<number>();
    for (const f of fragments) {
      for (const a of f.atoms) {
        if (a.id >= obj.nextAtomId || seen.has(a.id)) return null;
        seen.add(a.id);
      }
    }
    return { fragments, multiplicity: obj.multiplicity, nextAtomId: obj.nextAtomId };
  }

  return null;
}

/** A v1 fragment: same meta as a SceneFragment but atoms are raw (no id). */
interface V1Fragment {
  id: string;
  name: string;
  atoms: RawAtom[];
  charge: number;
  source: FragmentSource;
  sourceLabel?: string;
}

function migrateV1ToV2(fragments: V1Fragment[], multiplicity: number): Scene {
  let next = 0;
  const migrated: SceneFragment[] = fragments.map((f) => {
    const { atoms, nextAtomId } = stampFreshIds(f.atoms, next);
    next = nextAtomId;
    return {
      id: f.id,
      name: f.name,
      atoms,
      charge: f.charge,
      source: f.source,
      ...(f.sourceLabel !== undefined ? { sourceLabel: f.sourceLabel } : {}),
    };
  });
  return { fragments: migrated, multiplicity, nextAtomId: next };
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

/**
 * Whether adopting `newContent` as a fresh geometry should **preserve** the current
 * (possibly multi-fragment) Scene instead of collapsing it to a single text-parsed
 * fragment (the `debugging/014` bugfix). `sceneFromOrcaInput` always parses a
 * coordinate block into ONE fragment named "Molecule", so an *unconditional*
 * `text-adopt` on a scene the geometry didn't actually change (the "Generate Input"
 * case — it rewrites only the `!`/`%` keyword lines over the SAME coordinates)
 * silently destroys the substrate+reagent fragment layout, breaking
 * rotate/move/clash/per-fragment charge.
 *
 * Returns `true` — **keep the Scene** — exactly when a scene exists AND the new
 * content's geometry matches it atom-for-atom (same primitive, `xyzMatchesScene`,
 * that guards the live Scene↔Monaco sync — no second comparison). A genuinely
 * different geometry (Replace input with another molecule) or an absent block
 * (parse → `null`) returns `false`: a real re-adopt.
 */
export function adoptPreservesScene(current: Scene | null, newContent: string): boolean {
  if (!current) return false;
  const parsed = sceneFromOrcaInput(newContent);
  return parsed !== null && xyzMatchesScene(current, mergeToAtomLines(parsed));
}
