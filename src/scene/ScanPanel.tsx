import { useEffect, useState } from "react";

import type { Scene } from "./types";
import { describeAtom } from "./selection";
import { measureSelectionByIndex, formatMeasurementValue } from "./measure";
import { inspectScanBlock, injectScan, scanOptIssue, type ScanCoordinate } from "./scan";

/**
 * The Scan panel (Phase 4.5 Stage A2) — a **view over the input text**, never a
 * parallel store, exactly like `ConstraintPanel`. Its only source is
 * `inspectScanBlock(content)`; every edit goes back through `injectScan`. There is
 * no React state that *is* the scan — the three number fields keep a transient
 * keystroke draft only (seeded from, and re-synced to, the text), the same shape as
 * the constraint value input. If a parallel scan store crept in it would drift from
 * the text (the failure the constraint panel was built to avoid).
 *
 * Three block states (mirror the constraint discipline): **absent** → a hint at the
 * add path (Scan-from-selection lives in the Atom section); **parsed** → the
 * coordinate + editable start/end/npoints + remove; **unrecognised** → a read-only
 * notice, never rewritten (a multi-coordinate scan or an inline comment). Adding is
 * elsewhere ("Scan this coordinate"); this panel edits, reports, and removes.
 *
 * The `! Opt` guard (`scanOptIssue`) is surfaced inline: a relaxed scan without an
 * optimization keyword is silently a single point (measured, `wiki/orca/scan.md`).
 * The Run path (`NewJobScreen`) blocks on the SAME `scanOptIssue`.
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
  const inspection = inspectScanBlock(content);
  const optIssue = scanOptIssue(content);

  if (inspection.kind === "absent") {
    return (
      <div className="scan-panel">
        <div className="scan-head">Scan</div>
        <div className="muted scan-empty">
          No scan coordinate in the input. Pick 2–4 atoms in{" "}
          <b>Selection &amp; Measure</b>, then <b>Scan this coordinate</b> — a
          relaxed <code>%geom Scan</code> is added and shown here to edit.
        </div>
      </div>
    );
  }

  if (inspection.kind === "unrecognised") {
    return (
      <div className="scan-panel">
        <div className="scan-head">Scan</div>
        <div className="banner warn scan-warn">
          This <code>%geom Scan</code> block contains syntax OrcaStudio doesn&apos;t
          recognise (near <code className="mono">{inspection.sample}</code>) — e.g. a
          multi-coordinate scan. The panel won&apos;t rewrite it; edit it directly in
          the input editor so nothing here is lost.
        </div>
      </div>
    );
  }

  const scan = inspection.scan;
  const unit = KIND_UNIT[scan.kind];
  // Scan atoms are 0-based global indices (positional, from the text) — measured by
  // index, not AtomId (the same seam as a constraint's).
  const current = formatMeasurementValue(measureSelectionByIndex(scene, scan.atoms));

  const commit = (next: ScanCoordinate) => onChange(injectScan(content, next));
  // start/end preserve the exact typed text (the A1 startText/endText round-trip),
  // so an in-progress "1." never jumps. npoints is a plain integer ≥ 2.
  const setStart = (t: string) => commit({ ...scan, start: Number(t), startText: t });
  const setEnd = (t: string) => commit({ ...scan, end: Number(t), endText: t });
  const setN = (n: number) => commit({ ...scan, npoints: n });
  const remove = () => onChange(injectScan(content, null));

  return (
    <div className="scan-panel">
      <div className="scan-head">
        Scan
        <span className="muted" title="Scan atoms are ORCA 0-based global indices — the numbers written into %geom and reported in the ORCA output">
          {" "}· atoms are ORCA 0-based index
        </span>
      </div>

      {optIssue ? (
        <div className="banner warn scan-warn scan-opt-warn">
          {optIssue} The run is blocked until an optimization keyword is on the
          <code> ! </code> line.
        </div>
      ) : null}

      <div className="scan-row">
        <span className="scan-badge" title={KIND_TITLE[scan.kind]}>
          {scan.kind}
        </span>
        <span className="scan-atoms mono">{atomsLabel(scene, scan)}</span>
        {current ? <span className="scan-current muted mono">now {current}</span> : null}
        <button className="btn btn-sm scan-del" onClick={remove} title="Remove this scan">
          ×
        </button>
      </div>

      <div className="scan-range">
        <label className="scan-field">
          <span className="muted">start ({unit})</span>
          <NumberField text={fieldText(scan.start, scan.startText)} onCommit={setStart} />
        </label>
        <label className="scan-field">
          <span className="muted">end ({unit})</span>
          <NumberField text={fieldText(scan.end, scan.endText)} onCommit={setEnd} />
        </label>
        <label className="scan-field">
          <span className="muted">points</span>
          <IntField value={scan.npoints} onCommit={setN} />
        </label>
        <span className="muted scan-step">
          step {scanStep(scan)} {unit}
        </span>
      </div>
    </div>
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
