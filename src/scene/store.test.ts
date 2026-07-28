import { beforeEach, describe, it, expect } from "vitest";

import { useSceneStore } from "./store";
import { mergeToAtomLines, totalCharge, xyzMatchesScene } from "./scene";
import type { Scene, SceneAtom, SceneFragment } from "./types";

// Reset only the data fields between tests (merge, so the actions survive).
beforeEach(() =>
  useSceneStore.setState({ scene: null, previous: null, resetNotice: null }),
);

const get = () => useSceneStore.getState();

function frag(id: string, elements: string[], charge = 0): SceneFragment {
  return {
    id,
    name: id,
    charge,
    source: "editor",
    atoms: elements.map((element, i) => ({ element, x: i, y: 0, z: 0 })),
  };
}

function scene(multiplicity: number, ...fragments: SceneFragment[]): Scene {
  return { fragments, multiplicity };
}

describe("setScene + reference stability", () => {
  it("stores the exact object and clears any reset notice", () => {
    useSceneStore.setState({ resetNotice: { fragmentCount: 3 } });
    const s = scene(1, frag("a", ["O", "H", "H"]));
    get().setScene(s);
    expect(get().scene).toBe(s);
    expect(get().resetNotice).toBeNull();
  });

  it("returns the SAME object on repeated reads (viewer won't redraw on keystroke)", () => {
    const s = scene(1, frag("a", ["O", "H", "H"]));
    get().setScene(s);
    // `===`, not merely structurally equal — this identity is what keeps
    // MoleculeViewer's `useEffect([scene])` from firing when nothing changed.
    expect(get().scene).toBe(get().scene);
    expect(get().scene).toBe(s);
  });
});

describe("collapseToSingleFragment", () => {
  const atoms: SceneAtom[] = [
    { element: "O", x: 9, y: 0, z: 0 },
    { element: "H", x: 9, y: 1, z: 0 },
  ];

  it("keeps the previous scene for undo and collapses to one fragment", () => {
    const before = scene(1, frag("a", ["O", "H"]), frag("b", ["N"]));
    get().setScene(before);
    get().collapseToSingleFragment(atoms);
    expect(get().previous).toBe(before);
    expect(get().scene!.fragments).toHaveLength(1);
    expect(get().scene).not.toBe(before);
  });

  it("shows the notice only when >1 fragment was merged", () => {
    get().setScene(scene(1, frag("a", ["O"]), frag("b", ["H"]))); // 2 fragments
    get().collapseToSingleFragment(atoms);
    expect(get().resetNotice).toEqual({ fragmentCount: 2 });
  });

  it("does NOT show the notice for a single-fragment scene", () => {
    get().setScene(scene(1, frag("a", ["O", "H"]))); // 1 fragment
    get().collapseToSingleFragment(atoms);
    expect(get().resetNotice).toBeNull();
  });

  it("preserves total charge and multiplicity", () => {
    get().setScene(scene(3, frag("a", ["O"], -1), frag("b", ["H"], 0)));
    get().collapseToSingleFragment(atoms);
    expect(totalCharge(get().scene!)).toBe(-1);
    expect(get().scene!.multiplicity).toBe(3);
  });
});

describe("undoReset", () => {
  it("restores the previous scene and clears previous + notice", () => {
    const before = scene(1, frag("a", ["O"]), frag("b", ["H"]));
    get().setScene(before);
    get().collapseToSingleFragment([{ element: "O", x: 0, y: 0, z: 0 }]);
    expect(get().resetNotice).not.toBeNull();
    get().undoReset();
    expect(get().scene).toBe(before);
    expect(get().previous).toBeNull();
    expect(get().resetNotice).toBeNull();
  });

  it("is a no-op when there is nothing to undo", () => {
    const s = scene(1, frag("a", ["O"]));
    get().setScene(s);
    get().undoReset();
    expect(get().scene).toBe(s);
  });
});

describe("dismissResetNotice", () => {
  it("clears the notice without touching the scene", () => {
    const s = scene(1, frag("a", ["O"]), frag("b", ["H"]));
    get().setScene(s);
    get().collapseToSingleFragment([{ element: "O", x: 0, y: 0, z: 0 }]);
    const collapsed = get().scene;
    get().dismissResetNotice();
    expect(get().resetNotice).toBeNull();
    expect(get().scene).toBe(collapsed);
  });
});

describe("mutators are no-ops on a null scene (identity preserved)", () => {
  it("setMultiplicity / removeFragment leave a null scene null", () => {
    get().setMultiplicity(3);
    expect(get().scene).toBeNull();
    get().removeFragment("x");
    expect(get().scene).toBeNull();
  });
});

describe("sync decision (mirrors the NewJobScreen effect)", () => {
  it("matching geometry ⇒ no collapse; diverged geometry ⇒ collapse to a new ref", () => {
    const s = scene(1, frag("a", ["O", "H", "H"]));
    get().setScene(s);
    const before = get().scene!;

    // Re-serialised same geometry → matches → the screen calls no action.
    expect(xyzMatchesScene(before, mergeToAtomLines(before))).toBe(true);

    // Moved geometry → does not match → the screen collapses.
    const moved = before.fragments[0].atoms.map((a) => ({ ...a, x: a.x + 0.5 }));
    const movedScene: Scene = {
      ...before,
      fragments: [{ ...before.fragments[0], atoms: moved }],
    };
    expect(xyzMatchesScene(before, mergeToAtomLines(movedScene))).toBe(false);

    get().collapseToSingleFragment(moved);
    expect(get().scene).not.toBe(before);
  });
});
