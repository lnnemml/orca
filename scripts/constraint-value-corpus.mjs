// Part A2 of unit 1c — the SECOND front: constraint-value formatter parity.
//
// `constraintsBlock` renders a constraint's numeric value with `String(v)`
// (`formatValue`, the shortest round-trip decimal), NOT toFixed. The Rust
// constraints emitter needs an `fmt_value` with the same semantics. Measure
// `String(v)` vs Rust `format!("{}", v)` with the SAME bit-pattern method (doubles
// transferred as u64 bits, never decimal strings).
//
// Line: <16-hex-u64><TAB><String(v)>
//
// Run: node scripts/constraint-value-corpus.mjs [outfile]

import { writeFileSync } from "node:fs";

function bitsHex(n) {
  const b = Buffer.alloc(8);
  b.writeDoubleLE(n, 0);
  return b.readBigUInt64LE(0).toString(16).padStart(16, "0");
}
function mulberry32(a) {
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const rows = [];
const seen = new Set();
function add(v) {
  const hex = bitsHex(v);
  if (seen.has(hex)) return;
  seen.add(hex);
  rows.push(`${hex}\t${String(v)}`);
}

// Constraint-plausible explicit values: distances (Å), angles, dihedrals (° both
// signs), integer-valued and fractional, canonical short decimals.
for (const v of [
  0.5, 1, 1.234, 1.5, 1.54, 2, 2.5, 90, 109.47, 120, 180, 360, 0.1, 0.2, 0.3,
  0.615, 1.005, 8.575, 109.5, 111.111, 179.999, -60, -90, -120, -180, -0.5, 0,
]) {
  add(v);
}
// binary-tie class (odd/512) within the constraint range, both signs.
for (let odd = 1; odd < 4000; odd += 2) {
  add(odd / 512);
  add(90 + odd / 512);
  add(-(180 - odd / 512));
}
// bulk random in the constraint value range [-360, 360] (fixed seed).
{
  const rng = mulberry32(0x5eed1c);
  for (let i = 0; i < 500_000; i++) {
    add((rng() * 2 - 1) * 360);
  }
}

const out = process.argv[2] ?? "scripts/_constraint-value-corpus.txt";
writeFileSync(out, rows.join("\n") + "\n");
console.log(`wrote ${rows.length} distinct doubles to ${out}`);
