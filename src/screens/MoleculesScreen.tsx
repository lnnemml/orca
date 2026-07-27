import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { xyzToAtomLines, atomLinesToXyz } from "../viewer/xyz-format";
import { formatTimestamp } from "../format";
import type { Molecule, SidecarStatus } from "../types";

interface MoleculesScreenProps {
  /** Send a molecule to the New Job screen ("Use" action). */
  onUseMolecule: (mol: Molecule) => void;
}

/** Blank draft used by the Add Molecule form. */
interface Draft {
  name: string;
  formula: string;
  xyz: string | null;
  charge: number;
  multiplicity: number;
  tags: string;
}

const EMPTY_DRAFT: Draft = {
  name: "",
  formula: "",
  xyz: null,
  charge: 0,
  multiplicity: 1,
  tags: "",
};

export function MoleculesScreen({ onUseMolecule }: MoleculesScreenProps) {
  const [molecules, setMolecules] = useState<Molecule[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Molecule | null>(null);
  const [adding, setAdding] = useState(false);

  const load = useCallback(async () => {
    try {
      setMolecules(await invoke<Molecule[]>("list_molecules"));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  const remove = async (id: string) => {
    try {
      await invoke("delete_molecule", { id });
      if (selected?.id === id) setSelected(null);
      await load();
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="screen">
      <div
        className="row"
        style={{ justifyContent: "space-between", marginBottom: 14 }}
      >
        <h2 className="section-title" style={{ margin: 0 }}>
          Molecules
        </h2>
        <button
          className="btn btn-primary"
          onClick={() => {
            setSelected(null);
            setAdding((a) => !a);
          }}
        >
          {adding ? "Cancel" : "Add Molecule"}
        </button>
      </div>

      {error ? <div className="banner err">{error}</div> : null}

      {adding ? (
        <AddMoleculeForm
          onSaved={async () => {
            setAdding(false);
            await load();
          }}
          onError={setError}
        />
      ) : null}

      {loading ? (
        <div className="empty">Loading…</div>
      ) : molecules.length === 0 && !adding ? (
        <div className="empty">
          No saved molecules. Import from SMILES or .xyz on the New Job screen.
        </div>
      ) : molecules.length > 0 ? (
        <table className="jobs-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Formula</th>
              <th style={{ textAlign: "right" }}>Charge</th>
              <th>Tags</th>
              <th>Created</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {molecules.map((mol) => (
              <tr
                key={mol.id}
                className="clickable"
                onClick={() =>
                  setSelected((s) => (s?.id === mol.id ? null : mol))
                }
              >
                <td>{mol.name}</td>
                <td className="mono">{mol.formula || "—"}</td>
                <td className="mono" style={{ textAlign: "right" }}>
                  {mol.charge}
                </td>
                <td className="mono">{mol.tags || "—"}</td>
                <td className="mono">{formatTimestamp(mol.created_at)}</td>
                <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
                  <button
                    className="btn btn-primary btn-sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      onUseMolecule(mol);
                    }}
                  >
                    Use
                  </button>{" "}
                  <button
                    className="btn btn-sm"
                    onClick={(e) => {
                      e.stopPropagation();
                      remove(mol.id);
                    }}
                  >
                    Delete
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}

      {selected ? (
        <div className="molecule-detail">
          <div className="row" style={{ justifyContent: "space-between" }}>
            <h3 style={{ margin: "16px 0 8px" }}>{selected.name}</h3>
            <span className="muted">
              {selected.formula || "unknown formula"} · charge {selected.charge}{" "}
              · mult {selected.multiplicity}
              {selected.tags ? ` · ${selected.tags}` : ""}
            </span>
          </div>
          <div className="viewer-panel" style={{ height: 320 }}>
            <MoleculeViewer xyzData={selected.xyz} />
          </div>
        </div>
      ) : null}
    </div>
  );
}

interface AddMoleculeFormProps {
  onSaved: () => void;
  onError: (msg: string) => void;
}

/** Inline form to build and persist a new library molecule. */
function AddMoleculeForm({ onSaved, onError }: AddMoleculeFormProps) {
  const [draft, setDraft] = useState<Draft>(EMPTY_DRAFT);
  const [smiles, setSmiles] = useState("");
  const [generating, setGenerating] = useState(false);
  const [saving, setSaving] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) =>
    setDraft((d) => ({ ...d, [key]: value }));

  // Import a local .xyz file: read, validate, and normalise into a standard xyz
  // string for the preview and for saving.
  const importXyzFile = (file: File) => {
    const reader = new FileReader();
    reader.onload = () => {
      const atoms = xyzToAtomLines(String(reader.result ?? ""));
      if (!atoms) {
        onError(`"${file.name}" is not a valid .xyz file`);
        return;
      }
      setDraft((d) => ({
        ...d,
        xyz: atomLinesToXyz(atoms, d.name || file.name),
        name: d.name || file.name.replace(/\.[^.]+$/, ""),
        formula: "",
      }));
    };
    reader.onerror = () => onError(`Could not read "${file.name}"`);
    reader.readAsText(file);
  };

  // Generate a 3D structure from SMILES via the sidecar (RDKit ETKDG + MMFF).
  const generateFromSmiles = async () => {
    const s = smiles.trim();
    if (!s) return;
    setGenerating(true);
    try {
      const status = await invoke<SidecarStatus>("get_sidecar_status");
      if (!status.port) throw new Error("Sidecar is not ready yet");
      const resp = await fetch(`http://127.0.0.1:${status.port}/smiles-to-3d`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ smiles: s }),
      });
      if (!resp.ok) {
        let detail = `HTTP ${resp.status}`;
        try {
          const body = await resp.json();
          if (body?.detail) detail = String(body.detail);
        } catch {
          /* non-JSON error body — keep the status code */
        }
        throw new Error(detail);
      }
      const data = (await resp.json()) as {
        xyz: string;
        charge: number;
        formula: string;
      };
      setDraft((d) => ({
        ...d,
        xyz: data.xyz,
        formula: data.formula,
        charge: data.charge,
        name: d.name || data.formula,
      }));
    } catch (e) {
      onError(e instanceof Error ? e.message : String(e));
    } finally {
      setGenerating(false);
    }
  };

  const save = async () => {
    if (!draft.xyz) {
      onError("Generate or import a structure first");
      return;
    }
    setSaving(true);
    try {
      await invoke<Molecule>("create_molecule", {
        name: draft.name.trim() || "Untitled molecule",
        formula: draft.formula,
        xyz: draft.xyz,
        charge: draft.charge,
        multiplicity: draft.multiplicity,
        tags: draft.tags.trim(),
      });
      onSaved();
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="molecule-form card" style={{ maxWidth: "none" }}>
      <div className="row" style={{ gap: 16, marginBottom: 12 }}>
        <div className="field" style={{ flex: 1 }}>
          <label className="label" htmlFor="mol-name">
            Name
          </label>
          <input
            id="mol-name"
            className="input"
            placeholder="e.g. ethanol, substrate-17"
            value={draft.name}
            onChange={(e) => set("name", e.currentTarget.value)}
            spellCheck={false}
          />
        </div>
        <div className="field" style={{ width: 120 }}>
          <label className="label" htmlFor="mol-charge">
            Charge
          </label>
          <input
            id="mol-charge"
            className="input mono"
            type="number"
            value={draft.charge}
            onChange={(e) => set("charge", Number(e.currentTarget.value))}
          />
        </div>
        <div className="field" style={{ width: 120 }}>
          <label className="label" htmlFor="mol-mult">
            Multiplicity
          </label>
          <input
            id="mol-mult"
            className="input mono"
            type="number"
            min={1}
            value={draft.multiplicity}
            onChange={(e) => set("multiplicity", Number(e.currentTarget.value))}
          />
        </div>
      </div>

      <div className="import-row" style={{ marginBottom: 12 }}>
        <input
          ref={fileInputRef}
          type="file"
          accept=".xyz,.XYZ"
          hidden
          onChange={(e) => {
            const f = e.currentTarget.files?.[0];
            if (f) importXyzFile(f);
            e.currentTarget.value = "";
          }}
        />
        <button className="btn btn-sm" onClick={() => fileInputRef.current?.click()}>
          Import .xyz
        </button>
        <span className="import-or">or</span>
        <input
          className="input mono import-smiles"
          placeholder="SMILES, e.g. CCO"
          value={smiles}
          onChange={(e) => setSmiles(e.currentTarget.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") generateFromSmiles();
          }}
          spellCheck={false}
        />
        <button
          className="btn btn-sm"
          onClick={generateFromSmiles}
          disabled={generating || !smiles.trim()}
        >
          {generating ? "Generating…" : "Generate 3D"}
        </button>
      </div>

      <div className="field" style={{ marginBottom: 12 }}>
        <label className="label" htmlFor="mol-tags">
          Tags (comma-separated)
        </label>
        <input
          id="mol-tags"
          className="input"
          placeholder="substrate, reagent, fragment"
          value={draft.tags}
          onChange={(e) => set("tags", e.currentTarget.value)}
          spellCheck={false}
        />
      </div>

      <div className="viewer-panel" style={{ height: 260, marginBottom: 12 }}>
        {draft.xyz ? (
          <MoleculeViewer xyzData={draft.xyz} />
        ) : (
          <div className="viewer-empty muted">
            No structure yet — generate from SMILES or import a .xyz
          </div>
        )}
      </div>

      <div className="row" style={{ justifyContent: "flex-end" }}>
        <button
          className="btn btn-primary"
          onClick={save}
          disabled={saving || !draft.xyz}
        >
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  );
}
