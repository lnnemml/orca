# ORCA `NEB-TS` — transition state from a reactant + product pair (rule #10)

Recorded from a **real run** (not the manual): a NEB-TS search on the Menshutkin SN2
(methylamine + ethyl iodide, DMF/SMD) using the **E2 connectivity endpoints** as the
reactant and product, on **2026-08-10** (Phase 4.5 Stage E3a-1). Every claim is from that
invocation; where the manual and the run disagree, the run wins (domain rule #10).

## What NEB-TS is — and when it beats OptTS's scan-max seed

`! NEB-TS` (Nudged Elastic Band + TS) finds a saddle from **two endpoints** — a reactant
and a product minimum — with **no scan coordinate**. ORCA interpolates a band of images
between the endpoints, relaxes the band to the minimum-energy path (MEP) while keeping the
images spread out (the "nudged elastic" springs), and lets the highest image **climb** to
the saddle. It is the path-finder for the case OptTS's "click the scan maximum" cannot
reach: a **concerted** reaction with no single clean 1-D coordinate (`wiki/orca/optts.md`
§"the contrast that motivates NEB"). Once NEB has the climbing image, **OptTS refines THAT
guess** through the same source-agnostic engine (ADR-020) — that refine step is Stage E3a-2.

## The recipe that worked (measured)

```
! r2SCAN-3c NEB-TS SMD(dmf) TightSCF
%neb
  NEB_End_XYZFile "product.xyz"
  NImages 8
end
* xyz 0 1
  <reactant geometry>
*
```

- **`NImages 8` → 10 images** (im0 = reactant … im9 = product; the two endpoints plus 8
  interior). The band count in every artifact is **NImages + 2**.
- **The product is a SEPARATE file** `product.xyz` in the job dir, referenced by relative
  path. The app writes it as an aux file into the isolated job dir at run time (the
  two-file child, `create_neb_job`; rule #3).
- **Both images MUST share atom order** — element sequence AND count. NEB interpolates image
  k of the reactant into image k of the product atom-by-atom; a different order interpolates
  *different atoms* and the method silently fails. `buildNebInput` refuses a mismatched pair
  (throws) — the guard is the whole point of the builder.
- Method + solvation + charge **inherited from the reactant** verbatim (comparability + the
  charge footgun), exactly as `buildOptTSInput` inherits them. **Multi-line `%neb` block** —
  the exact form the converging run used; the single-line form is valid ORCA in principle
  but UNMEASURED, so the app emits what was measured to work (rule #10).

## The result (measured) — it recovered the KNOWN saddle

- **Converged in 24 iterations** (0…23); the **climbing image was #5**. Cost **≈ 12 min**
  wall (10 images, 15 atoms, DMF/SMD) on the dev machine — roughly one OptTS per image.
- The converged TS (`_NEB-TS_converged.xyz`) is the **known Menshutkin saddle**: **N···C
  2.353 Å / C···I 2.594 Å** — vs the independent OptTS result **2.353 / 2.592** (Δ **0.002
  Å**). NEB found the same saddle **from only the two endpoints**, with no scan and no TS
  guess. This is the decisive validation: the geometry, not just an energy.
- Final barrier (last iteration) ≈ **0.01555 Eh ≈ 9.76 kcal/mol** = E(climbing image) −
  E(reactant end). A screening value; the honest ΔE‡/ΔG‡ comes from refining the TS with
  OptTS + Freq (E3a-2 → E1b).

## Artifacts + formats (measured — the sixth reader, `parse/neb.rs`)

| File | what | format |
|---|---|---|
| `*.NEB.log` | per-iteration band (the E3a-2 "PES per iteration" source) | header (2 lines) then one block per iteration, `>`-delimited: `iteration : N`, `climbing : yes\|no`, `nim : 10`, `barrier : <Eh> (image: k)`, `distance : <10 floats>` (**arc length, Bohr**), `energy : <10 floats>` (**ABSOLUTE Eh**), plus force/step/angle rows (ignored) |
| `*.final.interp` | converged **smooth MEP** | an `Interp.:` section, rows `<norm> <distance_Bohr> <energy_Eh>` — energy **RELATIVE**, image 0 = 0. (An earlier `Images:` section = the 10 discrete points; not read) |
| `*_NEB-TS_converged.xyz` | the converged **TS geometry** | standard xyz, Å, reactant atom order |
| `*_MEP_trj.xyz` | the MEP path geometries (E3a-2's 3D band) | multi-frame xyz; **not parsed here** |

**Units (rule #11):** log/interp distances are **Bohr → Å at the boundary**; energies are Eh.
The **`.NEB.log` energy is absolute** (−472.768…) while the **`.final.interp` energy is
relative** (image 0 = 0) — the reader keeps them apart and never conflates the two.

**Post-conditions (rule #9):** the image count is constant across iterations AND equals
`NImages + 2`; arc-length distances are monotonic non-decreasing within an iteration; the
converged-TS element order equals the reactant order. A truncated/ragged log is a loud parse
error, never a silent partial band.

## See also

- `wiki/orca/optts.md` — refining the NEB climbing image into a located TS (E3a-2 → E1b);
  the scan-max seed contrast that motivates NEB.
- `wiki/modules/artifact-readers.md` — the `neb.rs` reader (sixth) on the parser template.
- `wiki/modules/scene.md` — `buildNebInput`, the `buildOptTSInput` sibling.
- `wiki/architecture/adr-020-optts-refinement-source-agnostic.md` — the refine engine that
  takes the NEB climbing image as one more generic seed.
