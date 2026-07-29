import { describe, it, expect } from "vitest";

import { formatXtbProgress } from "./xtb-progress";

describe("formatXtbProgress", () => {
  it("shows the cycle + elapsed once xtb reports a cycle", () => {
    expect(formatXtbProgress(5, 12)).toBe("optimization cycle 5 · 12s");
  });

  it("shows 'starting…' + elapsed before the first cycle (the hang window)", () => {
    expect(formatXtbProgress(null, 3)).toBe("starting… · 3s");
    expect(formatXtbProgress(null, 0)).toBe("starting… · 0s");
  });
});
