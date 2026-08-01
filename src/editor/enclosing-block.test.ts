import { describe, it, expect } from "vitest";

import { enclosingBlock } from "./enclosing-block";

// Line numbers are 0-based; each case is written so the asserted line is obvious.
const at = (text: string, line: number) => enclosingBlock(text, line);

describe("enclosingBlock — the qualifier for the hover (each A1 trap a separate test)", () => {
  it("basic multi-line %scf ... end", () => {
    const t = ["%scf", "  MaxIter 200", "end", "! B3LYP"].join("\n");
    expect(at(t, 1)).toBe("%scf"); // MaxIter is inside %scf
    expect(at(t, 2)).toBe("%scf"); // on the `end` line, still closing %scf's interior
    expect(at(t, 3)).toBeNull(); // the `!` line is top level
  });

  it("TRAP: `%maxcore 3000` is a single-line directive, NOT an open block", () => {
    // The most likely way to break everything silently: if %maxcore "opens", every
    // line below is forever "inside %maxcore".
    const t = ["%maxcore 3000", "%scf", "  MaxIter 200", "end"].join("\n");
    expect(at(t, 1)).toBeNull(); // %scf line: NOT inside %maxcore
    expect(at(t, 2)).toBe("%scf"); // MaxIter is inside %scf, not %maxcore
  });

  it("single-line block `%pal nprocs 4 end` opens and closes on one line", () => {
    const t = ["%pal nprocs 4 end", "%scf", "  MaxIter 200", "end"].join("\n");
    expect(at(t, 1)).toBeNull(); // after %pal closed → top level
    expect(at(t, 2)).toBe("%scf");
  });

  it("coordinate block `* xyz 0 1` ... `*` closes with `*`, not `end`", () => {
    const t = ["%scf", "end", "* xyz 0 1", "  C 0 0 0", "  O 0 0 1.2", "*", "%mp2", "  x", "end"].join(
      "\n",
    );
    expect(at(t, 3)).toBeNull(); // inside the coordinate block
    expect(at(t, 4)).toBeNull();
    expect(at(t, 7)).toBe("%mp2"); // after the coord block, inside %mp2
  });

  it("external `* xyzfile ...` is self-contained (no coord toggle)", () => {
    const t = ["* xyzfile 0 1 mol.xyz", "%scf", "  MaxIter 1", "end"].join("\n");
    expect(at(t, 2)).toBe("%scf"); // not stuck "inside coordinates"
  });

  it("`#` comments containing % and end do not open/close blocks", () => {
    const t = ["%scf", "  # %casscf end end end are just words here", "  MaxIter 1", "end"].join("\n");
    expect(at(t, 2)).toBe("%scf"); // the comment neither opened %casscf nor closed %scf
  });

  it("quoted strings with % and end are inert (`%moinp \"prev.gbw\"`)", () => {
    // %moinp is itself a no-end directive AND its value could contain tokens.
    const t = ['%moinp "prev_%scf_end.gbw"', "%casscf", "  nel 4", "end"].join("\n");
    expect(at(t, 0)).toBeNull(); // on the %moinp line: top level
    expect(at(t, 2)).toBe("%casscf"); // the string did not open %scf
  });

  it("NESTING (verified real, not assumed): `%geom … constraints … end end`", () => {
    const t = [
      "%geom",
      "  Constraints",
      "    {B 0 1 C}",
      "  end", // closes Constraints (a bare sub-block)
      "  MaxStep 0.1",
      "end", // closes %geom
    ].join("\n");
    expect(at(t, 2)).toBe("%geom"); // inside the constraint list → inside %geom
    // After the sub-block's `end`, the scanner is CONSERVATIVE: it pops %geom early and
    // returns null rather than risk a wrong block. Null > wrong (the unit's whole point).
    expect(at(t, 4)).toBeNull();
  });

  it("distinguishes sibling blocks — the exact wrong-answer risk", () => {
    const t = ["%scf", "  MaxIter 1", "end", "%casscf", "  MaxIter 1", "end"].join("\n");
    expect(at(t, 1)).toBe("%scf");
    expect(at(t, 4)).toBe("%casscf"); // NOT %scf — the confident-wrong case
  });

  it("token on the opener line itself is not 'inside' its own block", () => {
    const t = ["%scf", "  MaxIter 1", "end"].join("\n");
    expect(at(t, 0)).toBeNull(); // cursor on `%scf` → the block token, parent is top level
  });
});
