//! IR spectrum from a `.hess` frequency/intensity list — pure, node-tested,
//! React-free (the same discipline as `scene/measure.ts`).
//!
//! The data is already parsed and stored (unit 3.6, `data_json.frequencies`):
//! `frequencies_cm` (signed, 3N) index-aligned with `ir_intensity_km_mol`
//! (km/mol, measured — `wiki/orca/parse-sources.md`). This module turns that
//! **stick** list into a broadened curve and, first, splits it into the three
//! physically distinct kinds of mode — a split by MEASURED FACT, not a heuristic
//! threshold:
//!
//!  * **translation/rotation** modes — ORCA prints these as **exactly `0.0`**
//!    (measured: 5 for a linear molecule, 6 otherwise; already projected out, no
//!    small residue). Excluded by `=== 0`, never by a cutoff.
//!  * **imaginary** modes — ORCA writes them **negative** (measured `-33.66` on
//!    the saddle). Excluded from the broadened curve by SIGN — but kept and
//!    surfaced separately: an imaginary frequency is a *diagnosis* (a transition
//!    state), not noise.
//!  * **real vibrations** (`cm > 0`) — the modes that get a Lorentzian.
//!
//! Broadening choices made EXPLICIT (they are plot-construction parameters, not
//! properties of the molecule): the line shape (Lorentzian), the FWHM (a UI
//! control), the x-grid (range + step named, not eyeballed), and the
//! normalization (∫ of one mode's curve == its km/mol intensity, so the AREA
//! under a peak is physically meaningful). Cross-checked against ORCA's own
//! `orca_mapspc` — see `wiki/orca/parse-sources.md`.

/** One stick: a mode's wavenumber, its IR intensity, and its ORIGINAL index in
 * the full `frequencies_cm` array (so a peak on the chart maps back to the exact
 * row in the frequency table — the click-to-select seam). */
export interface IrMode {
  /** Wavenumber, cm⁻¹ (signed as stored; > 0 for `active`, < 0 for `imaginary`). */
  cm: number;
  /** IR intensity, km/mol (measured canonical unit). */
  kmMol: number;
  /** Index into the full `frequencies_cm` / `ir_intensity_km_mol` arrays. */
  index: number;
}

export interface ClassifiedModes {
  /** Real vibrations (`cm > 0`) — the modes that are broadened. */
  active: IrMode[];
  /** Imaginary modes (`cm < 0`) — shown separately as a transition-state
   * diagnosis, NOT broadened into the spectrum (a Lorentzian would smear a peak
   * into negative wavenumbers). */
  imaginary: IrMode[];
  /** Count of exact-zero translation/rotation modes (`cm === 0`). */
  zeroCount: number;
}

/**
 * Split the raw signed frequency list into the three kinds — by the measured
 * facts (exact zero; negative sign), never by a magnitude threshold. A tiny
 * positive frequency stays a real vibration; a tiny negative one is imaginary.
 */
export function classifyModes(freqCm: number[], irKmMol: number[]): ClassifiedModes {
  const active: IrMode[] = [];
  const imaginary: IrMode[] = [];
  let zeroCount = 0;
  for (let i = 0; i < freqCm.length; i++) {
    const cm = freqCm[i];
    const kmMol = irKmMol[i] ?? 0;
    if (cm === 0) {
      // Exact-zero trans/rot (measured: literally 0.0, already projected out).
      zeroCount += 1;
    } else if (cm < 0) {
      imaginary.push({ cm, kmMol, index: i });
    } else {
      active.push({ cm, kmMol, index: i });
    }
  }
  return { active, imaginary, zeroCount };
}

/** A default FWHM, cm⁻¹. A PLOT choice with a reasonable value — exposed as a UI
 * control because it is not a property of the molecule. Empirical IR line widths
 * are a few to a few tens of cm⁻¹; `orca_mapspc`'s own default is 50. */
export const DEFAULT_FWHM_CM = 10;
export const MIN_FWHM_CM = 1;
export const MAX_FWHM_CM = 60;

/**
 * Area-normalized Lorentzian: **∫ L dx = 1** over all x, so `intensity · L`
 * integrates to `intensity`. Written so the normalization is visible: with
 * half-width `g = FWHM/2`,
 *
 *     L(x) = (1/π) · g / ((x − x₀)² + g²)
 *
 * Peak value `L(x₀) = 1/(π·g) = 2/(π·FWHM)`; value at `x₀ ± g` is exactly half of
 * that — i.e. the Full Width at Half Maximum is `2g = FWHM`, which is why `g` is
 * `FWHM/2` and not `FWHM` (the convention `orca_mapspc` also uses — its `-w` is
 * labelled "Peak FWHM", measured from its `-h`).
 */
export function lorentzian(x: number, x0: number, fwhm: number): number {
  const g = fwhm / 2;
  return (1 / Math.PI) * g / ((x - x0) ** 2 + g * g);
}

export interface Grid {
  /** cm⁻¹ (inclusive). */
  min: number;
  /** cm⁻¹ (inclusive). */
  max: number;
  /** cm⁻¹ between samples. */
  step: number;
}

/**
 * Choose the x-axis explicitly from the active modes and the FWHM — never "by
 * eye". Range = the mode span padded by `PAD_FWHM` line widths on each side (so
 * every peak's wings are on-screen), clamped at 0 on the left (negative
 * wavenumbers are meaningless). Step = `FWHM / SAMPLES_PER_FWHM`, floored at
 * `MIN_STEP`, so a peak is drawn from enough points to look smooth regardless of
 * how narrow the FWHM is set.
 */
const PAD_FWHM = 8;
const SAMPLES_PER_FWHM = 8;
const MIN_STEP = 0.5;

export function autoGrid(active: IrMode[], fwhm: number): Grid {
  if (active.length === 0) return { min: 0, max: 1, step: 1 };
  let lo = Infinity;
  let hi = -Infinity;
  for (const m of active) {
    if (m.cm < lo) lo = m.cm;
    if (m.cm > hi) hi = m.cm;
  }
  const pad = PAD_FWHM * fwhm;
  const min = Math.max(0, Math.floor(lo - pad));
  const max = Math.ceil(hi + pad);
  const step = Math.max(MIN_STEP, fwhm / SAMPLES_PER_FWHM);
  return { min, max, step };
}

export interface SpectrumPoint {
  cm: number;
  /** Summed absorption at this wavenumber (km/mol · cm⁻¹⁻¹, i.e. the density
   * whose integral over a peak is the km/mol intensity). */
  absorbance: number;
}

/**
 * The broadened spectrum: the sum over active modes of `intensity · Lorentzian`,
 * sampled on `grid` with the given `fwhm`. Because each Lorentzian is
 * area-normalized, the integral of the returned curve over an isolated peak
 * equals that mode's km/mol intensity — `integrate()` + the area test lock this.
 */
export function spectrum(active: IrMode[], grid: Grid, fwhm: number): SpectrumPoint[] {
  const out: SpectrumPoint[] = [];
  const n = Math.round((grid.max - grid.min) / grid.step);
  for (let i = 0; i <= n; i++) {
    const cm = grid.min + i * grid.step;
    let absorbance = 0;
    for (const m of active) absorbance += m.kmMol * lorentzian(cm, m.cm, fwhm);
    out.push({ cm, absorbance });
  }
  return out;
}

/** Trapezoidal integral of a sampled curve — used by the area test and by the
 * panel to report "area under peak == intensity" if ever surfaced. */
export function integrate(points: SpectrumPoint[]): number {
  let area = 0;
  for (let i = 1; i < points.length; i++) {
    const dx = points[i].cm - points[i - 1].cm;
    area += 0.5 * (points[i].absorbance + points[i - 1].absorbance) * dx;
  }
  return area;
}
