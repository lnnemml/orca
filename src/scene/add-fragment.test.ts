import { beforeEach, describe, it, expect } from "vitest";

import { useSceneStore } from "./store";
import { placeFragment } from "./placement";
import { libraryFragmentToScene, FRAGMENT_LIBRARY } from "./fragment-library";
import {
  injectSceneIntoInput,
  mergeToAtomLines,
  sceneFromAtomLines,
  sceneFromOrcaInput,
  totalCharge,
  xyzMatchesScene,
} from "./scene";
import type { Scene, SceneFragment } from "./types";

beforeEach(() =>
  useSceneStore.setState({ scene: null, previous: null, resetNotice: null }),
);

const get = () => useSceneStore.getState();

const water = (): SceneFragment => ({
  id: "wat",
  name: "Water",
  charge: 0,
  source: "editor",
  atoms: [
    { element: "O", x: 0, y: 0, z: 0.11779 },
    { element: "H", x: 0, y: 0.755453, z: -0.471161 },
    { element: "H", x: 0, y: -0.755453, z: -0.471161 },
  ],
});

const bh4 = (): SceneFragment =>
  libraryFragmentToScene(FRAGMENT_LIBRARY.find((f) => f.key === "bh4-")!);

/** The exact "add a fragment" road NewJobScreen takes. */
function addFragmentToScene(fragment: SceneFragment) {
  const current = get().scene ?? { fragments: [], multiplicity: 1 };
  get().addFragment(placeFragment(current, fragment));
}

describe("add-fragment REGRESSION GUARD: the Scene→content→Scene round-trip", () => {
  // THE risk of the whole subsystem (task 2.5.0d-2b §1): adding a fragment makes
  // the scene multi-fragment; the Scene→content effect injects the merged block;
  // ~500 ms later the content→Scene effect re-parses it and asks
  // xyzMatchesScene. If ordering/formatting drift makes that comparison FALSE,
  // the scene silently collapses back to one fragment half a second after the
  // add — no error, just "the sidebar blinked and the fragments merged". This
  // test locks the round-trip. (No jsdom in the suite, so it drives the real
  // pure functions the effects call, not a rendered component + fake timers —
  // the comparison is exactly where the bug would live.)
  it("adding a 2nd fragment survives the round-trip without collapsing", () => {
    get().setScene({ fragments: [water()], multiplicity: 1 });
    addFragmentToScene(bh4());
    expect(get().scene!.fragments).toHaveLength(2);

    const two = get().scene!;
    // Effect A: Scene → content (into a realistic input with a `!` line).
    const content = injectSceneIntoInput("! r2SCAN-3c Opt\n", two);
    // Effect B: content → Scene decision.
    const parsed = sceneFromOrcaInput(content);
    expect(parsed).not.toBeNull();
    const stays = xyzMatchesScene(two, mergeToAtomLines(parsed!));
    expect(stays).toBe(true); // TRUE ⇒ effect B leaves the scene at 2 fragments
  });

  it("total charge shows through the merged header after the add (water + BH₄⁻ = −1)", () => {
    get().setScene({ fragments: [water()], multiplicity: 1 });
    addFragmentToScene(bh4());
    const content = injectSceneIntoInput("! r2SCAN-3c Opt\n", get().scene!);
    expect(content).toContain("* xyz -1 1");
  });
});

describe("add-fragment sources build the right fragment", () => {
  it("a reagent add carries fragment-library source + charge, placed clear", () => {
    get().setScene({ fragments: [water()], multiplicity: 1 });
    addFragmentToScene(bh4());
    const added = get().scene!.fragments[1];
    expect(added.source).toBe("fragment-library");
    expect(added.sourceLabel).toBe("bh4-");
    expect(added.charge).toBe(-1);
    // placeFragment kept it clear of the water (≥ default gap 3.5 Å).
    const w = get().scene!.fragments[0].atoms;
    let min = Infinity;
    for (const p of w)
      for (const q of added.atoms)
        min = Math.min(min, Math.hypot(p.x - q.x, p.y - q.y, p.z - q.z));
    expect(min).toBeGreaterThanOrEqual(3.5 - 1e-9);
  });

  it("an import add carries the import source (via sceneFromAtomLines)", () => {
    const s = sceneFromAtomLines(["N 0 0 0", "H 0 0 1"], {
      source: "import",
      sourceLabel: "amine.xyz",
      charge: 0,
    })!;
    get().setScene({ fragments: [water()], multiplicity: 1 });
    addFragmentToScene(s.fragments[0]);
    expect(get().scene!.fragments[1].source).toBe("import");
    expect(get().scene!.fragments[1].sourceLabel).toBe("amine.xyz");
  });

  it("the first fragment takes the same road (empty scene → one fragment)", () => {
    expect(get().scene).toBeNull();
    addFragmentToScene(bh4());
    expect(get().scene!.fragments).toHaveLength(1);
    expect(totalCharge(get().scene!)).toBe(-1);
  });
});

describe("remove / rename / undo through the store", () => {
  it("removing fragment 0 leaves a valid single-fragment scene", () => {
    const two: Scene = { fragments: [water(), bh4()], multiplicity: 1 };
    get().setScene(two);
    get().removeFragment("wat");
    expect(get().scene!.fragments).toHaveLength(1);
    expect(get().scene!.fragments[0].source).toBe("fragment-library");
  });

  it("renameFragment renames without touching geometry", () => {
    get().setScene({ fragments: [water()], multiplicity: 1 });
    get().renameFragment("wat", "Substrate");
    expect(get().scene!.fragments[0].name).toBe("Substrate");
    expect(get().scene!.fragments[0].atoms).toHaveLength(3);
  });

  it("undoReset restores both the scene and (via re-inject) its coordinates", () => {
    const two: Scene = { fragments: [water(), bh4()], multiplicity: 1 };
    get().setScene(two);
    // Simulate a manual coordinate edit collapsing the 2-fragment scene.
    get().collapseToSingleFragment(water().atoms);
    expect(get().resetNotice).toEqual({ fragmentCount: 2 });
    get().undoReset();
    expect(get().scene).toBe(two);
    // The restored scene re-injects its (two-fragment) coordinates into content.
    const content = injectSceneIntoInput("! HF\n", get().scene!);
    expect(sceneFromOrcaInput(content)).not.toBeNull();
    expect(get().resetNotice).toBeNull();
  });
});
