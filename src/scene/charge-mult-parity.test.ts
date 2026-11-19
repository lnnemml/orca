import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, expect, it } from "vitest";
import { sceneFromOrcaInput, totalCharge } from "./scene";

/**
 * The TypeScript half of the charge/multiplicity locator parity (ADR-027 decision 4, clarification
 * C1). The Rust half is `src-tauri/src/mcp/charge_mult_tests.rs`; both read the same cases file.
 *
 * - `wellFormed`: `sceneFromOrcaInput` and the Rust `locate_charge_mult` give the same charge and
 *   multiplicity.
 * - `malformed`: the Rust locator refuses (the MCP path turns that into REFUSED); this locator falls
 *   back to (0, 1) or returns null, as the human path expects. The test pins what TS gives, so the
 *   deliberate divergence stays visible if either side changes.
 */

interface Case {
  name: string;
  input: string;
}
interface WellFormed extends Case {
  charge: number;
  multiplicity: number;
}
interface Malformed extends Case {
  ts: { charge: number; multiplicity: number } | null;
}

const cases = JSON.parse(
  readFileSync(resolve(process.cwd(), "src-tauri/tests/fixtures/mcp/charge_mult_cases.json"), "utf8"),
) as { wellFormed: WellFormed[]; malformed: Malformed[] };

function located(input: string): { charge: number; multiplicity: number } | null {
  const scene = sceneFromOrcaInput(input);
  return scene === null ? null : { charge: totalCharge(scene), multiplicity: scene.multiplicity };
}

describe("charge/multiplicity locator parity with Rust (ADR-027 C1)", () => {
  it("has cases on both sides of the divergence", () => {
    expect(cases.wellFormed.length).toBeGreaterThanOrEqual(5);
    expect(cases.malformed.length).toBeGreaterThanOrEqual(5);
  });

  for (const c of cases.wellFormed) {
    it(`well-formed: ${c.name}`, () => {
      expect(located(c.input)).toEqual({ charge: c.charge, multiplicity: c.multiplicity });
    });
  }

  for (const c of cases.malformed) {
    it(`malformed (Rust refuses, TS gives ${JSON.stringify(c.ts)}): ${c.name}`, () => {
      expect(located(c.input)).toEqual(c.ts);
    });
  }
});
