# 023 — A valid negative Mayer bond order aborted a converged job's whole results-parse

**Symptom.** A real, cleanly-converged r²SCAN-3c CPCM **codeine-cation re-opt** (job
`91cd2f6c-75ed-450a-9e8c-de10884017e4`) sat `completed` — **never `parsed`** — with the results
dashboard empty and this error on the job:

```
results parse failed: mayer: malformed Mayer bond orders: non-positive bond order -0.1016 for B(10, 18)
```

The calculation was fine (`ORCA TERMINATED NORMALLY`, a full `.property.txt`/`.hess`/`_trj.xyz`, a
final energy of `-979.218412193007 Eh`). Only OUR parse of it refused — and it refused *everything*
(energy, geometry, frequencies, charges), not just the Mayer table.

## Root cause — TWO independent defects

### Defect 1 — a false "non-positive = malformed" premise

The Mayer reader guarded each entry with `!(order > 0.0) → Malformed`. That premise is wrong. ORCA's
block prints **every** atom pair with `|BO| > threshold`, and a **through-space, non-bonded** pair is
legitimately **negative**. The real ground-truth block (output.out line 11882 onward) contains:

```
  Mayer bond orders larger than 0.100000
...
B( 10-O , 18-C ) :  -0.1016 B( 11-C , 12-C ) :   1.5264 B( 11-C , 18-C ) :   1.4137
...
```

`B(10-O, 18-C) = -0.1016` is a valid O···C through-space interaction, not a corrupt row. The guard
rejected it as malformed. A Mayer bond order is stored **exactly** — no `abs()`, no sign flip, no drop
(honest-or-absent, domain rule #9). The bounds check on the atom indices (`index ≥ natoms → Malformed`)
is the reader's real, in-our-terms post-condition; the sign check was never one and is removed.

### Defect 2 — an auxiliary property held the essential results hostage

Even once a bad Mayer row is possible, a Mayer failure must not abort the whole parse. Mayer is an
**auxiliary** property — like `orca_2json` orbitals, whose failure has always been non-fatal ("No MO
data is a normal state"): a run with no orbitals still shows energy/geometry/frequencies. Mayer was
wrongly excluded from that canonical convention — its `Err` returned `ParseFailed` for the *entire*
job, so a single bad-looking bond order blanked the whole dashboard of a converged calculation.

The fix routes Mayer through the same non-fatal path orbitals use: a failure sets
`mayer_bond_orders = None` and continues. Mayer goes one step further than orbitals: it also pushes a
**visible** `parse_warnings` entry (`"Mayer bond orders not parsed: …"`), rendered in `ResultsCard`,
so the UI says *why* the table is empty — it FAILED, not "wasn't computed" (the honest-or-absent
surface, rule #9, that orbitals' `eprintln!`-only path lacks).

## The fix

- `src-tauri/src/parse/mayer.rs`: drop the `!(order > 0.0)` guard; keep the index bounds check as the
  reader's sole in-our-terms post-condition. A negative order is parsed and stored exactly.
- `src-tauri/src/results.rs`: the Mayer read is non-fatal — on `Err`, `eprintln!` + push a
  `parse_warnings` string + `mayer_bond_orders = None`; the essential results (already assembled) store
  and the job reaches `parsed`.
- `src/types.ts` + `src/screens/ResultsCard.tsx`: surface `parse_warnings` visibly (a warning banner,
  one line per warning).
- `reparse_job` (`src-tauri/src/commands/jobs.rs`, testable core `reparse_job_conn`): re-run the fixed
  parser on an already-`completed` job whose parse previously failed; on success advance to `parsed`,
  overwrite the header energy from the authoritative tier, **and clear the stale `error_message`**. A
  "Re-parse results" button in `JobDetailScreen` offers it only for `completed && error_message` jobs.

## Guards that bite (negative controls)

- `parse::mayer::tests::negative_bond_order_is_valid_data_stored_exactly` — the -0.1016 case parses and
  is stored unchanged.
- `parse::mayer::tests::negative_control_index_out_of_range_is_an_error_not_a_silent_bad_pair` and
  `…negative_control_the_real_table_would_fail_a_too_small_atom_count` — the retained bounds check still
  rejects a genuinely out-of-range index (so removing the sign guard did not disarm the reader).
- `commands::jobs::tests::reparse_that_still_fails_keeps_the_error_not_cleared` — a re-parse that STILL
  fails does NOT clear the error (proves the clear-on-success is guarded, not unconditional).
- `commands::jobs::tests::real_codeine_reparse_clears_error_and_stores_negative_mayer` (ignored, reads
  the real job dir): on the REAL `91cd2f6c…` dir, reparse succeeds → `parsed`, error cleared, 50 Mayer
  bonds stored including `B(10,18) = -0.1016` exactly, geometry intact, `E = -979.218412193007 Eh`.

## Related

- Auxiliary-reader convention Mayer now joins: `orca_2json` orbitals (non-fatal, "No MO data is a
  normal state") — see [modules/parser.md](../modules/parser.md) (fatal-vs-auxiliary matrix).
- Honest-or-absent / rule #9 process-boundary post-conditions: the same discipline as the
  Bohr→Å ratio signature of [debugging/019](019-orca2json-plain-opt-gbw-staleness.md).
