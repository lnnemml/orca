//! Hardcoded ORCA input templates for Phase 1.
//!
//! Each template is a complete, runnable `.inp` with an H2 placeholder geometry
//! the user replaces. Two method families:
//!   - **r²SCAN-3c** — a composite method; RIJCOSX / RI-approximations and the
//!     def2-mTZVPP basis are baked in, so the `!` line is just the method + task.
//!   - **B3LYP-D4 / def2-SVP** — a classic hybrid; we spell out the RI/dispersion
//!     keywords (`D4`, `RIJCOSX`, `def2/J`).
//!
//! NOTE: `%maxcore` is a simple directive, not a block — it takes NO `end`
//! (unlike `%pal ... end`). Emitting `%maxcore 2000 end` would be wrong ORCA
//! syntax, so these templates use the correct `%maxcore 2000` form.

export type OrcaCategory =
  | "single-point"
  | "optimization"
  | "frequency"
  | "opt+freq";

export interface OrcaTemplate {
  id: string;
  name: string;
  description: string;
  category: OrcaCategory;
  inputContent: string;
}

/** H2 placeholder geometry (neutral singlet). The user swaps in a real system. */
const PLACEHOLDER_COORDS = `* xyz 0 1
  H   0.00000   0.00000   0.00000
  H   0.00000   0.00000   0.74000
*`;

/** Assemble a full `.inp` from a `!`-line keyword string. */
function buildInput(keywords: string): string {
  return `! ${keywords}

%pal nprocs 4 end
%maxcore 2000

${PLACEHOLDER_COORDS}
`;
}

const R2SCAN = "r2SCAN-3c";
const B3LYP = "B3LYP D4 def2-SVP RIJCOSX def2/J";

export const ORCA_TEMPLATES: OrcaTemplate[] = [
  {
    id: "sp-r2scan3c",
    name: "SP · r²SCAN-3c",
    description: "Single point energy, r²SCAN-3c composite method",
    category: "single-point",
    inputContent: buildInput(`${R2SCAN} TightSCF`),
  },
  {
    id: "sp-b3lyp-d4",
    name: "SP · B3LYP-D4/def2-SVP",
    description: "Single point energy, B3LYP-D4 with def2-SVP",
    category: "single-point",
    inputContent: buildInput(`${B3LYP} TightSCF`),
  },
  {
    id: "opt-r2scan3c",
    name: "Opt · r²SCAN-3c",
    description: "Geometry optimization, r²SCAN-3c composite method",
    category: "optimization",
    inputContent: buildInput(`${R2SCAN} Opt TightSCF`),
  },
  {
    id: "opt-b3lyp-d4",
    name: "Opt · B3LYP-D4/def2-SVP",
    description: "Geometry optimization, B3LYP-D4 with def2-SVP",
    category: "optimization",
    inputContent: buildInput(`${B3LYP} Opt TightSCF`),
  },
  {
    id: "freq-r2scan3c",
    name: "Freq · r²SCAN-3c",
    description: "Harmonic frequencies on an optimized geometry, r²SCAN-3c",
    category: "frequency",
    inputContent: buildInput(`${R2SCAN} Freq TightSCF`),
  },
  {
    id: "freq-b3lyp-d4",
    name: "Freq · B3LYP-D4/def2-SVP",
    description: "Harmonic frequencies on an optimized geometry, B3LYP-D4",
    category: "frequency",
    inputContent: buildInput(`${B3LYP} Freq TightSCF`),
  },
  {
    id: "optfreq-r2scan3c",
    name: "Opt+Freq · r²SCAN-3c",
    description: "Optimization then frequencies, r²SCAN-3c composite method",
    category: "opt+freq",
    inputContent: buildInput(`${R2SCAN} Opt Freq TightSCF`),
  },
  {
    id: "optfreq-b3lyp-d4",
    name: "Opt+Freq · B3LYP-D4/def2-SVP",
    description: "Optimization then frequencies, B3LYP-D4 with def2-SVP",
    category: "opt+freq",
    inputContent: buildInput(`${B3LYP} Opt Freq TightSCF`),
  },
];

/** Human-facing labels + display order for the template picker groups. */
export const CATEGORY_LABELS: { category: OrcaCategory; label: string }[] = [
  { category: "single-point", label: "Single point" },
  { category: "optimization", label: "Optimization" },
  { category: "frequency", label: "Frequency" },
  { category: "opt+freq", label: "Opt + Freq" },
];
