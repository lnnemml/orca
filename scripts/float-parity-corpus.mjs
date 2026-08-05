// Part A of Phase 4.2 unit 1c — the float-formatting parity PROBE (generator half).
//
// Golden byte-identity of the Rust coordinate emit with the TS emit rests on
// JS `Number.prototype.toFixed(8)` and Rust `format!("{:.8}", x)` producing the
// SAME string for the SAME f64. They are different rounding algorithms, so this
// must be MEASURED, not assumed — on convenient coords they agree "for an unknown
// reason", and a divergence would surface later (1e) as a fake byte-identity.
//
// METHOD (the load-bearing bit): every double is transferred to the Rust reader
// as its exact **u64 bit pattern** (Buffer.writeDoubleLE), NEVER a decimal string —
// a decimal string re-parses with its own rounding, which would make the corpus
// measure a parser round-trip instead of the two FORMATTERS. The expected strings
// on each line are the JS output we are comparing Rust against, not a transport of
// the value.
//
// Line format:  <16-hex-u64><TAB><js toFixed(8)><TAB><js formatCoord = toFixed(8).padStart(14)>
//
// Run:  node scripts/float-parity-corpus.mjs [outfile]
// Output is large (~1M lines) and reproducible from a fixed seed → gitignored.

import { writeFileSync } from "node:fs";

// mergeToAtomLines' exact coordinate formatter (src/scene/scene.ts).
function formatCoord(n) {
  return n.toFixed(8).padStart(14);
}

// Exact IEEE-754 bits of a double, as 16 lowercase hex chars (big-endian value).
function bitsHex(n) {
  const b = Buffer.alloc(8);
  b.writeDoubleLE(n, 0);
  return b.readBigUInt64LE(0).toString(16).padStart(16, "0");
}

// Deterministic PRNG (mulberry32) — fixed seed for a reproducible corpus.
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
  if (seen.has(hex)) return; // one row per distinct bit pattern
  seen.add(hex);
  rows.push(`${hex}\t${v.toFixed(8)}\t${formatCoord(v)}`);
}

// ── Adversarial explicit values ──────────────────────────────────────────────
// signed zero + tiny values that round to zero (sign-of-zero is a real risk:
// Rust prints -0.0 as "-0.00000000", JS toFixed drops the sign).
add(0.0);
add(-0.0);
for (const t of [1e-12, 1e-9, 4.9e-9, 5e-9, 5.1e-9, 9.9e-9]) {
  add(t);
  add(-t);
}
// TRUE binary halves at the 8th decimal — the real rounding-tie class. A double x
// with x·10^8 exactly a half-integer means x = odd/2^9 = odd/512 (since 10^8 = 2^8·5^8
// and the ·0.5 adds one power of two). These are where JS toFixed (round-half-AWAY)
// and Rust {:.8} (round-half-to-EVEN) diverge — but only when the 8th digit is even
// (odd → both go up). The earlier (k+0.5)·1e-8 values are NOT binary halves and cannot
// probe this (corrected in unit 1c Part A2).
for (let odd = 1; odd < 4000; odd += 2) {
  add(odd / 512);
  add(-(odd / 512));
}
// integer-offset ties + near the padStart(14) width boundaries.
for (const k of [0, 1, 9, 99, 999, 9999]) {
  for (const odd of [1, 5, 7, 11, 2049]) {
    add(k + odd / 512);
    add(-(k + odd / 512));
  }
}
// classic decimal-tie traps (x.xx5 that the naive reader expects to round up).
for (const t of [1.005, 8.575, 2.675, 0.615, 1.255, 0.125, 0.375, 1.0000000050]) {
  add(t);
  add(-t);
}
// padStart(14) width boundaries: results of 13 / 14 / 15 chars, both signs.
for (const t of [
  9, 99, 999, 9999, 99999, 999999, // "9999.00000000"=13, "99999.00000000"=14, …
  99.99999999, 9999.99999999, 12345.6789, 999.123456789,
]) {
  add(t);
  add(-t);
}
// typical chemical coordinates, both signs, a range of magnitudes.
{
  const rng = mulberry32(0xc0ffee);
  for (let i = 0; i < 5000; i++) {
    const mag = [0.1, 1, 10, 100][i % 4];
    add((rng() * 2 - 1) * mag);
  }
}

// ── Bulk random doubles in [-1000, 1000] (fixed seed, reproducible) ──────────
{
  const rng = mulberry32(0x0badf00d);
  for (let i = 0; i < 1_000_000; i++) {
    add((rng() * 2 - 1) * 1000);
  }
}

const out = process.argv[2] ?? "scripts/_float-parity-corpus.txt";
writeFileSync(out, rows.join("\n") + "\n");
console.log(`wrote ${rows.length} distinct doubles to ${out}`);
