import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { confirm } from "@tauri-apps/plugin-dialog";

import type { Job, ParsedResults, Pathway, Reaction, ReferenceEnergy } from "../types";
import { formatTimestamp } from "../format";
import { isScanJob, isValidPathwayLabel, normalizePathwayLabel } from "../reactions/pathway";
import { isLocatedTsInput } from "../reactions/compare";
import { CompareView, type ComparePathway, type CompareReference } from "../reactions/CompareView";

interface ReactionsScreenProps {
  /** Open a job in the Jobs detail screen — used to prove a grouped job is still a
   *  fully-openable standalone job (the jobs-survive invariant, made reachable). */
  onOpenJob: (jobId: string) => void;
}

/** A completed/parsed job — the only kind offerable as a pathway — annotated with
 *  whether its results carry a scan profile (for the picker's mark/warn). */
interface Candidate {
  job: Job;
  isScan: boolean;
}

export function ReactionsScreen({ onOpenJob }: ReactionsScreenProps) {
  const [reactions, setReactions] = useState<Reaction[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectedId, setSelectedId] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);

  const loadReactions = useCallback(async () => {
    try {
      setReactions(await invoke<Reaction[]>("list_reactions"));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    loadReactions();
  }, [loadReactions]);

  const selected = reactions.find((r) => r.id === selectedId) ?? null;

  return (
    <div className="screen">
      <div className="row" style={{ justifyContent: "space-between", marginBottom: 14 }}>
        <h2 className="section-title" style={{ margin: 0 }}>
          Reactions
        </h2>
        <button
          className="btn btn-primary"
          onClick={() => {
            setSelectedId(null);
            setAdding((a) => !a);
          }}
        >
          {adding ? "Cancel" : "New reaction"}
        </button>
      </div>

      <p className="muted" style={{ margin: "0 0 12px" }}>
        A reaction groups scan jobs as labelled pathways to compare later (ΔΔE‡, next
        unit). Grouping is metadata only — your scan jobs always stay in the Jobs list;
        detaching or deleting a reaction only removes the grouping, never a job.
      </p>

      {error ? <div className="banner err">{error}</div> : null}

      {adding ? (
        <NewReactionForm
          onSaved={async (r) => {
            setAdding(false);
            await loadReactions();
            setSelectedId(r.id);
          }}
          onError={setError}
        />
      ) : null}

      {loading ? (
        <div className="empty">Loading…</div>
      ) : reactions.length === 0 && !adding ? (
        <div className="empty">
          No reactions yet — create one, then attach your scan jobs as pathways.
        </div>
      ) : reactions.length > 0 ? (
        <table className="jobs-table">
          <thead>
            <tr>
              <th>Name</th>
              <th>Description</th>
              <th>Created</th>
            </tr>
          </thead>
          <tbody>
            {reactions.map((r) => (
              <tr
                key={r.id}
                className={"clickable" + (r.id === selectedId ? " active" : "")}
                onClick={() => setSelectedId((s) => (s === r.id ? null : r.id))}
              >
                <td>{r.name}</td>
                <td className="muted">{r.description || "—"}</td>
                <td className="mono">{formatTimestamp(r.created_at)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      ) : null}

      {selected ? (
        <ReactionDetail
          reaction={selected}
          onOpenJob={onOpenJob}
          onError={setError}
          onChanged={loadReactions}
          onDeleted={async () => {
            setSelectedId(null);
            await loadReactions();
          }}
        />
      ) : null}
    </div>
  );
}

interface NewReactionFormProps {
  onSaved: (r: Reaction) => void;
  onError: (msg: string) => void;
}

function NewReactionForm({ onSaved, onError }: NewReactionFormProps) {
  const [name, setName] = useState("");
  const [description, setDescription] = useState("");
  const [saving, setSaving] = useState(false);

  const save = async () => {
    const n = name.trim();
    if (!n) {
      onError("A reaction needs a name");
      return;
    }
    setSaving(true);
    try {
      const r = await invoke<Reaction>("create_reaction", {
        name: n,
        description: description.trim() || null,
      });
      setName("");
      setDescription("");
      onSaved(r);
    } catch (e) {
      onError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="card" style={{ marginBottom: 14 }}>
      <div className="row" style={{ gap: 16 }}>
        <div className="field" style={{ flex: 1 }}>
          <label className="label" htmlFor="rxn-name">
            Name
          </label>
          <input
            id="rxn-name"
            className="input"
            placeholder="e.g. Ketone + BH₄ (si vs re)"
            value={name}
            onChange={(e) => setName(e.currentTarget.value)}
            spellCheck={false}
          />
        </div>
        <div className="field" style={{ flex: 2 }}>
          <label className="label" htmlFor="rxn-desc">
            Description (optional)
          </label>
          <input
            id="rxn-desc"
            className="input"
            placeholder="what you're testing"
            value={description}
            onChange={(e) => setDescription(e.currentTarget.value)}
          />
        </div>
      </div>
      <div className="row" style={{ justifyContent: "flex-end", marginTop: 12 }}>
        <button
          className="btn btn-primary"
          onClick={save}
          disabled={saving || !name.trim()}
        >
          {saving ? "Saving…" : "Create reaction"}
        </button>
      </div>
    </div>
  );
}

interface ReactionDetailProps {
  reaction: Reaction;
  onOpenJob: (jobId: string) => void;
  onError: (msg: string) => void;
  onChanged: () => Promise<void>;
  onDeleted: () => Promise<void>;
}

function ReactionDetail({ reaction, onOpenJob, onError, onChanged, onDeleted }: ReactionDetailProps) {
  const [pathways, setPathways] = useState<Pathway[]>([]);
  const [jobs, setJobs] = useState<Job[]>([]);
  const [resultsById, setResultsById] = useState<Map<string, ParsedResults>>(new Map());
  const [refEnergy, setRefEnergy] = useState<ReferenceEnergy | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [ps, js, re] = await Promise.all([
        invoke<Pathway[]>("list_pathways", { reactionId: reaction.id }),
        invoke<Job[]>("list_jobs"),
        invoke<ReferenceEnergy>("reaction_reference_energy", { reactionId: reaction.id }),
      ]);
      setPathways(ps);
      setJobs(js);
      setRefEnergy(re);

      // Read the parsed results of finished jobs once — reused for both the picker's
      // scan mark/warn and the compare view's profiles (only these jobs can be offered).
      const finished = js.filter((j) => j.status === "completed" || j.status === "parsed");
      const entries = await Promise.all(
        finished.map(async (j) => {
          try {
            return [j.id, await invoke<ParsedResults | null>("read_job_results", { id: j.id })] as const;
          } catch {
            return [j.id, null] as const;
          }
        }),
      );
      setResultsById(new Map(entries.filter((e): e is [string, ParsedResults] => e[1] != null)));
    } catch (e) {
      onError(String(e));
    }
  }, [reaction.id, onError]);

  // Jobs whose parsed results carry a scan profile (drives the picker's mark/warn).
  const scanIds = useMemo(
    () => new Set([...resultsById].filter(([, r]) => isScanJob(r)).map(([id]) => id)),
    [resultsById],
  );

  useEffect(() => {
    load();
  }, [load]);

  // Job lookup by id, so a pathway can name its attached job.
  const jobById = useMemo(() => new Map(jobs.map((j) => [j.id, j])), [jobs]);

  // The job attached to a pathway, if any: the job whose pathway_id points back at it
  // (one source of truth — the job carries the FK, not the pathway).
  const jobForPathway = useCallback(
    (pathwayId: string) => jobs.find((j) => j.pathway_id === pathwayId) ?? null,
    [jobs],
  );

  // Pathways whose attached job carries a plottable scan profile (≥1 point) — the
  // inputs to the comparative overlay. A pathway with no job / no scan is skipped.
  const comparePathways: ComparePathway[] = useMemo(() => {
    return pathways
      .map((p): ComparePathway | null => {
        // A pathway can now hold TWO jobs — the scan AND its OptTS refinement (E1a attaches
        // the located TS to the same pathway). Pick the SCAN job by its scan profile (not
        // just the first attached job), and the LOCATED TS by its `! OptTS` input, both parsed.
        const attached = jobs.filter((j) => j.pathway_id === p.id);
        const scanJob = attached.find((j) => {
          const r = resultsById.get(j.id);
          return r?.scan && r.scan.points.length > 0;
        });
        if (!scanJob) return null;
        const scan = resultsById.get(scanJob.id)!.scan!;
        // The located TS: a parsed OptTS job on this pathway. `eEh`/`gEh` from its parsed
        // results (G null unless it ran Freq — honest-or-absent, Stage E1b).
        const tsJob = attached.find(
          (j) => isLocatedTsInput(j.input_content) && resultsById.has(j.id),
        );
        const tsResults = tsJob ? resultsById.get(tsJob.id) : null;
        const locatedTs = tsJob
          ? {
              input: tsJob.input_content,
              eEh: tsResults?.final_energy_eh ?? null,
              gEh: tsResults?.thermochemistry?.free_energy_g_eh ?? null,
            }
          : undefined;
        return { id: p.id, label: p.label, scan, input: scanJob.input_content, locatedTs };
      })
      .filter((x): x is ComparePathway => x !== null);
  }, [pathways, jobs, resultsById]);

  // Attach candidates: completed/parsed jobs NOT already grouped anywhere.
  const candidates: Candidate[] = useMemo(
    () =>
      jobs
        .filter(
          (j) => (j.status === "completed" || j.status === "parsed") && j.pathway_id === null,
        )
        .map((j) => ({ job: j, isScan: scanIds.has(j.id) })),
    [jobs, scanIds],
  );

  // The reactant reference for absolute barriers (C2b-2b, ADR-018): the reference jobs'
  // inputs (for the method guard) + the honest summed E(ref) (null when incomplete) + the
  // count (0 = no reference → the C2b-1 state). Energies are read on demand from the
  // C2b-2a command — never cached here.
  const reference: CompareReference = useMemo(() => {
    const rjs = refEnergy?.jobs ?? [];
    const inputs = rjs
      .map((rj) => jobById.get(rj.job_id)?.input_content)
      .filter((x): x is string => x != null);
    return {
      inputs,
      energyEh: refEnergy?.energy_eh ?? null,
      jobCount: rjs.length,
      gibbsEh: refEnergy?.free_energy_g_eh ?? null,
    };
  }, [refEnergy, jobById]);

  // Reference candidates: completed/parsed jobs not already in the reference. A reference
  // is usually an optimized substrate/reagent (marked scan vs optimized in the picker),
  // independent of the pathways — so, unlike attach, we do NOT filter on pathway_id.
  const referencedIds = useMemo(
    () => new Set((refEnergy?.jobs ?? []).map((j) => j.job_id)),
    [refEnergy],
  );
  const referenceCandidates: Candidate[] = useMemo(
    () =>
      jobs
        .filter(
          (j) => (j.status === "completed" || j.status === "parsed") && !referencedIds.has(j.id),
        )
        .map((j) => ({ job: j, isScan: scanIds.has(j.id) })),
    [jobs, referencedIds, scanIds],
  );

  const addReferenceJob = async (jobId: string) => {
    setBusy(true);
    try {
      await invoke("add_reference_job", { reactionId: reaction.id, jobId });
      await load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const removeReferenceJob = async (jobId: string) => {
    setBusy(true);
    try {
      await invoke("remove_reference_job", { reactionId: reaction.id, jobId });
      await load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const [renaming, setRenaming] = useState(false);
  const [renameValue, setRenameValue] = useState(reaction.name);

  const detach = async (jobId: string) => {
    const ok = await confirm(
      "The scan job stays in your Jobs list with its results intact — only the grouping " +
        "is removed.",
      { title: "Detach job from pathway", kind: "warning" },
    );
    if (!ok) return;
    setBusy(true);
    try {
      await invoke("detach_job_from_pathway", { jobId });
      await load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const removePathway = async (pathwayId: string) => {
    const ok = await confirm(
      "If a scan job is attached it stays in your Jobs list (only the grouping is " +
        "removed) — no job is deleted.",
      { title: "Delete pathway", kind: "warning" },
    );
    if (!ok) return;
    setBusy(true);
    try {
      await invoke("delete_pathway", { id: pathwayId });
      await load();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const saveRename = async () => {
    const n = renameValue.trim();
    if (!n) {
      onError("A reaction needs a name");
      return;
    }
    setBusy(true);
    try {
      await invoke("rename_reaction", { id: reaction.id, name: n });
      setRenaming(false);
      await onChanged();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const removeReaction = async () => {
    const ok = await confirm(
      `All scan jobs under "${reaction.name}" stay in your Jobs list (only the grouping ` +
        "is removed) — no job is deleted.",
      { title: "Delete reaction", kind: "warning" },
    );
    if (!ok) return;
    setBusy(true);
    try {
      await invoke("delete_reaction", { id: reaction.id });
      await onDeleted();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card" style={{ marginTop: 16 }}>
      <div className="row" style={{ justifyContent: "space-between", alignItems: "baseline" }}>
        {renaming ? (
          <div className="row" style={{ gap: 8, flex: 1 }}>
            <input
              className="input"
              style={{ flex: 1, maxWidth: 360 }}
              value={renameValue}
              autoFocus
              onChange={(e) => setRenameValue(e.currentTarget.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") saveRename();
                if (e.key === "Escape") setRenaming(false);
              }}
              spellCheck={false}
            />
            <button
              className="btn btn-primary btn-sm"
              onClick={saveRename}
              disabled={busy || !renameValue.trim()}
            >
              Save
            </button>
            <button className="btn btn-sm" onClick={() => setRenaming(false)} disabled={busy}>
              Cancel
            </button>
          </div>
        ) : (
          <>
            <div>
              <h3 style={{ margin: "0 0 2px" }}>{reaction.name}</h3>
              {reaction.description ? (
                <span className="muted">{reaction.description}</span>
              ) : null}
            </div>
            <div className="row" style={{ gap: 8 }}>
              <button
                className="btn btn-sm"
                onClick={() => {
                  setRenameValue(reaction.name);
                  setRenaming(true);
                }}
                disabled={busy}
              >
                Rename
              </button>
              <button className="btn btn-sm" onClick={removeReaction} disabled={busy}>
                Delete reaction
              </button>
            </div>
          </>
        )}
      </div>

      <h4 style={{ margin: "16px 0 8px" }}>Pathways</h4>
      {pathways.length === 0 ? (
        <div className="empty">No pathways yet — attach a scan job below.</div>
      ) : (
        <table className="jobs-table">
          <thead>
            <tr>
              <th>Label</th>
              <th>Job</th>
              <th>Status</th>
              <th></th>
            </tr>
          </thead>
          <tbody>
            {pathways.map((p) => {
              const job = jobForPathway(p.id);
              return (
                <tr key={p.id}>
                  <td>{p.label}</td>
                  <td>
                    {job ? (
                      <button
                        className="linkish"
                        onClick={() => onOpenJob(job.id)}
                        title="Open this job — it's a standalone job, grouping aside"
                      >
                        {job.title}
                      </button>
                    ) : (
                      <span className="muted">(no job attached)</span>
                    )}
                  </td>
                  <td>
                    {job ? <span className={`badge ${job.status}`}>{job.status}</span> : "—"}
                  </td>
                  <td style={{ textAlign: "right", whiteSpace: "nowrap" }}>
                    {job ? (
                      <button
                        className="btn btn-sm"
                        onClick={() => detach(job.id)}
                        disabled={busy}
                      >
                        Detach
                      </button>
                    ) : null}{" "}
                    <button
                      className="btn btn-sm"
                      onClick={() => removePathway(p.id)}
                      disabled={busy}
                    >
                      Delete
                    </button>
                  </td>
                </tr>
              );
            })}
          </tbody>
        </table>
      )}

      <AttachPathwayForm
        reactionId={reaction.id}
        candidates={candidates}
        jobById={jobById}
        onError={onError}
        onAttached={load}
        busy={busy}
        setBusy={setBusy}
      />

      <ReferenceJobsSection
        refEnergy={refEnergy}
        candidates={referenceCandidates}
        onAdd={addReferenceJob}
        onRemove={removeReferenceJob}
        busy={busy}
      />

      <h4 style={{ margin: "20px 0 8px" }}>Compare — barriers &amp; ΔΔE‡</h4>
      {comparePathways.length >= 1 ? (
        // Per-pathway intrinsic + absolute barriers need ONE pathway; only ΔΔE‡ (a
        // difference of two maxima) needs two. So render the overlay at ≥ 1 — a single
        // pathway (e.g. an SN2 with no si/re face) still shows its barriers.
        <CompareView pathways={comparePathways} reference={reference} />
      ) : (
        <div className="empty">
          Attach a scan pathway to see its profile and barriers (intrinsic + absolute).
          Attach a second to also compute ΔΔE‡.
        </div>
      )}
    </div>
  );
}

interface AttachPathwayFormProps {
  reactionId: string;
  candidates: Candidate[];
  jobById: Map<string, Job>;
  onError: (msg: string) => void;
  onAttached: () => Promise<void>;
  busy: boolean;
  setBusy: (b: boolean) => void;
}

function AttachPathwayForm({
  reactionId,
  candidates,
  jobById,
  onError,
  onAttached,
  busy,
  setBusy,
}: AttachPathwayFormProps) {
  const [label, setLabel] = useState("");
  const [jobId, setJobId] = useState("");

  const selectedIsScan = candidates.find((c) => c.job.id === jobId)?.isScan ?? true;
  const canAttach = isValidPathwayLabel(label) && jobId !== "" && jobById.has(jobId);

  const attach = async () => {
    if (!canAttach) return;
    setBusy(true);
    try {
      // C1's create_pathway then attach: the pathway is a grouping row; the job carries
      // the FK back to it. Permissive about job kind — a non-scan job is warned above,
      // not blocked (the comparability guard is C2b).
      const pathway = await invoke<Pathway>("create_pathway", {
        reactionId,
        label: normalizePathwayLabel(label),
      });
      await invoke("attach_job_to_pathway", { jobId, pathwayId: pathway.id });
      setLabel("");
      setJobId("");
      await onAttached();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div style={{ marginTop: 16 }}>
      <h4 style={{ margin: "0 0 8px" }}>Attach a scan job as a pathway</h4>
      {candidates.length === 0 ? (
        <div className="empty">
          No unattached completed jobs to add. Run a relaxed scan, or detach a job from
          another pathway first.
        </div>
      ) : (
        <>
          <div className="row" style={{ gap: 16, alignItems: "flex-end", flexWrap: "wrap" }}>
            <div className="field" style={{ width: 200 }}>
              <label className="label" htmlFor="pw-label">
                Pathway label
              </label>
              <input
                id="pw-label"
                className="input"
                placeholder="e.g. si face"
                value={label}
                onChange={(e) => setLabel(e.currentTarget.value)}
                spellCheck={false}
              />
            </div>
            <div className="field" style={{ flex: 1, minWidth: 0 }}>
              <label className="label" htmlFor="pw-job">
                Job
              </label>
              <select
                id="pw-job"
                className="input"
                value={jobId}
                onChange={(e) => setJobId(e.currentTarget.value)}
              >
                <option value="">Choose a completed job…</option>
                {candidates.map((c) => (
                  <option key={c.job.id} value={c.job.id}>
                    {c.job.title}
                    {c.isScan ? "  ✓ scan" : "  (not a scan)"}
                  </option>
                ))}
              </select>
            </div>
            <button
              className="btn btn-primary"
              onClick={attach}
              disabled={busy || !canAttach}
            >
              Attach
            </button>
          </div>
          {jobId !== "" && !selectedIsScan ? (
            <div className="banner warn" style={{ marginTop: 10 }}>
              This job has no scan profile. C2b compares scan profiles (ΔΔE‡) — attaching
              it is allowed, but it won't appear in that comparison.
            </div>
          ) : null}
        </>
      )}
    </div>
  );
}

interface ReferenceJobsSectionProps {
  /** The summed reactant reference from `reaction_reference_energy` (C2b-2a): the jobs
   *  (each with `final_energy_eh`, null when unparsed) + the honest total (null when any
   *  job is unparsed). null before the first load. */
  refEnergy: ReferenceEnergy | null;
  /** Completed/parsed jobs not already in the reference — the add picker's options. */
  candidates: Candidate[];
  onAdd: (jobId: string) => Promise<void>;
  onRemove: (jobId: string) => Promise<void>;
  busy: boolean;
}

/**
 * The reactant-reference management surface (Phase 4.5 C2b-2b, ADR-018): add/remove the
 * optimized-reactant jobs whose energies SUM to E(ref) for absolute barriers. Shows each
 * job's parsed energy and the summed E(ref) — or, **honest-or-absent**, "incomplete — job
 * X has no parsed energy" when the C2b-2a command returns `energy_eh: null`, never a number
 * from a partial sum. Semantics are the user's (the app sums + labels): one job = a
 * pre-reaction complex; two+ = separated reactants.
 */
function ReferenceJobsSection({
  refEnergy,
  candidates,
  onAdd,
  onRemove,
  busy,
}: ReferenceJobsSectionProps) {
  const [jobId, setJobId] = useState("");

  const jobs = refEnergy?.jobs ?? [];
  const total = refEnergy?.energy_eh ?? null;
  const missing = jobs.filter((j) => j.final_energy_eh === null);

  const add = async () => {
    if (jobId === "") return;
    await onAdd(jobId);
    setJobId("");
  };

  return (
    <div style={{ marginTop: 20 }}>
      <h4 style={{ margin: "0 0 8px" }}>Reactant reference (absolute barriers)</h4>
      <p className="muted" style={{ margin: "0 0 10px" }}>
        Optimized-reactant jobs whose energies sum to E(ref) — the reference for the{" "}
        <strong>absolute barrier</strong> vs separated reactants. One job = a pre-reaction complex;
        two+ = separated reactants (e.g. substrate + BH₄⁻, each optimized separately). Optional; ΔΔE‡
        and intrinsic barriers do not need it.
      </p>

      {jobs.length === 0 ? (
        <div className="empty">
          No reference jobs yet — add an optimized substrate/reagent below to enable absolute barriers.
        </div>
      ) : (
        <>
          <table className="jobs-table">
            <thead>
              <tr>
                <th>Reference job</th>
                <th style={{ textAlign: "right" }}>Final energy</th>
                <th></th>
              </tr>
            </thead>
            <tbody>
              {jobs.map((j) => (
                <tr key={j.job_id}>
                  <td>{j.title}</td>
                  <td className="mono" style={{ textAlign: "right" }}>
                    {j.final_energy_eh !== null ? (
                      `${j.final_energy_eh.toFixed(6)} Eh`
                    ) : (
                      <span className="muted">no parsed energy</span>
                    )}
                  </td>
                  <td style={{ textAlign: "right" }}>
                    <button
                      className="btn btn-sm"
                      onClick={() => onRemove(j.job_id)}
                      disabled={busy}
                      title="Remove from the reference — the job stays in your Jobs list"
                    >
                      Remove
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>

          {total !== null ? (
            <div className="mono" style={{ marginTop: 8 }}>
              E(ref) = Σ ={" "}
              <strong>{total.toFixed(6)} Eh</strong>
              {jobs.length === 1 ? (
                <span className="muted"> (single job — a pre-reaction complex)</span>
              ) : (
                <span className="muted"> ({jobs.length} separated reactants)</span>
              )}
            </div>
          ) : (
            <div className="banner warn" style={{ marginTop: 8 }}>
              Reference <strong>incomplete</strong> — no E(ref) until every reference job has a parsed
              energy. Missing:{" "}
              {missing.map((m) => m.title).join(", ")}. (Run/parse the job, or remove it.)
            </div>
          )}
        </>
      )}

      <div className="row" style={{ gap: 12, alignItems: "flex-end", marginTop: 12, flexWrap: "wrap" }}>
        <div className="field" style={{ flex: 1, minWidth: 0 }}>
          <label className="label" htmlFor="ref-job">
            Add a reference job
          </label>
          <select
            id="ref-job"
            className="input"
            value={jobId}
            onChange={(e) => setJobId(e.currentTarget.value)}
          >
            <option value="">Choose an optimized reactant job…</option>
            {candidates.map((c) => (
              <option key={c.job.id} value={c.job.id}>
                {c.job.title}
                {c.isScan ? "  (scan — usually not a reference)" : "  ✓ optimized"}
              </option>
            ))}
          </select>
        </div>
        <button className="btn btn-primary" onClick={add} disabled={busy || jobId === ""}>
          Add reference
        </button>
      </div>
    </div>
  );
}
