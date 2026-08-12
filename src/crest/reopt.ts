//! CREST microsolvation seed → ORCA re-opt (Stage F F2) — the payoff. The grown cluster is
//! a GEOMETRY SEED (grown NEUTRAL at the xtb-ALPB level for an ion); this turns it into an
//! ORCA `Opt Freq` input **at the SOLUTE's charge with SMD** — the first point the cluster
//! becomes a defensible solvated number. Pure / React-free.
//!
//! **THE CHARGE FOOTGUN (rule #9), the twin of `scene/reopt.ts`.** There the (c, m) are
//! INHERITED from a source input; here they are passed **EXPLICITLY** — the solute's charge
//! (from the CREST launch, `result.intended_charge`), NEVER 0 and NEVER read from the
//! neutral-grown cluster. The cluster carries only geometry. The emitted `* xyz <charge>
//! <mult>` is asserted back out; a wrong-charge input terminates normally and is garbage.
//!
//! REUSES the scene→input machinery (`sceneFromAtomLines`, `buildOrcaInput`) exactly like
//! `scene/reopt.ts` — a re-opt PROPOSAL, not an order-bearing golden pair, so no Rust mirror.

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
} from "../scene/scene";
import { DEFAULT_REOPT_METHOD } from "../scene/reopt";

export interface ClusterReoptOpts {
  /** The ALPB/SMD solvent — emitted as the verified `SMD(<solvent>)` keyword (solvation.md). */
  solvent: string;
  /** DFT method keyword (composite). Defaults to {@link DEFAULT_REOPT_METHOD}. */
  method?: string;
  /** Emit `Freq` (the defensible ΔG path). Defaults to `true`. */
  freq?: boolean;
}

/**
 * Build the ORCA re-opt input for a grown microsolvation cluster.
 *
 * `charge`/`multiplicity` are the **solute's**, passed EXPLICITLY — never inherited, never a
 * default, never read from the cluster (which was grown NEUTRAL). The cluster's atoms (its
 * order) carry the geometry. Emits `Opt Freq` + `SMD(<solvent>)` + the method, with a `#`
 * provenance header (metadata only; the `!` line + `* xyz <c> <m>` are authoritative).
 *
 * @throws if the cluster has no atoms / mismatched element+coord lengths, or the emitted
 * input's (charge, mult) or atom count/order does not match what was passed.
 */
export function buildClusterReoptInput(
  cluster: { elements: string[]; xyz_angstrom: [number, number, number][] },
  charge: number,
  multiplicity: number,
  opts: ClusterReoptOpts,
): string {
  if (cluster.elements.length === 0) {
    throw new Error("cluster re-opt: the grown cluster has no atoms to re-optimize");
  }
  if (cluster.elements.length !== cluster.xyz_angstrom.length) {
    throw new Error(
      `cluster re-opt: ${cluster.elements.length} elements but ${cluster.xyz_angstrom.length} coordinate rows`,
    );
  }

  // Fresh single-fragment scene from the cluster atoms (order preserved), carrying the
  // EXPLICIT solute (charge, multiplicity) — the scene overrides state.charge/mult inside
  // buildOrcaInput, so the emitted header is exactly these.
  const atomLines = cluster.elements.map((el, i) => {
    const [x, y, z] = cluster.xyz_angstrom[i];
    return `${el} ${x} ${y} ${z}`;
  });
  const scene = sceneFromAtomLines(atomLines, {
    name: "microsolvation-cluster",
    charge,
    multiplicity,
    source: "editor",
  });
  if (!scene) {
    throw new Error("cluster re-opt: the grown cluster has no atoms to re-optimize");
  }

  const method = opts.method ?? DEFAULT_REOPT_METHOD;
  const freq = opts.freq ?? true;
  const state: BuilderState = {
    ...DEFAULT_BUILDER_STATE,
    methodFamily: "composite",
    composite: method,
    jobType: freq ? "Opt Freq" : "Opt",
    // SMD is what turns the xtb-ALPB seed into a defensible solvated calculation (the ionic
    // case needs SMD, not ALPB — solvation.md / ADR-018). Always emitted here.
    solvationModel: "SMD",
    solvent: opts.solvent,
    charge,
    multiplicity,
  };
  const body = buildOrcaInput(state, scene);

  // Post-condition (rule #9): re-parse and assert (c, m) == the passed solute values, and the
  // atoms are exactly the cluster's (count AND element order) — a wrong-charge/geometry input
  // must never reach the editor.
  const back = sceneFromOrcaInput(body);
  if (!back || totalCharge(back) !== charge || back.multiplicity !== multiplicity) {
    const got = back ? `${totalCharge(back)} ${back.multiplicity}` : "unparseable";
    throw new Error(
      `cluster re-opt: emitted (charge, mult) (${got}) does not match the solute (${charge} ${multiplicity}) — refusing a wrong-charge job`,
    );
  }
  const emitted = back.fragments[0]?.atoms ?? [];
  if (emitted.length !== cluster.elements.length) {
    throw new Error(
      `cluster re-opt: emitted ${emitted.length} atoms, cluster has ${cluster.elements.length}`,
    );
  }
  for (let i = 0; i < emitted.length; i++) {
    if (normalizeElement(emitted[i].element) !== normalizeElement(cluster.elements[i])) {
      throw new Error(
        `cluster re-opt: emitted atom ${i} is ${emitted[i].element}, cluster has ${cluster.elements[i]} — element order not preserved`,
      );
    }
  }

  // Provenance header — ORCA `#` comment lines (metadata only; K3 — no migration, no column,
  // no jobs row; the provenance rides the input text + the job title). Names the solvent, the
  // solute charge, and that this refines a neutral-grown QCG seed at the correct charge.
  const chargeStr = charge >= 0 ? `+${charge}` : `${charge}`;
  const header = [
    "# Microsolvation re-optimization (CREST/QCG seed -> ORCA)",
    `# Solute charge: ${chargeStr}  ·  solvent: SMD(${opts.solvent})`,
    "# The cluster geometry is a SEED from a QCG grow — grown NEUTRAL at the xtb-ALPB level.",
    `# This input refines it at the correct charge (${chargeStr}) with SMD — the defensible solvated result.`,
  ].join("\n");

  return `${header}\n${body}`;
}
