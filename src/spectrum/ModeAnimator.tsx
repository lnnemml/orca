import { useEffect, useMemo, useState } from "react";

import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { elementsAgree } from "../trajectory/frame";
import {
  modeDisplacements,
  modeFrameXyz,
  modeMinDistanceOverPeriod,
  DEFAULT_AMPLITUDE,
  MIN_AMPLITUDE,
  MAX_AMPLITUDE,
  AMPLITUDE_STEP,
  PHASE_FRAMES,
  MIN_SAFE_DISTANCE_ANGSTROM,
} from "./mode";

/**
 * Normal-mode animation (unit 3.12, Part B) — the click-a-peak → watch-the-atoms-move
 * view, gated behind unit 3.12's Kabsch determiner (the `.hess` frame is a pure
 * translation of the reference, so modes are added as-is; `mode.ts`).
 *
 * **Ownership is the trajectory's, verbatim (ADR-011).** The phase, the amplitude,
 * the play timer and the speed are APPLICATION state here; the viewer is handed ONE
 * frame's geometry and has no timer, no frame list, and no 3Dmol `animate`/`setFrame`.
 * `mode.ts` builds that one frame (`x_eq + A·sin(2π·phase)·v`).
 *
 * **Amplitude is a display choice** (the mode is normalized — no absolute amplitude),
 * defaulting to the measured `orca_pltvib` multiplier 2.0, labelled as such. A
 * collapse guard warns when the current amplitude drives atoms into each other
 * (rule #9) instead of drawing mush.
 */

/** Playback speeds — frames/second for the app-layer timer (a UI choice). At
 * PHASE_FRAMES per period, 24 fps ≈ a 1.7 s oscillation. */
const SPEEDS = [
  { label: "0.5×", fps: 12 },
  { label: "1×", fps: 24 },
  { label: "2×", fps: 48 },
];
const DEFAULT_FPS = 24;

interface ModeAnimatorProps {
  /** The `.hess $atoms` element order — the order the mode rows belong to. */
  elements: string[];
  /** The equilibrium geometry (the reference / final geometry the card is drawn in),
   * Å. The modes are added to THIS (gate 3.12: pure translation, same direction). */
  equilibrium: [number, number, number][];
  /** The order the reference geometry is drawn in — must equal `elements`. */
  referenceElements: string[];
  /** Row-major 3N×3N normal-mode matrix and its dimension (`f.normal_modes` / `f.n_modes`). */
  normalModes: number[];
  nModes: number;
  /** The selected mode's index (column) into the matrix and `frequencies_cm`. */
  modeIndex: number;
  /** The mode's signed wavenumber (cm⁻¹) — negative ⇒ imaginary (reaction coordinate). */
  frequencyCm: number;
}

export function ModeAnimator({
  elements,
  equilibrium,
  referenceElements,
  normalModes,
  nModes,
  modeIndex,
  frequencyCm,
}: ModeAnimatorProps) {
  // Phase as an integer 0…PHASE_FRAMES−1 (phase 0 = equilibrium exactly); the app
  // owns it, never the viewer. Auto-plays on select (clicking a peak starts it).
  const [phaseStep, setPhaseStep] = useState(0);
  const [playing, setPlaying] = useState(true);
  const [amplitude, setAmplitude] = useState(DEFAULT_AMPLITUDE);
  const [fps, setFps] = useState(DEFAULT_FPS);

  // Selecting a different mode restarts the oscillation from equilibrium.
  useEffect(() => {
    setPhaseStep(0);
    setPlaying(true);
  }, [modeIndex]);

  // The play timer lives HERE (app layer), looping the period forever while playing —
  // unlike the trajectory's play-once. Kept in the component so the amplitude, the
  // label and the guard all stay in lock-step with the drawn frame.
  useEffect(() => {
    if (!playing) return;
    const id = setInterval(() => {
      setPhaseStep((p) => (p + 1) % PHASE_FRAMES);
    }, 1000 / fps);
    return () => clearInterval(id);
  }, [playing, fps]);

  const identityOk = elementsAgree(elements, referenceElements);
  const shapeOk = normalModes.length === nModes * nModes && nModes > 0;

  // The mode's displacement vectors + the guard's worst-case min distance. Guarded so
  // a bad matrix/index throws into a caught null rather than crashing the card.
  const derived = useMemo(() => {
    if (!identityOk || !shapeOk) return null;
    try {
      const disp = modeDisplacements(normalModes, nModes, modeIndex);
      if (disp.length !== equilibrium.length) return null;
      const minDist = modeMinDistanceOverPeriod(equilibrium, disp, amplitude);
      return { disp, minDist };
    } catch {
      return null;
    }
  }, [identityOk, shapeOk, normalModes, nModes, modeIndex, equilibrium, amplitude]);

  const frameXyz = useMemo(() => {
    if (!derived) return "";
    return modeFrameXyz(elements, equilibrium, derived.disp, amplitude, phaseStep / PHASE_FRAMES);
  }, [derived, elements, equilibrium, amplitude, phaseStep]);

  if (!identityOk) {
    return (
      <div className="banner err" style={{ marginTop: 6 }}>
        Mode atom order does not match the result geometry ({elements.length} vs{" "}
        {referenceElements.length} atoms / different sequence). Not animating — this
        would move the wrong atoms.
      </div>
    );
  }
  if (!derived) return null; // no normal modes / bad shape — table stays, no animator

  const isImaginary = frequencyCm < 0;
  const collapsing = derived.minDist < MIN_SAFE_DISTANCE_ANGSTROM;

  return (
    <div className="mode-animator">
      <div className="mode-label mono">
        {isImaginary ? (
          <>
            <span className="mode-imag">imaginary {frequencyCm.toFixed(1)} cm⁻¹</span> —
            reaction coordinate: the motion the system follows downhill in BOTH
            directions off the transition state (not a real vibration).
          </>
        ) : (
          <>
            mode {frequencyCm.toFixed(1)} cm⁻¹ — the atoms oscillate along the
            normal-mode direction.
          </>
        )}
      </div>

      <div className="viewer-panel mode-viewer">
        <MoleculeViewer xyzData={frameXyz} preserveCameraOnUpdate />
      </div>

      {collapsing ? (
        <div className="banner warn" style={{ marginTop: 6 }}>
          At amplitude {amplitude.toFixed(2)} the atoms overlap (closest approach{" "}
          {derived.minDist.toFixed(2)} Å &lt; {MIN_SAFE_DISTANCE_ANGSTROM} Å). The
          amplitude is a viewing choice, not physical — reduce it to see this mode
          without the atoms passing through each other.
        </div>
      ) : null}

      <div className="mode-controls">
        <button
          className="btn btn-sm"
          onClick={() => setPlaying((p) => !p)}
          title={playing ? "Pause" : "Play"}
        >
          {playing ? "⏸" : "▶"}
        </button>

        <label className="mode-amp">
          amplitude {amplitude.toFixed(2)}
          <input
            type="range"
            min={MIN_AMPLITUDE}
            max={MAX_AMPLITUDE}
            step={AMPLITUDE_STEP}
            value={amplitude}
            onChange={(e) => setAmplitude(Number(e.target.value))}
            aria-label="mode amplitude"
          />
        </label>

        <select
          className="select select-sm"
          value={fps}
          onChange={(e) => setFps(Number(e.target.value))}
          aria-label="animation speed"
        >
          {SPEEDS.map((s) => (
            <option key={s.fps} value={s.fps}>
              {s.label}
            </option>
          ))}
        </select>
      </div>

      <div className="muted" style={{ fontSize: 11, marginTop: 2 }}>
        amplitude is a display choice — a normalized mode has no absolute amplitude.
        Default 2.0 is the multiplier <code>orca_pltvib</code> applies (measured).
      </div>
    </div>
  );
}
