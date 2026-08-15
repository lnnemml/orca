import { useMemo, useState } from "react";

import { sceneFromOrcaInput, setMultiplicity, totalCharge } from "../scene/scene";
import { checkElectronParity } from "../scene/parity";
import {
  JOB_TYPES,
  SCF_CONV,
  SOLVATION_MODELS,
  SOLVENTS,
} from "./orca-options";
import {
  buildKeywordLine,
  buildOrcaInput,
  DEFAULT_BUILDER_STATE,
  type BuilderState,
} from "./build-input";
import { OptionSelect } from "./OptionSelect";
import { MethodPicker } from "./MethodPicker";
import { NebBuilderSection, type NebPayload } from "../reactions/NebBuilderSection";

interface InputBuilderFormProps {
  /** Current editor content — the coordinate block is preserved. */
  currentContent: string;
  /**
   * Called with the regenerated input when the user applies the form. For a NEB-TS
   * job the second arg carries the product.xyz + the two source job ids (the NEB path
   * builds from picked endpoints, not the scene); undefined for every other job type.
   */
  onGenerate: (content: string, neb?: NebPayload) => void;
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

  // Semi-empirical xTB is self-contained — solvation and SCF-convergence do not
  // apply (the emit already suppresses them; here we only DISABLE the controls,
  // never clear the stored values, so switching back to DFT restores the choice).
  const isXtb = state.methodFamily === "xtb";
  const notForXtb = "Not applicable to semi-empirical xTB";

  return (
    <div className="builder-body">
      {/* Rows 1+2: job type (leading) + the reusable method sub-UI (family + per-family
          controls). The extraction is byte-identical for New Job — no `inherit` option here,
          so the picker is a concrete family exactly as before. */}
      <MethodPicker
        value={state}
        onChange={(patch) => setState((s) => ({ ...s, ...patch }))}
        leading={
          <OptionSelect
            label="Job type"
            value={state.jobType}
            options={JOB_TYPES}
            onChange={(v) => set("jobType", v)}
          />
        }
      />

      {/* Row 3: solvation + SCF — not applicable to xTB (disabled, not cleared) */}
      <div className="builder-row">
        <OptionSelect
          label="Solvation"
          value={state.solvationModel}
          options={SOLVATION_MODELS}
          onChange={(v) => set("solvationModel", v)}
          disabled={isXtb}
          title={isXtb ? notForXtb : undefined}
        />
        <div className="field">
          <label className="label">Solvent</label>
          <select
            className="input select"
            value={state.solvent}
            onChange={(e) => set("solvent", e.currentTarget.value)}
            disabled={isXtb || !state.solvationModel}
            title={isXtb ? notForXtb : undefined}
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
          disabled={isXtb}
          title={isXtb ? notForXtb : undefined}
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

      {/* Row 5: generate + live preview of the ! line. NEB-TS builds from picked
          endpoints (not the scene), so its "Generate" is the NebBuilderSection; the
          method/basis controls above still set the NEB level (the `!` preview shows it). */}
      {state.jobType === "NEB-TS" ? (
        <NebBuilderSection
          state={state}
          onGenerateNeb={(inp, payload) => onGenerate(inp, payload)}
        />
      ) : (
        <div className="builder-row builder-actions">
          <button className="btn btn-primary" onClick={generate}>
            Generate Input
          </button>
          <code className="builder-preview mono">! {buildKeywordLine(state)}</code>
        </div>
      )}
    </div>
  );
}
