import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { InputEditor } from "../editor/InputEditor";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { extractXyzFromInput } from "../viewer/parse-xyz-from-input";
import { injectXyzIntoInput } from "../viewer/inject-xyz-into-input";
import {
  CATEGORY_LABELS,
  ORCA_TEMPLATES,
  type OrcaTemplate,
} from "../templates/orca-templates";
import type { Job, SidecarStatus } from "../types";

interface NewJobScreenProps {
  /** A draft job was created; parent navigates to the Jobs list. */
  onCreatedDraft: () => void;
  /** Open a job's detail screen, optionally auto-running it. */
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
}

/**
 * Parse standard xyz text (`count`, comment, then `element x y z` rows) into
 * ORCA coordinate lines. Returns `null` if the first line isn't a positive atom
 * count or no valid coordinate rows follow.
 */
function xyzToAtomLines(xyz: string): string[] | null {
  const lines = xyz.split(/\r?\n/);
  if (lines.length < 3) return null;
  const count = Number(lines[0].trim());
  if (!Number.isInteger(count) || count <= 0) return null;

  const atoms: string[] = [];
  for (let i = 2; i < lines.length; i++) {
    const t = lines[i].trim();
    if (t.length === 0) continue;
    const parts = t.split(/\s+/);
    if (parts.length < 4) continue;
    const [element, x, y, z] = parts;
    if (![x, y, z].every((n) => Number.isFinite(Number(n)))) continue;
    atoms.push(`${element}   ${x}   ${y}   ${z}`);
  }
  return atoms.length > 0 ? atoms : null;
}

export function NewJobScreen({ onCreatedDraft, onOpenDetail }: NewJobScreenProps) {
  const [title, setTitle] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [smiles, setSmiles] = useState("");
  const [generating, setGenerating] = useState(false);
  const fileInputRef = useRef<HTMLInputElement>(null);
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
      setContent((c) => injectXyzIntoInput(c, atoms, data.charge, 1));
      if (!title.trim()) setTitle(data.formula);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setGenerating(false);
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
      </div>

      {error ? <div className="banner err">{error}</div> : null}

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
