# Module: Artifact readers (`src-tauri/src/parse/`)

**Status:** the authoritative result-parsing tier of ADR-012 — own Rust parsers over ORCA's
structured artifacts, replacing the abandoned cclib plan: `.property.txt` (unit 3.4/3.5, the
template), `.hess` (unit 3.6, frequencies / IR / normal modes), `_trj.xyz`/`.xyz` (unit 3.7,
trajectory), `orca_2json` (unit 3.7, MO energies/occupancies → HOMO/LUMO), and (Phase 4.5 B1)
`.relaxscanact/.relaxscanscf.dat` (the **fifth reader**, relaxed-scan profile). All are wired into
the job pipeline via `results.rs`: on completion the job is parsed and advanced to `parsed`.

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

## Typestate — the post-condition is ON the path, not beside it (rule #9)

An earlier shape had `parse()` return a value-bearing handle and left `verify_*`
as separate methods a caller could simply *not* call — numbers read unchecked, and
it compiled. Rule #9 says the post-condition must be unavoidable. So the reader is a
typestate, the same shape as ADR-010's `parse_output` (uncallable without the
`IndexMap` from a paired `emit_input`):

- `PropertyFile::parse` / `from_path` return an **unverified** handle. It has
  **no value accessors at all** — no `charges()`, `geometries()`, `final_energy()`.
  Only `unknown_block_names()` (rule-#10 diagnostics must work even when verification
  fails) and `verify()`.
- `verify(reference_angstrom)` runs all three post-conditions and, only on success,
  returns `Verified` — the **only** type with value accessors.
- So `PropertyFile::parse(text).charges()` **does not compile**: `charges` exists
  only on `Verified`, which exists only downstream of a passed `verify`. A caller
  cannot read a number without having verified it.
- The **caller supplies the reference** (each job has its own `input.inp`). The
  reader never reads `input.inp` itself — that would be a hidden cross-module
  dependency. `results::parse_and_store` extracts the reference from the job's
  stored `input_content` and passes it in.

## Post-conditions (rule #9 + #11), errors not warnings
- **Geometry** — given a known-Å reference (the input xyz), the reader recomputes the first
  `$Geometry` after conversion and asserts max Δ < 1e-4 Å. A missed Bohr→Å fails here, loudly.
- **Charge order** — each population block's `&ATNO` element sequence must equal the geometry's.
  Otherwise charges would render on the wrong atoms (the ADR-010/012 seam).
- **Lengths, measured not trusted** — charges = N, `&ATNO` = N, `&grad` = 3N, `&FREQ` = 3N,
  checked against N from the geometry, never read off `&Dim`.

## The `IndexMap` post-condition (unit 1d) — one function per reader, verified not trusted
Unit 1d pairs the readers with the ADR-016 identity core. `verify()` now takes the job's
`IndexMap<OrcaIndex>` (from `orcastudio-core`) alongside the reference, and the element-order
post-condition becomes **the map post-condition**: `parse::check_map_order(artifact_z, map,
reference, block)` asserts *the artifact's element sequence equals the order the map asserts* —
position `p` holds `map.to_atom(OrcaIndex(p))`, whose element the reference fixes **independently of
the map** (its `ids`↔`z` pairing). Each reader has exactly **one** such function — `property`'s
`check_map_order` (over the first `$Geometry`), `hess`'s (over `$atoms`), `xyz`'s (per frame),
`mo`'s (over `Atoms`) — the single seam ADR-016/Phase-3 promised would change; the accessors,
result structs, and stored JSON are byte-for-byte unchanged.

- **Identity ⇒ the pre-1d check.** For the identity map (every job in 1d) the lookup reduces to
  `artifact_z == reference.z`, so green data — and the dashboard — are identical.
- **The map is a REQUIRED argument, CROSS-CHECKED against the artifact — not trusted.** A permuted
  map (two non-equivalent atoms swapped) makes the map's lookup and the reference's independent
  element disagree → loud `OrderMismatch`; a wrong-count map → `LengthMismatch`. Both demonstrated
  to bite (`parse::map_order_controls`, `property::tests`); the decisive control is
  `check_order_ignoring_map`, a map-ignoring twin that goes green on the same permuted input,
  proving the map/artifact cross-check is what holds the permutation red.
- **Typed in-process / verified at the persistence boundary — stated verbatim, no over-reach.** The
  ADR-010 `emit_input`/`parse_output` pair is a *type-level* invariant only **within one process**
  (orcastudio-core on both sides — the compiler sees the `AtomId ↔ OrcaIndex` provenance). The map
  is minted at `create_job`, **serialized into SQLite**, and re-read at parse time — serialization
  **erases the type provenance**, so at this boundary the invariant honestly degrades to *a required
  argument cross-checked against the artifact* (a post-condition, rule #9), NOT a type guarantee.
  Writing "type-level invariant across persistence" would be exactly the over-reach ADR-010's
  empirical addendum warns against. The distinction is written verbatim on `check_map_order`.
- **Minted (unit 1e) vs derived (legacy) map.** `results::resolve_job_mapping` reads
  `jobs.index_map_json`:
  - **`{"minted":…}`** (minted at `create_job` from the text↔scene correspondence) + a readable scene
    → the stored map is used, and the AtomId→element anchor for `check_map_order` comes from the
    **scene** (`scene.atom_order()` read from `scene_json`, **independent of the stored map**). That
    independence is what makes a corrupted stored map bite: because the anchor is not derived from the
    map, a permuted stored map disagrees with the scene at the artifact and fails loudly, rather than
    cancelling itself out. (Negative control `minted_map_is_load_bearing…` in `results.rs`.)
  - **`{"skipped":…}` / NULL / minted-but-scene-gone** → the **derived identity map** from the input
    coordinate block, anchor `0..n` (the unit-1d path), still cross-checked against the artifact.
  An unreadable input coordinate block remains a **loud, named** parse failure (no `* xyz *` block).
  The reference's coords/`z` stay text-sourced; only the AtomId **anchor** switches to the scene for a
  minted job — and only the anchor needs to, because element and coord truth still come from the
  artifact/text, both independent of the stored map.

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

## Second reader — `.hess` (unit 3.6): the template held; one post-condition bent
`parse/hess.rs` copies the whole external contract — two layers, typestate
(`parse → verify(reference) → Verified`), unknown sections surfaced, post-conditions-as-errors,
units by type — so nothing new had to be invented. Two deliberate differences, both recorded:

- **Grammar differs, and that's fine.** `.property.txt` is uniform `$Block`/`&prop`; `.hess` is
  `$section` headers each with its **own** shape (a count then rows; a `3N 3N` dim then
  column-blocks; a bare scalar). The tokenizer is written to *this* grammar (raw lines per
  section) while the accessors/typestate above it are identical. A shared tokenizer would have
  been the wrong kind of reuse.
- **The geometry post-condition is distance-based, not coordinate-based.** Measured: `.hess
  $atoms` is the Freq geometry **rigidly reframed** — a **pure centre-of-mass translation, no
  rotation** (unit-3.12 Kabsch gate: `max|R−I| ≤ 3e-13` on all three jobs, incl. asymmetric
  dexketoprofen; the per-atom shift is identical for every atom — a translation's signature). A
  uniform ~1.10 Å shift on the saddle, 0 on symmetric ethane. A coordinate compare would
  false-alarm on that translation. Interatomic **distances** are translation/rotation invariant,
  so `.hess` compares those: a missed Bohr→Å still fails loudly (6.6 Å on the saddle), a reframe
  passes (4e-8 Å). The caller supplies the **optimized** geometry as the reference (the
  `.property.txt` final `$Geometry`), *not* `input.inp` (the start) — a Freq is computed at the
  minimum, so `$atoms` ≠ the input geometry. **No mode-rotation boundary exists** on the reader:
  the gate proved none is owed, so `$normal_modes` are added to the reference geometry as-is for
  animation (unit 3.12, `src/spectrum/mode.ts`).

Measured facts the `.hess` reader encodes in structure: `$vibrational_frequencies` keeps its
**sign** and `imaginary_count` is an **explicit field** (0 = minimum, 1 = TS, >1 = neither), not
a UI derivation; the 5–6 **exact-zero** trans/rot modes are matched by exact `== 0.0` (no
threshold), and 5 (linear) vs 6 (non-linear) is a distinction, not a failure; `$normal_modes` is
**Cartesian** (unit-3.6 gate — see `parse-sources.md`), consumed as-is with **no ÷√m**;
`$ir_spectrum` intensity is column 2 (km/mol); `$hessian` and `$dipole_derivatives` have
UNDETERMINED units and are **recognized but not read**, so no value is shown with a unit taken on
faith.

## Third & fourth readers — `_trj.xyz` and `orca_2json` (unit 3.7)
- **`_trj.xyz` / `.xyz`** (`parse/xyz.rs`) holds the template unchanged: multi-frame xmol grammar,
  typestate, Å via `from_angstrom` (the identity case — still guarded, and a `missed_conversion`
  test proves the wrong choice fails). The **comment line** was *measured*, not assumed:
  `Coordinates from ORCA-job input E <energy>`, identical on every frame of Opt / GOAT / scan and
  on `.xyz`, so the frame energy is parsed to `Option<f64>`. Post-conditions: natom constant across
  frames, element order per **frame** == reference, ≥ 1 frame, first-frame geometry. Frames are opt
  cycles, **never** "scan points" (26 for a 6-point scan). `.allxyz` / `.relaxscan*.dat` are
  Phase 4.5 and not fed here.
- **`orca_2json`** splits into two things kept apart:
  - **The spawn** (`crate::orca_json`, a top-level module, *not* under `parse/`) — process
    orchestration, so Rust owns it (ADR-009). The ORCA path comes from **settings** (rule #7, not
    hard-coded); `orca_2json` + libs sit beside it (`LD_LIBRARY_PATH`, `.gbw` extension — measured
    requirements). Generation is **lazy + cached** in the job dir (the only writable place, rule
    #3): regenerated only when the JSON is missing or older than the `.gbw`. A non-zero exit / no
    JSON (an xTB gbw) is `Ok(None)`, not a panic.
  - **The reader** (`parse/mo.rs`) — the pure template reader. **Rule #5 is the whole point here**
    (unit-3.7 gate): the JSON is ~52–62% `MOCoefficients` and extrapolates to *tens of MB*, so it
    is **streamed** with `serde_json::from_reader` into a struct that omits `MOCoefficients` —
    serde skips it as `IgnoredAny` (never allocated). Reading it whole into a `Value` would repeat
    the `.out` mistake. Coefficients are **never** stored in the DB (a test asserts they are not in
    the serialized results). Geometry check is distance-invariant (as `.hess`); the reference is
    the **final** geometry (orca_2json's coords are final, not the input).

This is the first reader whose input is *produced by spawning a binary* — the boundary is drawn so
the spawn (orchestration, Rust) and the parse (pure) never mix in one function.

## Fifth reader — `.relaxscanact/.relaxscanscf.dat` (Phase 4.5 B1): a runtime unit-confirmation
`parse/relaxscan.rs` reads the relaxed-scan **profile** — the two `.dat` files, **one row per scan
point** (N rows of `coordinate  energy`), NOT the per-cycle `.property.txt`/`_trj.xyz` (26 rows for a
6-point scan — measured 3.3; the property/xyz readers document this, and there is a test on each side).
It holds the template (two layers, `parse → verify → Verified`, post-conditions-as-errors), with two
reader-specific facts:
- **`act` and `scf` are different energies, kept both, labelled.** `act` = the final composite
  (actual) energy — r²SCAN-3c carries gCP+D4 terms; `scf` = the bare SCF energy. They genuinely
  differ (measured) and are **never conflated** to one "energy"; a cross-file post-condition asserts
  the two `.dat` share an identical coordinate column (same geometries).
- **The geometry cross-check is the runtime unit-confirmation (rule #11) — the load-bearing
  post-condition.** A bare 2-column `.dat` has **no unit literal**, so a Bohr coordinate would not
  crash, it would draw a plausible-but-wrong profile. So for a distance (`B`) scan `verify` recomputes
  the scanned distance from each point geometry (`input.NNN.xyz`, Å via the `xyz` reader's
  `pair_distance_angstrom` witness — no re-implemented xyz parsing) and asserts it equals column 1
  within 1e-3 Å. A Bohr coordinate fails ≈1.889×, loudly (a `GeometryMismatch`). This is where the
  coordinate stops being *measured-once* and becomes *confirmed-per-read*. Angle/dihedral (`A`/`D`)
  parse the same but their coordinate cross-check is deferred (the coordinate is degrees).

The scanned atom pair comes from a **minimal** parse of the input's `%geom Scan B a1 a2 = …` line
(`parse_scan_spec`, a regex requiring the `=` that distinguishes a scan line from a brace-wrapped
constraint — NOT the TS `scan.ts` parser), done in `results.rs` and passed in, so the reader never
reads `input.inp`. Absent `.relaxscanact.dat` → `Ok(None)` (an SP/Opt/GOAT job has no scan — the
absent-is-normal pattern). Rides in `data_json` as `ParsedResults.scan` (no new narrow column, no
migration — like the trajectory, unit 3.7); `parser_version` 3 → 4. Three negative controls
(`relaxscan/tests.rs`) bite red-then-green on real ethane-C–C fixtures: bohr-coordinate,
act/scf-conflated, per-cycle-source (26 rows can't pass the 6-point-file cross-check).

## Files
- `parse/mod.rs` — module overview + shared `ParseError` + shared `ReferenceGeometry`.
- `parse/units.rs` — `Angstrom` (canonical length; the type guard).
- `parse/elements.rs` — symbol ↔ Z, fragment-suffix stripping (`C(1)` → `C`).
- `parse/property.rs` — `.property.txt` (energies, geometry, charges, dipole, gradient, thermo).
- `parse/hess.rs` — `.hess` (frequencies, IR, normal modes; distance-based geometry check).
- `parse/xyz.rs` — `_trj.xyz` / `.xyz` (trajectory frames + comment energy).
- `parse/mo.rs` — `orca_2json` JSON (MO energies/occupancies; streamed, coefficients skipped).
- `parse/relaxscan.rs` — `.relaxscanact/.relaxscanscf.dat` (relaxed-scan profile; act+scf both, the
  per-point geometry cross-check confirming Å; Phase 4.5 B1).
- `orca_json.rs` (top level) — the `orca_2json` **spawn** (ADR-009), lazy-cached in the job dir.
- `parse/*/tests.rs` — against real SP / Opt+Freq / GOAT / scan / saddle fixtures in
  `src-tauri/tests/fixtures/` (incl. a 198 KB gbw-json with coefficients, to exercise the skip).

## Rule #5
`.property.txt` is tens–hundreds of KB (measured max ≈ 344 KB), so it is read whole — unlike
`output.out`. `from_path` still caps the size (16 MB) and refuses a pathological file.

## See also
- [ADR-012](../architecture/adr-012-output-parsing-ownership.md) + its unit-3.3 amendment.
- [`orca/parse-sources.md`](../orca/parse-sources.md) — the measured evidence and the units table.
- [`modules/parser.md`](parser.md) — the streaming tier (Tier 1) this sits beside.
