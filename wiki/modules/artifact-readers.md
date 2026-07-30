# Module: Artifact readers (`src-tauri/src/parse/`)

**Status:** the authoritative result-parsing tier of ADR-012 — own Rust parsers over ORCA's
structured artifacts, replacing the abandoned cclib plan. **One of four readers is built:**
`.property.txt`. The other three (`.hess`, `_trj.xyz`/`.xyz`, `orca_2json`) are **not started**.
Nothing here is wired into the job pipeline yet (a later Phase-3 unit); `mod parse` is
`#[allow(dead_code)]` until then.

## Why this exists
cclib 1.8.1 crashes on ORCA 6.1.0 output and ORCA 6 is outside its handled matrix
([ADR-012](../architecture/adr-012-output-parsing-ownership.md), `orca/parse-sources.md`). The
sources are stable ORCA files we already own on disk; reading them is plain text-to-struct, so
the tier lives in Rust (no chemistry library; `orca_2json` is a binary spawn → ADR-009).

## The template — every reader is two-layered
`property.rs` is the first reader and the **template** the other three copy. The shape is the
point:

1. **Generic tokenizer** — understands only the file *grammar*, not any block by name.
   `.property.txt`'s is `$Block … $End` with `&prop [&Type …, &Dim …, &Units …]` entries whose
   value is inline (scalar) or on following lines (a coordinate table / indexed array). It keeps
   **every** block it sees, including ones it has no accessor for.
2. **Typed accessors** — interpret known blocks and **convert to canonical units at this one
   boundary**. Raw file numbers do not escape layer 2 uninterpreted.

Not a bag of per-block regexes: that shape breaks silently on the next ORCA version. A grammar
tokenizer bends; unknown constructs surface instead of vanishing.

## Units held by type — the central guard (rule #11)
Measured: `.property.txt`/`.hess` geometry is **Bohr**, the app is **Å** everywhere; a forgotten
×0.529 renders a plausible, 1.889×-too-large molecule (no crash). So the canonical length is a
type, `parse::units::Angstrom`, with a **private** field in its **own** module. `property` is a
*sibling*, so it cannot write `Angstrom(x)` — it must call `from_bohr` (convert) or `from_angstrom`
(already canonical). Choosing wrong is the one remaining bug, and a **post-condition test**
(`missed_bohr_conversion_fails_loudly`) goes red at ≈1.889× when it happens. Canonical set:
length Å, energy Eh, frequency cm⁻¹, IR km/mol.

## Post-conditions (rule #9 + #11), errors not warnings
- **Geometry** — given a known-Å reference (the input xyz), the reader recomputes the first
  `$Geometry` after conversion and asserts max Δ < 1e-4 Å. A missed Bohr→Å fails here, loudly.
- **Charge order** — each population block's `&ATNO` element sequence must equal the geometry's.
  Otherwise charges would render on the wrong atoms (the ADR-010/012 seam).
- **Lengths, measured not trusted** — charges = N, `&ATNO` = N, `&grad` = 3N, `&FREQ` = 3N,
  checked against N from the geometry, never read off `&Dim`.

## Measured facts the code encodes (not comments — structure)
- **`entropyS` is not entropy.** Measured `entropyS == enthalpyH − freeEnergyG`, i.e. **T·S in
  Eh**. The field is named `t_times_s_eh` so it cannot be read as S in J/(mol·K).
- **Scan `$Geometry` blocks are optimization cycles, not scan points** (a 6-point scan has 26).
  `geometries()` documents this; per-point scan data is `.relaxscanact.dat`, a different reader.
- **`$SCF_Nuc_Gradient &grad` is bare positional** — its order is the `$Geometry` with the same
  `&GeometryIndex`, bound in the type (`Gradient.geometry_index` + `geometry_for`), not left as
  two loose fields.
- **Mayer's charge field is `&QA`**, not `&AtomicCharges` (Mulliken/Loewdin) — measured.
- **Absent blocks are normal, not errors** — SP has no thermochemistry/gradient; GOAT has only
  `$Geometry` + `$Single_Point_Data`. Missing → `Option::None`; a reader that crashes on GOAT is
  a bug.
- **Unknown blocks stay visible** — `unknown_block_names()` reports any block not in `KNOWN_BLOCKS`
  so an ORCA 6.2 addition is seen, not dropped (rule #10).

## Files
- `parse/mod.rs` — module overview + shared `ParseError` (`#[from]` into `AppError`).
- `parse/units.rs` — `Angstrom` (canonical length; the type guard).
- `parse/elements.rs` — symbol ↔ Z, fragment-suffix stripping (`C(1)` → `C`).
- `parse/property.rs` — the `.property.txt` reader (tokenizer + typed accessors + post-conditions).
- `parse/property/tests.rs` — against real SP / Opt+Freq / GOAT / scan fixtures in
  `src-tauri/tests/fixtures/`.

## Rule #5
`.property.txt` is tens–hundreds of KB (measured max ≈ 344 KB), so it is read whole — unlike
`output.out`. `from_path` still caps the size (16 MB) and refuses a pathological file.

## See also
- [ADR-012](../architecture/adr-012-output-parsing-ownership.md) + its unit-3.3 amendment.
- [`orca/parse-sources.md`](../orca/parse-sources.md) — the measured evidence and the units table.
- [`modules/parser.md`](parser.md) — the streaming tier (Tier 1) this sits beside.
