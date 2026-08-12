//! Keyword catalogs for the ORCA input builder form.
//!
//! Data only — no React. Every list is a plain array of {@link OrcaOption}s (or
//! grouped arrays) so the form and the generator share one source of truth.
//! Keywords are checked against the ORCA 6.1 manual
//! (https://www.faccts.de/docs/orca/6.1/manual/).

export interface OrcaOption {
  /** The token that goes into the `!` line (empty string = "no keyword"). */
  keyword: string;
  /** What the user sees in the dropdown. */
  label: string;
  /** Short hint shown next to / under the control. */
  description?: string;
  /**
   * The method already includes a dispersion correction (e.g. `-D4`, `-V`/VV10),
   * so the builder must NOT add a separate dispersion keyword. See `build-input.ts`.
   */
  builtInDispersion?: boolean;
  /**
   * A correlated WF method that uses the RI/DLPNO approximation and so needs a
   * `<basis>/C` correlation-fit aux + Coulomb aux + RIJCOSX. Canonical methods
   * (plain MP2/CCSD/CCSD(T)) leave this absent — they take NO aux. See
   * `methodNeedsCorrelationAux` in `build-input.ts`.
   */
  needsCorrelationAux?: boolean;
}

/** Job types. Empty keyword = plain Single Point (ORCA's default). */
export const JOB_TYPES: OrcaOption[] = [
  { keyword: "", label: "Single Point (SP)" },
  { keyword: "Opt", label: "Geometry Optimization" },
  { keyword: "Freq", label: "Frequencies" },
  { keyword: "Opt Freq", label: "Opt + Freq" },
  { keyword: "OptTS Freq", label: "Transition State + Freq" },
  { keyword: "NumFreq", label: "Numerical Frequencies" },
];

/**
 * Composite ("3c") methods. Each bundles its OWN basis set, dispersion, and
 * (where applicable) further corrections — so basis / dispersion / RI must be
 * left OFF when one of these is selected (see `build-input.ts`).
 */
export const COMPOSITE_METHODS: OrcaOption[] = [
  {
    keyword: "r2SCAN-3c",
    label: "r²SCAN-3c",
    description: "Fast, accurate general purpose. Recommended default.",
  },
  { keyword: "B97-3c", label: "B97-3c", description: "Faster, GGA-level" },
  { keyword: "PBEh-3c", label: "PBEh-3c", description: "Hybrid, small basis" },
  {
    keyword: "wB97X-3c",
    label: "ωB97X-3c",
    description: "Range-separated hybrid",
  },
  { keyword: "HF-3c", label: "HF-3c", description: "Minimal cost screening" },
];

/** Functionals, grouped by rung — used when NOT in composite mode. */
export const FUNCTIONAL_GROUPS: { label: string; options: OrcaOption[] }[] = [
  {
    label: "GGA",
    options: [
      { keyword: "BP86", label: "BP86" },
      { keyword: "PBE", label: "PBE" },
      { keyword: "BLYP", label: "BLYP" },
    ],
  },
  {
    label: "meta-GGA",
    options: [
      { keyword: "TPSS", label: "TPSS" },
      { keyword: "r2SCAN", label: "r²SCAN" },
      { keyword: "M06-L", label: "M06-L" },
    ],
  },
  {
    label: "Hybrid",
    options: [
      { keyword: "B3LYP", label: "B3LYP" },
      { keyword: "PBE0", label: "PBE0" },
      { keyword: "TPSSh", label: "TPSSh" },
      { keyword: "M06-2X", label: "M06-2X" },
    ],
  },
  {
    label: "Range-separated",
    options: [
      { keyword: "wB97X-D4", label: "ωB97X-D4", builtInDispersion: true },
      { keyword: "CAM-B3LYP", label: "CAM-B3LYP" },
      { keyword: "wB97M-V", label: "ωB97M-V", builtInDispersion: true },
    ],
  },
  {
    label: "Hartree-Fock",
    options: [{ keyword: "HF", label: "HF" }],
  },
];

/**
 * Semi-empirical tight-binding methods. `! XTB` on ORCA 6.1 = **GFN2-xTB**
 * (measured — see `wiki/orca/xtb-method.md`), run via ORCA's bundled
 * `otool_xtb`. Self-contained: NO basis, dispersion, RI, or SCFConv — the
 * emit must carry the method keyword + job type ONLY. GFN1/GFN0/GFN-FF are
 * unverified and deliberately NOT offered here.
 */
export const XTB_METHODS: OrcaOption[] = [
  {
    keyword: "XTB",
    label: "GFN2-xTB",
    description: "Semi-empirical tight-binding. Very fast — geometry/TS screening.",
  },
];

/**
 * Correlated wave-function methods, grouped by rung. The RI/DLPNO variants
 * (`needsCorrelationAux: true`) add a `<basis>/C` correlation-fit aux + a Coulomb
 * aux + RIJCOSX; the canonical variants take NO aux (a spurious /C would run a
 * different, wrong calculation — see `wiki/orca/correlated-methods.md`, measured
 * on ORCA 6.1). None takes a separate dispersion keyword: the correlation IS the
 * dispersion. `(T)` has no analytic gradient — single-point-oriented.
 */
export const WAVEFUNCTION_METHODS: { label: string; options: OrcaOption[] }[] = [
  {
    label: "MP2",
    options: [
      { keyword: "MP2", label: "MP2", description: "Canonical MP2. No aux — exact RI-free." },
      {
        keyword: "RI-MP2",
        label: "RI-MP2",
        description: "RI-approximated MP2. Adds the /C correlation aux.",
        needsCorrelationAux: true,
      },
      {
        keyword: "DLPNO-MP2",
        label: "DLPNO-MP2",
        description: "Linear-scaling local MP2. Adds the /C correlation aux.",
        needsCorrelationAux: true,
      },
    ],
  },
  {
    label: "Coupled cluster",
    options: [
      { keyword: "CCSD", label: "CCSD", description: "Canonical CCSD. No aux." },
      {
        keyword: "CCSD(T)",
        label: "CCSD(T)",
        description: "Gold-standard canonical. Single-point; no analytic gradient.",
      },
      {
        keyword: "DLPNO-CCSD(T)",
        label: "DLPNO-CCSD(T)",
        description: "Near-CCSD(T) accuracy, linear scaling. Single-point; no analytic gradient.",
        needsCorrelationAux: true,
      },
      {
        keyword: "DLPNO-CCSD(T1)",
        label: "DLPNO-CCSD(T1)",
        description: "Iterative triples — tighter than (T0). Single-point; no analytic gradient.",
        needsCorrelationAux: true,
      },
    ],
  },
];

/**
 * Orbital basis sets, grouped by family (mirrors {@link FUNCTIONAL_GROUPS}).
 * The form renders these as `<optgroup>`s. Aux-basis pairing lives in
 * `build-input.ts`: def2-* → `def2/J`|`def2/JK`; anything else under RI →
 * ORCA's general `AutoAux`.
 */
export const BASIS_GROUPS: { label: string; options: OrcaOption[] }[] = [
  {
    label: "Karlsruhe def2",
    options: [
      { keyword: "def2-SVP", label: "def2-SVP", description: "Small, fast. Screening only." },
      { keyword: "def2-TZVP", label: "def2-TZVP", description: "Standard workhorse. Recommended." },
      { keyword: "def2-TZVPP", label: "def2-TZVPP", description: "Larger, for accurate energies" },
      { keyword: "def2-QZVPP", label: "def2-QZVPP", description: "Near basis-set limit, expensive" },
      {
        keyword: "def2-TZVPD",
        label: "def2-TZVPD",
        description: "With diffuse functions (anions, excited states)",
      },
      { keyword: "def2-SVPD", label: "def2-SVPD", description: "Small with diffuse" },
      { keyword: "ma-def2-SVP", label: "ma-def2-SVP", description: "Minimally augmented (diffuse)" },
      { keyword: "ma-def2-TZVP", label: "ma-def2-TZVP", description: "Minimally augmented triple-ζ" },
      { keyword: "ma-def2-TZVPP", label: "ma-def2-TZVPP", description: "Minimally augmented, larger" },
    ],
  },
  {
    label: "Dunning",
    options: [
      { keyword: "cc-pVDZ", label: "cc-pVDZ", description: "Correlation-consistent double-ζ" },
      { keyword: "cc-pVTZ", label: "cc-pVTZ", description: "Correlation-consistent triple-ζ" },
      { keyword: "cc-pVQZ", label: "cc-pVQZ", description: "Correlation-consistent quadruple-ζ" },
      { keyword: "aug-cc-pVDZ", label: "aug-cc-pVDZ", description: "Augmented (diffuse) double-ζ" },
      { keyword: "aug-cc-pVTZ", label: "aug-cc-pVTZ", description: "Augmented (diffuse) triple-ζ" },
      { keyword: "aug-cc-pVQZ", label: "aug-cc-pVQZ", description: "Augmented (diffuse) quadruple-ζ" },
    ],
  },
  {
    label: "Pople",
    options: [
      { keyword: "6-31G*", label: "6-31G*", description: "Legacy double-ζ, one polarization set" },
      { keyword: "6-31G**", label: "6-31G**", description: "Legacy double-ζ, H polarization too" },
      { keyword: "6-311G**", label: "6-311G**", description: "Legacy triple-ζ" },
      { keyword: "6-311+G**", label: "6-311+G**", description: "Triple-ζ with diffuse on heavy" },
      { keyword: "6-311++G**", label: "6-311++G**", description: "Triple-ζ with diffuse on all" },
    ],
  },
];

/**
 * Flat list of every basis (concat of {@link BASIS_GROUPS}). Kept for importers
 * that want a simple array; the FORM uses `BASIS_GROUPS` for its optgroups.
 */
export const BASIS_SETS: OrcaOption[] = BASIS_GROUPS.flatMap((g) => g.options);

export const DISPERSION: OrcaOption[] = [
  { keyword: "", label: "None" },
  { keyword: "D4", label: "D4", description: "Newest, recommended" },
  { keyword: "D3BJ", label: "D3(BJ)", description: "Becke-Johnson damping" },
  { keyword: "D3Zero", label: "D3(0)", description: "Zero damping" },
  { keyword: "NL", label: "NL (VV10)", description: "Non-local, more expensive" },
];

export const RI_METHODS: OrcaOption[] = [
  { keyword: "", label: "None (exact)" },
  { keyword: "RIJCOSX", label: "RIJCOSX", description: "Hybrid DFT speedup. Needs AuxJ basis." },
  { keyword: "RI-JK", label: "RI-JK", description: "Accurate but needs AuxJK basis" },
  { keyword: "RI", label: "RI-J", description: "For pure (non-hybrid) functionals" },
];

export const SOLVATION_MODELS: OrcaOption[] = [
  { keyword: "", label: "Gas phase" },
  { keyword: "CPCM", label: "CPCM", description: "Conductor-like PCM. Fast, analytic gradients." },
  { keyword: "SMD", label: "SMD", description: "Includes non-electrostatic terms. Better ΔG_solv." },
];

/**
 * Curated subset of the most common solvents. ORCA supports 179; these 20
 * cover the overwhelming majority of routine work.
 */
export const SOLVENTS: string[] = [
  "water",
  "methanol",
  "ethanol",
  "acetonitrile",
  "dmso",
  "dmf",
  "thf",
  "dichloromethane",
  "chloroform",
  "toluene",
  "benzene",
  "hexane",
  "acetone",
  "ethylacetate",
  "diethylether",
  "1,4-dioxane",
  "pyridine",
  "aceticacid",
  "2-propanol",
  "cyclohexane",
];

export const SCF_CONV: OrcaOption[] = [
  { keyword: "", label: "Normal" },
  { keyword: "TightSCF", label: "Tight", description: "Recommended for Opt/Freq" },
  { keyword: "VeryTightSCF", label: "Very Tight", description: "For sensitive properties" },
];
