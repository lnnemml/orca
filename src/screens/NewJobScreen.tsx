import { useState } from "react";
import { invoke } from "@tauri-apps/api/core";

import { InputEditor } from "../editor/InputEditor";
import {
  CATEGORY_LABELS,
  ORCA_TEMPLATES,
  type OrcaTemplate,
} from "../templates/orca-templates";
import type { Job } from "../types";

interface NewJobScreenProps {
  /** A draft job was created; parent navigates to the Jobs list. */
  onCreatedDraft: () => void;
  /** Open a job's detail screen, optionally auto-running it. */
  onOpenDetail: (jobId: string, autoRun: boolean) => void;
}

export function NewJobScreen({ onCreatedDraft, onOpenDetail }: NewJobScreenProps) {
  const [title, setTitle] = useState("");
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [content, setContent] = useState("");
  const [creating, setCreating] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const pickTemplate = (t: OrcaTemplate) => {
    setSelectedId(t.id);
    setContent(t.inputContent);
    if (!title.trim()) setTitle(t.name);
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

      <div className="editor-wrap">
        <InputEditor value={content} onChange={setContent} />
      </div>
    </div>
  );
}
