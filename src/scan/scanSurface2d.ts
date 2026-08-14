//! 2D relaxed-surface scan parser — pure, node-tested, React-free (Phase 4.5 Stage 4b).
//!
//! Parses ORCA's `input.relaxscanact.dat` — the **ACTUAL-energy** curve; NEVER
//! `relaxscanscf.dat`, which is **zeros for an XTB scan** (reading scf would render a flat,
//! blank surface). The `.dat` is space-delimited triples `c1  c2  E_act`, **row-major with
//! OUTER=coord1 / INNER=coord2** (measured — pinned in `wiki/orca/scan.md`).
//!
//! The row-major → point-file map is **handoff-critical**: a grid node `(i1, i2)` → the
//! 1-based `.dat` row → point-file `input.<row>.xyz` → `read_scan_geometries()[row-1]`. A
//! transposed parse (inner/outer swapped) or an off-by-one would BOTH mirror the plotted
//! surface AND hand OptTS the WRONG node's geometry — the silent identity-error class the
//! export unit defended against. `nodeRow` is that map, pinned by a bite against the real
//! 10×10 (`scanSurface2d.test.ts`).

export interface ScanSurface2d {
  /** Unique coordinate-1 values in first-seen (OUTER-loop) order. */
  axis1: number[];
  /** Unique coordinate-2 values in first-seen (INNER-loop) order. */
  axis2: number[];
  /** `energies[i1][i2]` = actual energy (Eh) at `(axis1[i1], axis2[i2])`. */
  energies: number[][];
  /** The 1-based `.dat` row for a grid node — equal to the point-file `NNN`
   *  (`input.<row>.xyz`) and to `read_scan_geometries()[row - 1]`. Row-major:
   *  `i1 * N2 + i2 + 1`. */
  nodeRow(i1: number, i2: number): number;
}

interface Row {
  c1: number;
  c2: number;
  e: number;
}

/** Parse the flat triples. All-or-nothing: `null` on any non-`c1 c2 E` line (so we never
 *  half-understand a surface). Blank lines (incl. the fixture's leading/trailing) are skipped. */
function parseRows(datText: string): Row[] | null {
  const rows: Row[] = [];
  for (const raw of datText.split("\n")) {
    const line = raw.trim();
    if (line.length === 0) continue;
    const toks = line.split(/\s+/);
    if (toks.length !== 3) return null;
    const c1 = Number(toks[0]);
    const c2 = Number(toks[1]);
    const e = Number(toks[2]);
    if (!Number.isFinite(c1) || !Number.isFinite(c2) || !Number.isFinite(e)) return null;
    rows.push({ c1, c2, e });
  }
  return rows.length > 0 ? rows : null;
}

/** First-seen unique values. Identical source strings parse to identical floats, so exact
 *  Set membership is safe (the `.dat` writes the same coordinate string in every row of a
 *  block). */
function uniqueInOrder(vals: number[]): number[] {
  const seen = new Set<number>();
  const out: number[] = [];
  for (const v of vals) {
    if (!seen.has(v)) {
      seen.add(v);
      out.push(v);
    }
  }
  return out;
}

/**
 * Parse a 2D relaxed-surface scan `.dat` into a grid. `null` on malformed input OR a grid
 * that is not a clean rectangular product (`rows.length !== N1 × N2`) OR a layout that does
 * not match **row-major OUTER=coord1 / INNER=coord2** — refusing to mis-plot rather than
 * silently transpose. The grid need NOT be square (`N1 ≠ N2` is fine).
 */
export function parseScanSurface2d(datText: string): ScanSurface2d | null {
  const rows = parseRows(datText);
  if (!rows) return null;

  const axis1 = uniqueInOrder(rows.map((r) => r.c1));
  const axis2 = uniqueInOrder(rows.map((r) => r.c2));
  const n1 = axis1.length;
  const n2 = axis2.length;
  if (n1 < 1 || n2 < 1 || rows.length !== n1 * n2) return null;

  const energies: number[][] = [];
  for (let i1 = 0; i1 < n1; i1++) {
    const line: number[] = [];
    for (let i2 = 0; i2 < n2; i2++) {
      const row = rows[i1 * n2 + i2];
      // Pin the row-major OUTER=coord1 / INNER=coord2 layout: a transposed or ragged file
      // fails HERE (→ null) instead of plotting a mirrored surface + mis-mapping nodes.
      if (row.c1 !== axis1[i1] || row.c2 !== axis2[i2]) return null;
      line.push(row.e);
    }
    energies.push(line);
  }

  return {
    axis1,
    axis2,
    energies,
    nodeRow: (i1, i2) => i1 * n2 + i2 + 1,
  };
}
