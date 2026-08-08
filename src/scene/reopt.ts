/**
 * DFT re-optimization fan-out — the CREATE side (Phase 4.5 Stage D unit D2a).
 *
 * Given a GOAT job's source input and one conformer from its ensemble, build the
 * ORCA input for a single DFT re-opt child. The scientific point (ADR-007): xTB
 * ensemble energies are coarse, so the lowest few conformers are re-optimized at
 * DFT before any reaction center is built on them.
 *
 * THE CHARGE FOOTGUN (rule #9, the exact class that made CREST QCG useless on the
 * anion). Charge and multiplicity live ONLY in the input text's `* xyz <c> <m>`
 * line — never a jobs column, never a builder default. Each child MUST inherit
 * (c, m) from the SOURCE GOAT job; a BH₄⁻ child (`* xyz -1 1`) that silently
 * becomes `* xyz 0 1` terminates normally and is garbage. We extract (c, m) from
 * the source via the existing parser, propagate them, and assert them back out of
 * the emitted child — refusing to emit a wrong-charge input.
 *
 * Pure / React-free. This emits a PROPOSAL input by REUSING the existing
 * scene→input machinery (`sceneFromOrcaInput`, `sceneFromAtomLines`,
 * `buildOrcaInput`), NOT an order-bearing scan-coordinate golden pair — so there
 * is deliberately NO `orcastudio-core` Rust mirror (unlike `emit_scan_block`). No
 * byte-identical cross-language guarantee is owed for a re-opt proposal.
 */

import {
  DEFAULT_BUILDER_STATE,
  buildOrcaInput,
  type BuilderState,
} from "../input-builder/build-input";
import {
  normalizeElement,
  sceneFromAtomLines,
  sceneFromOrcaInput,
  totalCharge,
} from "./scene";
import type { Conformer } from "./ensemble";

/** The defensible default: a composite method that needs no basis/dispersion pairing. */
export const DEFAULT_REOPT_METHOD = "r2SCAN-3c";

export interface ReoptOptions {
  /** DFT method keyword (composite). Defaults to {@link DEFAULT_REOPT_METHOD}. */
  method?: string;
  /** Emit `Freq` (the defensible ΔG path). Defaults to `true`. */
  freq?: boolean;
  /** Implicit solvation. Only SMD is offered here (the ionic case needs it). */
  solvation?: { model: "smd"; solvent: string };
}

/**
 * Build the ORCA input for ONE DFT re-opt child of a GOAT conformer.
 *
 * `charge`/`multiplicity` are inherited from `sourceInputText`'s `* xyz` line via
 * {@link sceneFromOrcaInput} (the existing parser — NOT a new charge regex, NOT a
 * default). A source with no inline `* xyz` block (including the `* xyzfile` form)
 * throws — we never fall back to charge 0. The conformer's own atoms (its order)
 * carry the geometry.
 *
 * @throws if the source has no inline coordinate block, the conformer has no
 * atoms, or the emitted child's (charge, multiplicity) does not match the source.
 */
export function buildReoptInput(
  sourceInputText: string,
  conformer: Conformer,
  opts: ReoptOptions = {},
): string {
  // 1. Inherit (charge, mult) from the SOURCE — the existing parser, not a regex.
  //    `null` means no inline `* xyz` block (or `* xyzfile`); we refuse rather than
  //    default charge to 0 (the footgun). NOTE: `sceneFromOrcaInput` defaults to
  //    (0, 1) only when a `* xyz` header is present but its tokens aren't integers;
  //    a GOAT source (`goatInputForFragment`) always writes integer `<c> <m>`, so
  //    that degenerate case doesn't arise for our sources.
  const sourceScene = sceneFromOrcaInput(sourceInputText);
  if (!sourceScene) {
    throw new Error(
      "re-opt: source input has no inline `* xyz` coordinate block — cannot inherit charge/multiplicity",
    );
  }
  const charge = totalCharge(sourceScene);
  const multiplicity = sourceScene.multiplicity;

  // 2. Fresh single-fragment scene from THIS conformer, carrying the inherited
  //    (charge, mult). Reuses the atom-line parser; order is preserved.
  const atomLines = conformer.atoms.map(
    (a) => `${a.element} ${a.x} ${a.y} ${a.z}`,
  );
  const scene = sceneFromAtomLines(atomLines, {
    name: "conformer",
    charge,
    multiplicity,
    source: "editor",
  });
  if (!scene) {
    throw new Error("re-opt: conformer has no atoms to re-optimize");
  }

  // 3. Assemble via the existing builder — same `!`-line / SMD / `* xyz` emit as
  //    the New Job form. The scene OVERRIDES state.charge/state.multiplicity inside
  //    `buildOrcaInput`, so (c, m) come from the scene we just built.
  const freq = opts.freq ?? true;
  const state: BuilderState = {
    ...DEFAULT_BUILDER_STATE,
    useComposite: true,
    composite: opts.method ?? DEFAULT_REOPT_METHOD,
    jobType: freq ? "Opt Freq" : "Opt",
    solvationModel: opts.solvation ? "SMD" : "",
    solvent: opts.solvation?.solvent ?? DEFAULT_BUILDER_STATE.solvent,
    charge,
    multiplicity,
  };
  const input = buildOrcaInput(state, scene);

  // 4. Post-condition in OUR terms (rule #9): re-parse the EMITTED child and assert
  //    (a) its (charge, mult) equals the SOURCE's — a wrong-charge input terminates
  //    normally and is silently garbage; and (b) its atoms are exactly the
  //    conformer's, same count AND element order (the fan-out must re-optimize THIS
  //    conformer, not a reordered or truncated geometry). Either failure throws, so
  //    a bad child is never handed to the create boundary.
  const back = sceneFromOrcaInput(input);
  if (
    !back ||
    totalCharge(back) !== charge ||
    back.multiplicity !== multiplicity
  ) {
    const got = back ? `${totalCharge(back)} ${back.multiplicity}` : "unparseable";
    throw new Error(
      `re-opt: emitted child charge/multiplicity (${got}) does not match source (${charge} ${multiplicity}) — refusing to create a wrong-charge job`,
    );
  }
  const emitted = back.fragments[0]?.atoms ?? [];
  if (emitted.length !== conformer.atoms.length) {
    throw new Error(
      `re-opt: emitted child has ${emitted.length} atoms, conformer has ${conformer.atoms.length} — refusing to create a wrong-geometry job`,
    );
  }
  for (let i = 0; i < emitted.length; i++) {
    if (
      normalizeElement(emitted[i].element) !==
      normalizeElement(conformer.atoms[i].element)
    ) {
      throw new Error(
        `re-opt: emitted child atom ${i} is ${emitted[i].element}, conformer has ${conformer.atoms[i].element} — element order not preserved`,
      );
    }
  }

  return input;
}
