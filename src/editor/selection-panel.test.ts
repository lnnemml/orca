import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";

import { createSelectionController, type PanelContent, type PanelPosition } from "./selection-panel";
import { orcaEditorOptions } from "./editor-options";
import { setManualOpenHandler } from "./manual-open";
import { setExplainHandler } from "./explain-open";

// The WIRING — the selection listener + resolve + the Open action — is where the trigger
// change could break while the pure `resolveSelection` tests stay green (debugging/010:
// the wire is what breaks). Exercised here with a FAKE editor + FAKE view (no real Monaco,
// no jsdom): the DOM-free `createSelectionController` carries the whole wire.

interface FakeSel {
  startLineNumber: number;
  startColumn: number;
  endLineNumber: number;
  endColumn: number;
  isEmpty(): boolean;
}
function sel(startCol: number, endCol: number, line = 1): FakeSel {
  return {
    startLineNumber: line,
    startColumn: startCol,
    endLineNumber: line,
    endColumn: endCol,
    isEmpty: () => startCol === endCol,
  };
}

function valueInRange(text: string, r: FakeSel): string {
  const lines = text.split("\n");
  return (lines[r.startLineNumber - 1] ?? "").slice(r.startColumn - 1, r.endColumn - 1);
}

function makeFakeEditor(text: string, initial: FakeSel) {
  let current = initial;
  let cb: (() => void) | null = null;
  return {
    editor: {
      onDidChangeCursorSelection(fn: () => void) {
        cb = fn;
        return { dispose() {} };
      },
      getSelection: () => current,
      getModel: () => ({ getValue: () => text, getValueInRange: (r: FakeSel) => valueInRange(text, r) }),
    } as never,
    fire(next: FakeSel) {
      current = next;
      cb?.();
    },
    set(next: FakeSel) {
      current = next;
    },
  };
}

function makeFakeView() {
  const shown: { content: PanelContent; at: PanelPosition }[] = [];
  let hidden = 0;
  return {
    view: {
      show(content: PanelContent, at: PanelPosition) {
        shown.push({ content, at });
      },
      hide() {
        hidden++;
      },
    },
    shown,
    get hidden() {
      return hidden;
    },
  };
}

describe("selection panel wiring (fake editor + view, no jsdom)", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => {
    vi.useRealTimers();
    setManualOpenHandler(null);
    setExplainHandler(null);
  });

  it("a settled selection over a keyword → show(hit) with the normalized word", () => {
    // "! r2SCAN-3c": the keyword spans 1-based cols [3,12).
    const fe = makeFakeEditor("! r2SCAN-3c", sel(3, 12));
    const fv = makeFakeView();
    createSelectionController(fe.editor, fv.view);

    fe.fire(sel(3, 12)); // selection change
    vi.advanceTimersByTime(150); // debounce settles

    expect(fv.shown).toHaveLength(1);
    expect(fv.shown[0].content.kind).toBe("hit");
    if (fv.shown[0].content.kind === "hit") {
      expect(fv.shown[0].content.word).toBe("r2SCAN-3c");
      expect(fv.shown[0].content.typeLabel).toBe("simple keyword");
    }
  });

  it("the Open action calls the manual-open channel with the resolved descriptor", () => {
    const opened = vi.fn();
    setManualOpenHandler(opened); // ManualDrawer registers this in the app

    const fe = makeFakeEditor("! r2SCAN-3c", sel(3, 12));
    const fv = makeFakeView();
    const controller = createSelectionController(fe.editor, fv.view);

    controller.update(); // resolve now
    controller.open(); // the panel's "Open in manual" button

    expect(opened).toHaveBeenCalledTimes(1);
    expect(opened.mock.calls[0][0]).toMatchObject({ file: expect.any(String), title: expect.any(String) });
  });

  it("Explain hands the drawer exactly {word, line, descriptor} and writes NOTHING to the editor", () => {
    const explained = vi.fn();
    setExplainHandler(explained); // ManualDrawer registers this in the app

    const fe = makeFakeEditor("! r2SCAN-3c", sel(3, 12));
    // A spy for any editor mutation — tier-zero (ADR-014): explain must never write to the editor.
    const writeSpy = vi.fn();
    (fe.editor as unknown as { executeEdits: unknown }).executeEdits = writeSpy;
    const fv = makeFakeView();
    const c = createSelectionController(fe.editor, fv.view);

    c.update();
    c.explain(); // the panel's "Explain with Claude" action

    expect(explained).toHaveBeenCalledTimes(1);
    const req = explained.mock.calls[0][0];
    // Exactly the three fields — no coordinate/geometry/file could ride along (ADR-015 (3)).
    expect(Object.keys(req).sort()).toEqual(["descriptor", "line", "word"]);
    expect(req.word).toBe("r2SCAN-3c");
    expect(req.line).toBe("! r2SCAN-3c");
    expect(req.descriptor).toMatchObject({ file: expect.any(String), title: expect.any(String) });
    // The editor was never mutated (tier-zero: advice only, no insert path).
    expect(writeSpy).not.toHaveBeenCalled();
  });

  it("Explain is a no-op when there is no resolved section (a miss) — no drawer call", () => {
    const explained = vi.fn();
    setExplainHandler(explained);
    const fe = makeFakeEditor("! NotAKeyword", sel(3, 14));
    const fv = makeFakeView();
    const c = createSelectionController(fe.editor, fv.view);
    c.update();
    c.explain();
    expect(explained).not.toHaveBeenCalled();
  });

  it("a mid-token cut → show(malformed) with a format hint (not silence)", () => {
    // "! TightSCF": cutting `Tight` = cols [3,8).
    const fe = makeFakeEditor("! TightSCF", sel(3, 8));
    const fv = makeFakeView();
    const c = createSelectionController(fe.editor, fv.view);
    c.update();
    expect(fv.shown).toHaveLength(1);
    expect(fv.shown[0].content.kind).toBe("malformed");
  });

  it("a well-formed miss → hide (silence, no panel)", () => {
    const fe = makeFakeEditor("! NotAKeyword", sel(3, 14));
    const fv = makeFakeView();
    const c = createSelectionController(fe.editor, fv.view);
    c.update();
    expect(fv.shown).toHaveLength(0);
    expect(fv.hidden).toBeGreaterThan(0);
  });

  it("an empty selection → hide", () => {
    const fe = makeFakeEditor("! Opt", sel(3, 3));
    const fv = makeFakeView();
    const c = createSelectionController(fe.editor, fv.view);
    c.update();
    expect(fv.shown).toHaveLength(0);
    expect(fv.hidden).toBeGreaterThan(0);
  });

  it("still pins fixedOverflowWidgets — the panel is an overflow widget, un-clipped on the top line (debugging/010)", () => {
    expect(orcaEditorOptions.fixedOverflowWidgets).toBe(true);
  });
});
