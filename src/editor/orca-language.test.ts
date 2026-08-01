import { describe, it, expect } from "vitest";

import { orcaWordPattern } from "./orca-language";

/** What Monaco's `getWordAtPosition` returns for the token at a mid-token column:
 *  the match (under our wordPattern) that contains that column. */
function wordAt(token: string): string | null {
  const re = new RegExp(orcaWordPattern.source, "g");
  const col = Math.ceil(token.length / 2); // 0-based mid index
  let m: RegExpExecArray | null;
  while ((m = re.exec(token))) {
    if (col >= m.index && col < m.index + m[0].length) return m[0];
  }
  return null;
}

describe("orcaWordPattern keeps ORCA tokens whole (Monaco's default splits them)", () => {
  // Each of these is split by Monaco's default word definition (measured in Part A);
  // a fragment like `def2` handed to the lookup is a miss that reads as "not in map".
  it.each([
    ["def2-SVP", "def2-SVP"],
    ["NEB-TS", "NEB-TS"],
    ["M06-2X", "M06-2X"],
    ["%maxcore", "%maxcore"],
    ["RIJCOSX", "RIJCOSX"],
    ["def2/J", "def2/J"],
  ])("%s → whole", (token, expected) => {
    expect(wordAt(token)).toBe(expected);
  });
});
