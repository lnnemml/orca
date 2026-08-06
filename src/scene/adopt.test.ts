import { beforeEach, describe, it, expect } from "vitest";

import { useSceneStore } from "./store";
import { emptyLog } from "./oplog";
import {
  adoptPreservesScene,
  injectSceneIntoInput,
  sceneFromOrcaInput,
} from "./scene";
import { testScene, type RawFragment } from "./scene-test-util";

// ── Fragment-merge bugfix (debugging/014): "Generate Input" must NOT collapse a
// multi-fragment scene into one "Molecule" fragment ──────────────────────────
// MEASURED culprit (a live WebKitGTK repro): build substrate + reagent (2 fragments),
// click "Generate Input" → the store scene became "1 fragment · Molecule" (History:
// "Adopt geometry from input text"). Root cause: `adoptWholeInput` unconditionally
// `text-adopt`ed the GENERATED text — same geometry, only new `!`/`%` lines — which
// `sceneFromOrcaInput` parses back as ONE fragment. Fix: preserve the Scene when the
// adopted geometry matches it; re-seed only on genuinely new geometry.

beforeEach(() => useSceneStore.setState({ log: emptyLog(), scene: null }));

function twoFragmentScene() {
  const substrate: RawFragment = {
    id: "sub",
    name: "Dexketoprofen",
    charge: 0,
    source: "editor",
    atoms: [
      { element: "C", x: 0, y: 0, z: 0 },
      { element: "O", x: 1.2, y: 0, z: 0 },
      { element: "H", x: -0.6, y: 0.9, z: 0 },
    ],
  };
  const reagent: RawFragment = {
    id: "bh4",
    name: "BH4",
    charge: -1,
    source: "fragment-library",
    atoms: [
      { element: "B", x: 5, y: 0, z: 0 },
      { element: "H", x: 5.6, y: 0.6, z: 0.6 },
      { element: "H", x: 4.4, y: -0.6, z: 0.6 },
    ],
  };
  return testScene([substrate, reagent], 1);
}

describe("adoptPreservesScene (the guard)", () => {
  it("PRESERVES a multi-fragment scene when the adopted geometry matches (Generate Input)", () => {
    const scene = twoFragmentScene();
    // "Generate Input": new !/% keyword lines, SAME coordinate block.
    const generated = injectSceneIntoInput("! r2SCAN-3c Opt Freq TightSCF\n%pal nprocs 4 end\n", scene);
    expect(adoptPreservesScene(scene, generated)).toBe(true);
  });

  it("re-adopts (false) on a genuinely different geometry, or no scene, or no block", () => {
    const scene = twoFragmentScene();
    const differentGeom = "! HF def2-SVP\n* xyz 0 1\nO 0 0 0\nH 0 0 1\n*\n";
    expect(adoptPreservesScene(scene, differentGeom)).toBe(false); // Replace input: a new molecule
    expect(adoptPreservesScene(null, injectSceneIntoInput("! HF\n", scene))).toBe(false); // no current scene
    expect(adoptPreservesScene(scene, "! HF def2-SVP\n")).toBe(false); // no coordinate block → parse null
  });
});

describe("the store-level regression guard (2 fragments survive Generate Input)", () => {
  it("guarded adopt keeps 2 fragments; a blind text-adopt collapses to 1 'Molecule'", () => {
    const scene = twoFragmentScene();
    useSceneStore.getState().seedScene(scene, "library");
    expect(useSceneStore.getState().scene!.fragments.length).toBe(2);

    const generated = injectSceneIntoInput("! r2SCAN-3c Opt\n%pal nprocs 4 end\n", scene);

    // The FIXED path (mirrors `adoptWholeInput`): preserve when geometry matches.
    if (!adoptPreservesScene(useSceneStore.getState().scene, generated)) {
      useSceneStore.getState().seedScene(sceneFromOrcaInput(generated), "text-adopt");
    }
    expect(useSceneStore.getState().scene!.fragments.length).toBe(2); // ← preserved (guard bites)
    expect(useSceneStore.getState().scene!.fragments.map((f) => f.name)).toEqual([
      "Dexketoprofen",
      "BH4",
    ]);

    // Negative control — the OLD (blind) behaviour, the measured bug: a straight
    // text-adopt of the same generated content collapses to ONE "Molecule" fragment.
    useSceneStore.getState().seedScene(sceneFromOrcaInput(generated), "text-adopt");
    const collapsed = useSceneStore.getState().scene!;
    expect(collapsed.fragments.length).toBe(1);
    expect(collapsed.fragments[0].name).toBe("Molecule");
  });
});
