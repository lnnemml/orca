/**
 * NEB-TS input builder — the CREATE side (Phase 4.5 Stage E3a-1).
 *
 * A Nudged Elastic Band TS search finds a saddle from a **reactant + product pair**
 * when there is no clean 1-D scan coordinate (the concerted case OptTS's scan-max seed
 * cannot reach — `wiki/orca/optts.md`). ORCA interpolates a band of images between the
 * two endpoints and relaxes it to the minimum-energy path; the climbing image converges
 * on the TS. Validated on the real Menshutkin reactant+product (the E2 connectivity
 * endpoints): recovered the KNOWN saddle (N···C 2.353 / C···I 2.594) from the two ends
 * alone — `wiki/orca/neb.md`.
 *
 * MIRRORS `buildOptTSInput` (its sibling): inherit (charge, mult) from the reactant
 * input's `* xyz` line and ASSERT them back out (the charge footgun); method + solvation
 * default to the reactant's, verbatim via the shared `methodSolvationKeywords` reader
 * (comparability). Pure / React-free; REUSES `buildOrcaInput` (no golden pair — the NEB
 * input carries no atom-index-bearing directive; Fork 2 of ADR-016) and `finalGeometry
 * Xyz` for the product image (no second xyz builder).
 *
 * THE SAME-ORDER GUARD (the whole point): NEB interpolates image k of the reactant into
 * image k of the product atom-by-atom. If the two endpoints do not share atom order
 * (element sequence AND count) the interpolation crosses different atoms and the method
 * silently fails. So `buildNebInput` THROWS on any mismatch — it never emits a bad pair.
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
import { methodSolvationKeywords } from "../reactions/compare";
import { finalGeometryXyz } from "../export/exporters";

/** A geometry (the reactant/product optimized final geometry). Structurally the same
 * as `TsGuessGeometry` / the exporters' `Geometry`. */
export interface NebGeometry {
  elements: string[];
  xyz_angstrom: [number, number, number][];
}

export interface NebOptions {
  /** Interior images between the endpoints. Default 8 → 10 total (im0…im9). */
  nImages?: number;
  /** Override the method keyword(s). Defaults to the reactant's (verbatim). */
  method?: string;
  /** Override the solvation keyword (e.g. `SMD(DMF)`). Defaults to the reactant's. */
  solvation?: string;
}

/** Interior images ORCA interpolates; NImages 8 → 10 images (measured probe). */
export const DEFAULT_NIMAGES = 8;

/** An inline `SMD(...)`/`CPCM(...)` solvation keyword on the `!` line. */
function isSolvationToken(tok: string): boolean {
  return /^(SMD|CPCM)\(/i.test(tok);
}

/**
 * Build a NEB-TS input from a reactant + product pair. Returns the `.inp` text AND the
 * `product.xyz` content (a separate aux file the `%neb` block references by relative
 * path — written into the isolated job dir by `create_neb_job`).
 *
 * @throws if the reactant input has no inline `* xyz` block (never defaults charge to
 * 0); if reactant and product do NOT share atom order (element sequence AND count — the
 * NEB precondition); if a geometry's elements/coords lengths disagree; or if the emitted
 * input's (charge, mult) or atom count/order does not match the reactant.
 */
export function buildNebInput(
  reactantInput: string,
  reactantGeom: NebGeometry,
  productGeom: NebGeometry,
  options: NebOptions = {},
): { inp: string; productXyz: string } {
  // 1. Inherit (charge, mult) from the REACTANT — the existing parser, not a regex.
  //    `null` = no inline `* xyz` block; refuse rather than default charge to 0.
  const reactantScene = sceneFromOrcaInput(reactantInput);
  if (!reactantScene) {
    throw new Error(
      "NEB: reactant input has no inline `* xyz` coordinate block — cannot inherit charge/multiplicity",
    );
  }
  const charge = totalCharge(reactantScene);
  const multiplicity = reactantScene.multiplicity;

  // 2. THE SAME-ORDER GUARD — reactant and product must share element sequence AND
  //    count, or NEB interpolates the wrong atoms. Refuse honestly (the whole point).
  const re = reactantGeom.elements;
  const pr = productGeom.elements;
  if (re.length !== pr.length) {
    throw new Error(
      `NEB: reactant has ${re.length} atoms but product has ${pr.length} — the endpoints must share atom order`,
    );
  }
  for (let i = 0; i < re.length; i++) {
    if (normalizeElement(re[i]) !== normalizeElement(pr[i])) {
      throw new Error(
        `NEB: atom ${i} is ${re[i]} in the reactant but ${pr[i]} in the product — the endpoints must share atom order (element sequence)`,
      );
    }
  }

  // 3. Method + solvation DEFAULT to the reactant's, verbatim (comparability) — the same
  //    shared `!`-line reader the comparability guard uses, so the two cannot drift.
  const kws = methodSolvationKeywords(reactantInput);
  const sourceSolvation = kws.find(isSolvationToken) ?? "";
  const sourceMethod = kws.filter((t) => !isSolvationToken(t)).join(" ");
  const method = options.method ?? sourceMethod;
  const solvation = options.solvation ?? sourceSolvation;

  // 4. Fresh single-fragment scene from the REACTANT geometry, carrying (charge, mult).
  if (reactantGeom.elements.length !== reactantGeom.xyz_angstrom.length) {
    throw new Error(
      `NEB: reactant has ${reactantGeom.elements.length} elements but ${reactantGeom.xyz_angstrom.length} coordinate rows`,
    );
  }
  const atomLines = reactantGeom.elements.map((el, i) => {
    const [x, y, z] = reactantGeom.xyz_angstrom[i];
    return `${el} ${x} ${y} ${z}`;
  });
  const scene = sceneFromAtomLines(atomLines, {
    name: "neb-reactant",
    charge,
    multiplicity,
    source: "editor",
  });
  if (!scene) {
    throw new Error("NEB: reactant geometry has no atoms");
  }

  // 5. Emit via the existing builder — `! <method> <solvation> NEB-TS TightSCF`. The
  //    reactant's opt keyword cannot leak (buildOrcaInput builds `!` + body fresh).
  const nImages = options.nImages ?? DEFAULT_NIMAGES;
  const state: BuilderState = {
    ...DEFAULT_BUILDER_STATE,
    methodFamily: "composite",
    composite: [method, solvation].filter((t) => t.length > 0).join(" "),
    jobType: "NEB-TS",
    solvationModel: "", // carried inside the composite slot, verbatim
    scfConv: "TightSCF",
    charge,
    multiplicity,
  };
  const base = buildOrcaInput(state, scene);

  // Inject the `%neb` block right after `%maxcore`. MULTI-LINE — the exact block form
  // the probe that converged (24 iterations, recovered the known TS) used. The
  // single-line form is valid ORCA in principle but UNMEASURED; the project emits what
  // was measured to work, not what "should" (rule #10). Splice pattern as OptTS's `%geom`.
  const lines = base.split("\n");
  const maxcoreIdx = lines.findIndex((l) => l.startsWith("%maxcore"));
  const insertAt = maxcoreIdx >= 0 ? maxcoreIdx + 1 : 1;
  lines.splice(
    insertAt,
    0,
    "%neb",
    '  NEB_End_XYZFile "product.xyz"',
    `  NImages ${nImages}`,
    "end",
  );
  const inp = lines.join("\n");

  // 6. Post-condition in OUR terms (rule #9): re-parse the emitted input and assert (a)
  //    (charge, mult) equals the reactant's — the footgun — and (b) its atoms are exactly
  //    the reactant's, same count AND element order. Either failure throws before create.
  const back = sceneFromOrcaInput(inp);
  if (!back || totalCharge(back) !== charge || back.multiplicity !== multiplicity) {
    const got = back ? `${totalCharge(back)} ${back.multiplicity}` : "unparseable";
    throw new Error(
      `NEB: emitted input charge/multiplicity (${got}) does not match reactant (${charge} ${multiplicity}) — refusing to create a wrong-charge job`,
    );
  }
  const emitted = back.fragments[0]?.atoms ?? [];
  if (emitted.length !== reactantGeom.elements.length) {
    throw new Error(
      `NEB: emitted input has ${emitted.length} atoms, reactant has ${reactantGeom.elements.length} — refusing to create a wrong-geometry job`,
    );
  }
  for (let i = 0; i < emitted.length; i++) {
    if (normalizeElement(emitted[i].element) !== normalizeElement(reactantGeom.elements[i])) {
      throw new Error(
        `NEB: emitted input atom ${i} is ${emitted[i].element}, reactant has ${reactantGeom.elements[i]} — element order not preserved`,
      );
    }
  }

  // 7. The product image as an xyz string — REUSE finalGeometryXyz (no second builder;
  //    its atoms+2 post-condition is inherited). Same atom order (guarded in step 2).
  const productXyz = finalGeometryXyz(productGeom, "NEB product end image", null);

  return { inp, productXyz };
}
