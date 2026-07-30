import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import type { JobStatus, ParsedResults } from "../types";

/** 1 Hartree in J/mol = E_h (4.359744722e-18 J) × N_A (6.02214076e23 /mol).
 * CODATA 2018; the named factor, like BOHR_TO_ANGSTROM in the Rust readers. */
const EH_TO_J_PER_MOL = 2_625_499.6;

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
              dipole <strong>{results.dipole.magnitude_au.toFixed(4)}</strong> a.u.
            </div>
          )}
        </div>

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

        {results.frequencies && <FrequencyPanel f={results.frequencies} />}

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

/** Frequencies + IR intensities. The imaginary-mode count is shown prominently as
 * a teaching moment (ROADMAP): it says whether the geometry is a minimum, a
 * transition state, or neither — explained, not alarming. */
function FrequencyPanel({ f }: { f: NonNullable<ParsedResults["frequencies"]> }) {
  const verdict =
    f.imaginary_count === 0
      ? { text: "Minimum — 0 imaginary frequencies.", tone: "var(--muted)" }
      : f.imaginary_count === 1
        ? { text: "Transition state — exactly 1 imaginary frequency.", tone: "var(--text)" }
        : {
            text: `Neither a minimum nor a transition state — ${f.imaginary_count} imaginary frequencies; re-optimize.`,
            tone: "var(--text)",
          };

  // Real vibrational modes only: the 5–6 exact-zero translation/rotation modes are
  // not vibrations. Imaginary (negative) modes are kept and flagged.
  const modes = f.frequencies_cm
    .map((cm, i) => ({ cm, ir: f.ir_intensity_km_mol[i] ?? 0 }))
    .filter((m) => m.cm !== 0);

  return (
    <section>
      <div className="section-title" style={{ fontSize: 12 }}>
        Vibrational frequencies
      </div>
      <div
        className="mono"
        style={{ fontSize: 12, marginBottom: 6, color: verdict.tone, fontWeight: 600 }}
      >
        {verdict.text}
      </div>
      <table className="mono" style={{ fontSize: 12, borderCollapse: "collapse" }}>
        <thead>
          <tr>
            <th style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>#</th>
            <th style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>
              cm⁻¹
            </th>
            <th style={{ textAlign: "right", color: "var(--muted)" }}>IR km/mol</th>
          </tr>
        </thead>
        <tbody>
          {modes.map((m, i) => {
            const imaginary = m.cm < 0;
            return (
              <tr key={i}>
                <td style={{ textAlign: "right", paddingRight: 12, color: "var(--muted)" }}>
                  {i + 1}
                  {imaginary ? " ✕" : ""}
                </td>
                <td
                  style={{
                    textAlign: "right",
                    paddingRight: 12,
                    fontWeight: imaginary ? 700 : 400,
                  }}
                  title={imaginary ? "imaginary (negative) frequency" : undefined}
                >
                  {m.cm.toFixed(2)}
                </td>
                <td style={{ textAlign: "right" }}>{m.ir.toFixed(2)}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {f.scale_factor != null && f.scale_factor !== 1 && (
        <div className="muted" style={{ fontSize: 11, marginTop: 4 }}>
          frequency scale factor {f.scale_factor}
        </div>
      )}
    </section>
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
