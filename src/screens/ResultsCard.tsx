import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { JobStatus, ParsedResults } from "../types";
import { IrSpectrumPanel } from "../spectrum/IrSpectrumPanel";
import { TrajectoryPlayer } from "../trajectory/TrajectoryPlayer";

/** 1 Hartree in J/mol = E_h (4.359744722e-18 J) × N_A (6.02214076e23 /mol).
 * CODATA 2018; the named factor, like BOHR_TO_ANGSTROM in the Rust readers. */
const EH_TO_J_PER_MOL = 2_625_499.6;
/** 1 Hartree in eV (CODATA 2018). Chemists read orbital gaps in eV. */
const EH_TO_EV = 27.211_386_245_988;
/** 1 atomic unit of electric dipole moment (e·a₀) in debye (CODATA 2018:
 * 2.5417464519 D). Stored value stays a.u. (the measured artifact unit); debye
 * is a display-only conversion, like S from T·S and the eV gap. Chemists read
 * dipole moments in debye. */
const AU_TO_DEBYE = 2.541_746_451_9;

/** Minimal parsed-results card (Phase 3, ADR-012): final energy, dipole, atomic
 * charges (three schemes), thermochemistry with correct labels. Frequencies are
 * NOT here — they come from `.hess` (a separate reader).
 *
 * Absence is a normal state, not an error: GOAT has no charges/dipole/thermo, SP
 * no thermochemistry (measured). Empty sections are simply not rendered. The card
 * never crashes or shouts on a job that legitimately lacks a section. */
export function ResultsCard({ jobId, status }: { jobId: string; status: JobStatus }) {
  const [results, setResults] = useState<ParsedResults | null>(null);

  useEffect(() => {
    let live = true;
    // Results exist only once a job reached `parsed`. A `completed` job either
    // has no `.property.txt` or failed to parse (its reason shows elsewhere).
    if (status !== "parsed") {
      setResults(null);
      return;
    }
    invoke<ParsedResults | null>("read_job_results", { id: jobId })
      .then((r) => live && setResults(r))
      .catch(() => live && setResults(null));
    return () => {
      live = false;
    };
  }, [jobId, status]);

  if (!results) return null;

  const t = results.thermochemistry;
  return (
    <div className="input-builder" style={{ marginBottom: 10 }}>
      <div className="section-title" style={{ padding: "8px 10px 0" }}>
        Results
      </div>
      <div style={{ padding: 10, display: "grid", gap: 10 }}>
        <div className="mono" style={{ fontSize: 13 }}>
          {results.final_energy_eh != null && (
            <div>
              final energy <strong>{results.final_energy_eh.toFixed(6)}</strong> Eh
            </div>
          )}
          {results.dipole && (
            <div>
              dipole <strong>{results.dipole.magnitude_au.toFixed(4)}</strong> a.u.{" "}
              <span style={{ color: "var(--muted)" }}>
                ({(results.dipole.magnitude_au * AU_TO_DEBYE).toFixed(2)} D)
              </span>
            </div>
          )}
          {results.orbitals?.homo_lumo && (
            <div>
              HOMO–LUMO gap{" "}
              <strong>{results.orbitals.homo_lumo.gap_eh.toFixed(4)}</strong> Eh{" "}
              <span style={{ color: "var(--muted)" }}>
                ({(results.orbitals.homo_lumo.gap_eh * EH_TO_EV).toFixed(2)} eV)
              </span>
            </div>
          )}
          {results.trajectory && (
            <div>
              trajectory <strong>{results.trajectory.n_frames}</strong> frames
              <span style={{ color: "var(--muted)", fontSize: 11 }}>
                {" "}
                (optimization cycles)
              </span>
            </div>
          )}
        </div>

        {/* Optimization-trajectory playback (unit 3.8, Part A). Hidden for a
            single-point job (no trajectory). The reference element order is the
            final geometry's — the player refuses to animate on a mismatch. */}
        {results.trajectory && results.trajectory.n_frames >= 1 && (
          <TrajectoryPlayer
            elements={results.trajectory.elements}
            frames={results.trajectory.frames}
            referenceElements={results.final_geometry.elements}
          />
        )}

        {t && (
          <section>
            <div className="section-title" style={{ fontSize: 12 }}>
              Thermochemistry (Eh)
            </div>
            <table className="mono" style={{ fontSize: 12, borderCollapse: "collapse" }}>
              <tbody>
                <ThermoRow label="Electronic energy" value={t.el_energy_eh} />
                <ThermoRow label="Zero-point energy" value={t.zpe_eh} />
                <ThermoRow label="Inner energy U" value={t.inner_energy_u_eh} />
                <ThermoRow label="Enthalpy H" value={t.enthalpy_h_eh} />
                {/* Measured: this field is T·S, NOT the entropy S. Labelling it
                    "entropy" would show a number in the wrong units. */}
                <ThermoRow label="T·S (entropy term)" value={t.t_times_s_eh} />
                <ThermoRow label="Gibbs free energy G" value={t.free_energy_g_eh} />
              </tbody>
            </table>
            {/* Derived S in J/(mol·K), explicitly labelled as derived, so the
                entropy is available in its usual unit without mislabelling the
                stored T·S field. S = (T·S) / T. */}
            <div className="mono" style={{ fontSize: 12, marginTop: 4, color: "var(--muted)" }}>
              entropy S = {((t.t_times_s_eh / t.temperature_k) * EH_TO_J_PER_MOL).toFixed(2)}{" "}
              J/(mol·K){"  "}
              <span style={{ fontSize: 11 }}>
                (derived: T·S / T at T = {t.temperature_k.toFixed(2)} K)
              </span>
            </div>
          </section>
        )}

        {results.frequencies && <IrSpectrumPanel f={results.frequencies} />}

        {results.charges.length > 0 && (
          <section>
            <div className="section-title" style={{ fontSize: 12 }}>
              Atomic charges
            </div>
            <ChargesTable charges={results.charges} />
          </section>
        )}

        {results.unknown_blocks.length > 0 && (
          <div className="muted" style={{ fontSize: 11 }}>
            unrecognised property blocks: {results.unknown_blocks.join(", ")}
          </div>
        )}
      </div>
    </div>
  );
}

function ThermoRow({ label, value }: { label: string; value: number }) {
  return (
    <tr>
      <td style={{ paddingRight: 12, color: "var(--muted)" }}>{label}</td>
      <td style={{ textAlign: "right" }}>{value.toFixed(6)}</td>
    </tr>
  );
}

/** One column per scheme; one row per atom, keyed by the stored element sequence
 * (the charges carry their own element order — never positional alone). */
function ChargesTable({ charges }: { charges: ParsedResults["charges"] }) {
  const nAtoms = Math.max(0, ...charges.map((c) => c.charges.length));
  // Element labels from the first scheme (all schemes verified same order).
  const elements = charges[0]?.elements ?? [];
  return (
    <table className="mono" style={{ fontSize: 12, borderCollapse: "collapse" }}>
      <thead>
        <tr>
          <th style={{ textAlign: "left", paddingRight: 12, color: "var(--muted)" }}>atom</th>
          {charges.map((c) => (
            <th key={c.scheme} style={{ textAlign: "right", paddingLeft: 12, color: "var(--muted)" }}>
              {c.scheme}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {Array.from({ length: nAtoms }, (_, i) => (
          <tr key={i}>
            <td style={{ paddingRight: 12 }}>
              {i} {elements[i] ?? ""}
            </td>
            {charges.map((c) => (
              <td key={c.scheme} style={{ textAlign: "right", paddingLeft: 12 }}>
                {c.charges[i] != null ? c.charges[i].toFixed(4) : "—"}
              </td>
            ))}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
