# JS `toFixed(8)` vs Rust `{:.8}` — coordinate-formatter parity (measured)

**Verdict: the rounding algorithms AGREE** across 1,012,786 adversarial + random
doubles — **0 rounding divergences**. The **only** divergence is **signed zero**:
`-0.0` formats as `"0.00000000"` in JS (`toFixed` drops the sign) and
`"-0.00000000"` in Rust (`{:.8}` keeps it). That is one deterministic case with a
one-line rule, not a rounding-algorithm mismatch.

This page supports the ADR-016 decision (the order-bearing emit moves to Rust) —
its golden test asserts **byte-identity** of the Rust coordinate block with the TS
emit, and byte-identity is only safe if the two float formatters agree. Home is
`wiki/architecture/` per the measurement-placement rule (a measurement supporting a
specific decision lives with that decision, not under `wiki/orca/` — this is not
about ORCA's behaviour).

## Why this had to be measured, not assumed

`mergeToAtomLines` (`src/scene/scene.ts`) formats each coordinate as
`n.toFixed(8).padStart(14)`. The Rust emit (unit 1c, ADR-016) must reproduce that
**byte-for-byte**. But `Number.prototype.toFixed` (ECMA-262: "if two `n` are
equally near, pick the **larger** `n`" — round-half-up) and Rust's `{:.8}`
(`flt2dec`, round-**half-to-even**) are different rounding algorithms operating on
the same IEEE-754 double. On convenient coordinates they coincide — so a golden
test on hand-picked fixtures would pass *for an unknown reason*, and a divergence
would surface only later (unit 1e, when the authoritative and display emits are
compared) as a **fake** byte-identity. Rule #10: verify by a run.

## Method — bit patterns, never decimal strings (the load-bearing choice)

The generator (`scripts/float-parity-corpus.mjs`) transfers every double to the
Rust reader (`scripts/float_parity_reader.rs`) as its exact **u64 bit pattern**
(`Buffer.writeDoubleLE` → hex; Rust `f64::from_bits`). It never sends the value as
a decimal string: a decimal string re-parses with its own rounding, so the corpus
would measure a *parser round-trip*, not the two *formatters*. The decimal strings
on each corpus line are the JS output being compared against, not a transport of
the value. The Rust reader reconstructs the exact double from the bits, formats it
with `{:.8}` and `{:>14.8}`, and compares byte-for-byte.

## Corpus (fixed seed, reproducible)

- **Adversarial:** `+0.0`, `-0.0`; tiny values that round to zero (`±1e-12`,
  `±5e-9`, …); 2000 near-half points at the 8th decimal (`(k+0.5)·1e-8`) — the
  round-half stress; classic decimal-tie traps (`1.005`, `8.575`, `0.125`, …);
  `padStart(14)` width boundaries (results of 13 / 14 / 15 chars, both signs); and
  5000 typical chemical coordinates (0.1–100 Å, both signs).
- **Bulk:** 1,000,000 random doubles in `[-1000, 1000]` (mulberry32, seed
  `0x0badf00d`).
- Total distinct bit patterns compared: **1,012,786**.

## Result

| comparison | divergences / 1,012,786 |
|---|---|
| `toFixed(8)` vs `{:.8}` — **rounding** | **0** |
| `toFixed(8)` vs `{:.8}` — **sign-of-zero** | **1** |
| padded `formatCoord` vs `{:>14.8}` | 1 (the same `-0.0`) |

The single divergence, verbatim from the run:

```
bits=0x8000000000000000  js.toFixed(8)="0.00000000"  rust{:.8}="-0.00000000"
```

`0x8000000000000000` is exactly `-0.0`. No other value in the corpus — including
every half-grid and decimal-tie case — diverges. The round-half-up vs
round-half-to-even difference **did not surface**: for 8 decimals, a binary double
almost never lands exactly on a decimal half, so both algorithms round the same
real number the same way.

## Consequence / open decision (unit 1c, Part A → architect)

The dangerous class (rounding) is empty, so **golden byte-identity is viable**. The
`-0.0` case needs one rule, and which rule is the architect's call:

- **(A) Rust normalizes `-0.0 → +0.0`** in the coordinate formatter (e.g.
  `let x = if x == 0.0 { 0.0 } else { x };`, which maps `-0.0` to `+0.0` since
  `-0.0 == 0.0`). Matches JS, keeps the strict byte-identity golden. **Recommended**
  — minimal, deterministic, and the only place the two ever disagree.
- **(B)** Weaken the golden to token-numeric equality against a format spec.
  Heavier; loses the exact-bytes guarantee 1e wants.

Whether `-0.0` even reaches the emit is itself unlikely (parsed coords are `+0.0`
for `"0.0"`; `x + (-x)` yields `+0.0` in IEEE) — but "unlikely" is exactly the
silent-later-divergence trap this probe exists to close, so the rule is stated, not
left to chance.

## The permanent gate

Once the rule lands, the corpus comparison becomes a permanent **`#[ignore]`** test
in `orcastudio-core` (run by hand: it needs Node to regenerate the corpus), and the
adversarial values above are baked into the committed golden fixtures so the routine
`cargo test` still exercises them without Node. Re-run when the Rust or Node version
changes:

```
node scripts/float-parity-corpus.mjs scripts/_float-parity-corpus.txt
rustc -O scripts/float_parity_reader.rs -o /tmp/fpr && /tmp/fpr scripts/_float-parity-corpus.txt
```
