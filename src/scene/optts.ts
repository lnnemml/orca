/**
 * Source-agnostic OptTS refinement — the CREATE side (Phase 4.5 Stage E1a, ADR-020).
 *
 * OptTS is a **downstream refinement of a TS-guess geometry from ANY source**: a relaxed-scan
 * maximum today, a NEB-CI / NEB-TS climbing image in Stage E3, a hand guess later. This engine
 * (`buildOptTSInput` + the Rust `create_optts_job`) takes a **generic** seed geometry + a **context
 * input** (the source job's `!` line + `* xyz`); each source has its OWN entry-point UI that extracts
 * its guess and calls this one engine. Nothing here mentions "scan" — the scan is only today's caller.
 *
 * TWO risks, both rule #9:
 *  1. **THE CHARGE FOOTGUN** — (charge, mult) live ONLY in the source input's `* xyz <c> <m>` line.
 *     The child MUST inherit them and we ASSERT them back out of the emitted input (a wrong-charge
 *     OptTS terminates normally and is garbage). Same discipline as `reopt.ts`.
 *  2. **COMPARABILITY** — a TS is only meaningful on the SAME method + solvation as the guess's
 *     source. So method + solvation DEFAULT to the source's, extracted verbatim via
 *     `methodSolvationKeywords` (the shared `!`-line reader, NOT a new regex). The source's opt
 *     keyword (e.g. the scan's `LooseOpt`) and its `Scan`/`Constraints` `%geom` block must NOT leak —
 *     `buildOrcaInput` builds the `!` line + body FRESH from `BuilderState` + the seed, so they can't.
 *
 * Pure / React-free. REUSES the scene→input machinery (`sceneFromOrcaInput`, `sceneFromAtomLines`,
 * `buildOrcaInput`), NOT an order-bearing golden pair — no atom-index-bearing directive is emitted
 * (`Calc_Hess true` is a fixed flag), so there is deliberately NO `orcastudio-core` Rust mirror
 * (Fork 2 of ADR-016; `wiki/orca/optts.md`).
 */

import {
  DEFAULT_BUILDER_STATE,
  buildOrcaInput,
  type BuilderState,
  type MethodSlice,
} from "../input-builder/build-input";
import {
  normalizeElement,
  sceneFromAtomLines,
  sceneFromOrcaInput,
  totalCharge,
} from "./scene";
import { methodSolvationKeywords } from "../reactions/compare";

/** A TS-guess geometry from any source (a scan maximum, a NEB climbing image, a hand guess).
 * Deliberately NOT `ScanGeometry` — the engine is source-agnostic. */
export interface TsGuessGeometry {
  elements: string[];
  xyz_angstrom: [number, number, number][];
}

export interface OptTSOptions {
  /** Override the method keyword(s). Defaults to the source's (extracted verbatim). */
  method?: string;
  /** Override the solvation keyword (e.g. `SMD(DMF)`). Defaults to the source's. */
  solvation?: string;
  /**
   * Override the method via the **family model** (from `<MethodPicker>`), NOT a flattened
   * keyword string. When present, the child `!` line is built from this slice through
   * `buildOrcaInput`'s family logic — so a DFT override carries its functional + basis +
   * **paired** RI aux + dispersion, a composite carries no basis/RI, xtb suppresses the
   * solvation/SCF tail. Solvation still DEFAULTS to the source's (comparability), carried
   * as model+solvent so the per-family solvation rule applies. Absent → the current
   * inherit/composite-string path is used, byte-identical (no regression). Takes
   * precedence over {@link OptTSOptions.method} when both are set.
   */
  methodState?: MethodSlice;
}

/** An inline `SMD(...)`/`CPCM(...)` solvation keyword on the `!` line. */
function isSolvationToken(tok: string): boolean {
  return /^(SMD|CPCM)\(/i.test(tok);
}

/**
 * Whether the source job's method is semi-empirical (GFN2-xTB) — read from the shared
 * {@link methodSolvationKeywords} `!`-line reader, NOT a bespoke regex. Used ONLY to show an
 * inline note nudging the researcher to a DFT level before an OptTS refine (a semi-empirical
 * OptTS isn't publication-grade). This is a UI hint, never a method decision — it does not
 * touch the emit, and the default remains the byte-identical inherit path.
 */
export function sourceMethodIsXtb(sourceInput: string): boolean {
  return methodSolvationKeywords(sourceInput).some((t) => /^(XTB|GFN)/i.test(t));
}

/**
 * Split an inline solvation keyword (`SMD(DMF)`, `CPCM(water)`) into the `{model, solvent}`
 * pair `buildKeywordLine` reassembles as `${model}(${solvent})` — so an inherited solvation
 * flows through the family logic (emitted for dft/composite/wavefunction, suppressed for xtb).
 * This is a SOLVATION-token split, NOT a reverse-parse of the method family. A non-matching or
 * empty token yields `{model:"", solvent:""}` → no solvation emitted (`solvationModel` empty).
 */
function splitSolvation(token: string): { solvationModel: string; solvent: string } {
  const m = /^(SMD|CPCM)\((.+)\)$/i.exec(token.trim());
  return m
    ? { solvationModel: m[1], solvent: m[2] }
    : { solvationModel: "", solvent: "" };
}

/**
 * Build the ORCA input for an OptTS refinement of a TS-guess geometry.
 *
 * `charge`/`multiplicity` are inherited from `sourceInput`'s `* xyz` line via
 * {@link sceneFromOrcaInput} (the existing parser — NOT a regex, NOT a default) and asserted back out
 * of the emitted child. `method`/`solvation` default to the source's (extracted verbatim via
 * {@link methodSolvationKeywords}) so the TS is comparable to the guess's surface by construction.
 * Emits `! <method> <solvation> OptTS Freq TightSCF` + `%geom Calc_Hess true end` (keyword order is
 * immaterial to ORCA; the recipe is `wiki/orca/optts.md`).
 *
 * @throws if the source has no inline `* xyz` block (never defaults to charge 0), the seed has no
 * atoms or mismatched element/coordinate lengths, or the emitted child's (charge, mult) or
 * atom count/order does not match — a bad child never reaches the create boundary.
 */
export function buildOptTSInput(
  sourceInput: string,
  seedGeometry: TsGuessGeometry,
  options: OptTSOptions = {},
): string {
  // 1. Inherit (charge, mult) from the SOURCE — the existing parser, not a regex. `null` = no inline
  //    `* xyz` block (or `* xyzfile`); refuse rather than default charge to 0 (the footgun).
  const sourceScene = sceneFromOrcaInput(sourceInput);
  if (!sourceScene) {
    throw new Error(
      "OptTS: source input has no inline `* xyz` coordinate block — cannot inherit charge/multiplicity",
    );
  }
  const charge = totalCharge(sourceScene);
  const multiplicity = sourceScene.multiplicity;

  // 2. Method + solvation DEFAULT to the source's, verbatim (comparability). One shared `!`-line
  //    reader with the comparability guard — "what is method" is defined once (`compare.ts`).
  const kws = methodSolvationKeywords(sourceInput);
  const sourceSolvation = kws.find(isSolvationToken) ?? "";
  const sourceMethod = kws.filter((t) => !isSolvationToken(t)).join(" ");
  const method = options.method ?? sourceMethod;
  const solvation = options.solvation ?? sourceSolvation;

  // 3. Fresh single-fragment scene from the SEED, carrying the inherited (charge, mult).
  const { elements, xyz_angstrom } = seedGeometry;
  if (elements.length !== xyz_angstrom.length) {
    throw new Error(
      `OptTS: seed has ${elements.length} elements but ${xyz_angstrom.length} coordinate rows`,
    );
  }
  const atomLines = elements.map((el, i) => {
    const [x, y, z] = xyz_angstrom[i];
    return `${el} ${x} ${y} ${z}`;
  });
  const scene = sceneFromAtomLines(atomLines, {
    name: "ts-guess",
    charge,
    multiplicity,
    source: "editor",
  });
  if (!scene) {
    throw new Error("OptTS: seed geometry has no atoms to refine");
  }

  // 4. Emit via the existing builder. Two paths, one boundary:
  //    (a) methodState present (a `<MethodPicker>` override): build the child `!` line from the
  //        family MODEL — spread the method slice into the OptTS state so buildOrcaInput applies
  //        the family logic (a dft override keeps its functional + basis + PAIRED RI aux +
  //        dispersion; NEVER flattened into a string, which would drop the aux). Solvation still
  //        DEFAULTS to the source's, carried as model+solvent so the per-family rule applies
  //        (emitted for dft/composite/wf, suppressed for xtb).
  //    (b) methodState absent (inherit): the ORIGINAL path — method + solvation carried verbatim
  //        in the composite slot. Byte-identical to before; no regression for the default.
  //    Either way jobType = `OptTS Freq`, TightSCF, and the Scan/opt keyword of the source cannot
  //    leak — buildOrcaInput builds `!` + body fresh from THIS state + scene.
  const state: BuilderState = options.methodState
    ? {
        ...DEFAULT_BUILDER_STATE,
        ...options.methodState,
        jobType: "OptTS Freq",
        scfConv: "TightSCF",
        ...splitSolvation(solvation),
        charge,
        multiplicity,
      }
    : {
        ...DEFAULT_BUILDER_STATE,
        methodFamily: "composite",
        composite: [method, solvation].filter((t) => t.length > 0).join(" "),
        jobType: "OptTS Freq",
        solvationModel: "", // solvation is carried inside the composite slot, verbatim
        scfConv: "TightSCF",
        charge,
        multiplicity,
      };
  const base = buildOrcaInput(state, scene);

  // Inject the TS-guess Hessian directive right after `%maxcore` (a fixed flag — Calc_Hess true
  // alone sufficed on the clean 1-D coordinate, no Recalc_Hess; measured, `wiki/orca/optts.md`).
  const lines = base.split("\n");
  const maxcoreIdx = lines.findIndex((l) => l.startsWith("%maxcore"));
  const insertAt = maxcoreIdx >= 0 ? maxcoreIdx + 1 : 1;
  lines.splice(insertAt, 0, "%geom Calc_Hess true end");
  const input = lines.join("\n");

  // 5. Post-condition in OUR terms (rule #9): re-parse the EMITTED child and assert (a) its
  //    (charge, mult) equals the SOURCE's — the footgun — and (b) its atoms are exactly the seed's,
  //    same count AND element order. Either failure throws before the create boundary.
  const back = sceneFromOrcaInput(input);
  if (!back || totalCharge(back) !== charge || back.multiplicity !== multiplicity) {
    const got = back ? `${totalCharge(back)} ${back.multiplicity}` : "unparseable";
    throw new Error(
      `OptTS: emitted child charge/multiplicity (${got}) does not match source (${charge} ${multiplicity}) — refusing to create a wrong-charge job`,
    );
  }
  const emitted = back.fragments[0]?.atoms ?? [];
  if (emitted.length !== elements.length) {
    throw new Error(
      `OptTS: emitted child has ${emitted.length} atoms, seed has ${elements.length} — refusing to create a wrong-geometry job`,
    );
  }
  for (let i = 0; i < emitted.length; i++) {
    if (normalizeElement(emitted[i].element) !== normalizeElement(elements[i])) {
      throw new Error(
        `OptTS: emitted child atom ${i} is ${emitted[i].element}, seed has ${elements[i]} — element order not preserved`,
      );
    }
  }

  return input;
}
