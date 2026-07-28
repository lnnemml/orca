/**
 * Electron-parity validation (ADR-008 decision 8). Pure, React-free.
 *
 * The number of electrons in a system fixes the *parity* of its allowed spin
 * multiplicity: an even electron count can only be a singlet / triplet / quintet
 * (odd multiplicity), an odd count only a doublet / quartet / sextet (even
 * multiplicity). Getting this wrong is the exact error ORCA reports cryptically
 * ~30 s into a run ("Error: multiplicity … impossible for … electrons"); we can
 * say it instantly, in plain language, as a teaching moment.
 *
 * This validates *arithmetic* possibility only — NOT whether a triplet is a
 * physically sensible ground state for this molecule. That is the chemist's
 * call; we only catch the class of mistake that is provably impossible.
 */

import type { Scene } from "./types";
import { atomCount, electronCount } from "./scene";

export type ParityIssue = {
  kind: "parity-mismatch";
  electrons: number;
  multiplicity: number;
  /** Multiplicities of the correct parity, nearest first: e.g. [2, 4, 6]. */
  suggested: number[];
  message: string;
};

/** Human name for a small spin multiplicity (falls back to the bare number). */
function multiplicityName(m: number): string {
  const names: Record<number, string> = {
    1: "singlet",
    2: "doublet",
    3: "triplet",
    4: "quartet",
    5: "quintet",
    6: "sextet",
  };
  return names[m] ?? `multiplicity ${m}`;
}

/**
 * Check the scene's spin multiplicity against its electron count. Returns `null`
 * when the parity is consistent, when the scene is empty (nothing to validate),
 * or when the electron count can't be computed (an element outside the H–Kr
 * table) — in that last case we simply don't offer a parity opinion rather than
 * crash the caller.
 */
export function checkElectronParity(scene: Scene): ParityIssue | null {
  if (atomCount(scene) === 0) return null; // empty scene — nothing to validate

  let electrons: number;
  try {
    electrons = electronCount(scene);
  } catch {
    return null; // unknown element — can't count electrons, so no parity claim
  }

  const multiplicity = scene.multiplicity;
  const electronsEven = electrons % 2 === 0;
  // even electrons ⇒ odd multiplicity; odd electrons ⇒ even multiplicity.
  const multiplicityOk = electronsEven
    ? multiplicity % 2 === 1
    : multiplicity % 2 === 0;
  if (multiplicityOk) return null;

  const start = electronsEven ? 1 : 2; // nearest valid multiplicity
  const suggested = [start, start + 2, start + 4];

  const parityWord = electronsEven ? "even" : "odd";
  const requiredParity = electronsEven ? "odd" : "even";
  const options = suggested
    .map((m) => `${multiplicityName(m)} (${m})`)
    .join(", ");
  const message =
    `This scene has ${electrons} electrons (${parityWord}), so its spin ` +
    `multiplicity must be ${requiredParity} — ${options}. ` +
    `Multiplicity ${multiplicity} (${multiplicityName(multiplicity)}) has the ` +
    `wrong parity for ${electrons} electrons; the nearest valid value is ${start}.`;

  return { kind: "parity-mismatch", electrons, multiplicity, suggested, message };
}
