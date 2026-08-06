import { beforeEach, describe, it, expect } from "vitest";

import { useSceneStore } from "./store";
import { append, current, emptyLog } from "./oplog";
import {
  injectSceneIntoInput,
  mergeToAtomLines,
  sceneFromOrcaInput,
  totalCharge,
  xyzMatchesScene,
} from "./scene";
import type { RawAtom, Scene } from "./types";
import { testScene, type RawFragment } from "./scene-test-util";

// Reset to the empty log between tests (the actions survive — they're closures).
beforeEach(() =>
  useSceneStore.setState({ log: emptyLog(), scene: null, resetNotice: null }),
);

const get = () => useSceneStore.getState();

function frag(id: string, elements: string[], charge = 0): RawFragment {
  return {
    id,
    name: id,
    charge,
    source: "editor",
    atoms: elements.map((element, i) => ({ element, x: i, y: 0, z: 0 })),
  };
}

function scene(multiplicity: number, ...fragments: RawFragment[]): Scene {
  return testScene(fragments, multiplicity);
}

// ── The core invariant (negative control a): scene is DERIVED from the log ─────

describe("scene === current(log) after every action (the bypass is impossible)", () => {
  it("holds across a full walk of every store action", () => {
    const check = (label: string) => {
      expect(get().scene, label).toBe(current(get().log));
    };
    check("initial");

    get().seedScene(scene(1, frag("a", ["O", "H", "H"])), "library");
    check("seedScene(scene)");

    get().addFragment(frag("b", ["N", "H", "H", "H"]));
    check("addFragment");

    get().setMultiplicity(3);
    check("setMultiplicity");

    get().renameFragment("a", "Substrate");
    check("renameFragment");

    get().replaceFragmentAtoms(
      "b",
      [
        { element: "N", x: 5, y: 5, z: 5 },
        { element: "H", x: 6, y: 5, z: 5 },
        { element: "H", x: 5, y: 6, z: 5 },
        { element: "H", x: 5, y: 5, z: 6 },
      ],
      { via: "xtb" },
    );
    check("replaceFragmentAtoms");

    get().removeFragment("b");
    check("removeFragment");

    get().commit(
      { type: "set-multiplicity", multiplicity: 1 },
      { ...current(get().log)!, multiplicity: 1 },
    );
    check("commit");

    get().collapseFromText([{ element: "O", x: 0, y: 0, z: 0 }]);
    check("collapseFromText");

    get().undo();
    check("undo");
    get().redo();
    check("redo");
    get().jumpTo(0);
    check("jumpTo");

    get().seedScene(null, "text-adopt");
    check("seedScene(null)");
    expect(get().scene).toBeNull();

    get().installLog(append(emptyLog(), { type: "set-multiplicity", multiplicity: 5 }, scene(5, frag("z", ["He"]))));
    check("installLog");
  });
});

// ── Lifecycle: seedScene / installLog / reference stability ────────────────────

describe("seedScene + reference stability", () => {
  it("seeds a fresh single-entry log and stores the exact (frozen) object", () => {
    useSceneStore.setState({ resetNotice: { fragmentCount: 3 } });
    const s = scene(1, frag("a", ["O", "H", "H"]));
    get().seedScene(s, "library");
    expect(get().scene).toBe(s); // frozen in place → same reference
    expect(get().log.entries).toHaveLength(1);
    expect(get().log.entries[0].op).toEqual({
      type: "restore-snapshot",
      source: "library",
      fragmentCount: 1,
      atomCount: 3,
    });
    expect(get().resetNotice).toBeNull();
  });

  it("seedScene(null) installs the empty log (scene → null)", () => {
    get().seedScene(scene(1, frag("a", ["O"])), "text-adopt");
    get().seedScene(null, "text-adopt");
    expect(get().scene).toBeNull();
    expect(get().log.pointer).toBe(-1);
    expect(get().log.entries).toHaveLength(0);
  });

  it("returns the SAME object on repeated reads (viewer won't redraw on keystroke)", () => {
    get().seedScene(scene(1, frag("a", ["O", "H", "H"])), "library");
    expect(get().scene).toBe(get().scene);
  });
});

// ── collapseFromText ──────────────────────────────────────────────────────────

describe("collapseFromText", () => {
  const atoms: RawAtom[] = [
    { element: "O", x: 9, y: 0, z: 0 },
    { element: "H", x: 9, y: 1, z: 0 },
  ];

  it("appends a CollapseFromText op and collapses to one fragment", () => {
    const before = scene(1, frag("a", ["O", "H"]), frag("b", ["N"]));
    get().seedScene(before, "library");
    get().collapseFromText(atoms);
    expect(get().scene!.fragments).toHaveLength(1);
    expect(get().scene).not.toBe(before);
    expect(get().log.entries[get().log.pointer].op).toEqual({
      type: "collapse-from-text",
      fragmentCount: 2,
    });
  });

  it("shows the notice only when >1 fragment was merged", () => {
    get().seedScene(scene(1, frag("a", ["O"]), frag("b", ["H"])), "library");
    get().collapseFromText(atoms);
    expect(get().resetNotice).toEqual({ fragmentCount: 2 });
  });

  it("does NOT show the notice for a single-fragment scene", () => {
    get().seedScene(scene(1, frag("a", ["O", "H"])), "library");
    get().collapseFromText(atoms);
    expect(get().resetNotice).toBeNull();
  });

  it("preserves total charge and multiplicity", () => {
    get().seedScene(scene(3, frag("a", ["O"], -1), frag("b", ["H"], 0)), "library");
    get().collapseFromText(atoms);
    expect(totalCharge(get().scene!)).toBe(-1);
    expect(get().scene!.multiplicity).toBe(3);
  });
});

// ── Deep undo/redo (the dividend over the old one-step previous/undoReset) ─────

describe("undo / redo over the log", () => {
  it("undo restores the pre-collapse layout; redo re-applies it", () => {
    const before = scene(1, frag("a", ["O"]), frag("b", ["H"]));
    get().seedScene(before, "library");
    get().collapseFromText([{ element: "O", x: 0, y: 0, z: 0 }]);
    expect(get().resetNotice).not.toBeNull();

    get().undo();
    expect(get().scene).toBe(before); // the SAME frozen snapshot
    expect(get().resetNotice).toBeNull();

    get().redo();
    expect(get().scene!.fragments).toHaveLength(1); // back to the collapse
  });

  it("undoes MORE than one step (deep history)", () => {
    get().seedScene(scene(1, frag("a", ["O"])), "library"); // entry 0
    get().addFragment(frag("b", ["H"])); // entry 1
    get().addFragment(frag("c", ["N"])); // entry 2
    expect(get().scene!.fragments).toHaveLength(3);
    get().undo();
    get().undo();
    expect(get().scene!.fragments).toHaveLength(1); // back at entry 0
  });
});

describe("dismissResetNotice", () => {
  it("clears the notice without touching the scene", () => {
    get().seedScene(scene(1, frag("a", ["O"]), frag("b", ["H"])), "library");
    get().collapseFromText([{ element: "O", x: 0, y: 0, z: 0 }]);
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
    expect(get().log.entries).toHaveLength(0);
    get().removeFragment("x");
    expect(get().scene).toBeNull();
    expect(get().log.entries).toHaveLength(0);
  });
});

describe("sync decision (mirrors the NewJobScreen effect)", () => {
  it("matching geometry ⇒ no collapse; diverged geometry ⇒ collapse to a new ref", () => {
    get().seedScene(scene(1, frag("a", ["O", "H", "H"])), "library");
    const before = get().scene!;

    expect(xyzMatchesScene(before, mergeToAtomLines(before))).toBe(true);

    const moved = before.fragments[0].atoms.map((a) => ({ ...a, x: a.x + 0.5 }));
    const movedScene: Scene = {
      ...before,
      fragments: [{ ...before.fragments[0], atoms: moved }],
    };
    expect(xyzMatchesScene(before, mergeToAtomLines(movedScene))).toBe(false);

    get().collapseFromText(moved);
    expect(get().scene).not.toBe(before);
  });
});

// ── Control (c): the collapse ↔ undo loop is DEAD (sync integration) ───────────
// Replays the two NewJobScreen sync effects against the real store: a manual edit
// diverges → collapse (a logged op) → Undo restores the multi-fragment layout →
// Scene→Monaco re-injects it → Monaco→Scene sees a MATCH and does NOT re-collapse.
// If collapse were a non-undoable reset (or undo didn't restore), step 5 would
// re-collapse and the scene would stay at one fragment — the live loop.

describe("collapse ↔ undo loop is dead (control c)", () => {
  // The Monaco→Scene decision, verbatim from the NewJobScreen effect body.
  function syncMonacoToScene(content: string): "match" | "collapse" | "adopt" | "clear" {
    const parsed = sceneFromOrcaInput(content);
    const cur = get().scene;
    if (!parsed) {
      if (cur) get().seedScene(null, "text-adopt");
      return "clear";
    }
    if (!cur) {
      get().seedScene(parsed, "text-adopt");
      return "adopt";
    }
    if (xyzMatchesScene(cur, mergeToAtomLines(parsed))) return "match";
    get().collapseFromText(parsed.fragments[0].atoms);
    return "collapse";
  }

  it("a hand-edit collapses once; Undo + re-inject does not collapse again", () => {
    const two = scene(1, frag("a", ["O", "H", "H"]), frag("b", ["N", "H", "H"]));
    get().seedScene(two, "library");

    // 1) The user hand-edits a coordinate → the text block diverges from the scene.
    const moved: Scene = {
      ...two,
      fragments: two.fragments.map((f, fi) =>
        fi === 0
          ? { ...f, atoms: f.atoms.map((a, ai) => (ai === 0 ? { ...a, x: a.x + 0.7 } : a)) }
          : f,
      ),
    };
    const divergentContent = injectSceneIntoInput("! HF\n", moved);

    // 2) content→Scene: diverged → collapse to one fragment.
    expect(syncMonacoToScene(divergentContent)).toBe("collapse");
    expect(get().scene!.fragments).toHaveLength(1);

    // 3) The user clicks Undo → the 2-fragment layout comes back (deep undo).
    get().undo();
    expect(get().scene).toBe(two);
    expect(get().scene!.fragments).toHaveLength(2);

    // 4) Scene→Monaco re-injects the restored geometry into the text.
    const reInjected = injectSceneIntoInput(divergentContent, get().scene!);

    // 5) content→Scene runs again → MATCH, no second collapse (the loop is dead).
    expect(syncMonacoToScene(reInjected)).toBe("match");
    expect(get().scene).toBe(two);
    expect(get().scene!.fragments).toHaveLength(2);
  });
});
