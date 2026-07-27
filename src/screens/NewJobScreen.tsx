import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { InputEditor } from "../editor/InputEditor";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { extractXyzFromInput } from "../viewer/parse-xyz-from-input";
import { injectXyzIntoInput } from "../viewer/inject-xyz-into-input";
import { xyzToAtomLines, parseChargeMult } from "../viewer/xyz-format";
import { InputBuilderForm } from "../input-builder/InputBuilderForm";
import {
  CATEGORY_LABELS,
  ORCA_TEMPLATES,
  type OrcaTemplate,
} from "../templates/orca-templates";
import type { Job, Molecule, SidecarStatus } from "../types";

interface NewJobScreenProps {
  /** A draft job was created; parent navigates to the Jobs list. */
  onCreatedDraft: () => void;
  /** Open a job's detail screen, optionally auto-running it. */
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
  /** A library molecule to preload into the editor on mount ("Use" action). */
  initialMolecule?: Molecule;
}

export function NewJobScreen({
  onCreatedDraft,
  onOpenDetail,
  initialMolecule,
}: NewJobScreenProps) {
  const [title, setTitle] = useState(initialMolecule?.name ?? "");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [smiles, setSmiles] = useState("");
  const [generating, setGenerating] = useState(false);
  // Formula carried from the last SMILES generation, used when saving to the
  // library (a plain .xyz import leaves it empty). Cleared when the editor's
  // coordinate block is replaced by other means.
  const [formula, setFormula] = useState(initialMolecule?.formula ?? "");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  const [builderOpen, setBuilderOpen] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // Preload a library molecule's coordinates into the editor once, on mount.
  useEffect(() => {
    if (!initialMolecule) return;
    const atoms = xyzToAtomLines(initialMolecule.xyz);
    if (!atoms) return;
    setContent((c) =>
      injectXyzIntoInput(
        c,
        atoms,
        initialMolecule.charge,
        initialMolecule.multiplicity,
      ),
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  // xyz extracted from the editor for the live preview — debounced so we don't
  // re-parse on every keystroke.
  const [previewXyz, setPreviewXyz] = useState<string | null>(null);

  useEffect(() => {
    const id = setTimeout(() => {
      setPreviewXyz(extractXyzFromInput(content));
    }, 500);
    return () => clearTimeout(id);
  }, [content]);

  const pickTemplate = (t: OrcaTemplate) => {
    setSelectedId(t.id);
    setContent(t.inputContent);
    if (!title.trim()) setTitle(t.name);
  };

  // Import a local .xyz file: read it, validate, and replace the editor's
  // coordinate block (neutral charge 0, singlet). No sidecar needed.
  const importXyzFile = (file: File) => {
    const reader = new FileReader();
    reader.onload = () => {
      const atoms = xyzToAtomLines(String(reader.result ?? ""));
      if (!atoms) {
        setError(`"${file.name}" is not a valid .xyz file`);
        return;
      }
      setError(null);
      setSaved(false);
      setFormula(""); // a bare .xyz carries no formula
      setContent((c) => injectXyzIntoInput(c, atoms, 0, 1));
      const base = file.name.replace(/\.[^.]+$/, "");
      if (!title.trim()) setTitle(base);
    };
    reader.onerror = () => setError(`Could not read "${file.name}"`);
    reader.readAsText(file);
  };

  // Generate a 3D structure from SMILES via the sidecar (RDKit ETKDG + MMFF),
  // then inject the coordinates with the formal charge RDKit derived.
  const generateFromSmiles = async () => {
    const s = smiles.trim();
    if (!s) return;
    setGenerating(true);
    setError(null);
    try {
      const status = await invoke<SidecarStatus>("get_sidecar_status");
      if (!status.port) throw new Error("Sidecar is not ready yet");
      const resp = await fetch(
        `http://127.0.0.1:${status.port}/smiles-to-3d`,
        {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ smiles: s }),
        },
      );
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
      const atoms = xyzToAtomLines(data.xyz);
      if (!atoms) throw new Error("Sidecar returned a malformed structure");
      setSaved(false);
      setFormula(data.formula);
      setContent((c) => injectXyzIntoInput(c, atoms, data.charge, 1));
      if (!title.trim()) setTitle(data.formula);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setGenerating(false);
    }
  };

  // Save the editor's current coordinates as a new library molecule. Charge and
  // multiplicity come from the `* xyz charge mult` header; the formula is
  // whatever the last SMILES generation reported (empty for a plain .xyz).
  const saveToLibrary = async () => {
    const xyz = extractXyzFromInput(content);
    if (!xyz) {
      setError("No coordinates in the input to save");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const { charge, multiplicity } = parseChargeMult(content);
      await invoke<Molecule>("create_molecule", {
        name: title.trim() || "Untitled molecule",
        formula,
        xyz,
        charge,
        multiplicity,
        tags: "",
      });
      setSaved(true);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  const create = async (run: boolean) => {
    setCreating(true);
    setError(null);
    try {
      const job = await invoke<Job>("create_job", {
        title: title.trim() || "Untitled job",
        inputContent: content,
      });
      // The detail screen performs the actual submit (after attaching its log
      // listeners) so no early output lines are missed.
      if (run) onOpenDetail(job.id, true);
      else onCreatedDraft();
    } catch (e) {
      setError(String(e));
    } finally {
      setCreating(false);
    }
  };

  const canCreate = content.trim().length > 0 && !creating;

  return (
    <div className="screen new-job">
      <div className="row" style={{ gap: 16 }}>
        <div className="field" style={{ flex: 1 }}>
          <label className="label" htmlFor="job-title">
            Job title
          </label>
          <input
            id="job-title"
            className="input"
            placeholder="e.g. water opt/freq"
            value={title}
            onChange={(e) => setTitle(e.currentTarget.value)}
            spellCheck={false}
          />
        </div>
        <div className="row" style={{ alignSelf: "flex-end" }}>
          <button
            className="btn"
            onClick={() => create(false)}
            disabled={!canCreate}
          >
            {creating ? "Creating…" : "Create Job"}
          </button>
          <button
            className="btn btn-primary"
            onClick={() => create(true)}
            disabled={!canCreate}
          >
            Create &amp; Run
          </button>
        </div>
      </div>

      <div className="import-row">
        <input
          ref={fileInputRef}
          type="file"
          accept=".xyz,.XYZ"
          hidden
          onChange={(e) => {
            const f = e.currentTarget.files?.[0];
            if (f) importXyzFile(f);
            e.currentTarget.value = ""; // allow re-picking the same file
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
        <button
          className="btn btn-sm"
          onClick={saveToLibrary}
          disabled={saving || !previewXyz}
          title="Save the current coordinates as a library molecule"
        >
          {saving ? "Saving…" : "Save to Library"}
        </button>
      </div>

      {error ? <div className="banner err">{error}</div> : null}
      {saved ? <div className="banner ok">Saved to library</div> : null}

      <div className="input-builder">
        <button
          className="builder-toggle"
          onClick={() => setBuilderOpen((o) => !o)}
          aria-expanded={builderOpen}
        >
          <span className="builder-caret">{builderOpen ? "▾" : "▸"}</span>
          Input Builder
          <span className="muted" style={{ marginLeft: 8 }}>
            method · basis · solvation → generates the input
          </span>
        </button>
        {builderOpen ? (
          <InputBuilderForm currentContent={content} onGenerate={setContent} />
        ) : null}
      </div>

      <div className="template-groups">
        <div className="template-group-title">Templates</div>
        {CATEGORY_LABELS.map(({ category, label }) => (
          <div key={category}>
            <div
              className="template-group-title"
              style={{ color: "var(--muted-2)", marginTop: 4 }}
            >
              {label}
            </div>
            <div className="template-grid">
              {ORCA_TEMPLATES.filter((t) => t.category === category).map((t) => (
                <button
                  key={t.id}
                  className={
                    "template-card" + (selectedId === t.id ? " selected" : "")
                  }
                  onClick={() => pickTemplate(t)}
                >
                  <div className="tname">{t.name}</div>
                  <div className="tdesc">{t.description}</div>
                </button>
              ))}
            </div>
          </div>
        ))}
      </div>

      <div className="editor-viewer-split">
        <div className="editor-wrap">
          <InputEditor value={content} onChange={setContent} />
        </div>
        <div className="viewer-panel">
          {previewXyz ? (
            <MoleculeViewer xyzData={previewXyz} />
          ) : (
            <div className="viewer-empty muted">No coordinates in input</div>
          )}
        </div>
      </div>
    </div>
  );
}
