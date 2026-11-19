import { afterEach, describe, expect, it } from "vitest";

import { buildReoptInput } from "../scene/reopt";
import { buildOptTSInput } from "../scene/optts";
import { buildConnectivityChildren } from "../scene/connectivity";
import { buildClusterReoptInput } from "../crest/reopt";
import { DEFAULT_BUILDER_STATE } from "../input-builder/build-input";
import { FALLBACK_LOCAL_MAXCORE_MIB, localMaxcore, reloadLocalMaxcore, setLocalMaxcore } from "./local-maxcore";
import type { LocalFittedMaxcore } from "../types";

// ADR-024 o26.13.4: every derived local fan-out takes its %maxcore from the accessor, never from
// DEFAULT_BUILDER_STATE. 1600 is the laptop's P at 8 ranks (wiki/orca/memory.md).
const FITTED = 1600;

const SOURCE = "! r2SCAN-3c Opt\n* xyz 0 1\nN 0 0 0\nC 0 0 1.8\n*\n";
const SEED = { elements: ["N", "C"], xyz_angstrom: [[0, 0, 0], [0, 0, 2.353]] as [number, number, number][] };
const CONFORMER = {
  atoms: [
    { element: "N", x: 0, y: 0, z: 0 },
    { element: "C", x: 0, y: 0, z: 1.8 },
  ],
  energy: -1,
  index: 0,
};
const TS_INPUT = "! r2SCAN-3c OptTS Freq\n* xyz 0 1\nN 0 0 0\nC 2 0 0\n*\n";
const TS_GEOMETRY = { elements: ["N", "C"], xyz_angstrom: [[0, 0, 0], [2, 0, 0]] as [number, number, number][] };

const maxcoreLines = (input: string) => input.split("\n").filter((l) => l.startsWith("%maxcore"));

afterEach(() => setLocalMaxcore(null));

describe("the fan-out accessor (o26.13.4)", () => {
  it("is 2000 before it loads, the loaded value after, and keeps its value on a failed load", async () => {
    expect(localMaxcore()).toBe(FALLBACK_LOCAL_MAXCORE_MIB);
    expect(FALLBACK_LOCAL_MAXCORE_MIB).toBe(DEFAULT_BUILDER_STATE.maxcore);
    const reply: LocalFittedMaxcore = {
      ranks: 8,
      mem_total_kib: { state: "known", value: 15988408 },
      proposal_mib: 1600,
      default_maxcore_mib: 1600,
    };
    await reloadLocalMaxcore(async () => reply);
    expect(localMaxcore()).toBe(1600);
    await reloadLocalMaxcore(async () => {
      throw new Error("ipc down");
    });
    expect(localMaxcore()).toBe(1600);
  });

  // CONTROL: any one builder keeping DEFAULT_BUILDER_STATE.maxcore (its `maxcore: localMaxcore()`
  // removed) emits %maxcore 2000 → red.
  it("every fan-out builder emits the accessor's value", () => {
    setLocalMaxcore(FITTED);
    const emitted: Record<string, string> = {
      "scene/reopt (conformer re-opt)": buildReoptInput(SOURCE, CONFORMER),
      "scene/optts (inherit: scan/NEB refines)": buildOptTSInput(SOURCE, SEED),
      "scene/optts (methodState override)": buildOptTSInput(SOURCE, SEED, {
        methodState: {
          methodFamily: "dft",
          composite: DEFAULT_BUILDER_STATE.composite,
          functional: "B3LYP",
          basis: "def2-SVP",
          dispersion: DEFAULT_BUILDER_STATE.dispersion,
          ri: DEFAULT_BUILDER_STATE.ri,
          xtbMethod: DEFAULT_BUILDER_STATE.xtbMethod,
          wavefunction: DEFAULT_BUILDER_STATE.wavefunction,
        },
      }),
      "scene/connectivity forward": buildConnectivityChildren(TS_INPUT, TS_GEOMETRY, [0, 0, 0, 1, 0, 0], 0.1).forwardInput,
      "scene/connectivity backward": buildConnectivityChildren(TS_INPUT, TS_GEOMETRY, [0, 0, 0, 1, 0, 0], 0.1).backwardInput,
      "crest/reopt (cluster)": buildClusterReoptInput(SEED, 0, 1, { solvent: "water" }),
    };
    for (const [name, input] of Object.entries(emitted)) {
      expect(maxcoreLines(input), name).toEqual([`%maxcore ${FITTED}`]);
    }
  });
});
