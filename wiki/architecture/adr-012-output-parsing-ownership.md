# ADR-012: Output parsing ownership — own structured-artifact parsers, not cclib

**Status:** accepted · 2026-07-30
**Narrows:** [ADR-002](adr-002-python-sidecar.md) (the sidecar no longer owns result parsing;
cclib, named there, is not adopted)
**Refines:** [ADR-010](adr-010-editor-identity-state.md) amendment (i); complies with
[ADR-009](adr-009-process-orchestration.md)

Preceded by a two-part measurement unit; the evidence is in
[`wiki/orca/parse-sources.md`](../orca/parse-sources.md) and reproduced by
`sidecar/probes/parse_sources.py`. This ADR turns that evidence into a decision. It does not
restate the numbers — read them there.

## Decision

**The authoritative parse tier is our own parsers over ORCA's structured artifacts.** One
quantity, one home:

- `.property.txt` → energies, geometry, atomic charges (Mulliken / Loewdin / Mayer), dipole,
  thermochemistry (ZPE / H / S / G).
- `.hess` → signed vibrational frequencies, normal modes, IR intensities.
- `_trj.xyz` / `.xyz` → optimization trajectory / final geometry.
- `orca_2json` over `.gbw` → MO energies and occupations, HOMO/LUMO.
- `output.out` is **not** an authoritative source. It keeps only the streaming tier
  (`convergence.rs`), search (`output_search.rs`), and the two existing tail regexes
  (`extract_final_energy`, `extract_wall_time`). Domain rule #5 is then satisfied *by
  construction*: the tens-of-MB text is never parsed as a whole.
- **cclib is not adopted and is not added to `requirements.txt`.**

## Why — the durable reason, not the symptom

"cclib 1.8.1 crashes on ORCA 6.1.0 output" is true (measured on all four of our outputs —
`IndexError` at `orcaparser.py:2799 _append_scfvalues_scftargets`, `AssertionError` on GOAT)
but a *release could fix it*, so on its own it does not carry a decision. Three measured facts
carry it:

- **(a) The failure is not our environment.** It reproduces on every one of our four outputs,
  and the crash site is a generic SCF-table assumption in cclib's ORCA parser, not anything
  specific to our machine or job setup (parse-sources.md §1). *(An ORCA-wrapper project is
  reported to hit the same "list index out of range" on ORCA 6.1; that corroboration is
  second-hand and was not verified here — the decision does not rest on it.)*
- **(b) ORCA 6 is outside cclib's handled matrix.** Measured in the installed source: the only
  ORCA version markers `cclib/parser/orcaparser.py` knows are `Orca 2.6`, `ORCA4.0`, and
  `ORCA 5.0`. There is **no ORCA 6.x** anywhere in the parser. Even a release that stopped
  crashing would, until 6.x enters that matrix, leave the authoritative tier on an unverified
  path.
- **(c) cclib's release cadence is not ours.** `1.8.1` is the latest on PyPI (measured:
  `pip index versions cclib`) and predates ORCA 6.1; any 6.x support lives only in unreleased
  development. Binding the Results screen to that timeline is a standing risk we do not need —
  the structured artifacts are stable ORCA outputs we already own on disk.

**Reopening conditions (named, so a mere cclib release does not reopen this):** this decision
is reconsidered only when **both** hold — (1) `sidecar/probes/parse_sources.py` runs clean
(no crash, all attributes present) on our real outputs, **and** (2) ORCA 6.x appears in
cclib's supported/tested matrix (a version marker in `orcaparser.py`, not just "it didn't
crash on one file"). A new cclib version by itself opens nothing until the probe says so.

## Second consequence — the authoritative tier moves to Rust, not the sidecar

`.property.txt` and `.hess` are text-to-structure with **no chemistry library** — plain
parsing. `orca_2json` is an external-binary invocation, which by
[ADR-009](adr-009-process-orchestration.md) belongs to Rust, not the sidecar. So the
authoritative parse tier lives in `src-tauri/`, next to the existing `result_extraction.rs` /
`convergence.rs`. This removes the HTTP round-trip for results and one runtime boundary.

The sidecar keeps what genuinely needs a Python library: RDKit (SMILES→3D), the ASE geometry
kernel, format conversion, and the future manual indexer. ADR-002's line "the sidecar owns
chemistry intelligence" stands for *those*; it is narrowed only for **result parsing**.

## Relationship to ADR-002 (narrowed, not edited)

[ADR-002](adr-002-python-sidecar.md) named cclib as the reason for the sidecar and put result
parsing there. ADR-012 removes result parsing from the sidecar and declines cclib. Per the
ADR-008/ADR-010 precedent, the narrowing lives here; **ADR-002 is not edited** and history is
not rewritten. ADR-002 remains authoritative for RDKit/ASE/conversion in the sidecar.

## Relationship to ADR-010 amendment (i) (survives, narrowed)

ADR-010's correction (i) — "the sidecar returns POSITIONAL arrays; Rust builds the `IndexMap`
at the boundary" — was written about cclib. cclib is now gone, but the amendment **does not
lose its subject**: the sidecar still returns positional arrays from RDKit (SMILES→3D) and the
ASE geometry kernel, which know nothing of `AtomId`. So Rust still builds the `IndexMap` at the
sidecar boundary for exactly those flows. What changes is only *which* positional producer sits
behind the boundary — cclib is replaced by ORCA's structured artifacts, parsed in Rust, where
the same discipline applies: the module that reads an artifact owns the map from its positions
to `AtomId`.

Part A of the measurement unit is what makes that discipline safe here: **every artifact's
atom order was verified equal to the input order**, per frame and per block, on all four jobs
(parse-sources.md, Part A). The atomic-charge arrays are element-labelled (co-located `&ATNO`,
order verified); the one bare positional atom-ordered array — `$SCF_Nuc_Gradient &grad` — takes
its order from the co-located `$Geometry` block, and **that assumption is named, not hidden**.
A per-artifact mapping function is therefore the identity today; if a future ORCA reorders
(e.g. under `! UseSym` — the open Phase 4.5 probe), only that one function changes.

## Caveat — format stability is unknown (rule #10)

We have **not** measured whether the `.property.txt` / `.hess` / `orca_2json` formats are
stable across ORCA versions, and this ADR does **not** claim it. Mitigation, not assertion:
`sidecar/probes/parse_sources.py` stays in the repo and is re-run on every ORCA upgrade;
fixtures of real artifacts ship in the parser tests when the parsers are built; and the Part-A
gate result above is anchored to a specific measured version (ORCA 6.1.0, gbw `Git 679e74b`).
If a future ORCA changes a format, the probe catches it before the parsers do.

## Consequences

- New Rust parsers (one per artifact) are the Phase 3 work; the sidecar gets **no** cclib
  endpoint. `wiki/modules/parser.md` describes the current two Rust extractors as-is; the
  authoritative tier is "not started", now retargeted from cclib to structured artifacts.
- SQLite `results` schema design is deliberately **out of scope** here (a later unit).
- The scan (`%geom Scan`) and TD-DFT sources remain **unmeasured gaps** (no such job exists
  yet); they are not covered by this decision and need their own probe with a real run.

## References

- [`wiki/orca/parse-sources.md`](../orca/parse-sources.md) — the measured evidence (cclib
  crash, per-artifact atom-order gate, source table). Source document for this ADR.
- `sidecar/probes/parse_sources.py` — the re-runnable probe.
- [ADR-002](adr-002-python-sidecar.md), [ADR-009](adr-009-process-orchestration.md),
  [ADR-010](adr-010-editor-identity-state.md) — narrowed / complied-with / refined above.
- `ROADMAP.md` Phase 3 — the artifact-reading items that implement this ADR.
