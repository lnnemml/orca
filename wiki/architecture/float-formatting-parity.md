# JS vs Rust number formatting — emit parity (measured)

Two formatters must agree byte-for-byte for the ADR-016 golden test (Rust emit ==
TS emit). Both were measured with the **bit-pattern** method (below). Results:

| formatter pair | where used | result |
|---|---|---|
| JS `toFixed(8)` vs Rust `{:.8}` | coordinate block (`mergeToAtomLines`) | **diverges** on signed-zero **and** the true tie class (odd/512) — **solved** by `fmt_coord` (0 divergences) |
| JS `String(v)` vs Rust `format!("{}")` | `%geom` constraint value (`constraintsBlock`) | **diverges** on 17-digit shortest-round-trip ties — **open** (architect's rule) |

Home is `wiki/architecture/` per the measurement-placement rule (supports the
ADR-016 decision, not about ORCA behaviour).

## Method — bit patterns, never decimal strings (the load-bearing choice)

The generators transfer every double to the Rust reader as its exact **u64 bit
pattern** (`Buffer.writeDoubleLE` → hex; Rust `f64::from_bits`), never a decimal
string — a decimal string re-parses with its own rounding, so the corpus would
measure a *parser round-trip*, not the two *formatters*. The decimal strings on
each corpus line are the JS output being compared against, not a transport of the
value. Scripts: `scripts/float-parity-corpus.mjs` + `float_parity_reader.rs`
(coordinates); `scripts/constraint-value-corpus.mjs` + `value_parity_reader.rs`
(constraint values). Toolchain: Node (V8) + rustc 1.97.1.

---

## Front 1 — coordinate block: `toFixed(8)` vs `{:.8}`

`mergeToAtomLines` formats each coordinate as `n.toFixed(8).padStart(14)`. The Rust
emit must reproduce that byte-for-byte.

### Correction (Part A2): the first corpus could not see the real ties

The first pass reported "0 rounding divergences" — **a false claim in scope**. Its
tie stress used `(k+0.5)·1e-8`, which are **not** representable binary values at the
8th decimal, so they can never expose a round-half tie. **The true 8th-decimal ties
are `x = odd/512`:** `x·10⁸ + ½ ∈ ℤ` for a representable `x` iff `x = odd/2⁹`
(because `10⁸ = 2⁸·5⁸`, and the `½` adds the ninth power of two). At such `x`, JS
`toFixed` rounds half **away from zero** while Rust `{:.8}` rounds half **to even**,
so they diverge **when the 8th digit is even** (odd → both go up, no divergence).

Measured, my toolchain (matches the architect's):

```
1/512  JS "0.00195313"  Rust {:.8} "0.00195312"   (diverge: 8th digit 2, even)
3/512  JS "0.00585938"  Rust {:.8} "0.00585938"   (agree:   8th digit 7→8, odd)
5/512  JS "0.00976563"  Rust {:.8} "0.00976562"   (diverge)
-1e-12 JS "-0.00000000" Rust {:.8} "-0.00000000"  (agree: sign kept in both)
-0.0   JS "0.00000000"  Rust {:.8} "-0.00000000"  (diverge: signed zero)
```

### Corpus and result

Corpus (fixed seed, 1,008,832 distinct doubles): `±0.0`; tiny round-to-zero
values; **the odd/512 tie class both signs**, with integer offsets and near the
`padStart(14)` width boundaries; decimal-tie traps; typical chem coords; 1,000,000
random in `[-1000,1000]`.

| comparison | divergences / 1,008,832 |
|---|---|
| **bare** `{:>14.8}` vs JS `formatCoord` | **2025** (sign-of-zero 1, tie 2024) |
| **`fmt_coord`** vs JS `formatCoord` | **0** |

The bare Rust formatter is wrong 2025 times — that is the negative control, the
executable proof of *why* `fmt_coord` exists.

### The rule that ships (`orcastudio-core::emit::fmt_coord`)

One function; every coordinate goes through it, no direct `{:.8}` on a coordinate
elsewhere. Three parts (architect's rule, unit 1c):

1. **Signed zero:** `let x = if x == 0.0 { 0.0 } else { x };`. `x == 0.0` is true
   for both `±0.0` and nothing else, so `-0.0 → +0.0` and `-1e-12` (not zero) keeps
   its sign. Not `.abs()`, not string post-processing.
2. **Exact halves (`x = odd/512`):** detect with `y = |x|·512.0` — scaling by a
   power of two **never rounds**, so there are **no false positives**; tie iff `y`
   is an **odd integer** (guarded `y < 2⁵³`). On a tie render **away from zero**:
   `m = (|x|·1e8).floor() + 1` — `|x|·1e8` at a tie equals `odd·195312.5`, a
   half-integer `< 2⁵³`, so it is exact and `floor()+1` is the larger `n`; render
   `m` as a signed fixed-8 string.
3. **Else:** plain `{:.8}`. Then `padStart(14)` (`{:>14}`).

`fmt_coord` is verified byte-identical to JS across the whole corpus **including
negative ties** (`-1/512 → "-0.00195313"`). The corpus comparison becomes a
permanent `#[ignore]` gate in `orcastudio-core` (needs Node to regenerate); the
odd/512 and `-0.0` values are also baked into the committed golden fixtures so
routine `cargo test` exercises them without Node.

---

## Front 2 — constraint value: `String(v)` vs `format!("{}")`

`constraintsBlock` renders a constraint's numeric value with `formatValue(v) =
String(v)` (shortest round-trip), **not** `toFixed`. Measured with the same
bit-pattern method over constraint-plausible values (0.5–360 both signs, full double
precision, the odd/512 tie class, + 500,000 random in `[-360,360]`; 505,972
distinct):

| comparison | divergences / 505,972 |
|---|---|
| JS `String(v)` vs Rust `format!("{}")` | **14** |

**Every divergence is the same class:** a full-precision value whose **shortest
round-trip needs 17 significant digits**, where the 17th digit is itself ambiguous —
two adjacent 17-digit strings both round-trip to the same double, and JS (V8 dtoa)
picks the lower while Rust (`flt2dec`) picks the higher. Verbatim:

```
bits=0xc06909bb40000000  String(v)="-200.30410766601562"  rust{} "-200.30410766601563"
bits=0x405aa7ea80000000  String(v)= "106.62368774414062"  rust{}  "106.62368774414063"
bits=0xc037656500000000  String(v)= "-23.396072387695312" rust{}  "-23.396072387695313"
```

These arise only for **raw full-precision doubles** (e.g. a bond length measured off
the geometry). A user-typed value is preserved verbatim by `valueText` and never
reaches `formatValue`; a canonical short number (`1.5`, `90`, `109.47`) has a unique
shortest form and does not diverge.

**Named boundary (not left open):** JS `String` switches to **exponential** notation
for `|v| ≥ 1e21` and `0 < |v| < 1e-6` (`(1e-7).toString() === "1e-7"`); Rust `{}`
**never** uses exponential. The constraint value range (~0.5–360°/Å) is far inside
`[1e-6, 1e21)`, so this boundary is unreachable for constraints — but it is a real
`String(v)` vs `{}` divergence outside that range and is stated here rather than
discovered later.

### Status: OPEN — architect's rule (Part A2 STOP)

Per the second-front protocol (`>0 divergences → STOP, report bits`), `fmt_value`
is **not** baked yet. `fmt_value = format!("{}")` is correct for canonical/`valueText`
values but diverges on raw 17-digit doubles. Candidate rules (for decision):

- **(B1) Constrain the input:** the core `Constraint` value, when present, is either
  a `valueText` (user's exact text, emitted verbatim) or a number whose `String(v)`
  is short — a "freeze at the measured value" omits the number entirely (the ORCA
  idiom: `{B i j C}` with no value freezes at the current geometry), so a raw
  17-digit double never reaches the formatter. Emit relies on / asserts this.
  Matches how the TS path already behaves. **Recommended.**
- **(B2)** Rust reimplements V8's dtoa tie-break — heavy, brittle across engine/rustc
  versions.
- **(B3)** Weaken the constraint-value golden to numeric equality — loses byte-identity.
