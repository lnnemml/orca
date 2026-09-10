# 024 — A malformed dipole block silently fabricated `(0,0,0)` instead of null — FIXED

**Status: FIXED (2026-09-10).** `dipole()` is now honest-or-absent: it distinguishes an absent
block (`Ok(None)`, normal) from a present-but-malformed one (`Err → parse_warnings`), and never
fabricates a dipole component. The fix ships with a negative control that bites (below).

## Symptom (was latent — no observed failure, caught by inspection)

`src-tauri/src/parse/property.rs`, in the old `dipole()`:

```rust
let total = b.prop("dipoleTotal").map(|p| p.numbers()).unwrap_or_default();
let total_au = [
    *total.first().unwrap_or(&0.0),
    *total.get(1).unwrap_or(&0.0),
    *total.get(2).unwrap_or(&0.0),
];
```

If the `SCF_Dipole_Moment` block was **present** but its `dipoleTotal` property was **missing or
truncated** (`prop("dipoleTotal")` was `None`, or `numbers()` yielded fewer than three components),
the `unwrap_or_default()` / `unwrap_or(&0.0)` chain silently substituted a fabricated dipole vector
of `(0.0, 0.0, 0.0)`.

## Why this was a defect — a default masquerading as data

`(0, 0, 0)` is **indistinguishable from a valid zero dipole** (a symmetric molecule genuinely has a
near-zero dipole). So a *present-but-broken* dipole block was recorded as if the physics said "zero",
not "unparseable". This is the **honest-or-absent violation** of domain rule #9: a dropped/failed
value must be recorded as absent WITH A REASON, never as a plausible invented stand-in. It is the
**−60127 class** (a wrong number that renders as believable physics), NOT the orbitals
`eprintln!`-class (a clean `None` that merely lacked a UI trail — that class was closed by 023).

## Probe (rule #10) — what settled the design

Measured on **141 real `$SCF_Dipole_Moment` blocks across 91 `.property.txt` files** (the author's own
runs, ORCA 6.1.0): `&dipoleMagnitude` and `&dipoleTotal` are **ALWAYS co-present when the block
exists — zero exceptions**. There is NO legitimate "present block without a magnitude/total" format.
Separately, **26/117 files carry no `SCF_Dipole_Moment` block at all** (GOAT, some xTB, SPs that
didn't request it) — block-absent is a common, legitimate state.

This overturned the OLD page's claim that a present block missing `&dipoleMagnitude` was
absent-is-normal (`.scalar_f64()?` → `None`). The probe establishes magnitude is **mandatory when the
block is present**, so its absence is a corruption, not an optional-field `None`.

## The final design (the fix that shipped)

`dipole()` returns `Result<Option<Dipole>, ParseError>`:

- **block absent** (`last_block("SCF_Dipole_Moment")` is `None`) → `Ok(None)`. No warning, no error —
  absent is normal.
- **block present**, then the block is valid COMPLETELY or malformed COMPLETELY (no partial /
  half-fabricated dipole):
  - `&dipoleMagnitude` key absent → `Err(Malformed)`; present-but-not-a-number (`scalar_f64` → `None`)
    → `Err(Malformed)`.
  - `&dipoleTotal` key absent → `Err(Malformed)`; component count `!= 3` (empty / truncated / garbage)
    → `Err(Malformed)` carrying the observed count.
  - both valid → `Ok(Some(Dipole { .. }))`.
- No `unwrap_or_default()`, no `unwrap_or(&0.0)` — a component is never fabricated.

`ParseError::Malformed { field: "SCF_Dipole_Moment", detail }` — same variant/shape the Mayer reader
uses; `detail` names WHICH key was absent vs unparseable, and for total the observed component count.

### Why a naive `Option → Err` would have been a trap

Treating **whole-block-absent as `Err`** (the obvious "just make it fallible") would spew a warning on
**every legitimately dipole-less job** — 26/117 real files. The probe is what let the design draw the
line precisely: *absent block* = `Ok(None)` (quiet), *present-but-malformed* = `Err` (loud). The two
halves are tested separately (`absent_dipole_block_stays_quiet` pins the quiet half).

## Consumer wiring (results.rs, `from_verified`)

Dipole is computed in `from_verified` **before** the `ParsedResults` struct literal exists (same shape
as the orbitals fix). So the `Err` warning is buffered into a local `dipole_warnings: Vec<String>` and
used to **seed** `parse_warnings` in the struct literal; the caller (`parse_and_store`) then `append`s
orbital warnings and `push`es the Mayer warning — the three coexist, none clobbers another. Dipole is
**AUXILIARY, non-fatal** (like Mayer/orbitals): a malformed dipole warns and sets `dipole: None` but
the essential results (geometry/energy) still reach `Parsed`, not `ParseFailed`. The other constructors
(`from_scan_profile`/`from_2d_scan`/`from_neb`) set `dipole: None` without calling `v.dipole()` and
leave `parse_warnings` empty — correct, no dipole warning there.

## Negative control that bites (rule / `d9a6492` convention)

Temporarily restoring `*total.get(n).unwrap_or(&0.0)` on `dipoleTotal` fabricates
`total_au: [-0.60.., -0.59.., 0.0]` for a truncated block, and:

- `parse::property::tests::dipole_present_but_total_truncated_is_malformed` fails
  (`Some(Dipole { .. total_au: [.., .., 0.0] })` where an `Err` was asserted),
- `results::tests::malformed_dipole_is_non_fatal_and_leaves_a_visible_warning` fails
  ("the malformed dipole is dropped to None, never (0,0,0)").

Reverting the three lines to `total.as_slice().try_into()` returns both to green — so the guards
distinguish "invariant holds" from "test checks nothing".

## Related

- The caught-`Err` auxiliary visibility class (Mayer + orbitals): dipole now joins it — all three
  surface via `parse_warnings`. See [modules/parser.md](../modules/parser.md) (fatal-vs-auxiliary
  matrix) and [debugging/023](023-mayer-negative-bond-order-and-fatal-auxiliary.md).
- The domain-fact home for the probe: [orca/parse-sources.md](../orca/parse-sources.md).
- Units-and-post-conditions discipline (rule #11) and the honest-or-absent surface: same lineage as
  [debugging/019](019-orca2json-plain-opt-gbw-staleness.md).
