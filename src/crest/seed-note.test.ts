import { describe, it, expect } from "vitest";

import { crestSeedNote } from "./seed-note";
import { SOLVENT_LIBRARY } from "./solvents";

describe("crestSeedNote — the charge-aware seed label (always present)", () => {
  it("seed_note_warns_for_nonzero_charge", () => {
    // BITE: a nonzero intended charge is LOUDLY flagged — the grown cluster is neutral, so
    // its energy is the wrong species'. A version that treated all charges the same fails.
    const n = crestSeedNote(-1);
    expect(n.severity).toBe("warning");
    expect(n.text.toLowerCase()).toContain("neutral");
    expect(n.text.toLowerCase()).toContain("seed");
    expect(n.text).toContain("SMD");
    // The submitted charge is named in the guidance.
    expect(n.text).toContain("-1");
  });

  it("seed_note_is_coarse_for_neutral", () => {
    const n = crestSeedNote(0);
    expect(n.severity).toBe("note");
    // Still steers to an ORCA + SMD refinement — never presented as the answer.
    expect(n.text).toContain("SMD");
    expect(n.text.toLowerCase()).toContain("seed");
  });
});

describe("SOLVENT_LIBRARY — exactly the two probed solvents", () => {
  it("solvent_library_has_water_and_methanol_with_alpb_names", () => {
    expect(SOLVENT_LIBRARY).toHaveLength(2);
    const byName = new Map(SOLVENT_LIBRARY.map((s) => [s.alpbName, s]));
    expect(byName.has("water")).toBe(true);
    expect(byName.has("methanol")).toBe(true);
    for (const s of SOLVENT_LIBRARY) {
      expect(s.xyz.trim().length).toBeGreaterThan(0);
      // The xyz's first line is the atom count (a real geometry, not a placeholder).
      expect(Number(s.xyz.trim().split(/\r?\n/)[0])).toBeGreaterThan(0);
    }
  });
});
