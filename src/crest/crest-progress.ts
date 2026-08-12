//! The live-progress line for a CREST/QCG grow (Stage F F1c) — the sibling of
//! `formatXtbProgress`. CREST does not print a simple cycle counter the way xtb does, and
//! the F1b runner emits no `crest:progress`, so the only live signal is a ticking elapsed
//! clock — enough to show the grow is moving (it is ~10 s) rather than five minutes of
//! silence. Pure, so the display contract is testable without a window.
export function formatCrestProgress(elapsedSecs: number): string {
  return `growing solvent shell… ${elapsedSecs}s`;
}
