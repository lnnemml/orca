import { describe, it, expect } from "vitest";

import { restoreScene } from "./restore";
import {
  deserializeScene,
  injectSceneIntoInput,
  serializeScene,
  translateFragment,
} from "./scene";
import type { RawFragment, Scene } from "./types";
import { testScene } from "./scene-test-util";

// A REAL v1 scene_json string, emitted by the pre-1b shipping code and copied
// verbatim (2 fragments: Water charge 0 + Chloride charge −1). Loaded as a raw
// string (Vite `?raw`). The format is not synthesized from memory — it is what
// old jobs actually hold in `jobs.scene_json`.
import V1_SNAPSHOT from "./__fixtures__/scene-v1.json?raw";

const water = (): RawFragment => ({
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

const bh4 = (): RawFragment => ({
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
    const input = inputFor(testScene([water()], 1));
    const r = restoreScene(input, null);
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(false);
  });

  it("(2b) empty-string snapshot is treated as no snapshot", () => {
    const input = inputFor(testScene([water()], 1));
    const r = restoreScene(input, "");
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(false);
  });

  it("(3) malformed snapshot → text fragment, rejected", () => {
    const input = inputFor(testScene([water()], 1));
    const r = restoreScene(input, "{ not json");
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(true);
  });

  it("(3b) wrong-version snapshot → text fragment, rejected", () => {
    const scene: Scene = testScene([water()], 1);
    const input = inputFor(scene);
    const bad = JSON.parse(serializeScene(scene));
    bad.version = 99;
    const r = restoreScene(input, JSON.stringify(bad));
    expect(r.scene!.fragments).toHaveLength(1);
    expect(r.snapshotRejected).toBe(true);
  });

  it("(4) valid snapshot matching the input → the snapshot is returned", () => {
    const scene: Scene = testScene([water(), bh4()], 1);
    const input = inputFor(scene);
    const r = restoreScene(input, serializeScene(scene));
    expect(r.snapshotRejected).toBe(false);
    // The multi-fragment layout survives — the whole point of persisting it.
    expect(r.scene!.fragments).toHaveLength(2);
    expect(r.scene!.fragments[1].charge).toBe(-1);
  });

  it("(4b) snapshot with the right atom count but shifted coords → rejected", () => {
    const scene: Scene = testScene([water(), bh4()], 1);
    const input = inputFor(scene);
    // Same composition, but the BH₄⁻ fragment nudged 1 Å — no longer the input.
    const stale: Scene = testScene(
      [water(), translateFragment(bh4(), 1, 0, 0)],
      1,
    );
    const r = restoreScene(input, serializeScene(stale));
    expect(r.snapshotRejected).toBe(true);
    expect(r.scene!.fragments).toHaveLength(1); // fell back to the text
  });
});

describe("v1 scene_json migration (unit 1b) — a real pre-1b snapshot", () => {
  it("HONOURS a v1 snapshot (migrated), keeping its multi-fragment layout alive", () => {
    // The exact silent failure this migration prevents: without it, deserializeScene
    // returns null for a v1 string, restoreScene reports snapshotRejected=true, and
    // every existing multi-fragment job collapses to ONE fragment on open.
    const migrated = deserializeScene(V1_SNAPSHOT)!;
    const input = injectSceneIntoInput("! r2SCAN-3c Opt\n", migrated);
    const r = restoreScene(input, V1_SNAPSHOT);
    expect(r.snapshotRejected).toBe(false); // honoured, NOT collapsed
    expect(r.scene!.fragments).toHaveLength(2); // both fragments survive
    expect(r.scene!.fragments.map((f) => f.name)).toEqual(["Water", "Chloride"]);
  });

  it("mints ids 0..N-1 scene-wide and sets nextAtomId on migration", () => {
    const s = deserializeScene(V1_SNAPSHOT)!;
    const ids = s.fragments.flatMap((f) => f.atoms.map((a) => a.id));
    expect(ids).toEqual([0, 1, 2, 3]); // 3 water atoms + 1 chloride, scene-wide
    expect(s.nextAtomId).toBe(4);
  });

  // Negative control: a v1 snapshot must MIGRATE, never return null. If it returned
  // null, restoreScene would silently collapse the layout (the case above). This is
  // the test that fails if v1 is rejected instead of migrated.
  it("does NOT return null for a valid v1 (the collapse guard bites)", () => {
    expect(deserializeScene(V1_SNAPSHOT)).not.toBeNull();
  });
});
