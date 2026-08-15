import type { ReactNode } from "react";

import {
  BASIS_GROUPS,
  COMPOSITE_METHODS,
  DISPERSION,
  FUNCTIONAL_GROUPS,
  RI_METHODS,
  WAVEFUNCTION_METHODS,
  XTB_METHODS,
} from "./orca-options";
import {
  functionalHasBuiltInDispersion,
  type MethodFamily,
  type MethodSlice,
} from "./build-input";
import { OptionSelect } from "./OptionSelect";

/**
 * The method sub-UI, extracted from `InputBuilderForm` so it is reusable (Phase 4.5).
 *
 * It renders the **method-family selector** (composite / dft / wavefunction / xtb) and the
 * per-family controls over a {@link MethodSlice} + an `onChange` patch — the exact markup the
 * New Job form used, so consuming it there is a no-behaviour-change refactor (the generated
 * input is byte-identical; guarded by the InputBuilderForm test).
 *
 * The one addition is the **"Inherit from source"** option (`inherit` prop), used by the
 * OptTS-derived spawn sites (scan / NEB / connectivity). Selecting it means **no override** —
 * the caller passes NOTHING to `buildOptTSInput`, which then re-uses the source's method
 * verbatim (the byte-identical inherit path of `src/scene/optts.ts`). Inherit is deliberately
 * NOT a reconstructed "equals-source" slice — that would be a back-door reverse-parse of the
 * source `!` line and could drift from the real inherit path. Inherit = pass nothing. New Job
 * omits the `inherit` prop entirely, so it never sees the option and behaves exactly as before.
 */
export interface MethodPickerProps {
  /** The current method slice (the concrete override under construction). */
  value: MethodSlice;
  /** Patch one or more method-slice fields. */
  onChange: (patch: Partial<MethodSlice>) => void;
  /**
   * Rendered in the same row as the family radios, before them. New Job passes its Job-type
   * control here so the extraction preserves that row's layout; the derived sites omit it.
   */
  leading?: ReactNode;
  /**
   * When present, prepends an "Inherit from source" radio (the derived-spawn default). `active`
   * = it is selected (no override; per-family controls are hidden and `note` shown instead).
   * Absent → New Job behaviour: no inherit option, the method is always a concrete family.
   */
  inherit?: {
    active: boolean;
    onChange: (active: boolean) => void;
    /** Shown in place of the controls while inherit is active (e.g. the XTB-source note). */
    note?: ReactNode;
  };
}

const FAMILIES: { value: MethodFamily; label: string }[] = [
  { value: "composite", label: "Composite (3c)" },
  { value: "dft", label: "Functional + Basis" },
  { value: "wavefunction", label: "Wave-function (correlated)" },
  { value: "xtb", label: "GFN2-xTB (semi-emp.)" },
];

export function MethodPicker({ value, onChange, leading, inherit }: MethodPickerProps) {
  const family = value.methodFamily;
  const dispersionBuiltIn = functionalHasBuiltInDispersion(value.functional);
  const inheriting = inherit?.active ?? false;

  return (
    <>
      {/* Family selector — optionally led by a caller control (New Job's Job type). */}
      <div className="builder-row">
        {leading}
        <div className="field">
          <label className="label">Method</label>
          <div className="radio-row">
            {inherit ? (
              <label className="radio">
                <input
                  type="radio"
                  checked={inheriting}
                  onChange={() => inherit.onChange(true)}
                />
                Inherit from source
              </label>
            ) : null}
            {FAMILIES.map((f) => (
              <label className="radio" key={f.value}>
                <input
                  type="radio"
                  checked={!inheriting && family === f.value}
                  onChange={() => {
                    // Picking a concrete family turns off inherit (an override) and sets it.
                    inherit?.onChange(false);
                    onChange({ methodFamily: f.value });
                  }}
                />
                {f.label}
              </label>
            ))}
          </div>
        </div>
      </div>

      {/* Inherit active → the source's method is used verbatim; show the note, no controls. */}
      {inheriting ? (
        inherit?.note ? <div className="builder-row">{inherit.note}</div> : null
      ) : family === "composite" ? (
        <div className="builder-row">
          <OptionSelect
            label="Composite method"
            value={value.composite}
            options={COMPOSITE_METHODS}
            onChange={(v) => onChange({ composite: v })}
          />
          <div className="builder-note muted">
            Basis, dispersion and RI are built into the composite method.
          </div>
        </div>
      ) : family === "xtb" ? (
        <div className="builder-row">
          <OptionSelect
            label="Semi-empirical method"
            value={value.xtbMethod}
            options={XTB_METHODS}
            onChange={(v) => onChange({ xtbMethod: v })}
          />
          <div className="builder-note muted">
            Gas phase, no basis/RI/SCFConv — semi-empirical is self-contained.
          </div>
        </div>
      ) : family === "wavefunction" ? (
        <div className="builder-row">
          <div className="field">
            <label className="label">Method</label>
            <select
              className="input select"
              value={value.wavefunction}
              onChange={(e) => onChange({ wavefunction: e.currentTarget.value })}
            >
              {WAVEFUNCTION_METHODS.map((g) => (
                <optgroup key={g.label} label={g.label}>
                  {g.options.map((o) => (
                    <option key={o.keyword} value={o.keyword}>
                      {o.label}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          </div>
          <div className="field">
            <label className="label">Basis set</label>
            <select
              className="input select"
              value={value.basis}
              onChange={(e) => onChange({ basis: e.currentTarget.value })}
            >
              {BASIS_GROUPS.map((g) => (
                <optgroup key={g.label} label={g.label}>
                  {g.options.map((o) => (
                    <option key={o.keyword} value={o.keyword}>
                      {o.label}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          </div>
          <div className="builder-note muted">
            Correlated: DLPNO variants add the /C correlation aux + RIJCOSX
            automatically. (T) has no analytic gradient — single-point
            recommended; Opt/Freq is numerical &amp; expensive.
          </div>
        </div>
      ) : (
        <div className="builder-row">
          <div className="field">
            <label className="label">Functional</label>
            <select
              className="input select"
              value={value.functional}
              onChange={(e) => onChange({ functional: e.currentTarget.value })}
            >
              {FUNCTIONAL_GROUPS.map((g) => (
                <optgroup key={g.label} label={g.label}>
                  {g.options.map((o) => (
                    <option key={o.keyword} value={o.keyword}>
                      {o.label}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          </div>
          <div className="field">
            <label className="label">Basis set</label>
            <select
              className="input select"
              value={value.basis}
              onChange={(e) => onChange({ basis: e.currentTarget.value })}
            >
              {BASIS_GROUPS.map((g) => (
                <optgroup key={g.label} label={g.label}>
                  {g.options.map((o) => (
                    <option key={o.keyword} value={o.keyword}>
                      {o.label}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          </div>
          <OptionSelect
            label="Dispersion"
            value={value.dispersion}
            options={DISPERSION}
            onChange={(v) => onChange({ dispersion: v })}
            disabled={dispersionBuiltIn}
            title={dispersionBuiltIn ? "Included in the functional" : undefined}
          />
          <OptionSelect
            label="RI approximation"
            value={value.ri}
            options={RI_METHODS}
            onChange={(v) => onChange({ ri: v })}
          />
        </div>
      )}
    </>
  );
}
