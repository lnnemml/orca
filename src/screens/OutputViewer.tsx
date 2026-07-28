import { forwardRef, useImperativeHandle, useRef } from "react";
import Editor, { type OnMount, type Monaco } from "@monaco-editor/react";
import type { editor as MonacoEditor } from "monaco-editor";

// Side-effect: point Monaco at the bundled package + worker (offline). Shared
// with the input editor — see editor/monaco-setup.ts. Import once here too.
import "../editor/monaco-setup";

export interface Hit {
  line_no: number;
  col_start: number;
  col_end: number;
}

export interface OutputViewerHandle {
  /** Reveal a file line (absolute numbering) and mark it as the current hit.
   *  Returns false when the line is above the loaded (tail) window. */
  revealFileLine(fileLineNo: number, colStart?: number, colEnd?: number): boolean;
  /** Decorate every hit line; pass [] to clear. */
  setHits(hits: Hit[]): void;
}

interface OutputViewerProps {
  content: string;
  firstLineNo: number;
  truncated: boolean;
  totalLines: number;
  /** F3 / Shift+F3 while the viewer is focused. */
  onFindNext?: () => void;
  onFindPrev?: () => void;
}

const FONT =
  "ui-monospace, 'SF Mono', Menlo, Consolas, monospace";

export const OutputViewer = forwardRef<OutputViewerHandle, OutputViewerProps>(
  function OutputViewer(
    { content, firstLineNo, truncated, totalLines, onFindNext, onFindPrev },
    ref,
  ) {
    const editorRef = useRef<MonacoEditor.IStandaloneCodeEditor | null>(null);
    const monacoRef = useRef<Monaco | null>(null);
    const allHitsRef = useRef<MonacoEditor.IEditorDecorationsCollection | null>(null);
    const currentRef = useRef<MonacoEditor.IEditorDecorationsCollection | null>(null);

    // Kept in refs so the stable imperative handle + F3 commands always read the
    // latest values (Reload changes firstLineNo; nav callbacks change on hits).
    const firstLineNoRef = useRef(firstLineNo);
    firstLineNoRef.current = firstLineNo;
    const findNextRef = useRef(onFindNext);
    findNextRef.current = onFindNext;
    const findPrevRef = useRef(onFindPrev);
    findPrevRef.current = onFindPrev;

    // Calls made before Monaco mounts are buffered and flushed in onMount.
    const pendingHits = useRef<Hit[] | null>(null);
    const pendingReveal = useRef<{ line: number; cs?: number; ce?: number } | null>(
      null,
    );

    const applyHits = (hits: Hit[]) => {
      const ed = editorRef.current;
      const monaco = monacoRef.current;
      if (!ed || !monaco || !allHitsRef.current) {
        pendingHits.current = hits;
        return;
      }
      const first = firstLineNoRef.current;
      const decos = hits
        .map((h) => {
          const ml = h.line_no - first + 1;
          if (ml < 1) return null; // above the loaded window
          return {
            range: new monaco.Range(ml, h.col_start, ml, h.col_end),
            options: { inlineClassName: "hit-all" },
          };
        })
        .filter((d): d is NonNullable<typeof d> => d !== null);
      allHitsRef.current.set(decos);
    };

    const applyReveal = (line: number, cs?: number, ce?: number): boolean => {
      const ed = editorRef.current;
      const monaco = monacoRef.current;
      if (!ed || !monaco || !currentRef.current) {
        pendingReveal.current = { line, cs, ce };
        return true; // optimistic — will apply on mount
      }
      const ml = line - firstLineNoRef.current + 1;
      if (ml < 1) return false; // above the loaded window
      ed.revealLineInCenter(ml);
      ed.setPosition({ lineNumber: ml, column: cs ?? 1 });
      currentRef.current.set([
        {
          range: new monaco.Range(ml, cs ?? 1, ml, ce ?? (cs ?? 1)),
          options: { inlineClassName: "hit-current" },
        },
        {
          range: new monaco.Range(ml, 1, ml, 1),
          options: { isWholeLine: true, className: "hit-current-line" },
        },
      ]);
      return true;
    };

    useImperativeHandle(
      ref,
      (): OutputViewerHandle => ({
        revealFileLine: (fileLineNo, colStart, colEnd) =>
          applyReveal(fileLineNo, colStart, colEnd),
        setHits: (hits) => applyHits(hits),
      }),
      [],
    );

    const onMount: OnMount = (ed, monaco) => {
      editorRef.current = ed;
      monacoRef.current = monaco;
      allHitsRef.current = ed.createDecorationsCollection();
      currentRef.current = ed.createDecorationsCollection();

      // F3 / Shift+F3 step our search hits (overrides Monaco's own find-next).
      ed.addCommand(monaco.KeyCode.F3, () => findNextRef.current?.());
      ed.addCommand(monaco.KeyMod.Shift | monaco.KeyCode.F3, () =>
        findPrevRef.current?.(),
      );

      // Flush anything requested before mount.
      if (pendingHits.current) {
        applyHits(pendingHits.current);
        pendingHits.current = null;
      }
      if (pendingReveal.current) {
        const p = pendingReveal.current;
        pendingReveal.current = null;
        applyReveal(p.line, p.cs, p.ce);
      }
    };

    const shown = totalLines - firstLineNo + 1;

    return (
      <div className="output-viewer">
        {truncated ? (
          <div className="viewer-warn muted">
            Large file: showing the last {shown.toLocaleString()} of{" "}
            {totalLines.toLocaleString()} lines. Line numbers are absolute.
          </div>
        ) : null}
        <div className="output-viewer-editor">
          <Editor
            language="plaintext"
            theme="vs-dark"
            value={content}
            onMount={onMount}
            options={{
              readOnly: true,
              wordWrap: "off",
              minimap: { enabled: true },
              renderWhitespace: "none",
              scrollBeyondLastLine: false,
              fontFamily: FONT,
              fontSize: 12,
              automaticLayout: true,
              // Absolute file line numbers even when the head is truncated.
              lineNumbers: (n) => String(n + firstLineNo - 1),
            }}
          />
        </div>
      </div>
    );
  },
);
