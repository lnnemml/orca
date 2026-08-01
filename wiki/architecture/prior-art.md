# Prior art — how other tools handle molecular / reaction geometry

**Source: the author's practical experience with these tools, as recorded in our design
discussions. This page deliberately does NOT invent feature details or cite the internet** — it
captures only what has already been established in conversation, at the altitude of "what they do,
what's missing, what OrcaStudio does differently on purpose." Correct or extend it from hands-on
use, not from memory of a website.

## The tools

- **Avogadro 2** — a capable open molecular builder/editor with a plugin architecture; good at
  building and cleaning up single molecules and at driving several back ends. Strong general
  editor; not organised around a *reaction* (substrate + reagent as distinct fragments in one
  coordinate space with a reaction coordinate between them).
- **IQmol** — a polished free front end (built around Q-Chem in origin) for building molecules,
  setting up jobs and viewing orbitals/surfaces. Pleasant viewer; its workflow centres on a single
  molecule and job, not on comparing competing reaction pathways.
- **Gabedit** — a broad multi-package GUI (input generation + result visualisation for many QC
  codes). Wide format coverage; the UI is a general front end rather than a mechanism workstation.
- **Chemcraft** — a strong, fast *visualisation* tool for output files (geometries, vibrations,
  orbitals) with convenient geometry read-outs. Excellent for reading results; input construction
  and reaction setup are not its focus.
- **WebMO** — a browser front end that lowers the barrier to running jobs on a shared server; good
  for teaching and remote submission. Server/job oriented; not a fine geometric-control editor.
- **GaussView** — the mature commercial builder/viewer for Gaussian; includes the classic
  "modify redundant coordinate" primitive (set a bond length / angle / dihedral and apply). Solid
  builder; tied to the Gaussian ecosystem and organised around a molecule, not a reaction.

## What is genuinely common — and where OrcaStudio diverges

The primitive **"set a distance / angle / dihedral to a target value"** exists in several of these
programs (GaussView's redundant-coordinate edit is the canonical example, and Avogadro/others have
equivalents). So d/θ/φ editing is **not** novel on its own — 2.5.2's value is not that it exists but
how it's wired into the rest of the system.

Two things, as established in our discussions, are **not** how the tools above work — and are the
deliberate difference (they trace to ADR-007 and ADR-008):

1. **The ASE mask is DERIVED from scene structure.** In OrcaStudio a fragment *is* a known set of
   atom indices (`fragmentAtomIndices`), so "move this reagent rigidly" or "constrain these atoms"
   comes straight from the substrate/reagent decomposition — the mask is a function of the scene,
   not something the user hand-selects each time. The prior-art tools edit a coordinate on a single
   molecule; they don't carry a first-class substrate-vs-reagent fragment boundary that the mask
   falls out of.
2. **One atom index survives the whole chain, unbroken, to the ORCA launch.** The picked index =
   merged-xyz line = ASE mask index = the index ORCA sees — one index space end to end (ADR-008).
   Nowhere in the tools above (as we've discussed) does a single atom identity provably persist from
   the pick, through the geometry operation, to the exact atom the engine runs on; that invariant is
   what the Scene model buys and what the measurement/edit units are being built to preserve.

Everything else OrcaStudio does (build, view orbitals, submit remotely) these tools also do, often
better and more maturely. The bet is narrow and specific: a **reaction-mechanism workstation** where
the fragment decomposition drives the geometry operations and the index space is unbroken to the
run — not a better general molecular editor.

*This page is about builders / viewers / geometry; the agentic AI layer over computational chemistry
lives in [ai-landscape.md](ai-landscape.md).*
