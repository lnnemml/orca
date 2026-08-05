import { describe, it, expect } from "vitest";

import { canExplain } from "./explain-open";

// TASK 3 appearance condition, pure: a usable key AND a resolved section. No key → the button
// is ABSENT (not an error on click), so the predicate is false without a usable key state.
describe("canExplain — the Explain-button appearance condition", () => {
  it("needs BOTH a usable key state and a section", () => {
    expect(canExplain("stored-in-keyring", true)).toBe(true);
    expect(canExplain("from-environment", true)).toBe(true);
  });
  it("is false without a usable key (absent / unavailable / unknown)", () => {
    expect(canExplain("absent", true)).toBe(false);
    expect(canExplain("unavailable", true)).toBe(false);
    expect(canExplain(undefined, true)).toBe(false);
  });
  it("is false without a resolved section, even with a key", () => {
    expect(canExplain("stored-in-keyring", false)).toBe(false);
  });
});
