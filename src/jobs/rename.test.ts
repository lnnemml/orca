import { describe, it, expect } from "vitest";

import { sanitizeRenameInput } from "./rename";

describe("sanitizeRenameInput — the UX echo of the Rust rename_job guard", () => {
  it("trims leading/trailing whitespace", () => {
    expect(sanitizeRenameInput("  foo  ")).toBe("foo");
    expect(sanitizeRenameInput("\tbar\n")).toBe("bar");
  });

  it("returns null for an empty or whitespace-only input (what Rust refuses)", () => {
    expect(sanitizeRenameInput("")).toBeNull();
    expect(sanitizeRenameInput("   ")).toBeNull();
    expect(sanitizeRenameInput("\t\n ")).toBeNull();
  });

  it("passes a non-empty title through as its trimmed value", () => {
    expect(sanitizeRenameInput("HCN opt")).toBe("HCN opt");
  });

  it("does NOT collapse internal whitespace — same contract as Rust str::trim", () => {
    // Rust's `.trim()` keeps internal spaces; the echo must too, or the two guards diverge
    // ("frontend rewrote it, Rust stored something else").
    expect(sanitizeRenameInput("  a  b  ")).toBe("a  b");
  });
});
