import { useMemo, useState } from "react";

import { sceneFromOrcaInput, setMultiplicity, totalCharge } from "../scene/scene";
import { checkElectronParity } from "../scene/parity";
import {
  BASIS_SETS,
  COMPOSITE_METHODS,
  DISPERSION,
  FUNCTIONAL_GROUPS,
  JOB_TYPES,
  RI_METHODS,
  SCF_CONV,
  SOLVATION_MODELS,
  SOLVENTS,
  type OrcaOption,
} from "./orca-options";
import {
  buildKeywordLine,
  buildOrcaInput,
  DEFAULT_BUILDER_STATE,
  functionalHasBuiltInDispersion,
  type BuilderState,
} from "./build-input";

interface InputBuilderFormProps {
  /** Current editor content — the coordinate block is preserved. */
  currentContent: string;
  /** Called with the regenerated input when the user applies the form. */
  onGenerate: (newContent: string) => void;
}

/** A labelled `<select>` over a list of {@link OrcaOption}s. */
function OptionSelect({
  label,
  value,
  options,
  onChange,
  disabled,
  title,
}: {
  label: string;
  value: string;
  options: OrcaOption[];
  onChange: (v: string) => void;
  disabled?: boolean;
  /** Tooltip on the control — e.g. why it is disabled. */
  title?: string;
}) {
  return (
    <div className="field">
      <label className="label">{label}</label>
      <select
        className="input select"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
        disabled={disabled}
        title={title}
      >
        {options.map((o) => (
          <option key={o.keyword || "__none__"} value={o.keyword}>
            {o.label}
          </option>
        ))}
      </select>
    </div>
  );
}

export function InputBuilderForm({
  currentContent,
  onGenerate,
}: InputBuilderFormProps) {
  // Seed charge/multiplicity from the current geometry header so an imported
  // ion keeps its charge; the rest are plain defaults. (This reads only the
  // `* xyz c m` line — the `!` keyword line is never parsed back into the form.)
  const [state, setState] = useState<BuilderState>(() => {
    const seed = sceneFromOrcaInput(currentContent);
    return {
      ...DEFAULT_BUILDER_STATE,
      charge: seed ? totalCharge(seed) : DEFAULT_BUILDER_STATE.charge,
      multiplicity: seed ? seed.multiplicity : DEFAULT_BUILDER_STATE.multiplicity,
    };
  });

  const set = <K extends keyof BuilderState>(key: K, value: BuilderState[K]) =>
    setState((s) => ({ ...s, [key]: value }));

  // The Scene derived from the current buffer (single-fragment in 2.5.0b —
  // 2.5.0d threads a real multi-fragment Scene through a store). Its
  // multiplicity is the user's editable choice, not the parsed header value, so
  // charge derives from the fragments while multiplicity stays under the form's
  // control. `null` when the buffer has no inline coordinate block.
  const scene = useMemo(() => {
    const parsed = sceneFromOrcaInput(currentContent);
    return parsed ? setMultiplicity(parsed, state.multiplicity) : null;
  }, [currentContent, state.multiplicity]);

  const parity = scene ? checkElectronParity(scene) : null;

  const generate = () => {
    // Scene present → charge/mult/coords come from the Scene; absent → the
    // form's own fields drive a placeholder geometry (unchanged behaviour).
    onGenerate(buildOrcaInput(state, scene));
  };

  const compositeMode = state.useComposite;
  const dispersionBuiltIn = functionalHasBuiltInDispersion(state.functional);

  return (
    <div className="builder-body">
      {/* Row 1: job type + mode toggle */}
      <div className="builder-row">
        <OptionSelect
          label="Job type"
          value={state.jobType}
          options={JOB_TYPES}
          onChange={(v) => set("jobType", v)}
        />
        <div className="field">
          <label className="label">Method</label>
          <div className="radio-row">
            <label className="radio">
              <input
                type="radio"
                checked={compositeMode}
                onChange={() => set("useComposite", true)}
              />
              Composite (3c)
            </label>
            <label className="radio">
              <input
                type="radio"
                checked={!compositeMode}
                onChange={() => set("useComposite", false)}
              />
              Functional + Basis
            </label>
          </div>
        </div>
      </div>

      {/* Row 2: method-specific controls */}
      {compositeMode ? (
        <div className="builder-row">
          <OptionSelect
            label="Composite method"
            value={state.composite}
            options={COMPOSITE_METHODS}
            onChange={(v) => set("composite", v)}
          />
          <div className="builder-note muted">
            Basis, dispersion and RI are built into the composite method.
          </div>
        </div>
      ) : (
        <div className="builder-row">
          <div className="field">
            <label className="label">Functional</label>
            <select
              className="input select"
              value={state.functional}
              onChange={(e) => set("functional", e.currentTarget.value)}
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
          <OptionSelect
            label="Basis set"
            value={state.basis}
            options={BASIS_SETS}
            onChange={(v) => set("basis", v)}
          />
          <OptionSelect
            label="Dispersion"
            value={state.dispersion}
            options={DISPERSION}
            onChange={(v) => set("dispersion", v)}
            disabled={dispersionBuiltIn}
            title={
              dispersionBuiltIn ? "Included in the functional" : undefined
            }
          />
          <OptionSelect
            label="RI approximation"
            value={state.ri}
            options={RI_METHODS}
            onChange={(v) => set("ri", v)}
          />
        </div>
      )}

      {/* Row 3: solvation + SCF */}
      <div className="builder-row">
        <OptionSelect
          label="Solvation"
          value={state.solvationModel}
          options={SOLVATION_MODELS}
          onChange={(v) => set("solvationModel", v)}
        />
        <div className="field">
          <label className="label">Solvent</label>
          <select
            className="input select"
            value={state.solvent}
            onChange={(e) => set("solvent", e.currentTarget.value)}
            disabled={!state.solvationModel}
          >
            {SOLVENTS.map((s) => (
              <option key={s} value={s}>
                {s}
              </option>
            ))}
          </select>
        </div>
        <OptionSelect
          label="SCF convergence"
          value={state.scfConv}
          options={SCF_CONV}
          onChange={(v) => set("scfConv", v)}
        />
      </div>

      {/* Row 4: numeric settings */}
      <div className="builder-row">
        <div className="field builder-num">
          <label className="label">Charge</label>
          {scene ? (
            <>
              <input
                className="input mono"
                type="number"
                value={totalCharge(scene)}
                readOnly
                disabled
                title="Sum of fragment charges — set per fragment, not here"
              />
              <span className="builder-charge-note muted">
                Σ of {scene.fragments.length} fragment
                {scene.fragments.length === 1 ? "" : "s"}
              </span>
            </>
          ) : (
            <input
              className="input mono"
              type="number"
              value={state.charge}
              onChange={(e) => set("charge", Number(e.currentTarget.value))}
            />
          )}
        </div>
        <div className="field builder-num">
          <label className="label">Multiplicity</label>
          <input
            className="input mono"
            type="number"
            min={1}
            value={state.multiplicity}
            onChange={(e) => set("multiplicity", Number(e.currentTarget.value))}
          />
        </div>
        <div className="field builder-num">
          <label className="label">nprocs</label>
          <input
            className="input mono"
            type="number"
            min={1}
            value={state.nprocs}
            onChange={(e) => set("nprocs", Number(e.currentTarget.value))}
          />
        </div>
        <div className="field builder-num">
          <label className="label">maxcore (MB)</label>
          <input
            className="input mono"
            type="number"
            min={100}
            step={100}
            value={state.maxcore}
            onChange={(e) => set("maxcore", Number(e.currentTarget.value))}
          />
        </div>
      </div>

      {/* Electron-parity warning — informational, never blocks Generate. */}
      {parity && (
        <div className="builder-row">
          <div className="builder-parity" role="status">
            ⚠ {parity.message}
          </div>
        </div>
      )}

      {/* Row 5: generate + live preview of the ! line */}
      <div className="builder-row builder-actions">
        <button className="btn btn-primary" onClick={generate}>
          Generate Input
        </button>
        <code className="builder-preview mono">! {buildKeywordLine(state)}</code>
      </div>
    </div>
  );
}
