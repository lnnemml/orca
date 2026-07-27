import { useState } from "react";

import { extractXyzFromInput } from "../viewer/parse-xyz-from-input";
import { xyzToAtomLines, parseChargeMult } from "../viewer/xyz-format";
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
}: {
  label: string;
  value: string;
  options: OrcaOption[];
  onChange: (v: string) => void;
  disabled?: boolean;
}) {
  return (
    <div className="field">
      <label className="label">{label}</label>
      <select
        className="input select"
        value={value}
        onChange={(e) => onChange(e.currentTarget.value)}
        disabled={disabled}
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
  const [state, setState] = useState<BuilderState>(() => ({
    ...DEFAULT_BUILDER_STATE,
    ...parseChargeMult(currentContent),
  }));

  const set = <K extends keyof BuilderState>(key: K, value: BuilderState[K]) =>
    setState((s) => ({ ...s, [key]: value }));

  const generate = () => {
    const std = extractXyzFromInput(currentContent);
    const atomLines = std ? xyzToAtomLines(std) : null;
    const atomBlock = atomLines ? atomLines.join("\n") : null;
    onGenerate(buildOrcaInput(state, atomBlock));
  };

  const compositeMode = state.useComposite;

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
          <input
            className="input mono"
            type="number"
            value={state.charge}
            onChange={(e) => set("charge", Number(e.currentTarget.value))}
          />
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
