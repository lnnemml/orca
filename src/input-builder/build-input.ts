//! Pure ORCA-input generator for the builder form.
//!
//! No React, no DOM — just `BuilderState` → text, so the domain rules (composite
//! self-sufficiency, RI aux-basis pairing, keyword order, solvation syntax) are
//! unit-testable in isolation. See `wiki/orca/input-format.md`.

import { FUNCTIONAL_GROUPS } from "./orca-options";
import { mergeToAtomLines, totalCharge } from "../scene/scene";
import type { Scene } from "../scene/types";

/** Everything the form collects. Mirrors the controls 1:1. */
export interface BuilderState {
  jobType: string;
  /** Toggle: a composite (3c) method vs an explicit functional + basis. */
  useComposite: boolean;
  composite: string;
  functional: string;
  basis: string;
  dispersion: string;
  ri: string;
  solvationModel: string; // "" | "CPCM" | "SMD"
  solvent: string;
  scfConv: string;
  charge: number;
  multiplicity: number;
  nprocs: number;
  maxcore: number; // MB per core
}

/** Sensible starting point: the recommended composite method, Opt+Freq, tight SCF. */
export const DEFAULT_BUILDER_STATE: BuilderState = {
  jobType: "Opt Freq",
  useComposite: true,
  composite: "r2SCAN-3c",
  functional: "B3LYP",
  basis: "def2-TZVP",
  dispersion: "D4",
  ri: "RIJCOSX",
  solvationModel: "",
  solvent: "water",
  scfConv: "TightSCF",
  charge: 0,
  multiplicity: 1,
  nprocs: 4,
  maxcore: 2000,
};

/**
 * The Coulomb/exchange fitting basis an RI approximation needs, or `""` when
 * none applies. Only def2-* orbital bases get an automatic auxiliary basis:
 * RIJCOSX and RI-J pair with `def2/J`, RI-JK with `def2/JK`.
 */
function auxBasisFor(basis: string, ri: string): string {
  if (!ri) return "";
  if (!basis.toLowerCase().startsWith("def2")) return "";
  return ri === "RI-JK" ? "def2/JK" : "def2/J";
}

/**
 * True when a functional already bundles its own dispersion correction (e.g.
 * `wB97X-D4`, `wB97M-V`) — in which case the builder must NOT append a separate
 * dispersion keyword, or the correction would be double-counted. Looks the
 * functional up in {@link FUNCTIONAL_GROUPS}; unknown functionals return `false`.
 */
export function functionalHasBuiltInDispersion(functional: string): boolean {
  for (const group of FUNCTIONAL_GROUPS) {
    const opt = group.options.find((o) => o.keyword === functional);
    if (opt) return opt.builtInDispersion === true;
  }
  return false;
}

/**
 * Build the `!` keyword line (without the leading `! `) in the canonical order
 * `method basis auxbasis RI dispersion solvation jobtype scfconv`. Exposed on
 * its own so the form can show a live preview before the user hits Generate.
 */
export function buildKeywordLine(state: BuilderState): string {
  const tokens: string[] = [];

  if (state.useComposite) {
    // Composite methods are self-contained: no basis, dispersion, or RI.
    tokens.push(state.composite);
  } else {
    tokens.push(state.functional);
    tokens.push(state.basis);
    const aux = auxBasisFor(state.basis, state.ri);
    if (aux) tokens.push(aux);
    if (state.ri) tokens.push(state.ri);
    // Skip dispersion for functionals that already include it (e.g. wB97X-D4,
    // wB97M-V) — appending D4/VV10 again would double-count the correction.
    if (state.dispersion && !functionalHasBuiltInDispersion(state.functional)) {
      tokens.push(state.dispersion);
    }
  }

  if (state.solvationModel && state.solvent) {
    tokens.push(`${state.solvationModel}(${state.solvent})`);
  }
  if (state.jobType) tokens.push(state.jobType);
  if (state.scfConv) tokens.push(state.scfConv);

  return tokens.filter((t) => t.length > 0).join(" ");
}

const PLACEHOLDER_COORDS = "# paste coordinates here, or import a molecule above";

/**
 * Assemble a complete, runnable ORCA input: the `!` line, the `%pal`/`%maxcore`
 * directives, and the `* xyz charge mult ... *` coordinate block.
 *
 * Geometry comes from a {@link Scene} (ADR-008): the header `charge` is
 * `totalCharge(scene)`, the multiplicity is `scene.multiplicity`, and the
 * coordinates are the canonical merged rows `mergeToAtomLines(scene)` — so the
 * Scene **overrides** `state.charge` / `state.multiplicity`. Pass `null` (or an
 * empty scene) to emit a commented placeholder using the form's own
 * charge/multiplicity.
 *
 * `%maxcore` is a simple directive (NO `end`); `%pal` is a block (WITH `end`).
 */
export function buildOrcaInput(
  state: BuilderState,
  scene: Scene | null,
): string {
  let charge = state.charge;
  let multiplicity = state.multiplicity;
  let coords: string;

  if (scene) {
    charge = totalCharge(scene);
    multiplicity = scene.multiplicity;
    const rows = mergeToAtomLines(scene);
    coords = rows.length > 0 ? rows.join("\n") : PLACEHOLDER_COORDS;
  } else {
    coords = PLACEHOLDER_COORDS;
  }

  return [
    `! ${buildKeywordLine(state)}`,
    `%pal nprocs ${state.nprocs} end`,
    `%maxcore ${state.maxcore}`,
    "",
    `* xyz ${charge} ${multiplicity}`,
    coords,
    "*",
    "",
  ].join("\n");
}
