import { describe as vitestDescribe, expect, it } from "vitest";

import { makeAtomId } from "./ids";
import {
  append,
  canRedo,
  canUndo,
  current,
  deserializeLog,
  describe,
  emptyLog,
  goto,
  logInvariant,
  redo,
  serializeLog,
  undo,
  type Op,
} from "./oplog";
import { testScene } from "./scene-test-util";
import type { Scene } from "./types";

// ── Small scene fixtures (valid v2 scenes; ids minted like production) ─────────

function waterScene(): Scene {
  return testScene([
    {
      id: "wat",
      name: "Water",
      charge: 0,
      source: "fragment-library",
      atoms: [
        { element: "O", x: 0, y: 0, z: 0 },
        { element: "H", x: 0.76, y: 0.59, z: 0 },
        { element: "H", x: -0.76, y: 0.59, z: 0 },
      ],
    },
  ]);
}

function twoFragmentScene(): Scene {
  return testScene([
    {
      id: "wat",
      name: "Water",
      charge: 0,
      source: "fragment-library",
      atoms: [
        { element: "O", x: 0, y: 0, z: 0 },
        { element: "H", x: 0.76, y: 0.59, z: 0 },
        { element: "H", x: -0.76, y: 0.59, z: 0 },
      ],
    },
    {
      id: "bh4",
      name: "BH₄⁻",
      charge: -1,
      source: "fragment-library",
      atoms: [
        { element: "B", x: 5, y: 0, z: 0 },
        { element: "H", x: 5.7, y: 0.7, z: 0.7 },
      ],
    },
  ]);
}

const ADD: Op = {
  type: "add-fragment",
  fragmentId: "bh4",
  name: "BH₄⁻",
  source: "fragment-library",
  sourceLabel: "borohydride",
  atomCount: 5,
};
const RENAME: Op = { type: "rename-fragment", fragmentId: "wat", from: "Water", to: "Solvent" };
const MULT: Op = { type: "set-multiplicity", multiplicity: 3 };

vitestDescribe("pointer invariants", () => {
  it("emptyLog is the before-first state (pointer -1, current null)", () => {
    const log = emptyLog();
    expect(log.entries).toHaveLength(0);
    expect(log.pointer).toBe(-1);
    expect(current(log)).toBeNull();
    expect(logInvariant(log)).toBe(true);
    expect(canUndo(log)).toBe(false);
    expect(canRedo(log)).toBe(false);
  });

  it("a populated, not-fully-undone log satisfies 0 ≤ pointer < len (the architect's invariant)", () => {
    let log = emptyLog();
    log = append(log, ADD, waterScene());
    log = append(log, MULT, waterScene());
    expect(log.pointer).toBeGreaterThanOrEqual(0);
    expect(log.pointer).toBeLessThan(log.entries.length);
    expect(logInvariant(log)).toBe(true);
  });

  it("logInvariant holds across an arbitrary append/undo/redo walk", () => {
    let log = emptyLog();
    const steps: ((l: typeof log) => typeof log)[] = [
      (l) => append(l, ADD, waterScene()),
      (l) => append(l, MULT, twoFragmentScene()),
      undo,
      undo,
      undo, // past the start → no-op
      redo,
      (l) => append(l, RENAME, waterScene()),
      redo, // past the tip → no-op
    ];
    for (const step of steps) {
      log = step(log);
      expect(logInvariant(log)).toBe(true);
      expect(log.pointer).toBeGreaterThanOrEqual(-1);
      expect(log.pointer).toBeLessThan(Math.max(1, log.entries.length));
    }
  });

  it("undo from the first entry reaches the empty scene (pointer -1)", () => {
    const log = append(emptyLog(), ADD, waterScene());
    const undone = undo(log);
    expect(undone.pointer).toBe(-1);
    expect(current(undone)).toBeNull();
    expect(logInvariant(undone)).toBe(true);
  });
});

vitestDescribe("undo / redo round-trip is identity", () => {
  it("walks back to null and forward to the same frozen snapshots", () => {
    const a = waterScene();
    const b = twoFragmentScene();
    const log = append(append(emptyLog(), ADD, a), MULT, b);

    expect(current(log)).toBe(log.entries[1].scene); // identity, not just equality
    expect(current(log)).toEqual(b);

    const u1 = undo(log);
    expect(current(u1)).toBe(log.entries[0].scene);
    expect(current(u1)).toEqual(a);

    const u2 = undo(u1);
    expect(current(u2)).toBeNull();

    const r1 = redo(u2);
    expect(current(r1)).toBe(log.entries[0].scene);

    const r2 = redo(r1);
    expect(current(r2)).toBe(log.entries[1].scene); // same object we started at
  });

  it("goto jumps the pointer directly (the history-panel click)", () => {
    const log = append(
      append(append(emptyLog(), ADD, waterScene()), MULT, twoFragmentScene()),
      RENAME,
      waterScene(),
    ); // pointer 2
    expect(goto(log, 0).pointer).toBe(0); // jump back two
    expect(current(goto(log, 0))).toBe(log.entries[0].scene);
    expect(goto(log, -1).pointer).toBe(-1); // jump to empty scene
    expect(current(goto(log, -1))).toBeNull();
    expect(goto(log, 2)).toBe(log); // already current → same ref
    expect(goto(log, 5)).toBe(log); // out of range → no-op
    expect(goto(log, -2)).toBe(log); // below -1 → no-op
  });

  it("undo/redo at the boundaries are no-ops (same reference)", () => {
    const log = append(emptyLog(), ADD, waterScene());
    expect(redo(log)).toBe(log); // already at the tip
    const undone = undo(log);
    const twice = undo(undone);
    expect(twice).toBe(undone); // already at the start (pointer -1)
  });
});

vitestDescribe("append truncates the redo tail (negative control a)", () => {
  it("an append after an undo discards the undone future — redo becomes impossible", () => {
    const a = waterScene();
    const b = twoFragmentScene();
    const c = waterScene();
    const log = append(append(emptyLog(), ADD, a), MULT, b); // pointer 1
    const undone = undo(log); // pointer 0, one redoable step
    expect(canRedo(undone)).toBe(true);

    const appended = append(undone, RENAME, c); // must drop [MULT, b]
    expect(appended.entries).toHaveLength(2); // [ADD, RENAME], not 3
    expect(appended.entries[1].op).toEqual(RENAME);
    expect(canRedo(appended)).toBe(false);
    expect(redo(appended)).toBe(appended); // no future to redo into
    expect(current(appended)).toBe(appended.entries[1].scene); // scene c, not b
  });
});

vitestDescribe("entries are immutable (negative control b)", () => {
  it("the materialized snapshot is deep-frozen; a write throws and the value survives", () => {
    const scene = waterScene();
    const origMult = scene.multiplicity;
    const log = append(emptyLog(), ADD, scene);
    const entry = log.entries[0];

    expect(Object.isFrozen(entry)).toBe(true);
    expect(Object.isFrozen(entry.scene)).toBe(true);
    expect(Object.isFrozen(entry.scene.fragments)).toBe(true);
    expect(Object.isFrozen(entry.scene.fragments[0])).toBe(true);
    expect(Object.isFrozen(entry.scene.fragments[0].atoms[0])).toBe(true);

    // ES modules are strict mode → a write to a frozen field throws, not no-ops.
    expect(() => {
      (entry.scene as unknown as { multiplicity: number }).multiplicity = 999;
    }).toThrow();
    expect(() => {
      (entry.scene.fragments[0].atoms[0] as unknown as { x: number }).x = 999;
    }).toThrow();
    expect(entry.scene.multiplicity).toBe(origMult);
  });
});

vitestDescribe("describe() — one human line per variant", () => {
  it("covers every Op variant", () => {
    const cases: [Op, string][] = [
      [ADD, "Add fragment BH₄⁻ (borohydride)"],
      [
        {
          type: "add-fragment",
          fragmentId: "wat",
          name: "Water",
          source: "fragment-library",
          sourceLabel: null,
          atomCount: 3,
        },
        "Add fragment Water",
      ],
      [{ type: "remove-fragment", fragmentId: "bh4", name: "BH₄⁻" }, "Remove fragment BH₄⁻"],
      [RENAME, "Rename fragment Water → Solvent"],
      [
        { type: "set-fragment-charge", fragmentId: "bh4", name: "BH₄⁻", charge: -1 },
        "Set charge of BH₄⁻ to -1",
      ],
      [MULT, "Set multiplicity to 3"],
      [
        { type: "translate-fragment", fragmentId: "bh4", name: "BH₄⁻", delta: [1.5, 0, -2] },
        "Move BH₄⁻ by (1.500, 0, -2) Å",
      ],
      [
        {
          type: "replace-fragment-atoms",
          fragmentId: "sub",
          name: "Ibuprofen",
          edit: {
            via: "set-internal",
            kind: "dihedral",
            atoms: [4, 7, 12, 15].map(makeAtomId),
            target: 30,
            unit: "°",
          },
        },
        "Set dihedral 4-7-12-15 to 30°",
      ],
      [
        {
          type: "replace-fragment-atoms",
          fragmentId: "sub",
          name: "Ibuprofen",
          edit: {
            via: "set-internal",
            kind: "distance",
            atoms: [3, 8].map(makeAtomId),
            target: 1.85,
            unit: "Å",
          },
        },
        "Set distance 3-8 to 1.850 Å",
      ],
      [
        {
          type: "replace-fragment-atoms",
          fragmentId: "sub",
          name: "Ibuprofen",
          edit: { via: "xtb" },
        },
        "Pre-optimize Ibuprofen (xtb)",
      ],
      [
        {
          type: "replace-fragment-atoms",
          fragmentId: "but",
          name: "Butane",
          edit: { via: "conformer", conformerIndex: 2, deltaEKcal: 0.42 },
        },
        "Replace Butane with conformer #2 (ΔE 0.420 kcal/mol)",
      ],
      [
        {
          type: "replace-fragment-atoms",
          fragmentId: "but",
          name: "Butane",
          edit: { via: "conformer", conformerIndex: 0, deltaEKcal: null },
        },
        "Replace Butane with conformer #0",
      ],
      [{ type: "replace-all-atoms", edit: { via: "xtb" } }, "Pre-optimize all fragments (xtb)"],
      [
        { type: "collapse-from-text", fragmentCount: 3 },
        "Edit coordinates as text (3 fragments → 1)",
      ],
      [
        { type: "collapse-from-text", fragmentCount: 1 },
        "Edit coordinates as text (1 fragment → 1)",
      ],
      [
        { type: "restore-snapshot", source: "new-iteration", fragmentCount: 2, atomCount: 12 },
        "Restore snapshot (New iteration) — 2 fragments, 12 atoms",
      ],
      [
        { type: "restore-snapshot", source: "text-adopt", fragmentCount: 1, atomCount: 3 },
        "Adopt geometry from input text — 1 fragment, 3 atoms",
      ],
      [
        { type: "restore-snapshot", source: "library", fragmentCount: 1, atomCount: 8 },
        "Load from library — 1 fragment, 8 atoms",
      ],
    ];
    for (const [op, expected] of cases) {
      expect(describe(op)).toBe(expected);
    }
  });
});

vitestDescribe("serialization round-trip", () => {
  it("empty log round-trips", () => {
    const back = deserializeLog(serializeLog(emptyLog()));
    expect(back).toEqual({ entries: [], pointer: -1 });
  });

  it("a populated log round-trips structurally (ops + scenes + pointer)", () => {
    const log = append(
      append(append(emptyLog(), ADD, waterScene()), MULT, twoFragmentScene()),
      RENAME,
      waterScene(),
    );
    const undone = undo(log); // pointer 1, a redo tail present
    const back = deserializeLog(serializeLog(undone));
    expect(back).not.toBeNull();
    expect(back!.pointer).toBe(undone.pointer);
    expect(back!.entries).toHaveLength(3);
    for (let i = 0; i < 3; i++) {
      expect(back!.entries[i].op).toEqual(undone.entries[i].op);
      expect(back!.entries[i].scene).toEqual(undone.entries[i].scene);
    }
    // scenes survive the embedded serializeScene codec (v2 ids intact)
    expect(back!.entries[1].scene.fragments[1].atoms[0].id).toBe(
      undone.entries[1].scene.fragments[1].atoms[0].id,
    );
  });

  it("deserialized entries are re-frozen (immutability survives persistence)", () => {
    const back = deserializeLog(serializeLog(append(emptyLog(), ADD, waterScene())));
    expect(Object.isFrozen(back!.entries[0].scene)).toBe(true);
  });

  it("rejects malformed / wrong-version / bad-pointer / bad-op input (returns null, never throws)", () => {
    expect(deserializeLog("not json")).toBeNull();
    expect(deserializeLog(JSON.stringify({ version: 2, pointer: -1, entries: [] }))).toBeNull();
    expect(deserializeLog(JSON.stringify({ version: 1, pointer: 5, entries: [] }))).toBeNull();
    const goodScene = serializeLog(append(emptyLog(), ADD, waterScene()));
    // a good log with its op tag corrupted must be rejected
    const corrupt = JSON.parse(goodScene) as { entries: { op: { type: string } }[] };
    corrupt.entries[0].op.type = "bogus-op";
    expect(deserializeLog(JSON.stringify(corrupt))).toBeNull();
  });
});
