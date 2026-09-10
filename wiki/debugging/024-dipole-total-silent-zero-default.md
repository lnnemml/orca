# 024 — A malformed dipole block silently fabricates `(0,0,0)` instead of null — NOT YET FIXED

**Status: TRACKED DEBT, NOT YET FIXED.** This page records a known honest-or-absent violation as an
explicit next unit. No code fix ships with it; the visibility unit that surfaced it
([debugging/023](023-mayer-negative-bond-order-and-fatal-auxiliary.md), orbitals visibility,
2026-09-10) is deliberately scoped NOT to touch `property.rs`.

## Symptom (latent — no observed failure yet)

`src-tauri/src/parse/property.rs:404`, in `dipole()`:

```rust
let total = b.prop("dipoleTotal").map(|p| p.numbers()).unwrap_or_default();
let total_au = [
    *total.first().unwrap_or(&0.0),
    *total.get(1).unwrap_or(&0.0),
    *total.get(2).unwrap_or(&0.0),
];
```

If the `SCF_Dipole_Moment` block is **present** but its `dipoleTotal` property is **missing or
malformed** (`prop("dipoleTotal")` is `None`, or `numbers()` yields fewer than three components), the
`unwrap_or_default()` / `unwrap_or(&0.0)` chain silently substitutes a fabricated dipole vector of
`(0.0, 0.0, 0.0)`.

## Why this is a defect — a default masquerading as data

`(0, 0, 0)` is **indistinguishable from a valid zero dipole** (a symmetric molecule genuinely has a
near-zero dipole). So a *present-but-broken* dipole block is recorded as if the physics said "zero",
not "unparseable". This is the **honest-or-absent violation** of domain rule #9: a dropped/failed value
must be recorded as absent WITH A REASON, never as a plausible invented stand-in. It is the **−60127
class** (a wrong number that renders as believable physics), NOT the orbitals `eprintln!`-class (a clean
`None` that merely lacked a UI trail — that class is now closed by 023).

Note the contrast with the block-level guard: `dipole()` correctly returns `None` when the *whole*
`SCF_Dipole_Moment` block is absent (`last_block(...)?`) and when `dipoleMagnitude` is absent
(`.scalar_f64()?`) — those are absent-is-normal. It is only the `dipoleTotal` **vector** that defaults
instead of failing.

## Priority — HIGHER than cosmetic

The electric (and magnetic) **transition dipole moments** feed **ECD rotatory strength**. A silent
`(0,0,0)` there does not blank a spectrum — it produces a **plausible WRONG spectrum** (a rotatory
strength computed from a fabricated-zero dipole). This is exactly the failure mode rule #9 exists to
prevent, so this debt is **not deferred namelessly**: it is a tracked next unit above cosmetic work.

## The fix (a SEPARATE unit — semantics change)

Make the accessor **fallible**: `dipoleTotal` (and the transition-dipole accessors it generalizes to)
should return `Result`/`Option` distinguishing "absent → `None` dipole (absent-is-normal)" from
"present-but-malformed → a loud error / recorded warning", never a defaulted zero vector. That is a
semantics change to `PropertyFile` and **all its callers**, so it is its own unit with its own
negative control (a present-but-truncated `dipoleTotal` fixture must NOT read as `(0,0,0)`), not a
rider on the orbitals-visibility change.

## Related

- Same class this debt is NOT: the caught-`Err` auxiliary visibility class (Mayer + orbitals), now
  **closed** — both surface via `parse_warnings`. See
  [modules/parser.md](../modules/parser.md) (fatal-vs-auxiliary matrix) and
  [debugging/023](023-mayer-negative-bond-order-and-fatal-auxiliary.md).
- The distinct open class this debt belongs to: **absent-is-normal `Option` accessors that silently
  default** rather than distinguishing absent from malformed.
- Units-and-post-conditions discipline (rule #11) and the honest-or-absent surface: same lineage as
  [debugging/019](019-orca2json-plain-opt-gbw-staleness.md).
