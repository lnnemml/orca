import { useEffect, useState } from "react";

import type { Scene } from "./types";
import { describeAtom } from "./selection";
import { measureSelectionByIndex, formatMeasurementValue } from "./measure";
import {
  inspectScanBlock,
  injectScan,
  parseScanCoordinates,
  scanOptIssue,
  type ScanCoordinate,
} from "./scan";

/**
 * The Scan panel (Phase 4.5 Stage A2; 2D grid = Stage 4a) — a **view over the input
 * text**, never a parallel store, exactly like `ConstraintPanel`. Its source is
 * `parseScanCoordinates(content)` (the N-aware read); every edit goes back through
 * `injectScan`. There is no React state that *is* the scan — the number fields keep a
 * transient keystroke draft only (seeded from, and re-synced to, the text). If a
 * parallel scan store crept in it would drift from the text (the failure the constraint
 * panel was built to avoid).
 *
 * States: **absent** → a hint at the add path (Scan-from-selection lives in the Atom
 * section); **parsed** → coordinate 1 (editable start/end/npoints + remove) plus an
 * optional **second coordinate** that makes it a native N₁×N₂ relaxed surface grid (for
 * a concerted reaction — Diels-Alder — mapped as a 2D PES); **unrecognised** (a `#`
 * comment or a line we can't parse) → a read-only notice, never rewritten.
 *
 * The first coordinate is still added elsewhere ("Scan this coordinate", 2–4 atoms). The
 * second is an **atom-PAIR** (a bond, B) entered here — its two 0-based indices + range —
 * so a 2D grid needs no viewer round-trip. The point count (N₁×N₂) is shown so the user
 * sees the cost before running.
 *
 * The `! Opt` guard (`scanOptIssue`) is surfaced inline: a relaxed scan without an
 * optimization keyword is silently a single point (measured, `wiki/orca/scan.md`).
 */
export function ScanPanel({
  scene,
  content,
  onChange,
}: {
  scene: Scene;
  content: string;
  onChange: (newContent: string) => void;
}) {
  const coords = parseScanCoordinates(content);
  const optIssue = scanOptIssue(content);

  if (!coords) {
    // parseScanCoordinates is null for BOTH absent and unrecognised — inspectScanBlock
    // distinguishes them for the right message (absent = add hint; else = don't-rewrite).
    const inspection = inspectScanBlock(content);
    if (inspection.kind === "unrecognised") {
      return (
        <div className="scan-panel">
          <div className="scan-head">Scan</div>
          <div className="banner warn scan-warn">
            This <code>%geom Scan</code> block contains syntax OrcaStudio doesn&apos;t
            recognise (near <code className="mono">{inspection.sample}</code>). The panel
            won&apos;t rewrite it; edit it directly in the input editor so nothing is lost.
          </div>
        </div>
      );
    }
    return (
      <div className="scan-panel">
        <div className="scan-head">Scan</div>
        <div className="muted scan-empty">
          No scan coordinate in the input. Pick 2–4 atoms in{" "}
          <b>Selection &amp; Measure</b>, then <b>Scan this coordinate</b> — a relaxed{" "}
          <code>%geom Scan</code> is added and shown here to edit.
        </div>
      </div>
    );
  }

  const c1 = coords[0];
  const c2 = coords[1]; // may be undefined (1D)
  const pointCount = coords.reduce((n, c) => n * c.npoints, 1);

  // Replace one coordinate in place, keeping the others (and their order) → re-emit all.
  const patchCoord = (i: number, patch: Partial<ScanCoordinate>) =>
    onChange(injectScan(content, coords.map((c, k) => (k === i ? ({ ...c, ...patch } as ScanCoordinate) : c))));
  const removeAll = () => onChange(injectScan(content, null));
  // Add a second coordinate (a bond) — a valid default, edited in place afterwards. Its
  // range seeds from coordinate 1 so the point count is immediately sensible.
  const addSecond = () =>
    onChange(
      injectScan(content, [
        c1,
        { kind: "B", atoms: [0, 1], start: c1.start, end: c1.end, npoints: c1.npoints },
      ]),
    );
  const removeSecond = () => onChange(injectScan(content, [c1])); // back to 1D

  return (
    <div className="scan-panel">
      <div className="scan-head">
        Scan
        <span
          className="muted"
          title="Scan atoms are ORCA 0-based global indices — the numbers written into %geom and reported in the ORCA output"
        >
          {" "}· atoms are ORCA 0-based index
        </span>
        <span className="muted scan-count" title="Total single-point optimizations this scan runs">
          {" "}· {pointCount} point{pointCount === 1 ? "" : "s"}
          {c2 ? ` (${c1.npoints} × ${c2.npoints})` : ""}
        </span>
      </div>

      {optIssue ? (
        <div className="banner warn scan-warn scan-opt-warn">
          {optIssue} The run is blocked until an optimization keyword is on the
          <code> ! </code> line.
        </div>
      ) : null}

      <CoordEditor
        scene={scene}
        coord={c1}
        onStart={(t) => patchCoord(0, { start: Number(t), startText: t })}
        onEnd={(t) => patchCoord(0, { end: Number(t), endText: t })}
        onN={(n) => patchCoord(0, { npoints: n })}
        onRemove={removeAll}
        removeTitle="Remove this scan"
      />

      {c2 ? (
        <>
          <div className="muted scan-second-head">second coordinate (2D grid)</div>
          <CoordEditor
            scene={scene}
            coord={c2}
            // The second coordinate is an atom PAIR (bond) — its two 0-based indices are editable here.
            onAtoms={c2.kind === "B" ? (a) => patchCoord(1, { atoms: a }) : undefined}
            onStart={(t) => patchCoord(1, { start: Number(t), startText: t })}
            onEnd={(t) => patchCoord(1, { end: Number(t), endText: t })}
            onN={(n) => patchCoord(1, { npoints: n })}
            onRemove={removeSecond}
            removeTitle="Remove the second coordinate (back to a 1D scan)"
          />
        </>
      ) : (
        <button
          className="btn btn-sm scan-add-second"
          onClick={addSecond}
          title="Add a second bond coordinate — a native N₁×N₂ relaxed surface grid (e.g. a concerted Diels-Alder)"
        >
          + Add 2nd coordinate (2D grid)
        </button>
      )}
    </div>
  );
}

/** One coordinate row + its start/end/npoints editors. `onAtoms` (B only) reveals two
 * 0-based atom-index inputs for a second-coordinate bond; absent → the atoms are shown
 * read-only (coordinate 1, whose atoms come from the selection add-path). */
function CoordEditor({
  scene,
  coord,
  onAtoms,
  onStart,
  onEnd,
  onN,
  onRemove,
  removeTitle,
}: {
  scene: Scene;
  coord: ScanCoordinate;
  onAtoms?: (atoms: [number, number]) => void;
  onStart: (t: string) => void;
  onEnd: (t: string) => void;
  onN: (n: number) => void;
  onRemove: () => void;
  removeTitle: string;
}) {
  const unit = KIND_UNIT[coord.kind];
  const current = formatMeasurementValue(measureSelectionByIndex(scene, coord.atoms));

  return (
    <>
      <div className="scan-row">
        <span className="scan-badge" title={KIND_TITLE[coord.kind]}>
          {coord.kind}
        </span>
        {onAtoms ? (
          <span className="scan-atoms-edit">
            <AtomIndexField value={coord.atoms[0]} onCommit={(a) => onAtoms([a, coord.atoms[1]])} />
            <span className="muted"> ··· </span>
            <AtomIndexField value={coord.atoms[1]} onCommit={(b) => onAtoms([coord.atoms[0], b])} />
          </span>
        ) : (
          <span className="scan-atoms mono">{atomsLabel(scene, coord)}</span>
        )}
        {current ? <span className="scan-current muted mono">now {current}</span> : null}
        <button className="btn btn-sm scan-del" onClick={onRemove} title={removeTitle}>
          ×
        </button>
      </div>

      <div className="scan-range">
        <label className="scan-field">
          <span className="muted">start ({unit})</span>
          <NumberField text={fieldText(coord.start, coord.startText)} onCommit={onStart} />
        </label>
        <label className="scan-field">
          <span className="muted">end ({unit})</span>
          <NumberField text={fieldText(coord.end, coord.endText)} onCommit={onEnd} />
        </label>
        <label className="scan-field">
          <span className="muted">points</span>
          <IntField value={coord.npoints} onCommit={onN} />
        </label>
        <span className="muted scan-step">
          step {scanStep(coord)} {unit}
        </span>
      </div>
    </>
  );
}

const KIND_TITLE: Record<ScanCoordinate["kind"], string> = {
  B: "bond / distance scan",
  A: "angle scan",
  D: "dihedral scan",
};
const KIND_UNIT: Record<ScanCoordinate["kind"], string> = { B: "Å", A: "°", D: "°" };

/** The field's committed text: the exact user text if preserved, else the number. */
function fieldText(value: number, text: string | undefined): string {
  return text ?? String(value);
}

/** `(end − start)/(N − 1)`, the value between adjacent scan points, for the readout. */
function scanStep(s: ScanCoordinate): string {
  const step = (s.end - s.start) / (s.npoints - 1);
  return Number.isFinite(step) ? step.toFixed(3) : "—";
}

/** Atoms in OrcaStudio's own terms: "C#0 ··· C#1". An out-of-range index (a scan
 * written before a fragment was removed) reads "#7 (out of range)". */
function atomsLabel(scene: Scene, s: ScanCoordinate): string {
  const sep = s.kind === "B" ? " ··· " : " – ";
  return s.atoms
    .map((gi) => {
      const d = describeAtom(scene, gi);
      return d ? `${d.element}#${gi}` : `#${gi} (out of range)`;
    })
    .join(sep);
}

/**
 * A number text field over the input text. `text` is the committed value from the
 * parse; the local `draft` is a transient keystroke buffer (NOT the scan) so an
 * empty / partial entry is tolerated without corrupting the text. It re-seeds when
 * the committed text changes (an external edit / a different coordinate), and
 * commits only a finite, non-empty value — otherwise the text keeps its last good
 * value.
 */
function NumberField({ text, onCommit }: { text: string; onCommit: (t: string) => void }) {
  const [draft, setDraft] = useState(text);
  useEffect(() => setDraft(text), [text]);
  const valid = draft.trim() !== "" && Number.isFinite(Number(draft));
  return (
    <input
      className={"input mono scan-num" + (valid ? "" : " input-invalid")}
      type="text"
      inputMode="decimal"
      value={draft}
      onChange={(e) => {
        const v = e.currentTarget.value;
        setDraft(v);
        if (v.trim() !== "" && Number.isFinite(Number(v))) onCommit(v);
      }}
      spellCheck={false}
    />
  );
}

/** A 0-based atom-index field (integer ≥ 0) for a second-coordinate bond. Same transient-
 * draft discipline as `NumberField`; commits only a valid non-negative integer. An index
 * past the atom count is not blocked here — it surfaces as "(out of range)" in the label and
 * is caught by the run-time index guard, exactly as for the first coordinate. */
function AtomIndexField({ value, onCommit }: { value: number; onCommit: (n: number) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const n = Number(draft);
  const valid = draft.trim() !== "" && Number.isInteger(n) && n >= 0;
  return (
    <input
      className={"input mono scan-num scan-num-int scan-atom-idx" + (valid ? "" : " input-invalid")}
      type="number"
      min={0}
      step={1}
      value={draft}
      aria-label="Atom index (0-based)"
      onChange={(e) => {
        const v = e.currentTarget.value;
        setDraft(v);
        const m = Number(v);
        if (v.trim() !== "" && Number.isInteger(m) && m >= 0) onCommit(m);
      }}
    />
  );
}

/** Integer field for npoints (≥ 2). Same transient-draft discipline as `NumberField`. */
function IntField({ value, onCommit }: { value: number; onCommit: (n: number) => void }) {
  const [draft, setDraft] = useState(String(value));
  useEffect(() => setDraft(String(value)), [value]);
  const n = Number(draft);
  const valid = draft.trim() !== "" && Number.isInteger(n) && n >= 2;
  return (
    <input
      className={"input mono scan-num scan-num-int" + (valid ? "" : " input-invalid")}
      type="number"
      min={2}
      step={1}
      value={draft}
      onChange={(e) => {
        const v = e.currentTarget.value;
        setDraft(v);
        const m = Number(v);
        if (v.trim() !== "" && Number.isInteger(m) && m >= 2) onCommit(m);
      }}
    />
  );
}
