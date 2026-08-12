import { describe, it, expect } from "vitest";

import { buildClusterReoptInput } from "./reopt";

/** A small grown-cluster stand-in — a 5-atom mixed set with a definite element order. */
const CLUSTER = {
  elements: ["B", "H", "H", "O", "C"],
  xyz_angstrom: [
    [0, 0, 0],
    [1.2, 0, 0],
    [-1.2, 0, 0],
    [3.0, 0, 0],
    [3.0, 1.4, 0],
  ] as [number, number, number][],
};

const bangLine = (inp: string) =>
  inp.split(/\r?\n/).find((l) => l.trim().startsWith("!")) ?? "";
const xyzHeader = (inp: string) =>
  inp.split(/\r?\n/).find((l) => l.trim().startsWith("* xyz")) ?? "";
/** The element symbols of the emitted `* xyz` coordinate block, in order. */
const emittedElements = (inp: string) => {
  const lines = inp.split(/\r?\n/);
  const start = lines.findIndex((l) => l.trim().startsWith("* xyz"));
  const out: string[] = [];
  for (let i = start + 1; i < lines.length; i++) {
    const t = lines[i].trim();
    if (t === "*" || t === "") break;
    out.push(t.split(/\s+/)[0]);
  }
  return out;
};

describe("buildClusterReoptInput — seed → ORCA re-opt at the SOLUTE charge + SMD", () => {
  it("cluster_reopt_uses_explicit_solute_charge", () => {
    // BITE: the re-opt charge is the PASSED solute charge (−1), on the neutral-grown cluster —
    // never 0, never read from the cluster. A version that defaulted to 0 emits `* xyz 0 1`.
    const inp = buildClusterReoptInput(CLUSTER, -1, 1, { solvent: "methanol" });
    expect(xyzHeader(inp)).toBe("* xyz -1 1");
  });

  it("cluster_reopt_emits_smd_opt_freq", () => {
    // BITE: the xtb-ALPB seed becomes a real solvated ΔG calculation — SMD + Opt + Freq.
    const line = bangLine(buildClusterReoptInput(CLUSTER, -1, 1, { solvent: "methanol" }));
    expect(line).toContain("SMD(methanol)");
    expect(line).toContain("Opt");
    expect(line).toContain("Freq");
    expect(line).toContain("r2SCAN-3c"); // the default composite method
  });

  it("cluster_reopt_preserves_atom_order", () => {
    const inp = buildClusterReoptInput(CLUSTER, 0, 1, { solvent: "water" });
    expect(emittedElements(inp)).toEqual(CLUSTER.elements);
  });

  it("cluster_reopt_provenance_comment_present", () => {
    const inp = buildClusterReoptInput(CLUSTER, -1, 1, { solvent: "methanol" });
    // Begins with a `#` provenance line…
    expect(inp.split(/\r?\n/)[0].startsWith("#")).toBe(true);
    // …naming the solvent, the solute charge, and that this is a neutral-grown SEED refined w/ SMD.
    const header = inp.slice(0, inp.indexOf("!"));
    expect(header).toContain("methanol");
    expect(header).toContain("-1");
    expect(header.toLowerCase()).toContain("seed");
    expect(header.toLowerCase()).toContain("neutral");
    expect(header).toContain("SMD");
  });

  it("multiplicity is the passed solute value, not a default", () => {
    // A radical solute (doublet) keeps its multiplicity — solvent is closed-shell.
    const inp = buildClusterReoptInput(CLUSTER, 0, 2, { solvent: "water" });
    expect(xyzHeader(inp)).toBe("* xyz 0 2");
  });

  it("throws on an empty cluster (never emits a zero-atom job)", () => {
    expect(() =>
      buildClusterReoptInput({ elements: [], xyz_angstrom: [] }, -1, 1, { solvent: "methanol" }),
    ).toThrow();
  });
});
