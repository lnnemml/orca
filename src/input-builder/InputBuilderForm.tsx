import { useEffect, useLayoutEffect, useMemo, useState } from "react";

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
  parseNprocsText,
  type BuilderState,
} from "./build-input";
import { OptionSelect } from "./OptionSelect";
import { MethodPicker } from "./MethodPicker";
import { NebBuilderSection, type NebPayload } from "../reactions/NebBuilderSection";
import { useLocalMaxcore } from "../memory/local-maxcore";
import { builderMaxcoreDefault } from "../memory/preflight";
import { builderStateOnOpen, useDraftField, useNewJobDraft } from "../screens/new-job-draft";

interface InputBuilderFormProps {
  /** Current editor content — the coordinate block is preserved. */
  currentContent: string;
  /**
   * Called with the regenerated input when the user applies the form. For a NEB-TS
   * job the second arg carries the product.xyz + the two source job ids (the NEB path
   * builds from picked endpoints, not the scene); undefined for every other job type.
   */
  onGenerate: (content: string, neb?: NebPayload) => void;
  /** The selected Run On target (a profile id; null/absent = this machine): the `%maxcore` field's
   *  default follows it (ADR-024 o26.5, o26.13.1). */
  runTarget?: string | null;
}

export function InputBuilderForm({
  currentContent,
  onGenerate,
  runTarget = null,
}: InputBuilderFormProps) {
  // The `%maxcore` default (o26.5, o26.13.1): the local fitted value (the fan-out accessor's cache)
  // for this machine, 2000 for a server — applied only while the user has not touched the field.
  const localDefault = useLocalMaxcore();
  const maxcoreDefault = builderMaxcoreDefault(runTarget, localDefault);

  // The builder's state lives in the New Job draft (ADR-026 item 4), so closing the accordion or
  // leaving the tab keeps the method. On every open, charge and multiplicity are re-read from the
  // current `* xyz c m` header (rule #9: a remembered multiplicity must never override a geometry
  // that changed since — `builderStateOnOpen`), and an untouched `%maxcore` takes the default at
  // once, so the remembered value never shows. The opened state is computed once, rendered until
  // it is written to the draft before paint, and the draft is the state from then on.
  const [remembered, setState] = useDraftField("builder");
  const [maxcoreTouched, setMaxcoreTouched] = useDraftField("maxcoreTouched");
  const [opened] = useState<BuilderState>(() => {
    const d = useNewJobDraft.getState();
    const s = builderStateOnOpen(d.builder, currentContent);
    return d.maxcoreTouched ? s : { ...s, maxcore: maxcoreDefault };
  });
  const [seeded, setSeeded] = useState(false);
  useLayoutEffect(() => {
    setState(opened);
    setSeeded(true);
  }, [opened, setState]);
  const state = seeded ? remembered : opened;
  useEffect(() => {
    if (maxcoreTouched) return;
    setState((s) => (s.maxcore === maxcoreDefault ? s : { ...s, maxcore: maxcoreDefault }));
  }, [maxcoreDefault, maxcoreTouched, setState]);

  // The nprocs field's RAW text (ADR-025 item 2), kept in the draft with the builder. `state.nprocs`
  // is set only from a valid parse of it (blank → null = auto, a positive integer → that number);
  // invalid text never reaches the state, and while it stands both Generate controls are disabled
  // (item 9).
  const [nprocsText, setNprocsText] = useDraftField("nprocsText");
  const nprocsInvalid = parseNprocsText(nprocsText).kind === "invalid";
  const onNprocsText = (raw: string) => {
    setNprocsText(raw);
    const parsed = parseNprocsText(raw);
    if (parsed.kind === "auto") set("nprocs", null);
    else if (parsed.kind === "explicit") set("nprocs", parsed.nprocs);
  };

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
    if (nprocsInvalid) return; // the button is disabled too; never emit an invalid nprocs
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

      {/* Row 4: numeric settings — top-aligned so captioned fields don't lift their inputs */}
      <div className="builder-row builder-row-top">
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
          <label className="label" htmlFor="builder-nprocs">
            nprocs
          </label>
          {/* type="text", not "number": a number input reports half-typed text ("-", "1e") as
              "" — the auto case — and its WebKitGTK behaviour is unmeasured (ADR-025 item 2). */}
          <input
            id="builder-nprocs"
            className="input mono"
            type="text"
            inputMode="numeric"
            placeholder="auto"
            value={nprocsText}
            onChange={(e) => onNprocsText(e.currentTarget.value)}
            aria-invalid={nprocsInvalid}
          />
          <span className="builder-charge-note muted">blank = auto: the run target sets it</span>
        </div>
        <div className="field builder-num">
          <label className="label" htmlFor="builder-maxcore">
            maxcore (MB)
          </label>
          <input
            id="builder-maxcore"
            className="input mono"
            type="number"
            min={100}
            step={100}
            value={state.maxcore}
            onChange={(e) => {
              setMaxcoreTouched(true);
              set("maxcore", Number(e.currentTarget.value));
            }}
          />
        </div>
      </div>

      {/* Invalid nprocs — a blocking error (role="alert"), unlike the parity warning below. */}
      {nprocsInvalid && (
        <div className="builder-row">
          <div className="builder-error" role="alert">
            nprocs must be a positive whole number, or blank for auto.
          </div>
        </div>
      )}

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
          nprocsValid={!nprocsInvalid}
          onGenerateNeb={(inp, payload) => onGenerate(inp, payload)}
        />
      ) : (
        <div className="builder-row builder-actions">
          <button className="btn btn-primary" onClick={generate} disabled={nprocsInvalid}>
            Generate Input
          </button>
          <code className="builder-preview mono">! {buildKeywordLine(state)}</code>
        </div>
      )}
    </div>
  );
}
