import { useState } from "react";

import type { Scene } from "./types";
import type { AtomId } from "./ids";
import { describeAtom, describeAtomById } from "./selection";
import { globalIndexOfAtom } from "./scene";
import { measureSelection, type Measurement } from "./measure";
import { bondingDistance, bondOrderEstimate } from "./bond-edit";
import type { FormalChargeConsistency } from "./formal-charge";
import { fragmentColor } from "../viewer/fragment-colors";

/**
 * The atom panel on New Job (2.5.2a): names the LAST picked atom, its
 * coordinates, and its global index — and, when more than one atom is picked, a
 * compact click-ordered row of all of them with fragment-colour swatches.
 *
 * Selection state lives in `NewJobScreen`, not the scene store (the store stays
 * a pure geometry wrapper). This is a display of that state: describe-only, no
 * geometry logic. `describeAtom` (pure) does the fragment lookup.
 *
 * The global index is labelled **0-based** on purpose — and that is exactly the
 * base ORCA's `%geom Constraints` use (settled by a real run, 2.5.4a;
 * `wiki/orca/constraints.md`), so the index shown here is the index a constraint
 * line will carry. "Constrain selection" (2.5.4b) writes it straight through.
 *
 * `onConstrain` (2.5.4b): when the selection is 2/3/4 atoms, offer to freeze that
 * coordinate. It calls back into `NewJobScreen`, which owns the input text — the
 * panel never touches the text directly (one data path). Value optional (empty →
 * freeze as-is, the common TS-guess case).
 */
export function AtomInspector({
  scene,
  selection,
  onClear,
  onConstrain,
  constrainDisabledReason,
  onScan,
  scanDisabledReason,
  formalCharges,
  onSetFormalCharge,
  chargeConsistency,
}: {
  scene: Scene;
  selection: AtomId[];
  onClear: () => void;
  onConstrain?: (value?: number) => void;
  /** When set, "Constrain selection" is disabled with this as the tooltip — used
   * when the constraint block is unrecognised and must not be rewritten (2.5.5). */
  constrainDisabledReason?: string | null;
  /** "Scan this coordinate" (Stage A2): build a relaxed-scan coordinate from the
   * 2/3/4-atom selection (kind from count) with an editable default range. Mirrors
   * `onConstrain` — one data path, `NewJobScreen` owns the input text. */
  onScan?: () => void;
  /** When set, "Scan this coordinate" is disabled with this tooltip — the scan
   * block is unrecognised and must not be rewritten. */
  scanDisabledReason?: string | null;
  /** Per-atom formal charges (display-only bookkeeping, keyed by AtomId). ORCA takes
   * only the TOTAL charge — these annotations must SUM to it (see `chargeConsistency`).
   * Not part of the Scene geometry; owned by `NewJobScreen` like `hiddenBonds`. */
  formalCharges?: ReadonlyMap<AtomId, number>;
  /** Set the last-picked atom's formal charge. When absent, the control is hidden. */
  onSetFormalCharge?: (id: AtomId, value: number) => void;
  /** Σ-formal-vs-total check (from `formalChargeConsistency`); shown as ✓ / a
   * bookkeeping mismatch. Absent when no formal charge is set. */
  chargeConsistency?: FormalChargeConsistency | null;
}) {
  const [constrainValue, setConstrainValue] = useState("");
  if (selection.length === 0) return null;
  const lastId = selection[selection.length - 1];
  const last = describeAtomById(scene, lastId);
  if (!last) return null; // id left the scene — filterSelection normally prevents this
  // The global index the last-picked atom occupies NOW — resolved from its id, so
  // the labelled number is the current 0-based global/ORCA index (never a stale one).
  const lastGlobal = globalIndexOfAtom(scene, lastId);

  // The measurement read off the pick list (2.5.2b), positionally: 2 → distance,
  // 3 → angle (middle pick = vertex), 4 → dihedral. `none` for 1 atom or a
  // degenerate pick — the panel then shows only the atom description.
  const measurement = measureSelection(scene, selection);
  const readout = describeMeasurement(scene, measurement);
  // Bond-order ANALYZER (geometric-editor completion): for a 2-atom distance that
  // is within bonding range, the nearest single/double/triple order — an honest
  // GEOMETRIC estimate (never the computed Mayer order, which the editor lacks).
  const bondOrderLabel = analyzeBondOrder(scene, measurement);

  const lastCharge = formalCharges?.get(lastId) ?? 0;

  // Fragment 0 keeps CPK colours (no palette entry) — show a hollow swatch, the
  // same convention FragmentList uses.
  const swatch = (fragmentIndex: number) => {
    const color = fragmentColor(fragmentIndex);
    return {
      background: color ?? "transparent",
      borderColor: color ?? "var(--muted-2)",
    };
  };

  return (
    <div className="atom-inspector">
      <div className="atom-inspector-head">
        <span className="fragment-swatch" style={swatch(last.fragmentIndex)} />
        <span className="atom-inspector-title">
          atom {last.localIndex} of {last.fragmentName} ({last.element})
        </span>
        {lastCharge !== 0 ? (
          <span
            className="atom-inspector-charge-badge"
            title="Formal charge (bookkeeping) — ORCA uses the total charge, not this"
          >
            {signedCharge(lastCharge)}
          </span>
        ) : null}
        <button
          className="btn btn-sm"
          onClick={onClear}
          style={{ marginLeft: "auto" }}
          title="Clear selection (Esc)"
        >
          Clear
        </button>
      </div>
      {readout ? (
        <div className="atom-inspector-readout">
          <span className="atom-inspector-measure mono">
            {readout.chain}
            {"  "}
            {readout.value}
          </span>
          {readout.interFragment ? (
            <span
              className="atom-inspector-badge"
              title="The two atoms are in different fragments — this distance is a candidate reaction coordinate."
            >
              inter-fragment
            </span>
          ) : null}
        </div>
      ) : null}
      {bondOrderLabel ? (
        <div
          className="atom-inspector-bondorder muted"
          title="A geometric estimate from bond length + covalent radii — NOT a computed bond order. A computed (Mayer) order will appear in results, once available."
        >
          {bondOrderLabel}
        </div>
      ) : null}
      {onConstrain && selection.length >= 2 && selection.length <= 4 ? (
        <div className="atom-inspector-constrain">
          <button
            className="btn btn-sm"
            disabled={!!constrainDisabledReason}
            onClick={() => {
              const v = constrainValue.trim();
              onConstrain(v === "" ? undefined : Number(v));
              setConstrainValue("");
            }}
            title={
              constrainDisabledReason ??
              "Add a %geom constraint on this selection (frozen during the optimization)"
            }
          >
            Constrain {constrainKindLabel(selection.length)}
          </button>
          <input
            className="input atom-inspector-constrain-value"
            type="number"
            step="any"
            placeholder="freeze as-is"
            value={constrainValue}
            onChange={(e) => setConstrainValue(e.target.value)}
            disabled={!!constrainDisabledReason}
            aria-label="constraint target value (optional)"
          />
          {constrainDisabledReason ? (
            <span className="muted atom-inspector-constrain-note">
              {constrainDisabledReason}
            </span>
          ) : null}
        </div>
      ) : null}
      {onScan && selection.length >= 2 && selection.length <= 4 ? (
        <div className="atom-inspector-scan">
          <button
            className="btn btn-sm"
            disabled={!!scanDisabledReason}
            onClick={onScan}
            title={
              scanDisabledReason ??
              "Add a %geom relaxed scan over this coordinate — start defaults to the current value; edit the range in the Scan panel"
            }
          >
            Scan this {constrainKindLabel(selection.length)}
          </button>
          {scanDisabledReason ? (
            <span className="muted atom-inspector-constrain-note">
              {scanDisabledReason}
            </span>
          ) : null}
        </div>
      ) : null}
      <div className="atom-inspector-coords mono">
        x {last.x.toFixed(4)} · y {last.y.toFixed(4)} · z {last.z.toFixed(4)}
      </div>
      <div className="atom-inspector-index muted">
        local index {last.localIndex} · global index {lastGlobal} (both 0-based)
      </div>
      {onSetFormalCharge ? (
        <div className="atom-inspector-formal">
          <span className="atom-inspector-formal-label">Formal charge</span>
          <button
            className="btn btn-sm"
            onClick={() => onSetFormalCharge(lastId, clampCharge(lastCharge - 1))}
            disabled={lastCharge <= FORMAL_CHARGE_MIN}
            aria-label="decrease formal charge"
            title="Decrease this atom's formal charge (bookkeeping)"
          >
            −
          </button>
          <span className="atom-inspector-formal-value mono">{signedCharge(lastCharge)}</span>
          <button
            className="btn btn-sm"
            onClick={() => onSetFormalCharge(lastId, clampCharge(lastCharge + 1))}
            disabled={lastCharge >= FORMAL_CHARGE_MAX}
            aria-label="increase formal charge"
            title="Increase this atom's formal charge (bookkeeping)"
          >
            +
          </button>
          <span className="muted atom-inspector-formal-note">
            bookkeeping — ORCA uses the total charge, not per-atom charges
          </span>
        </div>
      ) : null}
      {chargeConsistency ? (
        <div
          className={
            "atom-inspector-formal-sum " +
            (chargeConsistency.matches ? "ok" : "warn")
          }
          title="ORCA is given only the total charge; per-atom formal charges are bookkeeping and must sum to it."
        >
          {chargeConsistency.matches
            ? `Σ formal ${signedCharge(chargeConsistency.sum)} = total ${signedCharge(chargeConsistency.total)} ✓`
            : `Σ formal ${signedCharge(chargeConsistency.sum)} ≠ total ${signedCharge(chargeConsistency.total)} — bookkeeping only (ORCA still uses total ${signedCharge(chargeConsistency.total)})`}
        </div>
      ) : null}
      {selection.length > 1 ? (
        <div className="atom-inspector-list">
          {selection.map((id) => {
            const d = describeAtomById(scene, id);
            if (!d) return null;
            const gi = globalIndexOfAtom(scene, id);
            const q = formalCharges?.get(id) ?? 0;
            return (
              <span key={id} className="atom-chip" title={`${d.fragmentName}`}>
                <span className="fragment-swatch" style={swatch(d.fragmentIndex)} />
                {d.element}
                {q !== 0 ? (
                  <span className="atom-chip-charge" title="formal charge (bookkeeping)">
                    {signedCharge(q)}
                  </span>
                ) : null}
                <span className="muted" title="global index (0-based)">
                  {" "}global #{gi}
                </span>
              </span>
            );
          })}
        </div>
      ) : null}
    </div>
  );
}

/** The coordinate a selection of N atoms constrains — same length→kind rule as
 * `measureSelection` / `constraintFromSelection`. */
function constrainKindLabel(len: number): string {
  return len === 2 ? "distance" : len === 3 ? "angle" : "dihedral";
}

/** Formal-charge control bounds — bookkeeping, so a small sane range. */
const FORMAL_CHARGE_MIN = -4;
const FORMAL_CHARGE_MAX = 4;
function clampCharge(v: number): number {
  return Math.max(FORMAL_CHARGE_MIN, Math.min(FORMAL_CHARGE_MAX, v));
}

/** A signed charge label, e.g. `+1`, `−1`, `0` (a real minus sign for display). */
function signedCharge(v: number): string {
  if (v === 0) return "0";
  return v > 0 ? `+${v}` : `−${Math.abs(v)}`;
}

const ORDER_WORD: Record<number, string> = { 1: "single", 2: "double", 3: "triple" };

/**
 * The honest bond-order analyzer line for a 2-atom distance selection, or `null`.
 * Only for atoms within bonding range (≤ single-bond sum × 1.3) — a through-space
 * contact (a forming/breaking ~2.2 Å distance, or two far atoms) is NOT a bond, so
 * no order is claimed. The label is always tagged "(geometric estimate)" — it is
 * derived from length + covalent radii, never a computed order.
 */
function analyzeBondOrder(scene: Scene, m: Measurement): string | null {
  if (m.kind !== "distance") return null;
  const [gi, gj] = m.atoms;
  const ea = describeAtom(scene, gi)?.element;
  const eb = describeAtom(scene, gj)?.element;
  if (!ea || !eb) return null;
  try {
    // Beyond ~1.3× the single-bond sum it isn't a bond — don't label an order.
    if (m.value > bondingDistance(ea, eb, 1) * 1.3) return null;
    const { order } = bondOrderEstimate(ea, eb, m.value);
    return `≈ ${ORDER_WORD[order]} · ${m.value.toFixed(3)} Å (geometric estimate)`;
  } catch {
    return null; // an element with no radius → no estimate, never a crash
  }
}

/** A rendered measurement line: the atom chain in click order, the value, and
 * whether it crosses fragments. Null when there is no measurement (0/1 atoms or
 * a degenerate pick). Distance uses `···` (a through-space contact), angle and
 * dihedral use `–` (a bonded chain), matching how a chemist writes them. */
function describeMeasurement(
  scene: Scene,
  m: Measurement,
): { chain: string; value: string; interFragment: boolean } | null {
  if (m.kind === "none") return null;
  const symbols = m.atoms.map((gi) => describeAtom(scene, gi)?.element ?? "?");
  const sep = m.kind === "distance" ? "···" : "–";
  const chain = symbols.join(sep);
  if (m.kind === "distance") {
    return {
      chain,
      value: `${m.value.toFixed(3)} ${m.unit}`,
      interFragment: !m.sameFragment,
    };
  }
  const label = m.kind === "dihedral" ? "dihedral " : "";
  return {
    chain,
    value: `${label}${m.value.toFixed(1)}${m.unit}`,
    interFragment: !m.sameFragment,
  };
}
