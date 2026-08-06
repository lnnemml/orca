import { describe, it, expect, beforeEach } from "vitest";

import { useSceneStore } from "../scene/store";
import { emptyLog } from "../scene/oplog";
import { testScene, type RawFragment } from "../scene/scene-test-util";
import { makeDragController, type WorldDelta } from "./fragment-drag";

// The pure drag controller (unit 3.1), driven against the REAL store — the same
// "simulate the interaction without jsdom" approach as `syncMonacoToScene` (2d).
// It proves the two ADR-010 ephemeral-layer contracts: ONE op on release with the
// SUMMED delta (c2), and the Scene UNTOUCHED during the drag (c3). The 3Dmol/mouse
// wiring lives in MoleculeViewer and is the manual gate (m1–m4).

beforeEach(() => useSceneStore.setState({ log: emptyLog(), scene: null }));
const get = () => useSceneStore.getState();

function frag(id: string, elements: string[]): RawFragment {
  return {
    id,
    name: id,
    charge: 0,
    source: "editor",
    atoms: elements.map((element, i) => ({ element, x: i, y: 0, z: 0 })),
  };
}

// A deterministic stand-in for 3Dmol's screenOffsetToModel: 1 px → 0.01 Å in the
// screen plane (z = 0). The real one is measured pixel-exact in `wiki/debugging/013`.
const fakeUnproject = (dxPx: number, dyPx: number): WorldDelta => [dxPx * 0.01, dyPx * 0.01, 0];

describe("fragment drag controller", () => {
  it("commits exactly ONE op with the SUMMED delta on release (c2)", () => {
    get().seedScene(testScene([frag("a", ["O", "H", "H"]), frag("b", ["N", "H", "H"])], 1), "library");
    const lenBefore = get().log.entries.length;
    let ephemeralCalls = 0;
    let commits = 0;

    const ctrl = makeDragController({
      unproject: fakeUnproject,
      showEphemeral: () => {
        ephemeralCalls++;
      },
      commit: (fid, d) => {
        commits++;
        get().translateFragment(fid, d[0], d[1], d[2]);
      },
      restore: () => {},
    });

    ctrl.begin("b", [100, 100]);
    ctrl.move([120, 100]); // three intermediate frames
    ctrl.move([140, 130]);
    ctrl.move([150, 160]); // net from grab: +50 px x, +60 px y
    ctrl.end([150, 160]);

    // Exactly one op appended — NOT one per move.
    expect(commits).toBe(1);
    expect(ephemeralCalls).toBe(3);
    expect(get().log.entries.length).toBe(lenBefore + 1);
    const op = get().log.entries[get().log.pointer].op;
    expect(op).toMatchObject({ type: "translate-fragment", fragmentId: "b" });
    // The committed delta is the TOTAL from the grab, not the last step:
    // unproject(150-100, 160-100) = (50*0.01, 60*0.01, 0).
    expect((op as { delta: number[] }).delta).toEqual([0.5, 0.6, 0]);
  });

  it("does NOT touch the Scene during the drag — only on release (c3)", () => {
    get().seedScene(testScene([frag("a", ["O", "H", "H"]), frag("b", ["N", "H"])], 1), "library");
    const pre = get().scene; // the frozen pre-drag snapshot
    const lenBefore = get().log.entries.length;

    const ctrl = makeDragController({
      unproject: fakeUnproject,
      showEphemeral: () => {}, // viewer-only overlay — must NOT write the store
      commit: (fid, d) => get().translateFragment(fid, d[0], d[1], d[2]),
      restore: () => {},
    });

    ctrl.begin("b", [100, 100]);
    ctrl.move([130, 100]);
    ctrl.move([160, 140]);
    // NO end() — the drag is still in flight.

    expect(get().scene).toBe(pre); // === the SAME frozen snapshot; geometry untouched
    expect(get().log.entries.length).toBe(lenBefore); // nothing logged mid-drag
  });

  it("a click (begin then end with no move) commits nothing", () => {
    get().seedScene(testScene([frag("a", ["O", "H"])], 1), "library");
    const lenBefore = get().log.entries.length;
    let restored = false;
    const ctrl = makeDragController({
      unproject: fakeUnproject,
      showEphemeral: () => {},
      commit: (fid, d) => get().translateFragment(fid, d[0], d[1], d[2]),
      restore: () => {
        restored = true;
      },
    });
    ctrl.begin("a", [100, 100]);
    ctrl.end([100, 100]); // released at the grab point — a click, not a drag
    expect(get().log.entries.length).toBe(lenBefore);
    expect(restored).toBe(true);
  });
});
