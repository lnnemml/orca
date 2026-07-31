import { useEffect, useMemo, useState } from "react";

import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { elementsAgree, frameToXyz } from "../trajectory/frame";
import {
  modeDisplacements,
  modeFrameXyz,
  modeMinDistanceOverPeriod,
  atomicMasses,
  zeroPointAmplitudeAngstrom,
  DEFAULT_AMPLITUDE_ANGSTROM,
  MIN_AMPLITUDE_ANGSTROM,
  MAX_AMPLITUDE_ANGSTROM,
  AMPLITUDE_STEP_ANGSTROM,
  PHASE_FRAMES,
  MIN_SAFE_DISTANCE_ANGSTROM,
} from "./mode";

/**
 * Normal-mode animation (unit 3.12, made chemically honest in unit 3.13). Click a
 * peak → the atoms move along the mode.
 *
 * **Ownership is the trajectory's, verbatim (ADR-011).** The phase, amplitude, play
 * timer and speed are APPLICATION state here; the viewer is handed ONE frame's
 * geometry, with no timer, no frame list, and no 3Dmol `animate`/`setFrame`.
 *
 * **Amplitude = the maximum atomic displacement, in Å** (unit 3.13) — a display
 * choice with a labelled physical reference (the mode's real zero-point amplitude,
 * computed from verified masses). Not the old `A × mode` (which over-drove localized
 * stretches — the mode is normalized over 3N, so the busiest atom got the lot).
 *
 * **Bond topology is frozen at equilibrium** (unit 3.13): the graph is a function of
 * the equilibrium geometry only, so it is perceived once and held for the whole
 * cycle (`bondTopologyReference`), never re-perceived per distorted frame.
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
  // Amplitude = maximum atomic displacement in Å (unit 3.13).
  const [amplitude, setAmplitude] = useState(DEFAULT_AMPLITUDE_ANGSTROM);
  const [fps, setFps] = useState(DEFAULT_FPS);

  // Selecting a different mode restarts the oscillation from equilibrium.
  useEffect(() => {
    setPhaseStep(0);
    setPlaying(true);
  }, [modeIndex]);

  // The play timer lives HERE (app layer), looping the period forever while playing.
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
      const masses = atomicMasses(elements);
      const zeroPoint = masses
        ? zeroPointAmplitudeAngstrom(disp, masses, frequencyCm)
        : null;
      return { disp, minDist, zeroPoint };
    } catch {
      return null;
    }
  }, [identityOk, shapeOk, normalModes, nModes, modeIndex, equilibrium, amplitude, elements, frequencyCm]);

  // The equilibrium geometry as xyz — the reference from which bond topology is
  // perceived ONCE and frozen for the whole animation (unit 3.13). Independent of the
  // mode and the phase, so bonds never flicker as the atoms move.
  const referenceXyz = useMemo(() => {
    try {
      return frameToXyz(elements, { energy_eh: null, xyz_angstrom: equilibrium });
    } catch {
      return "";
    }
  }, [elements, equilibrium]);

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
        <MoleculeViewer
          xyzData={frameXyz}
          preserveCameraOnUpdate
          bondTopologyReference={referenceXyz || undefined}
        />
      </div>

      {collapsing ? (
        <div className="banner warn" style={{ marginTop: 6 }}>
          At {amplitude.toFixed(2)} Å the atoms overlap (closest approach{" "}
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
          amplitude {amplitude.toFixed(2)} Å
          <input
            type="range"
            min={MIN_AMPLITUDE_ANGSTROM}
            max={MAX_AMPLITUDE_ANGSTROM}
            step={AMPLITUDE_STEP_ANGSTROM}
            value={amplitude}
            onChange={(e) => setAmplitude(Number(e.target.value))}
            aria-label="mode amplitude, maximum atomic displacement in ångström"
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
        <strong>A</strong> is the maximum atomic displacement (a viewing choice) — the
        busiest atom moves this far, so localized and delocalized modes are comparably
        visible.
        {derived.zeroPoint != null ? (
          <>
            {" "}This mode's real zero-point amplitude is ≈ {derived.zeroPoint.toFixed(3)} Å
            (from its reduced mass); we exaggerate to {amplitude.toFixed(2)} Å for
            visibility.
          </>
        ) : null}
      </div>
    </div>
  );
}
