import { beforeEach, describe, it, expect } from "vitest";

import { useSceneStore } from "./store";
import { append, current, emptyLog } from "./oplog";
import {
  injectSceneIntoInput,
  mergeToAtomLines,
  sceneFromOrcaInput,
  sceneFromXyz,
  xyzMatchesScene,
} from "./scene";
import type { Scene } from "./types";
import { testScene, type RawFragment } from "./scene-test-util";

// Reset to the empty log between tests (the actions survive — they're closures).
beforeEach(() => useSceneStore.setState({ log: emptyLog(), scene: null }));

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

// ── Door 1 (unit 2d): Import xyz as fragment — add-fragment op, count+order (c1) ─
// The typical way coordinate hand-editing survives the now read-only block: paste
// an xyz, it becomes a NEW fragment. Breaks (mint fresh ids that reorder / wrong
// length in the parse→add path) turn the element-order / count assertions red.

describe("import xyz as a fragment (c1)", () => {
  it("appends an add-fragment op adding exactly N atoms in input order", () => {
    // A pre-existing fragment so the import is an ADD, not a seed.
    get().seedScene(scene(1, frag("host", ["C", "C"])), "library");

    const xyz = "3\nwater\nO 0 0 0\nH 0.757 0.586 0\nH -0.757 0.586 0";
    const built = sceneFromXyz(xyz, { source: "import", name: "water" });
    expect(built).not.toBeNull();

    get().addFragment(built!.fragments[0]);

    const op = get().log.entries[get().log.pointer].op;
    expect(op).toMatchObject({ type: "add-fragment", atomCount: 3 });

    const frags = get().scene!.fragments;
    const added = frags[frags.length - 1];
    expect(added.atoms).toHaveLength(3);
    // ORDER preserved: the xyz's row order, element by element.
    expect(added.atoms.map((a) => a.element)).toEqual(["O", "H", "H"]);
    // The host fragment is untouched (composition invariant across the add).
    expect(get().scene!.fragments[0].atoms).toHaveLength(2);
  });
});

// ── Door 2 (unit 2d): Replace input — a fresh text-adopt log, no leak (c2) ──────
// "Replace input" adopts the buffer via seedScene(text-adopt), which installs a
// FRESH log. Break (installLog appending instead of replacing) → the entries count
// jumps and the old scene leaks in → red.

describe("replace input re-seed (c2)", () => {
  it("seedScene(text-adopt) replaces the whole log; the prior lineage does not leak", () => {
    get().seedScene(scene(1, frag("a", ["O", "H", "H"])), "library");
    get().addFragment(frag("b", ["N"])); // 2 entries, 2 fragments
    expect(get().log.entries.length).toBeGreaterThan(1);

    const pasted = sceneFromOrcaInput("! HF\n* xyz 0 1\nHe 0 0 0\n*");
    expect(pasted).not.toBeNull();

    get().seedScene(pasted, "text-adopt"); // the Replace-input adopt

    expect(get().log.entries).toHaveLength(1); // FRESH log
    expect(get().log.pointer).toBe(0);
    expect(get().log.entries[0].op).toMatchObject({
      type: "restore-snapshot",
      source: "text-adopt",
    });
    // The new molecule, not the old — the prior lineage did not leak in.
    expect(get().scene!.fragments).toHaveLength(1);
    expect(get().scene!.fragments[0].atoms.map((a) => a.element)).toEqual(["He"]);
  });
});

// ── Deep undo/redo (the dividend over the old one-step previous/undoReset) ─────

describe("undo / redo over the log", () => {
  it("undo restores the pre-op layout; redo re-applies it", () => {
    const before = scene(1, frag("a", ["O"]), frag("b", ["H"]));
    get().seedScene(before, "library");
    get().removeFragment("b");
    expect(get().scene!.fragments).toHaveLength(1);

    get().undo();
    expect(get().scene).toBe(before); // the SAME frozen snapshot
    expect(get().scene!.fragments).toHaveLength(2);

    get().redo();
    expect(get().scene!.fragments).toHaveLength(1); // back to the removal
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

// ── Rigid-body drag commit (Stage 3, unit 3.1) — one op, rigid, others untouched (c1) ─
// The store mutator a fragment drag commits on release. Post-condition (rule #9):
// ALL mover atoms shift by the SAME delta, internal pairwise distances are invariant
// (a rigid move), count/order/AtomId are invariant, and other fragments are untouched.

describe("translateFragment (c1)", () => {
  const pairwise = (atoms: { x: number; y: number; z: number }[]): number[] => {
    const ds: number[] = [];
    for (let i = 0; i < atoms.length; i++)
      for (let j = i + 1; j < atoms.length; j++) {
        const a = atoms[i];
        const b = atoms[j];
        ds.push(Math.hypot(a.x - b.x, a.y - b.y, a.z - b.z));
      }
    return ds;
  };

  it("commits ONE op; shifts every mover atom equally; distances + other fragments invariant", () => {
    const two = scene(1, frag("a", ["O", "H", "H"]), frag("b", ["N", "H", "H"]));
    get().seedScene(two, "library");
    const before = get().scene!;
    const moverBefore = before.fragments[1];
    const otherBefore = before.fragments[0];
    const [dx, dy, dz] = [1.5, -2.0, 0.5];
    const lenBefore = get().log.entries.length;

    get().translateFragment("b", dx, dy, dz);

    // Exactly one op, with the total delta.
    expect(get().log.entries.length).toBe(lenBefore + 1);
    expect(get().log.entries[get().log.pointer].op).toMatchObject({
      type: "translate-fragment",
      fragmentId: "b",
      delta: [dx, dy, dz],
    });

    const moverAfter = get().scene!.fragments[1];
    moverAfter.atoms.forEach((a, i) => {
      expect(a.x).toBeCloseTo(moverBefore.atoms[i].x + dx, 10); // shifted by the SAME delta
      expect(a.y).toBeCloseTo(moverBefore.atoms[i].y + dy, 10);
      expect(a.z).toBeCloseTo(moverBefore.atoms[i].z + dz, 10);
      expect(a.id).toBe(moverBefore.atoms[i].id); // AtomId invariant
      expect(a.element).toBe(moverBefore.atoms[i].element); // order invariant
    });
    // Rigid: internal pairwise distances unchanged.
    pairwise(moverAfter.atoms).forEach((d, k) =>
      expect(d).toBeCloseTo(pairwise(moverBefore.atoms)[k], 10),
    );
    // The other fragment is untouched.
    expect(get().scene!.fragments[0]).toEqual(otherBefore);
  });

  it("a zero delta is a no-op (no op appended)", () => {
    get().seedScene(scene(1, frag("a", ["O", "H"])), "library");
    const len = get().log.entries.length;
    get().translateFragment("a", 0, 0, 0);
    expect(get().log.entries.length).toBe(len);
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

// ── The coordinate block is a READ-ONLY PROJECTION of the Scene (unit 2d) ───────
// Pure replay of the NewJobScreen Monaco→Scene effect body. A block hand-edit is
// REVERTED (never collapsed into geometry — that path is gone); a keyword edit
// outside the block passes through. This is the pure core of manual gates m1/m2;
// the Monaco effect itself is exercised by the manual gate (no jsdom in the suite).

describe("coordinate block is a read-only projection — Monaco→Scene decision", () => {
  // The decision, verbatim from the effect: seed on an empty scene, keep on a
  // matching block, revert on a diverged/absent one. It NEVER mutates geometry on
  // a block edit.
  function syncMonacoToScene(content: string): { action: string; content: string } {
    const parsed = sceneFromOrcaInput(content);
    const cur = get().scene;
    if (!cur) {
      if (parsed) {
        get().seedScene(parsed, "text-adopt");
        return { action: "seed", content };
      }
      return { action: "noop", content };
    }
    if (parsed && xyzMatchesScene(cur, mergeToAtomLines(parsed))) {
      return { action: "keep", content };
    }
    return { action: "revert", content: injectSceneIntoInput(content, cur) };
  }

  it("a coordinate hand-edit is reverted; the multi-fragment scene is untouched (m1, pure)", () => {
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
    const edited = injectSceneIntoInput("! HF\n", moved);

    // 2) content→Scene: diverged → REVERT (not collapse). The scene is the SAME
    //    frozen 2-fragment snapshot; geometry did not change and did not collapse.
    const r = syncMonacoToScene(edited);
    expect(r.action).toBe("revert");
    expect(get().scene).toBe(two);
    expect(get().scene!.fragments).toHaveLength(2);

    // 3) The reverted content's block matches the scene again (projection restored),
    //    so a second pass is a no-op "keep" — there is no revert↔edit loop.
    expect(syncMonacoToScene(r.content).action).toBe("keep");
    expect(get().scene).toBe(two);
  });

  it("a keyword edit outside the block passes through — no revert (m2, pure)", () => {
    const two = scene(1, frag("a", ["O", "H", "H"]), frag("b", ["N", "H", "H"]));
    get().seedScene(two, "library");

    // Same coordinates as the scene, only the `!` line / a `%` block differ.
    const withKeywords = injectSceneIntoInput("! HF TightOpt\n%pal nprocs 4 end\n", get().scene!);
    const r = syncMonacoToScene(withKeywords);
    expect(r.action).toBe("keep"); // block matches → the keyword edit is accepted as-is
    expect(get().scene).toBe(two); // geometry untouched
  });

  it("a block typed into an empty editor SEEDS the scene (text-adopt seed stays alive)", () => {
    expect(get().scene).toBeNull();
    const r = syncMonacoToScene("! HF\n* xyz 0 1\nHe 0 0 0\n*");
    expect(r.action).toBe("seed");
    expect(get().scene!.fragments[0].atoms.map((a) => a.element)).toEqual(["He"]);
  });
});
