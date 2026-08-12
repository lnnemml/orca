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
 * MIRRORS `buildOptTSInput` (its sibling) for the CHARGE FOOTGUN: inherit (charge, mult)
 * from the reactant input's `* xyz` line and ASSERT them back out. But the METHOD now
 * comes from the **builder's `BuilderState`** (family-aware — composite / DFT / DLPNO /
 * xtb), NOT from inheriting the reactant's `!` line: N2 moved NEB creation into the Input
 * Builder, so the method/basis/solvation the user picked there drives the NEB level (this
 * is what enables NEB-on-xtb from DFT-optimized endpoints). Pure / React-free; REUSES
 * `buildOrcaInput` (family-aware emit; no golden pair — the NEB input carries no atom-
 * index-bearing directive; Fork 2 of ADR-016) and `finalGeometryXyz` for the product image.
 *
 * THE SAME-ORDER GUARD (the whole point): NEB interpolates image k of the reactant into
 * image k of the product atom-by-atom. If the two endpoints do not share atom order
 * (element sequence AND count) the interpolation crosses different atoms and the method
 * silently fails. So `buildNebInput` THROWS on any mismatch — it never emits a bad pair.
 */

import {
  buildOrcaInput,
  type BuilderState,
} from "../input-builder/build-input";
import {
  normalizeElement,
  sceneFromAtomLines,
  sceneFromOrcaInput,
  totalCharge,
} from "./scene";
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
}

/** Interior images ORCA interpolates; NImages 8 → 10 images (measured probe). */
export const DEFAULT_NIMAGES = 8;

/**
 * True when an ORCA input's `!` line carries a `NEB` token (case-insensitive) — mirrors
 * Rust `input_has_neb`. `NewJobScreen.create()` uses it to gate the `create_neb_job`
 * branch, so a stale NEB payload cannot create a NEB job once the buffer holds a non-NEB
 * input.
 */
export function hasNebKeyword(content: string): boolean {
  return content
    .split(/\r?\n/)
    .filter((l) => l.trim().startsWith("!"))
    .some((l) => l.split(/\s+/).some((t) => /neb/i.test(t)));
}

/**
 * Build a NEB-TS input from a reactant + product pair, at the level set by `state` (the
 * builder's method/family/basis/solvation/SCF). Returns the `.inp` text AND the
 * `product.xyz` content (a separate aux file the `%neb` block references by relative
 * path — written into the isolated job dir by `create_neb_job`).
 *
 * The METHOD is the builder's; the (charge, multiplicity) are the REACTANT's (the
 * footgun — never the builder's charge field, never 0).
 *
 * @throws if the reactant input has no inline `* xyz` block (never defaults charge to
 * 0); if reactant and product do NOT share atom order (element sequence AND count — the
 * NEB precondition); if a geometry's elements/coords lengths disagree; or if the emitted
 * input's (charge, mult) or atom count/order does not match the reactant.
 */
export function buildNebInput(
  state: BuilderState,
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

  // 3. Fresh single-fragment scene from the REACTANT geometry, carrying (charge, mult).
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

  // 4. Emit via the family-aware builder at the BUILDER's level — the method/family/basis/
  //    solvation/SCF come from `state`; only jobType is forced to NEB-TS and (charge, mult)
  //    come from the reactant (the footgun). xtb → `! XTB NEB-TS` (no solvation/SCFConv);
  //    DFT → `! <func> <basis> … NEB-TS <scf>`; DLPNO → the /C aux line. The reactant's own
  //    `!` line does NOT leak (buildOrcaInput builds `!` + body fresh from this state+scene).
  const nImages = options.nImages ?? DEFAULT_NIMAGES;
  const emitState: BuilderState = {
    ...state,
    jobType: "NEB-TS",
    charge,
    multiplicity,
  };
  const base = buildOrcaInput(emitState, scene);

  // Inject the `%neb` block right before the `* xyz` coordinate block. MULTI-LINE — the
  // exact block form the probe that converged (24 iterations, recovered the known TS) used.
  // The single-line form is valid ORCA in principle but UNMEASURED; the project emits what
  // was measured to work, not what "should" (rule #10). Anchor on `* xyz` (family-
  // independent, ALWAYS present) rather than `%maxcore` — an emit that ever dropped the
  // `%` directives (e.g. a future self-contained family) must not silently lose `%neb`.
  const lines = base.split("\n");
  const xyzIdx = lines.findIndex((l) => l.trim().startsWith("* xyz"));
  const insertAt =
    xyzIdx >= 0
      ? xyzIdx
      : (() => {
          const m = lines.findIndex((l) => l.startsWith("%maxcore"));
          return m >= 0 ? m + 1 : 1;
        })();
  lines.splice(
    insertAt,
    0,
    "%neb",
    '  NEB_End_XYZFile "product.xyz"',
    `  NImages ${nImages}`,
    "end",
  );
  const inp = lines.join("\n");

  // 5. Post-condition in OUR terms (rule #9): re-parse the emitted input and assert (a)
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

  // 6. The product image as an xyz string — REUSE finalGeometryXyz (no second builder;
  //    its atoms+2 post-condition is inherited). Same atom order (guarded in step 2).
  const productXyz = finalGeometryXyz(productGeom, "NEB product end image", null);

  return { inp, productXyz };
}
