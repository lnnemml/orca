# Correlated wave-function methods in the input builder (N1b)

**Measured 2026-08-12 on ORCA 6.1.0** (rule #10 — recorded from a real run, not docs).

The builder's fourth method family (`methodFamily: "wavefunction"`) covers the correlated
post-HF tier: MP2 / RI-MP2 / DLPNO-MP2 and CCSD / CCSD(T) / DLPNO-CCSD(T) / DLPNO-CCSD(T1).
The one load-bearing rule is **which methods take an auxiliary-basis chain and which take none**.

## The probe

Single point on HCN:

```
! DLPNO-CCSD(T) def2-TZVP def2-TZVP/C def2/J RIJCOSX TightSCF
```

**Result: ORCA TERMINATED NORMALLY.** Both auxiliaries were used, confirmed in the output:

- *"utilizes the auxiliary basis: def2/J"* — the Coulomb fit.
- *"def2-TZVP/C"* + *"Auxiliary Correlation fitting basis … AVAILABLE"* — the correlation fit.
- *"K(C) Formation … RI-DLPNO"* and *"DLPNO BASED TRIPLES CORRECTION"* — the RI-DLPNO machinery ran.
- **FINAL SINGLE POINT ENERGY −93.274453** Eh. Only benign post-HF warnings.

So the measured RI/DLPNO emit is:

```
<method>  <basis>  <basis>/C  <Coulomb-aux>  RIJCOSX
```

where `<basis>/C` is the correlation-fitting aux and `<Coulomb-aux>` follows the **same rule as
N1a's `auxBasisFor`** (def2 → `def2/J`, else `AutoAux`).

## Two distinct auxiliaries — `/J` (Coulomb) vs `/C` (correlation)

Post-HF RI needs **two** fit sets, not one:

- **`/J`** (or the seminumerical COSX in RIJCOSX) fits the **Coulomb** integrals of the SCF. This
  is the same `def2/J` the N1a DFT+RI path uses.
- **`/C`** fits the **correlation** integrals (the MP2/CC amplitudes). It is a *different* basis
  from `/J` and is required in addition to it — you cannot substitute one for the other.

## RI/DLPNO → aux chain; canonical → nothing

| Method | `needsCorrelationAux` | Emitted aux |
|---|---|---|
| `MP2`, `CCSD`, `CCSD(T)` | — | **none** (canonical; ORCA fails loud on a spurious aux) |
| `RI-MP2`, `DLPNO-MP2`, `DLPNO-CCSD(T)`, `DLPNO-CCSD(T1)` | ✓ | `<basis>/C` + Coulomb-aux + `RIJCOSX` |

A blanket "correlated → add /C" is **wrong**: a canonical `CCSD(T)` with a spurious `/C RIJCOSX`
is a *different, RI-approximated* calculation, not the canonical one the user asked for. The fact
lives once, in `WAVEFUNCTION_METHODS[*].needsCorrelationAux` (`orca-options.ts`), read by
`methodNeedsCorrelationAux` (`build-input.ts`).

## No separate dispersion keyword

Correlated methods recover dispersion from the correlation treatment itself. Appending a `D4`/`NL`
keyword to an MP2/CC line **double-counts** it. The WF branch of `buildKeywordLine` emits no
dispersion token at all (unlike the DFT branch).

## Basis → which correlation aux

Only the **def2 family** has a probed, native `/C` set in this build, so the builder emits
`<basis>/C` **only for def2**. Everything non-def2 falls back to bare **`AutoAux`** (which ORCA
generates to cover *both* J and C together) — the same non-def2 path N1a already uses, and
guaranteed valid:

| Basis under DLPNO/RI | Builder emits |
|---|---|
| `def2-TZVP` (measured) | `def2-TZVP def2-TZVP/C def2/J RIJCOSX` |
| `cc-pVTZ`, `aug-cc-pVQZ`, … | `cc-pVTZ AutoAux RIJCOSX` |
| `6-31G*`, `6-311++G**`, … | `6-31G* AutoAux RIJCOSX` |

### Future refinement (NOT shipped — needs its own probe, rule #10)

Dunning bases *do* have native `cc-pVTZ/C` correlation sets, and `def2/J` is a **universal**
Coulomb aux usable with any orbital basis. So the tighter, more optimal Dunning emit is likely:

```
cc-pVTZ  cc-pVTZ/C  def2/J  RIJCOSX
```

We deliberately do **not** ship this yet. The only alternative that would reuse the existing
`correlationAux` + `auxBasisFor` pieces is the **mixed form `cc-pVTZ/C AutoAux`**, and that is
**unmeasured**: `AutoAux` sets a *global* auto-generation flag whose interaction with an explicit
`/C` is unknown (does it override the explicit set? warn about a redundant aux? just work?). Rather
than emit something whose ORCA behaviour we cannot state, non-def2 uses bare `AutoAux` until the
`cc-pVTZ/C def2/J RIJCOSX` combination is confirmed by a real run.

## `(T)` has no analytic gradient — single-point-oriented

`CCSD(T)`/`DLPNO-CCSD(T)`/`(T1)` have **no analytic gradient** in ORCA: a geometry optimization or
frequency job falls back to **numerical** differentiation, which is very expensive. These methods
are single-point-oriented; the builder surfaces this in the WF note. The builder does **not** block
`Opt`/`Freq` (the user may knowingly want a numerical run) — it informs.

## How the builder encodes this

`build-input.ts`, `methodFamily === "wavefunction"` branch: push method + basis; if
`methodNeedsCorrelationAux(method)` → if `correlationAux(basis)` is non-null (def2) push it +
`auxBasisFor(basis, "RIJCOSX")` + `RIJCOSX`, else push `AutoAux` + `RIJCOSX`. No dispersion. The
solvation + SCFConv tail is **kept** (WF is not xtb — C-PCM/CCSD is valid, and post-HF *needs* a
tight SCF; the tail-suppression guard is `methodFamily !== "xtb"`). Bite tests in
`build-input.test.ts`: `dlpno_ccsdt_def2_emits_c_j_rijcosx`, `canonical_ccsdt_emits_no_aux`,
`dlpno_with_pople_falls_back_to_autoaux`, `dlpno_dunning_uses_autoaux_not_explicit_C`,
`wf_keeps_solvation_and_scfconv`, `wf_emits_no_dispersion`.

## Related

- `wiki/orca/input-format.md` — Rule 0 (method families), Rule 2 (aux-basis pairing), the WF row.
- `wiki/orca/xtb-method.md` — N1a's semi-empirical family (the *other* self-contained tier).
