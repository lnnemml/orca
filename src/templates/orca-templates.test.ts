import { describe, it, expect } from "vitest";

import { ORCA_TEMPLATES } from "./orca-templates";

describe("ORCA_TEMPLATES — no `%pal` (ADR-025: the run target supplies the rank count)", () => {
  it("has templates to check", () => {
    // Guards the loop below against passing vacuously on an emptied list.
    expect(ORCA_TEMPLATES.length).toBeGreaterThan(0);
  });

  it.each(ORCA_TEMPLATES.map((t) => [t.id, t.inputContent] as const))(
    "%s has zero lines starting `%pal`",
    (_id, content) => {
      // BITE: restoring `%pal nprocs 4 end` in any template turns its row red.
      expect(content.split(/\r?\n/).filter((l) => l.trimStart().startsWith("%pal"))).toEqual([]);
      expect(content).toContain("%maxcore 2000"); // the neighbouring directive stays
    },
  );
});
