import { describe, it, expect } from "vitest";

import { restoreScene } from "./restore";
import {
  injectSceneIntoInput,
  serializeScene,
  translateFragment,
} from "./scene";
import type { Scene, SceneFragment } from "./types";

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

const bh4 = (): SceneFragment => ({
  id: "bh4",
  name: "BH4-",
  charge: -1,
  source: "fragment-library",
  atoms: [
    { element: "B", x: 5, y: 0, z: 0 },
    { element: "H", x: 5.71, y: 0.71, z: 0.71 },
    { element: "H", x: 4.29, y: -0.71, z: 0.71 },
    { element: "H", x: 4.29, y: 0.71, z: -0.71 },
    { element: "H", x: 5.71, y: -0.71, z: -0.71 },
  ],
});

/** ORCA input whose coordinate block is `scene`'s merged geometry. */
const inputFor = (scene: Scene) => injectSceneIntoInput("! r2SCAN-3c Opt\n", scene);

describe("restoreScene", () => {
  it("(1) no coordinate block in the input → null scene, not rejected", () => {
    const r = restoreScene("! r2SCAN-3c Opt\n%pal nprocs 2 end", null);
    expect(r.scene).toBeNull();
    expect(r.snapshotRejected).toBe(false);
  });

  it("(2) no snapshot → single fragment from the text, not rejected (pre-v4 job)", () => {
    const input = inputFor({ fragments: [water()], multiplicity: 1 });
    const r = restoreScene(input, null);
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(false);
  });

  it("(2b) empty-string snapshot is treated as no snapshot", () => {
    const input = inputFor({ fragments: [water()], multiplicity: 1 });
    const r = restoreScene(input, "");
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(false);
  });

  it("(3) malformed snapshot → text fragment, rejected", () => {
    const input = inputFor({ fragments: [water()], multiplicity: 1 });
    const r = restoreScene(input, "{ not json");
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(true);
  });

  it("(3b) wrong-version snapshot → text fragment, rejected", () => {
    const scene: Scene = { fragments: [water()], multiplicity: 1 };
    const input = inputFor(scene);
    const bad = JSON.parse(serializeScene(scene));
    bad.version = 99;
    const r = restoreScene(input, JSON.stringify(bad));
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(true);
  });

  it("(4) valid snapshot matching the input → the snapshot is returned", () => {
    const scene: Scene = { fragments: [water(), bh4()], multiplicity: 1 };
    const input = inputFor(scene);
    const r = restoreScene(input, serializeScene(scene));
    expect(r.snapshotRejected).toBe(false);
    // The multi-fragment layout survives — the whole point of persisting it.
    expect(r.scene!.fragments).toHaveLength(2);
    expect(r.scene!.fragments[1].charge).toBe(-1);
  });

  it("(4b) snapshot with the right atom count but shifted coords → rejected", () => {
    const scene: Scene = { fragments: [water(), bh4()], multiplicity: 1 };
    const input = inputFor(scene);
    // Same composition, but the BH₄⁻ fragment nudged 1 Å — no longer the input.
    const stale: Scene = {
      ...scene,
      fragments: [scene.fragments[0], translateFragment(bh4(), 1, 0, 0)],
    };
    const r = restoreScene(input, serializeScene(stale));
    expect(r.snapshotRejected).toBe(true);
    expect(r.scene!.fragments).toHaveLength(1); // fell back to the text
  });
});
