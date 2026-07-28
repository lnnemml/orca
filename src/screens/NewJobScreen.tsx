import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { InputEditor } from "../editor/InputEditor";
import { MoleculeViewer } from "../viewer/MoleculeViewer";
import { importStructureFile, IMPORT_ACCEPT } from "../viewer/import-file";
import { InputBuilderForm } from "../input-builder/InputBuilderForm";
import { useSceneStore } from "../scene/store";
import { FragmentList } from "../scene/FragmentList";
import { placeFragment } from "../scene/placement";
import {
  FRAGMENT_LIBRARY,
  libraryFragmentToScene,
  type LibraryFragment,
} from "../scene/fragment-library";
import {
  injectSceneIntoInput,
  mergeToAtomLines,
  mergeToXyz,
  sceneFromAtomLines,
  sceneFromOrcaInput,
  sceneFromXyz,
  serializeScene,
  totalCharge,
  xyzMatchesScene,
} from "../scene/scene";
import { restoreScene } from "../scene/restore";
import type { SceneFragment } from "../scene/types";
import {
  CATEGORY_LABELS,
  ORCA_TEMPLATES,
  type OrcaTemplate,
} from "../templates/orca-templates";
import type { Job, Molecule, SidecarStatus } from "../types";

/** Charge with an explicit sign: `0`, `-1`, `+1`. */
function signed(n: number): string {
  return n > 0 ? `+${n}` : String(n);
}

interface NewJobScreenProps {
  /** A draft job was created; parent navigates to the Jobs list. */
  onCreatedDraft: () => void;
  /** Open a job's detail screen, optionally auto-running it. */
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
  /** A library molecule to preload into the editor on mount ("Use" action). */
  initialMolecule?: Molecule;
  /** An existing job to seed a new iteration from ("New iteration" action):
   * its input + fragment snapshot, nothing from its results/status/dir. */
  initialJob?: Job;
}

export function NewJobScreen({
  onCreatedDraft,
  onOpenDetail,
  initialMolecule,
  initialJob,
}: NewJobScreenProps) {
  // Seed title/content from an iterated job (marked so kinship shows in the job
  // list) or a library molecule; both before first paint via useState.
  const [title, setTitle] = useState(
    initialJob ? `${initialJob.title} (iteration)` : initialMolecule?.name ?? "",
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [content, setContent] = useState(initialJob?.input_content ?? "");
  // Set when an iterated job's fragment snapshot was discarded because it no
  // longer matched its input (distinct from a plain no-snapshot job — no note).
  const [snapshotRejected, setSnapshotRejected] = useState(false);
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [smiles, setSmiles] = useState("");
  const [generating, setGenerating] = useState(false);
  // Formula carried from the last SMILES generation, used when saving to the
  // library. It is library-molecule metadata (RDKit's molecular formula),
  // orthogonal to the Scene geometry — SceneFragment has no formula concept — so
  // it stays a separate state rather than moving into the fragment.
  const [formula, setFormula] = useState(initialMolecule?.formula ?? "");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);
  // Input Builder and Templates behave as a two-section accordion: at most one
  // open at a time, both closed by default so the editor + viewer are visible
  // immediately. Opening one closes the other; picking a template or generating
  // an input collapses the accordion (the user has what they wanted).
  const [openSection, setOpenSection] = useState<
    "add" | "builder" | "templates" | null
  >(null);
  // Library molecules for the Add-Fragment "From library" source (lazy-loaded
  // when the panel opens).
  const [libMolecules, setLibMolecules] = useState<Molecule[]>([]);
  const fileInputRef = useRef<HTMLInputElement>(null);

  // ── Scene store: the single source of truth for geometry (ADR-008) ──────────
  // Selectors return the stored objects directly, so an unchanged scene keeps
  // its identity and MoleculeViewer does not redraw on every keystroke.
  const scene = useSceneStore((s) => s.scene);
  const resetNotice = useSceneStore((s) => s.resetNotice);
  const setScene = useSceneStore((s) => s.setScene);
  const addFragment = useSceneStore((s) => s.addFragment);
  const collapseToSingleFragment = useSceneStore(
    (s) => s.collapseToSingleFragment,
  );
  const undoReset = useSceneStore((s) => s.undoReset);
  const dismissResetNotice = useSceneStore((s) => s.dismissResetNotice);

  // Lazy-load saved molecules whenever the Add-Fragment panel opens.
  useEffect(() => {
    if (openSection !== "add") return;
    invoke<Molecule[]>("list_molecules")
      .then(setLibMolecules)
      .catch(() => setLibMolecules([]));
  }, [openSection]);

  /**
   * The single road every "add a fragment" source takes (ADR-008): place the new
   * fragment clear of whatever is already there, then append it. Works for the
   * first fragment too — `placeFragment` on an empty scene is the identity and
   * `addFragment` seeds a one-fragment scene — so there is no "first replaces,
   * rest add" branch (d-1 removed exactly that split).
   */
  const addFragmentToScene = (fragment: SceneFragment) => {
    const current = useSceneStore.getState().scene ?? {
      fragments: [],
      multiplicity: 1,
    };
    addFragment(placeFragment(current, fragment));
    setSaved(false);
  };

  // Initialise the (module-singleton) scene store for this screen. useLayoutEffect
  // (not useEffect) so the reset runs before paint — the screen remounts on every
  // navigation, and a plain effect would flash the previous visit's molecule.
  //  - iterated job → restore its scene, reconciling the snapshot with its input;
  //  - library molecule → a single "library" fragment;
  //  - otherwise → empty, clearing any scene left over from a prior visit.
  useLayoutEffect(() => {
    if (initialJob) {
      const { scene: restored, snapshotRejected: rejected } = restoreScene(
        initialJob.input_content,
        initialJob.scene_json,
      );
      setScene(restored);
      setSnapshotRejected(rejected);
    } else if (initialMolecule) {
      setScene(
        sceneFromXyz(initialMolecule.xyz, {
          source: "library",
          sourceLabel: initialMolecule.id,
          name: initialMolecule.name,
          charge: initialMolecule.charge,
          multiplicity: initialMolecule.multiplicity,
        }),
      );
    } else {
      setScene(null);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Scene → content (ADR-008 #6): inject the merged coordinate block, leaving
  // the `!` line / `%` blocks / comments untouched. The guard skips the write
  // when the text already reflects this scene — preventing an echo after a
  // content→scene sync and never reformatting a manual edit.
  useEffect(() => {
    if (!scene) return;
    setContent((c) => {
      const parsed = sceneFromOrcaInput(c);
      if (parsed && xyzMatchesScene(scene, mergeToAtomLines(parsed))) return c;
      return injectSceneIntoInput(c, scene);
    });
  }, [scene]);

  // content → Scene (ADR-008 #6): on the same 500 ms debounce the preview used,
  // compare the editor's coordinate block against the scene via xyzMatchesScene
  // (parsed floats, never string compare). Match → leave the scene (the user was
  // editing keywords). Diverged → the text wins: collapse to it. Block gone →
  // clear. Scene absent but a block appeared (template / generated input) → adopt.
  useEffect(() => {
    const id = setTimeout(() => {
      const parsed = sceneFromOrcaInput(content);
      const current = useSceneStore.getState().scene;
      if (!parsed) {
        if (current) setScene(null);
        return;
      }
      if (!current) {
        setScene(parsed);
        return;
      }
      if (xyzMatchesScene(current, mergeToAtomLines(parsed))) return;
      collapseToSingleFragment(parsed.fragments[0].atoms);
    }, 500);
    return () => clearTimeout(id);
  }, [content, setScene, collapseToSingleFragment]);

  const pickTemplate = (t: OrcaTemplate) => {
    setSelectedId(t.id);
    setContent(t.inputContent);
    if (!title.trim()) setTitle(t.name);
    setOpenSection(null); // got the input — collapse so the editor is visible
  };

  // Builder's "Generate Input" → replace editor content and collapse the panel.
  const handleGenerate = (newContent: string) => {
    setContent(newContent);
    setOpenSection(null);
  };

  // Add a curated reagent (BH₄⁻, H₂O, …) as a fragment.
  const addReagent = (lf: LibraryFragment) => {
    setError(null);
    addFragmentToScene(libraryFragmentToScene(lf));
  };

  // Add a saved library molecule as a fragment.
  const addLibraryMolecule = (m: Molecule) => {
    const s = sceneFromXyz(m.xyz, {
      source: "library",
      sourceLabel: m.id,
      name: m.name,
      charge: m.charge,
    });
    if (!s) return;
    setError(null);
    addFragmentToScene(s.fragments[0]);
    if (!title.trim()) setTitle(m.name);
  };

  // Import a structure file: `.xyz` is parsed locally, other formats are
  // converted to xyz by the sidecar (see import-file.ts). Added as an "import"
  // fragment (neutral charge 0).
  const importFile = async (file: File) => {
    try {
      const { atomLines } = await importStructureFile(file);
      setError(null);
      const base = file.name.replace(/\.[^.]+$/, "");
      const s = sceneFromAtomLines(atomLines, {
        source: "import",
        sourceLabel: file.name,
        name: base,
        charge: 0,
        multiplicity: 1,
      });
      if (!s) return;
      // Formula only means something for a single-molecule save; clear it when
      // this import *is* the whole molecule (empty scene), leave it otherwise.
      if (!useSceneStore.getState().scene) setFormula("");
      addFragmentToScene(s.fragments[0]);
      if (!title.trim()) setTitle(base);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  };

  // Generate a 3D structure from SMILES via the sidecar (RDKit ETKDG + MMFF);
  // added as a "smiles" fragment with the formal charge RDKit derived.
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
      const built = sceneFromXyz(data.xyz, {
        source: "smiles",
        sourceLabel: s,
        name: data.formula,
        charge: data.charge,
        multiplicity: 1,
      });
      if (!built) throw new Error("Sidecar returned a malformed structure");
      // Only the first molecule's formula is meaningful for Save to Library.
      if (!useSceneStore.getState().scene) setFormula(data.formula);
      addFragmentToScene(built.fragments[0]);
      if (!title.trim()) setTitle(data.formula);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setGenerating(false);
    }
  };

  // Save the current geometry as a new library molecule. Geometry (canonical)
  // comes from the scene; charge/multiplicity from the live `* xyz` header (so a
  // manual header edit is honoured, matching the pre-store behaviour); the
  // formula is whatever the last SMILES generation reported.
  const saveToLibrary = async () => {
    if (!scene) {
      setError("No coordinates in the input to save");
      return;
    }
    setSaving(true);
    setError(null);
    try {
      const header = sceneFromOrcaInput(content);
      const charge = header ? totalCharge(header) : totalCharge(scene);
      const multiplicity = header ? header.multiplicity : scene.multiplicity;
      await invoke<Molecule>("create_molecule", {
        name: title.trim() || "Untitled molecule",
        formula,
        xyz: mergeToXyz(scene, title.trim()),
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
        // Persist the fragment layout so a later iteration restores it (ADR-008
        // #5). Written once, here; the job's input is immutable, so is this.
        sceneJson: scene ? serializeScene(scene) : null,
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

      <input
        ref={fileInputRef}
        type="file"
        accept={IMPORT_ACCEPT}
        hidden
        onChange={(e) => {
          const f = e.currentTarget.files?.[0];
          if (f) importFile(f);
          e.currentTarget.value = ""; // allow re-picking the same file
        }}
      />

      <div className="import-row">
        <button
          className="btn btn-sm"
          onClick={() => setOpenSection((s) => (s === "add" ? null : "add"))}
          aria-expanded={openSection === "add"}
        >
          {openSection === "add" ? "▾" : "＋"} Add Fragment
        </button>
        <button
          className="btn btn-sm"
          onClick={saveToLibrary}
          disabled={saving || !scene}
          title="Save the current geometry as a library molecule"
        >
          {saving ? "Saving…" : "Save to Library"}
        </button>
      </div>

      {openSection === "add" ? (
        <div className="add-fragment-body">
          <div className="add-source">
            <div className="add-source-title muted">Reagents</div>
            <div className="reagent-chips">
              {FRAGMENT_LIBRARY.map((lf) => (
                <button
                  key={lf.key}
                  className="chip"
                  title={lf.provenance}
                  onClick={() => addReagent(lf)}
                >
                  {lf.name}{" "}
                  <span className="muted">
                    {signed(lf.charge)} · {lf.atoms.length}a
                  </span>
                </button>
              ))}
            </div>
          </div>

          <div className="add-source">
            <div className="add-source-title muted">Import file or SMILES</div>
            <div className="row" style={{ gap: 8, flexWrap: "wrap" }}>
              <button
                className="btn btn-sm"
                onClick={() => fileInputRef.current?.click()}
              >
                Import file
              </button>
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
          </div>

          <div className="add-source">
            <div className="add-source-title muted">From library</div>
            {libMolecules.length === 0 ? (
              <div className="muted" style={{ fontSize: 13 }}>
                No saved molecules yet.
              </div>
            ) : (
              <div className="lib-mol-list">
                {libMolecules.map((m) => (
                  <button
                    key={m.id}
                    className="lib-mol-row"
                    onClick={() => addLibraryMolecule(m)}
                  >
                    <span>{m.name}</span>
                    <span className="muted mono">
                      {m.formula || "—"} · {signed(m.charge)}
                    </span>
                  </button>
                ))}
              </div>
            )}
          </div>
        </div>
      ) : null}

      {error ? <div className="banner err">{error}</div> : null}
      {saved ? <div className="banner ok">Saved to library</div> : null}
      {snapshotRejected ? (
        <div className="banner warn">
          The fragment layout from the source job didn't match its input, so it
          was dropped — this starts from a single fragment.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={() => setSnapshotRejected(false)}
          >
            Dismiss
          </button>
        </div>
      ) : null}
      {resetNotice ? (
        <div className="banner warn">
          Coordinates were edited manually — {resetNotice.fragmentCount} fragments
          merged into one.
          <button
            className="btn btn-sm"
            style={{ marginLeft: 10 }}
            onClick={() => undoReset()}
          >
            Undo
          </button>
          <button
            className="btn btn-sm"
            style={{ marginLeft: 6 }}
            onClick={() => dismissResetNotice()}
          >
            Dismiss
          </button>
        </div>
      ) : null}

      <div className="input-builder">
        <button
          className="builder-toggle"
          onClick={() =>
            setOpenSection((s) => (s === "builder" ? null : "builder"))
          }
          aria-expanded={openSection === "builder"}
        >
          <span className="builder-caret">
            {openSection === "builder" ? "▾" : "▸"}
          </span>
          Input Builder
          <span className="muted" style={{ marginLeft: 8 }}>
            method · basis · solvation → generates the input
          </span>
        </button>
        {openSection === "builder" ? (
          <InputBuilderForm
            currentContent={content}
            onGenerate={handleGenerate}
          />
        ) : null}
      </div>

      <div className="input-builder">
        <button
          className="builder-toggle"
          onClick={() =>
            setOpenSection((s) => (s === "templates" ? null : "templates"))
          }
          aria-expanded={openSection === "templates"}
        >
          <span className="builder-caret">
            {openSection === "templates" ? "▾" : "▸"}
          </span>
          Templates
          <span className="muted" style={{ marginLeft: 8 }}>
            ready-made inputs for common job types
          </span>
        </button>
        {openSection === "templates" ? (
          <div className="template-groups">
            {CATEGORY_LABELS.map(({ category, label }) => (
              <div key={category}>
                <div
                  className="template-group-title"
                  style={{ color: "var(--muted-2)", marginTop: 4 }}
                >
                  {label}
                </div>
                <div className="template-grid">
                  {ORCA_TEMPLATES.filter((t) => t.category === category).map(
                    (t) => (
                      <button
                        key={t.id}
                        className={
                          "template-card" +
                          (selectedId === t.id ? " selected" : "")
                        }
                        onClick={() => pickTemplate(t)}
                      >
                        <div className="tname">{t.name}</div>
                        <div className="tdesc">{t.description}</div>
                      </button>
                    ),
                  )}
                </div>
              </div>
            ))}
          </div>
        ) : null}
      </div>

      <div className="editor-viewer-split">
        <div className="editor-wrap">
          <InputEditor value={content} onChange={setContent} />
        </div>
        <div className="viewer-column">
          <div className="viewer-panel">
            {scene ? (
              <MoleculeViewer scene={scene} />
            ) : (
              <div className="viewer-empty muted">No coordinates in input</div>
            )}
          </div>
          <FragmentList />
        </div>
      </div>
    </div>
  );
}
