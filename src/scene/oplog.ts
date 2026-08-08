/**
 * The operation log — pure types and pointer semantics (Phase 4.2 Stage 2, unit
 * 2a; ADR-017). Editor state becomes a **fold over a log of typed operations**
 * (ADR-010): undo/redo are a moving pointer, and a new editor capability adds an
 * *operation*, not a new piece of state. This module is the log itself —
 * **pure**, node-testable, and free of any store / viewer / Monaco / fetch
 * import (the store lands in unit 2b, the renderer in 2c). It depends only on the
 * Scene codec (`scene.ts`) and the id/type layer.
 *
 * ─────────────────────────────────────────────────────────────────────────────
 * WHY EACH ENTRY MATERIALIZES ITS RESULTANT SNAPSHOT — the one rule that must
 * not be "optimized" away (ADR-017 decision 1; carried here verbatim on purpose)
 * ─────────────────────────────────────────────────────────────────────────────
 * A `LogEntry` stores the **Scene that resulted** from its `Op`, not a recipe for
 * recomputing it. The fold is therefore trivial — `current(log)` is just the
 * snapshot at the pointer — and that triviality is the point, not a shortcut.
 *
 * The dangerous "optimization" a future reader will be tempted by: drop the
 * snapshots and keep only the ops, then replay them to reconstruct any state
 * ("it's smaller, it's DRY"). DO NOT. The geometry ops (`replace-fragment-atoms`
 * via set-internal / xtb / conformer, `replace-all-atoms`) are executed by **ASE
 * in the Python sidecar**. A log that stores only ops and recomputes state makes
 * the reconstructed history a **function of the installed ASE version** — a
 * dependency bump would silently rewrite geometries that were computed months
 * ago. The history of a scientific instrument must not change retroactively. So:
 *
 *   • The snapshot in an entry is the **source of truth**, not a cache.
 *   • The `Op` descriptor is **provenance** (a lab-journal line), not a recipe.
 *   • `current` reads a stored snapshot; it never re-invokes a geometry backend.
 *
 * Phase 4.5 (ReactionPath) will reuse these `Op` types, but it **replays the
 * materialized snapshots**, it does not recompute them. See ADR-017.
 */

import type { AtomId } from "./ids";
import type { FragmentSource, Scene } from "./types";
import { deserializeScene, globalIndexOfAtom, serializeScene } from "./scene";

// ── The operation vocabulary ─────────────────────────────────────────────────
// One variant per existing Scene mutator (the checklist lives in ADR-017 so unit
// 2b does not discover a hole). Every geometry-carrying variant references atoms
// by their stable `AtomId` (not a positional index) — the log is AtomId-native
// ahead of the 2c2 pipeline move, so a materialized op stays legible after a
// fragment is removed. Params carry human quantities (kind / target / unit) so
// `describe` is a cheap, honest lab-journal line already at the type layer.

/** How a `distance` / `angle` / `dihedral` geometry edit is measured. */
export type MeasurementKind = "distance" | "angle" | "dihedral";
/** Canonical readout unit — Å for a distance, ° for an angle/dihedral. */
export type MeasurementUnit = "Å" | "°";

/**
 * Provenance of a single-fragment geometry replacement — a discriminated
 * sub-union because the three producers are genuinely different acts, and
 * `describe` reads them apart:
 *  - `set-internal` — the geometry editor set an internal coordinate to a target
 *    (the ASE `set_distance/angle/dihedral` path); carries the picked `atoms`
 *    (AtomIds), the `kind`, and the human `target`+`unit`;
 *  - `xtb` — an xtb pre-optimization of that one fragment in isolation;
 *  - `conformer` — a GOAT conformer was substituted back in place.
 */
export type FragmentGeometryVia =
  | {
      via: "set-internal";
      kind: MeasurementKind;
      atoms: AtomId[];
      target: number;
      unit: MeasurementUnit;
    }
  | { via: "xtb" }
  | {
      via: "conformer";
      conformerIndex: number;
      deltaEKcal: number | null;
      /**
       * Which level the carried geometry is at (D3). Absent/`"xTB"` = the raw GOAT
       * frame (2.5.1b); `"DFT"` = the re-optimised child geometry, with `method`.
       * The user must know which level they carried forward — so the log records it.
       */
      level?: "xTB" | "DFT";
      method?: string;
    };

/** Provenance of a whole-scene geometry replacement (today: xtb pre-opt). */
export type WholeSceneVia = { via: "xtb" };

/**
 * Where a whole-scene snapshot entered the log — the seed of a geometry lineage:
 *  - `new-iteration` — the "New iteration" action restored a saved job's scene;
 *  - `text-adopt` — a coordinate block appeared in the input text (a template /
 *    a generated / a pasted input) and the Scene adopted it;
 *  - `library` — a molecule was loaded from the library.
 * All three replace the whole geometry, so all three are a `restore-snapshot`.
 */
export type SnapshotSource = "new-iteration" | "text-adopt" | "library";

/**
 * A typed editor operation. The tagged union mirrors the Scene mutators of
 * `scene.ts` (plus the store-level act `restore-snapshot`, and the legacy
 * `collapse-from-text` no post-2d path emits — see its member below).
 * ADR-017 carries the mutator↔Op correspondence table.
 */
export type Op =
  | {
      type: "add-fragment";
      fragmentId: string;
      name: string;
      source: FragmentSource;
      sourceLabel: string | null;
      atomCount: number;
    }
  | { type: "remove-fragment"; fragmentId: string; name: string }
  | { type: "rename-fragment"; fragmentId: string; from: string; to: string }
  | { type: "set-fragment-charge"; fragmentId: string; name: string; charge: number }
  | { type: "set-multiplicity"; multiplicity: number }
  | {
      type: "translate-fragment";
      fragmentId: string;
      name: string;
      delta: [number, number, number];
    }
  | {
      type: "rotate-fragment";
      fragmentId: string;
      name: string;
      /** The two picked atoms that define the approach axis: `[P, Q]` with P the
       * pivot (on the rotating fragment) and Q the direction. Stored as atoms, not
       * a raw vector, because the axis IS two atoms by definition (ADR-007) and the
       * journal "about O→C" serves the teaching mission — resolved at apply time in
       * the op's own snapshot, where both are present by construction (ADR-017). */
      axisAtoms: [AtomId, AtomId];
      /** Signed rotation angle in radians (canonical unit; the UI collects degrees). */
      angleRad: number;
    }
  | {
      type: "replace-fragment-atoms";
      fragmentId: string;
      name: string;
      edit: FragmentGeometryVia;
    }
  | { type: "replace-all-atoms"; edit: WholeSceneVia }
  // LEGACY — produced pre-2d; kept for deserialization, never emitted. Before 2d
  // a manual text edit of the coordinate block collapsed the multi-fragment scene
  // to one fragment and logged this op (the block was hand-editable). Unit 2d made
  // the xyz block a **read-only projection of the Scene** (ADR-010 authority split:
  // text owns chemistry, Scene owns geometry), so no path collapses-from-text any
  // more — geometry changes go through the two doors (Import xyz as fragment /
  // Replace input). The type + `describe` + the deserialize case remain so old
  // jobs whose `scene_log_json` carries this op still open (persist not broken).
  | { type: "collapse-from-text"; fragmentCount: number }
  | {
      type: "restore-snapshot";
      source: SnapshotSource;
      fragmentCount: number;
      atomCount: number;
    };

/** Every `Op["type"]` tag — the deserialize discriminant allow-list. */
const OP_TYPES: readonly Op["type"][] = [
  "add-fragment",
  "remove-fragment",
  "rename-fragment",
  "set-fragment-charge",
  "set-multiplicity",
  "translate-fragment",
  "rotate-fragment",
  "replace-fragment-atoms",
  "replace-all-atoms",
  "collapse-from-text",
  "restore-snapshot",
];

// ── describe(): one human line per op (the lab-journal payoff) ────────────────

/** Compact number: integers bare, else 3 decimals (`30` not `30.000`). */
function num(x: number): string {
  return Number.isInteger(x) ? String(x) : x.toFixed(3);
}

/** Radians → degrees for display, rounded to 3 dp so float noise off a clean
 * degree input (30° → rad → deg = 29.9999…) reads back as `30`, not `30.000`. */
function deg(angleRad: number): number {
  return Math.round((angleRad * 180) / Math.PI * 1000) / 1000;
}

/** Signed formal charge, e.g. `+1`, `-1`, `0`. */
function signed(x: number): string {
  return x > 0 ? `+${x}` : String(x);
}

/** `30°` for an angle/dihedral, `1.5 Å` for a distance (unit spacing convention). */
function formatTarget(target: number, unit: MeasurementUnit): string {
  return unit === "°" ? `${num(target)}°` : `${num(target)} Å`;
}

/** Pluralize a noun by count. */
function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

function describeReplaceFragment(op: Extract<Op, { type: "replace-fragment-atoms" }>): string {
  const e = op.edit;
  switch (e.via) {
    case "set-internal":
      // "Set dihedral 4-7-12-15 to 30°"
      return `Set ${e.kind} ${e.atoms.join("-")} to ${formatTarget(e.target, e.unit)}`;
    case "xtb":
      return `Pre-optimize ${op.name} (xtb)`;
    case "conformer": {
      const kind =
        e.level === "DFT"
          ? `DFT-optimised conformer #${e.conformerIndex}` +
            (e.method ? ` (${e.method})` : "")
          : `conformer #${e.conformerIndex}` +
            (e.deltaEKcal === null ? "" : ` (ΔE ${num(e.deltaEKcal)} kcal/mol)`);
      return `Replace ${op.name} with ${kind}`;
    }
  }
}

/**
 * A single human-readable line describing an operation — the lab-journal entry.
 * Deliberately cheap and total: every `Op` variant is covered, so the log reads
 * as a geometry provenance record from the moment the types exist.
 */
export function describe(op: Op): string {
  switch (op.type) {
    case "add-fragment":
      return `Add fragment ${op.name}${op.sourceLabel ? ` (${op.sourceLabel})` : ""}`;
    case "remove-fragment":
      return `Remove fragment ${op.name}`;
    case "rename-fragment":
      return `Rename fragment ${op.from} → ${op.to}`;
    case "set-fragment-charge":
      return `Set charge of ${op.name} to ${signed(op.charge)}`;
    case "set-multiplicity":
      return `Set multiplicity to ${op.multiplicity}`;
    case "translate-fragment": {
      const [dx, dy, dz] = op.delta;
      return `Move ${op.name} by (${num(dx)}, ${num(dy)}, ${num(dz)}) Å`;
    }
    case "rotate-fragment": {
      const [p, q] = op.axisAtoms;
      return `Rotate ${op.name} ${num(deg(op.angleRad))}° about ${p}→${q}`;
    }
    case "replace-fragment-atoms":
      return describeReplaceFragment(op);
    case "replace-all-atoms":
      return "Pre-optimize all fragments (xtb)";
    case "collapse-from-text":
      return `Edit coordinates as text (${plural(op.fragmentCount, "fragment")} → 1)`;
    case "restore-snapshot": {
      const lead =
        op.source === "new-iteration"
          ? "Restore snapshot (New iteration)"
          : op.source === "text-adopt"
            ? "Adopt geometry from input text"
            : "Load from library";
      return `${lead} — ${plural(op.fragmentCount, "fragment")}, ${plural(op.atomCount, "atom")}`;
    }
  }
}

/**
 * A **scene-aware** rendering of an op — the history panel's line (unit 2c2,
 * Variant A). {@link describe} is the pure provenance record and stays
 * AtomId-native (ADR-017: the op is a lab-journal line, not a recipe); this is a
 * *presentation* over it that shows the picked atoms by the **global index they
 * occupied in the passed scene**, so the journal reads in the same 0-based space
 * the rest of the UI is labelled with. Only `set-internal` names atoms; every
 * other variant delegates to {@link describe} verbatim.
 *
 * Call it with the entry's OWN snapshot (`entry.scene`). A `set-internal` op
 * preserves atom count + order, so its atoms are always present in that snapshot
 * and the resolution always succeeds — a `[removed]` case does not arise (an atom
 * cannot be both edited-in and absent-from the very scene the edit produced).
 * Should an id somehow not resolve, it falls back to the AtomId so the line is
 * never blank.
 */
export function describeInScene(op: Op, scene: Scene): string {
  // Resolve an AtomId to the global index it occupies in `scene`, falling back to
  // the raw id so the line is never blank (the atom is present by construction —
  // the op's own snapshot always carries the atoms it names).
  const gi = (id: AtomId): string => {
    const g = globalIndexOfAtom(scene, id);
    return g === null ? String(id) : String(g);
  };
  if (op.type === "replace-fragment-atoms" && op.edit.via === "set-internal") {
    const e = op.edit;
    const chain = e.atoms.map(gi).join("-");
    return `Set ${e.kind} ${chain} to ${formatTarget(e.target, e.unit)}`;
  }
  if (op.type === "rotate-fragment") {
    const [p, q] = op.axisAtoms;
    return `Rotate ${op.name} ${num(deg(op.angleRad))}° about ${gi(p)}→${gi(q)}`;
  }
  return describe(op);
}

// ── The log: a pointer into a list of {op, resultant snapshot} entries ────────

/**
 * One step in the log: the operation and the **Scene it produced** (the
 * materialized snapshot — see the file header). `scene` is deep-frozen when the
 * entry is created, so an entry is immutable at runtime.
 */
export interface LogEntry {
  readonly op: Op;
  readonly scene: Scene;
}

/**
 * A log with a pointer at the current entry. `pointer` semantics:
 *  - `-1` — before any op / the empty scene. `current` returns `null`. This is
 *    the state of a fresh `emptyLog()` and of a log that has been fully undone.
 *  - `0 ≤ pointer < entries.length` — the pointer indexes the current snapshot.
 *
 * So the full invariant is **`-1 ≤ pointer < entries.length`, with `pointer ===
 * -1` exactly when the log is empty OR everything has been undone**. (The
 * architect's `0 ≤ pointer < len` is the populated, not-fully-undone case; the
 * `-1` sentinel extends it so undo can reach the empty scene — undoing the first
 * op must return a blank canvas, `current → null`, which the 2b store renders as
 * `scene: null`. `logInvariant` is the machine-checkable form.)
 */
export interface SceneLog {
  readonly entries: readonly LogEntry[];
  readonly pointer: number;
}

/** The empty log — no ops, pointer before the first (empty scene). */
export function emptyLog(): SceneLog {
  return { entries: [], pointer: -1 };
}

/** The pointer invariant, machine-checkable (see {@link SceneLog}). */
export function logInvariant(log: SceneLog): boolean {
  const n = log.entries.length;
  if (n === 0) return log.pointer === -1;
  return log.pointer >= -1 && log.pointer < n;
}

/**
 * Append an operation and its resultant snapshot, **truncating any redo tail**
 * (everything after the pointer). This is the load-bearing rule: after an undo,
 * a new op discards the futures that were undone — otherwise the log would hold
 * two divergent futures and `redo` would resurrect a state the user did not
 * expect (negative control (a) proves the truncation bites). The new snapshot is
 * deep-frozen; the entry becomes the new current.
 */
export function append(log: SceneLog, op: Op, scene: Scene): SceneLog {
  const kept = log.entries.slice(0, log.pointer + 1); // drop the redo tail
  const entry = freezeEntry({ op, scene: deepFreezeScene(scene) });
  const entries = [...kept, entry];
  return { entries, pointer: entries.length - 1 };
}

/** Move the pointer back one step (no-op — same reference — at the start). */
export function undo(log: SceneLog): SceneLog {
  return log.pointer >= 0 ? { entries: log.entries, pointer: log.pointer - 1 } : log;
}

/** Move the pointer forward one step (no-op — same reference — at the tip). */
export function redo(log: SceneLog): SceneLog {
  return log.pointer < log.entries.length - 1
    ? { entries: log.entries, pointer: log.pointer + 1 }
    : log;
}

/**
 * Set the pointer directly — the history panel's "jump to this step". A no-op
 * (same reference) if `pointer` is out of `[-1, len)` or already current. This
 * is the same mechanism as {@link undo}/{@link redo}, not a new one; it just
 * moves more than one step.
 */
export function goto(log: SceneLog, pointer: number): SceneLog {
  if (pointer < -1 || pointer >= log.entries.length || pointer === log.pointer) {
    return log;
  }
  return { entries: log.entries, pointer };
}

/** Whether {@link undo} would change the log. */
export function canUndo(log: SceneLog): boolean {
  return log.pointer >= 0;
}

/** Whether {@link redo} would change the log. */
export function canRedo(log: SceneLog): boolean {
  return log.pointer < log.entries.length - 1;
}

/**
 * The current scene — the materialized snapshot at the pointer, or `null` when
 * the pointer is before the first op (empty / fully undone). Never recomputes:
 * it reads the stored snapshot (file header, ADR-017 decision 1).
 */
export function current(log: SceneLog): Scene | null {
  return log.pointer < 0 ? null : log.entries[log.pointer].scene;
}

// ── Serialization: log format v1, scenes v2 (versioned independently) ─────────

/**
 * The log-format version, versioned **separately** from the Scene JSON. A log
 * entry's scene is embedded as its `serializeScene` string (Scene format v2), so
 * the two version numbers evolve independently and the Scene codec is reused
 * verbatim — including its v1→v2 migration on read. (The nested-JSON-as-string
 * is a deliberate reuse of the one Scene codec, not a second scene schema.)
 */
const LOG_VERSION = 1;

/** Serialize a log to the versioned JSON persisted as `scene_log_json` (unit 2b). */
export function serializeLog(log: SceneLog): string {
  return JSON.stringify({
    version: LOG_VERSION,
    pointer: log.pointer,
    entries: log.entries.map((e) => ({ op: e.op, scene: serializeScene(e.scene) })),
  });
}

/**
 * Parse a `scene_log_json` string back into a log. Validates shape, version, the
 * pointer invariant, and every embedded scene (via `deserializeScene`, which
 * also migrates a v1 scene); returns `null` on anything unexpected — never
 * throws on user/DB data (same contract as `deserializeScene`).
 */
export function deserializeLog(json: string): SceneLog | null {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    return null;
  }
  if (typeof parsed !== "object" || parsed === null) return null;
  const obj = parsed as Record<string, unknown>;
  if (obj.version !== LOG_VERSION) return null;
  if (typeof obj.pointer !== "number" || !Number.isInteger(obj.pointer)) return null;
  if (!Array.isArray(obj.entries)) return null;

  const entries: LogEntry[] = [];
  for (const raw of obj.entries) {
    if (typeof raw !== "object" || raw === null) return null;
    const r = raw as Record<string, unknown>;
    if (!isOp(r.op)) return null;
    if (typeof r.scene !== "string") return null;
    const scene = deserializeScene(r.scene);
    if (scene === null) return null;
    entries.push(freezeEntry({ op: r.op, scene: deepFreezeScene(scene) }));
  }

  // Pointer invariant: -1 for an empty log, else -1 ≤ pointer < len.
  const n = entries.length;
  const okPointer = n === 0 ? obj.pointer === -1 : obj.pointer >= -1 && obj.pointer < n;
  if (!okPointer) return null;

  return { entries, pointer: obj.pointer };
}

// ── Runtime validation + immutability ─────────────────────────────────────────

/**
 * Structural validation of an `Op` on the deserialize path. Checks the
 * discriminant against the allow-list and the fields `describe` reads for that
 * variant, so a round-tripped op can never make `describe` emit `undefined`. The
 * data is our own serialization, so this guards against corruption, not against
 * an adversary.
 */
function isOp(v: unknown): v is Op {
  if (typeof v !== "object" || v === null) return false;
  const o = v as Record<string, unknown>;
  if (typeof o.type !== "string" || !OP_TYPES.includes(o.type as Op["type"])) return false;
  const str = (k: string): boolean => typeof o[k] === "string";
  const int = (k: string): boolean => typeof o[k] === "number" && Number.isInteger(o[k] as number);
  switch (o.type as Op["type"]) {
    case "add-fragment":
      return (
        str("fragmentId") &&
        str("name") &&
        str("source") &&
        (o.sourceLabel === null || typeof o.sourceLabel === "string") &&
        int("atomCount")
      );
    case "remove-fragment":
      return str("fragmentId") && str("name");
    case "rename-fragment":
      return str("fragmentId") && str("from") && str("to");
    case "set-fragment-charge":
      return str("fragmentId") && str("name") && typeof o.charge === "number";
    case "set-multiplicity":
      return typeof o.multiplicity === "number";
    case "translate-fragment":
      return (
        str("fragmentId") &&
        str("name") &&
        Array.isArray(o.delta) &&
        o.delta.length === 3 &&
        o.delta.every((n) => typeof n === "number")
      );
    case "rotate-fragment":
      return (
        str("fragmentId") &&
        str("name") &&
        Array.isArray(o.axisAtoms) &&
        o.axisAtoms.length === 2 &&
        o.axisAtoms.every((n) => typeof n === "number") &&
        typeof o.angleRad === "number"
      );
    case "replace-fragment-atoms":
      return str("fragmentId") && str("name") && isFragmentVia(o.edit);
    case "replace-all-atoms":
      return isWholeSceneVia(o.edit);
    case "collapse-from-text":
      return int("fragmentCount");
    case "restore-snapshot":
      return (
        (o.source === "new-iteration" ||
          o.source === "text-adopt" ||
          o.source === "library") &&
        int("fragmentCount") &&
        int("atomCount")
      );
  }
}

function isFragmentVia(v: unknown): v is FragmentGeometryVia {
  if (typeof v !== "object" || v === null) return false;
  const e = v as Record<string, unknown>;
  switch (e.via) {
    case "set-internal":
      return (
        (e.kind === "distance" || e.kind === "angle" || e.kind === "dihedral") &&
        Array.isArray(e.atoms) &&
        e.atoms.every((a) => typeof a === "number") &&
        typeof e.target === "number" &&
        (e.unit === "Å" || e.unit === "°")
      );
    case "xtb":
      return true;
    case "conformer":
      return (
        typeof e.conformerIndex === "number" &&
        Number.isInteger(e.conformerIndex) &&
        (e.deltaEKcal === null || typeof e.deltaEKcal === "number")
      );
    default:
      return false;
  }
}

function isWholeSceneVia(v: unknown): v is WholeSceneVia {
  return typeof v === "object" && v !== null && (v as Record<string, unknown>).via === "xtb";
}

/**
 * Deep-freeze a scene so an entry's materialized snapshot is immutable at
 * runtime (ADR-017 decision 1 — the snapshot is the source of truth, so nothing
 * downstream may mutate it in place). `Object.freeze` is a **real runtime
 * guarantee in every environment**, not a dev-only construct: the freeze holds
 * in production, and because every module here is an ES module (always strict
 * mode) a write to a frozen field *throws* rather than silently no-ops — so
 * negative control (b) bites in prod too, not just under a dev flag. Idempotent
 * (skips an already-frozen scene) so re-freezing a shared reference is cheap.
 */
function deepFreezeScene(scene: Scene): Scene {
  if (Object.isFrozen(scene)) return scene;
  for (const f of scene.fragments) {
    for (const a of f.atoms) Object.freeze(a);
    Object.freeze(f.atoms);
    Object.freeze(f);
  }
  Object.freeze(scene.fragments);
  return Object.freeze(scene);
}

/** Freeze the entry wrapper (its `op` and the `{op, scene}` object). */
function freezeEntry(entry: LogEntry): LogEntry {
  Object.freeze(entry.op);
  return Object.freeze(entry);
}
