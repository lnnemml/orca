//! Export builders — pure, node-tested, React-free. They turn the ALREADY-PARSED,
//! ALREADY-STORED results (never a re-parse) into xyz / CSV text. Units live in every
//! CSV header (rule #11 holds in a file as on screen); numbers carry the full stored
//! precision, not the rounded display value; and the `entropyS` field is exported as
//! **T·S in Eh**, never as "entropy" (measured — it is `H − G`).

import type { ParsedResults } from "../types";
import { classifyModes } from "../spectrum/ir";

/** 1 Hartree in eV (CODATA 2018). */
const EH_TO_EV = 27.211_386_245_988;
/** 1 Hartree in J/mol (CODATA 2018) — for the derived entropy S = T·S / T. */
const EH_TO_J_PER_MOL = 2_625_499.6;

type Geometry = ParsedResults["final_geometry"];
type Frequencies = NonNullable<ParsedResults["frequencies"]>;
type Charges = ParsedResults["charges"];
type Orbitals = NonNullable<ParsedResults["orbitals"]>;
type Thermo = NonNullable<ParsedResults["thermochemistry"]>;

/** Join CSV cells; our values (numbers, element symbols, scheme names) never contain
 * commas, so no quoting is needed. */
const row = (...cells: (string | number)[]) => cells.join(",");

/**
 * Final geometry as a standard `.xyz`: `count / comment / SYM x y z…` in Å (the stored
 * canonical unit), element order from storage. The comment carries the job title and the
 * final energy. **Post-condition (rule #9):** the body has exactly one line per atom, or
 * it throws — never a silently short/long file.
 */
export function finalGeometryXyz(
  geometry: Geometry,
  jobTitle: string,
  finalEnergyEh: number | null,
): string {
  const { elements, xyz_angstrom } = geometry;
  if (elements.length !== xyz_angstrom.length) {
    throw new Error(
      `geometry has ${elements.length} elements but ${xyz_angstrom.length} coordinate rows`,
    );
  }
  const energy = finalEnergyEh != null ? ` · E = ${finalEnergyEh} Eh` : "";
  const comment = `${jobTitle}${energy}`.replace(/\r?\n/g, " ");
  const body = elements.map((el, i) => {
    const [x, y, z] = xyz_angstrom[i];
    return `${el} ${x} ${y} ${z}`;
  });
  const lines = [String(elements.length), comment, ...body];
  // Post-condition: count line + comment line + one line per atom.
  if (lines.length !== elements.length + 2) {
    throw new Error(`xyz line count ${lines.length} ≠ atoms + 2 (${elements.length + 2})`);
  }
  return lines.join("\n") + "\n";
}

/**
 * Frequencies CSV — the ACTIVE (real, cm > 0) vibrations, matching the on-screen table:
 * mode #, raw wavenumber (cm⁻¹), IR intensity (km/mol). When a display `scale ≠ 1` is in
 * effect, a `scaled ×N (derived)` column is added — labelled derived, never presented as
 * the measured value.
 */
export function frequenciesCsv(freq: Frequencies, scale = 1): string {
  const { active } = classifyModes(freq.frequencies_cm, freq.ir_intensity_km_mol);
  const scaled = scale !== 1;
  const header = scaled
    ? row("mode", "wavenumber_cm-1_raw", `wavenumber_cm-1_scaled_x${scale}_derived`, "IR_intensity_km/mol")
    : row("mode", "wavenumber_cm-1_raw", "IR_intensity_km/mol");
  const rows = active.map((m, i) =>
    scaled
      ? row(i + 1, m.cm, m.cm * scale, m.kmMol)
      : row(i + 1, m.cm, m.kmMol),
  );
  return [header, ...rows].join("\n") + "\n";
}

/**
 * Atomic charges CSV — one row per atom, one column per scheme (Mulliken / Loewdin /
 * Mayer, whichever are present), keyed by the stored element order.
 */
export function chargesCsv(charges: Charges): string {
  if (charges.length === 0) return "";
  const schemes = charges.map((c) => c.scheme);
  const nAtoms = Math.max(0, ...charges.map((c) => c.charges.length));
  const elements = charges[0]?.elements ?? [];
  const header = row("atom_index", "element", ...schemes.map((s) => `${s}_charge_e`));
  const rows = Array.from({ length: nAtoms }, (_, i) =>
    row(i, elements[i] ?? "", ...charges.map((c) => (c.charges[i] != null ? c.charges[i] : ""))),
  );
  return [header, ...rows].join("\n") + "\n";
}

/** MO CSV — number, energy (Eh and eV), occupancy. */
export function orbitalsCsv(orbitals: Orbitals): string {
  const header = row("mo_index", "energy_Eh", "energy_eV", "occupancy");
  const rows = orbitals.orbitals.map(([eh, occ], i) => row(i, eh, eh * EH_TO_EV, occ));
  return [header, ...rows].join("\n") + "\n";
}

/**
 * Thermochemistry CSV — `quantity,value,unit`, all fields. `entropyS` is exported as
 * **T·S in Eh** (its measured meaning), NEVER as "entropy"; the entropy S proper is a
 * separate, explicitly-derived row in J/(mol·K) (`S = T·S / T`).
 */
export function thermochemistryCsv(t: Thermo): string {
  const sJ = (t.t_times_s_eh / t.temperature_k) * EH_TO_J_PER_MOL;
  const lines = [
    row("quantity", "value", "unit"),
    row("temperature", t.temperature_k, "K"),
    row("electronic_energy", t.el_energy_eh, "Eh"),
    row("zero_point_energy", t.zpe_eh, "Eh"),
    row("inner_energy_U", t.inner_energy_u_eh, "Eh"),
    row("enthalpy_H", t.enthalpy_h_eh, "Eh"),
    row("T*S_entropy_term", t.t_times_s_eh, "Eh"), // T·S, NOT S
    row("gibbs_free_energy_G", t.free_energy_g_eh, "Eh"),
    row("entropy_S_derived", sJ, "J/(mol*K)"), // derived: T·S / T
  ];
  return lines.join("\n") + "\n";
}
