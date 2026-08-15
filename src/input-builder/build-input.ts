//! Pure ORCA-input generator for the builder form.
//!
//! No React, no DOM — just `BuilderState` → text, so the domain rules (composite
//! self-sufficiency, RI aux-basis pairing, keyword order, solvation syntax) are
//! unit-testable in isolation. See `wiki/orca/input-format.md`.

import { FUNCTIONAL_GROUPS, WAVEFUNCTION_METHODS } from "./orca-options";
import { mergeToAtomLines, totalCharge } from "../scene/scene";
import type { Scene } from "../scene/types";

/**
 * Which family of method the `!` line is built from. Each family drives a
 * different branch of {@link buildKeywordLine}:
 *  - `composite`    — a self-contained 3c method (no basis/disp/RI).
 *  - `dft`          — an explicit functional + basis (+ aux/RI/dispersion).
 *  - `wavefunction` — a correlated WF method (MP2 … DLPNO-CCSD(T)); the RI/DLPNO
 *                     variants add a `<basis>/C` correlation aux + Coulomb aux +
 *                     RIJCOSX, canonical variants add nothing. No dispersion (the
 *                     correlation IS the dispersion). Keeps the solvation/SCF tail.
 *  - `xtb`          — a semi-empirical tight-binding method, self-contained
 *                     (no basis/disp/RI AND no solvation/SCFConv tail).
 */
export type MethodFamily = "composite" | "dft" | "wavefunction" | "xtb";

/** Everything the form collects. Mirrors the controls 1:1. */
export interface BuilderState {
  jobType: string;
  /** Which method family drives the `!` line (composite | dft | xtb). */
  methodFamily: MethodFamily;
  composite: string;
  /** The semi-empirical method keyword (only "XTB"/GFN2 verified). */
  xtbMethod: string;
  /** The correlated WF method keyword (MP2 … DLPNO-CCSD(T)). */
  wavefunction: string;
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

/**
 * The **method-relevant** slice of {@link BuilderState} — the fields that drive
 * {@link buildKeywordLine}'s family branch (`methodFamily` + the per-family method
 * keywords). Everything else (jobType, solvation, SCF, charge/mult, pal/maxcore) is
 * NOT here: those are supplied by the caller that consumes a slice.
 *
 * The point of extracting this as its own type is that a slice can be spread into a
 * full `BuilderState` and let `buildOrcaInput` apply the correct family logic — so a
 * DFT override still carries its functional + basis + **paired** RI aux + dispersion
 * (the pairing lives in the `dft` branch, never flattened into a composite string).
 * Produced by the reusable `<MethodPicker>` and consumed by `buildOptTSInput`.
 */
export type MethodSlice = Pick<
  BuilderState,
  | "methodFamily"
  | "composite"
  | "functional"
  | "basis"
  | "dispersion"
  | "ri"
  | "xtbMethod"
  | "wavefunction"
>;

/** Project a full {@link BuilderState} (or another slice) down to its {@link MethodSlice} —
 * the single place the slice's exact fields are enumerated, reused by `<MethodPicker>` callers
 * that seed an override from the defaults (`methodSliceOf(DEFAULT_BUILDER_STATE)`). */
export function methodSliceOf(state: MethodSlice): MethodSlice {
  return {
    methodFamily: state.methodFamily,
    composite: state.composite,
    functional: state.functional,
    basis: state.basis,
    dispersion: state.dispersion,
    ri: state.ri,
    xtbMethod: state.xtbMethod,
    wavefunction: state.wavefunction,
  };
}

/** Sensible starting point: the recommended composite method, Opt+Freq, tight SCF. */
export const DEFAULT_BUILDER_STATE: BuilderState = {
  jobType: "Opt Freq",
  methodFamily: "composite",
  composite: "r2SCAN-3c",
  xtbMethod: "XTB",
  wavefunction: "DLPNO-CCSD(T)",
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
 * none applies. def2-* orbital bases pair with the tuned Weigend auxiliaries:
 * RIJCOSX and RI-J with `def2/J`, RI-JK with `def2/JK`. Any other basis
 * (Dunning, Pople) has no matching def2 fit set, so under RI we fall back to
 * ORCA's general `AutoAux` — an auto-generated auxiliary. ORCA fails loud on a
 * bad aux (never silently wrong), so AutoAux is the honest default here.
 */
function auxBasisFor(basis: string, ri: string): string {
  if (!ri) return "";
  if (!basis.toLowerCase().startsWith("def2")) return "AutoAux";
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
 * True when a correlated WF method uses the RI/DLPNO approximation and therefore
 * needs a correlation-fitting auxiliary basis (`<basis>/C`) + a Coulomb aux +
 * RIJCOSX. The canonical (non-RI) MP2/CCSD/CCSD(T) take NO aux chain. The fact
 * lives once, in {@link WAVEFUNCTION_METHODS} (`needsCorrelationAux`). Unknown
 * keywords return `false` (canonical is the safe default — ORCA fails loud on a
 * spurious aux rather than silently running the wrong method).
 */
export function methodNeedsCorrelationAux(keyword: string): boolean {
  for (const group of WAVEFUNCTION_METHODS) {
    const opt = group.options.find((o) => o.keyword === keyword);
    if (opt) return opt.needsCorrelationAux === true;
  }
  return false;
}

/**
 * The correlation-fitting auxiliary for an RI/DLPNO calculation: `${basis}/C`
 * for the def2 family ONLY (the one combination measured — `def2-TZVP/C` +
 * `def2/J`, ORCA 6.1, see `wiki/orca/correlated-methods.md`), else `null`.
 *
 * Everything non-def2 (Dunning cc-pV*Z / aug-cc-pV*Z, Pople) returns `null` so
 * the caller emits bare `AutoAux` — which ORCA generates to cover BOTH J and C.
 * This keeps N1b consistent with N1a (`auxBasisFor` already sends non-def2 →
 * AutoAux) and never emits the UNMEASURED mixed form "<basis>/C AutoAux", whose
 * ORCA behaviour (does the global AutoAux flag override the explicit /C?) is
 * unknown. The tighter Dunning form "cc-pVTZ/C def2/J RIJCOSX" (def2/J is a
 * universal Coulomb aux) is a future refinement, to ship only after that exact
 * combination is probed (rule #10).
 */
export function correlationAux(basis: string): string | null {
  if (basis.toLowerCase().startsWith("def2")) {
    return `${basis}/C`;
  }
  return null;
}

/**
 * Build the `!` keyword line (without the leading `! `) in the canonical order
 * `method basis auxbasis RI dispersion solvation jobtype scfconv`. Exposed on
 * its own so the form can show a live preview before the user hits Generate.
 */
export function buildKeywordLine(state: BuilderState): string {
  const tokens: string[] = [];

  if (state.methodFamily === "composite") {
    // Composite methods are self-contained: no basis, dispersion, or RI.
    tokens.push(state.composite);
  } else if (state.methodFamily === "xtb") {
    // Semi-empirical tight-binding is fully self-contained: the method keyword
    // ONLY. No basis/aux/RI/dispersion here, and no solvation/SCFConv tail
    // below — `! XTB def2-TZVP SMD(water) TightSCF` would be invalid.
    tokens.push(state.xtbMethod);
  } else if (state.methodFamily === "wavefunction") {
    // Correlated WF: method + basis. RI/DLPNO variants add the correlation-fit
    // aux (`<basis>/C`) + a Coulomb aux + RIJCOSX (measured, see
    // wiki/orca/correlated-methods.md); canonical MP2/CCSD/CCSD(T) add NOTHING
    // — a spurious /C+RIJCOSX on a canonical method is a different, wrong run.
    // NO dispersion keyword: the correlation IS the dispersion (a D4 here
    // double-counts).
    tokens.push(state.wavefunction);
    tokens.push(state.basis);
    if (methodNeedsCorrelationAux(state.wavefunction)) {
      const c = correlationAux(state.basis);
      if (c) {
        tokens.push(c);
        const j = auxBasisFor(state.basis, "RIJCOSX");
        if (j) tokens.push(j);
        tokens.push("RIJCOSX");
      } else {
        // Pople etc.: no native /C — AutoAux generates J+C together.
        tokens.push("AutoAux");
        tokens.push("RIJCOSX");
      }
    }
  } else {
    // dft: an explicit functional + basis (+ aux/RI/dispersion).
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

  // Solvation and SCF-convergence are electronic-structure concepts that do
  // not apply to semi-empirical xTB — suppress the whole tail for that family.
  if (state.methodFamily !== "xtb") {
    if (state.solvationModel && state.solvent) {
      tokens.push(`${state.solvationModel}(${state.solvent})`);
    }
  }
  // Job type is valid for every family (`! XTB Opt`, `! r2SCAN-3c Opt Freq`).
  if (state.jobType) tokens.push(state.jobType);
  if (state.methodFamily !== "xtb" && state.scfConv) tokens.push(state.scfConv);

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
