import { describe, it, expect } from "vitest";

import { clampDrawerWidth, readDrawerWidth, storeDrawerWidth, DRAWER_MIN } from "./drawer-width";

const fakeStore = (init: Record<string, string> = {}) => {
  const m = new Map(Object.entries(init));
  return {
    getItem: (k: string) => m.get(k) ?? null,
    setItem: (k: string, v: string) => void m.set(k, v),
    map: m,
  };
};

describe("drawer width — clamp + persistence (pure)", () => {
  it("clamps below the minimum up to DRAWER_MIN", () => {
    expect(clampDrawerWidth(100, 1600)).toBe(DRAWER_MIN);
  });
  it("clamps above 85% of the viewport", () => {
    expect(clampDrawerWidth(9999, 1000)).toBe(850);
  });
  it("passes a sane width through", () => {
    expect(clampDrawerWidth(500, 1600)).toBe(500);
  });

  it("defaults to min(480, 42% viewport) when nothing is stored", () => {
    expect(readDrawerWidth(fakeStore(), 2000)).toBe(480); // 42% of 2000 = 840, capped at 480
    expect(readDrawerWidth(fakeStore(), 1000)).toBe(420); // 42% of 1000 = 420
  });
  it("reads a stored width, re-clamped to TODAY's viewport (saved on a wider screen)", () => {
    expect(readDrawerWidth(fakeStore({ manualDrawerWidth: "700" }), 1600)).toBe(700);
    expect(readDrawerWidth(fakeStore({ manualDrawerWidth: "1400" }), 1000)).toBe(850); // > 85% → clamped
  });
  it("round-trips through the store", () => {
    const s = fakeStore();
    storeDrawerWidth(s, 640.6);
    expect(s.map.get("manualDrawerWidth")).toBe("641");
    expect(readDrawerWidth(s, 1600)).toBe(641);
  });
});
