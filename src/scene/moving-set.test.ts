import { describe, it, expect } from "vitest";

import { resolveMovingSet, type MoveMode } from "./moving-set";
import { makeAtomId } from "./ids";
import type { AtomId } from "./ids";

// A small id vocabulary. A fragment of four atoms; its "component" (the grabbed
// atom's perceived connected component) is a disconnected 2-atom piece — so
// Fragment and Selection give DIFFERENT answers (that is the whole point).
const ids = (...ns: number[]): AtomId[] => ns.map(makeAtomId);
const GRABBED = makeAtomId(0);
const FRAGMENT = ids(0, 1, 2, 3); // whole fragment
const COMPONENT = ids(0, 1); // grabbed atom's connected component (a broken-off piece)

function input(selection: AtomId[], toggle: MoveMode) {
  return { grabbed: GRABBED, selection, toggle };
}

describe("resolveMovingSet — THE ONE RULE", () => {
  it("an explicit selection wins over the toggle (both toggle values)", () => {
    const sel = ids(2, 3);
    // BITE: an impl that consulted the toggle first (ignoring the selection) would
    // return FRAGMENT or COMPONENT here, not the selection.
    expect(resolveMovingSet(input(sel, "fragment"), FRAGMENT, COMPONENT)).toEqual(sel);
    expect(resolveMovingSet(input(sel, "selection"), FRAGMENT, COMPONENT)).toEqual(sel);
  });

  it("no selection + Fragment toggle → the whole fragment", () => {
    expect(resolveMovingSet(input([], "fragment"), FRAGMENT, COMPONENT)).toEqual(FRAGMENT);
  });

  it("no selection + Selection toggle → the grabbed atom's connected component", () => {
    // BITE: an impl that returned the whole fragment for the "selection" toggle
    // (ignoring the disconnected-piece distinction) would return FRAGMENT here.
    // COMPONENT ⊊ FRAGMENT, so the two are distinguishable.
    expect(resolveMovingSet(input([], "selection"), FRAGMENT, COMPONENT)).toEqual(COMPONENT);
    expect(resolveMovingSet(input([], "selection"), FRAGMENT, COMPONENT)).not.toEqual(FRAGMENT);
  });

  it("Fragment vs Selection agree only when the fragment is fully connected", () => {
    // A fully-bonded fragment: its component IS its whole atom set → both toggles
    // give the same whole-fragment move (the backward-compatible case).
    const wholeConnected = FRAGMENT;
    expect(resolveMovingSet(input([], "fragment"), FRAGMENT, wholeConnected)).toEqual(
      resolveMovingSet(input([], "selection"), FRAGMENT, wholeConnected),
    );
    // …and they DIFFER once a piece is broken off (component ⊊ fragment).
    expect(resolveMovingSet(input([], "fragment"), FRAGMENT, COMPONENT)).not.toEqual(
      resolveMovingSet(input([], "selection"), FRAGMENT, COMPONENT),
    );
  });

  it("returns a fresh array (callers may keep it) — not the injected reference", () => {
    const out = resolveMovingSet(input([], "fragment"), FRAGMENT, COMPONENT);
    expect(out).toEqual(FRAGMENT);
    expect(out).not.toBe(FRAGMENT);
  });
});
