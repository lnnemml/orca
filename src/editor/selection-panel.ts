//! The selection-triggered manual lookup (unit 4.13) — the successor to the Monaco hover.
//! A floating panel appears over a STABLE, non-empty selection (a short debounce after the
//! selection settles, not mid-drag) showing the recognized keyword + an "Open in manual"
//! action. The trigger changed for three measured reasons (`wiki/modules/frontend.md`):
//! hover interrupted edits and popped up unbidden; it gave exactly ONE `wordPattern` token
//! (which we patched twice — `NEB-TS`, `def2/J` are not one token), and selection lets the
//! author name the boundary himself; and a selection is DELIBERATE, so help becomes a
//! request, not an interruption (the "learning instrument" mission).
//!
//! Split in two so the WIRE is testable without jsdom (the debugging/010 lesson — the pure
//! functions were tested, the wiring was not, and that is where it broke): the pure,
//! DOM-free `createSelectionController` (selection → `resolveSelection` → show/hide + the
//! Open action) is exercised by `selection-panel.test.ts`; the Monaco content-widget VIEW
//! (below) is the thin DOM adapter. The panel is an overflow widget (`allowEditorOverflow`),
//! so `editor-options.ts`'s `fixedOverflowWidgets: true` un-clips it on the top line — the
//! SAME path debugging/010 fixed for the hover.

import type { Monaco } from "@monaco-editor/react";
import type { editor as MonacoEditor, IDisposable } from "monaco-editor";

import { invoke } from "@tauri-apps/api/core";

import { resolveSelection } from "../manual/selection-lookup";
import { recordSections, type HoverMatch, type SectionDescriptor } from "../manual/keyword-lookup";
import { openManualSection } from "./manual-open";
import { requestExplain } from "./explain-open";
import type { KeySource } from "../types";

export interface PanelPosition {
  lineNumber: number;
  column: number;
}
export type PanelContent =
  | { kind: "hit"; word: string; typeLabel: string; descriptors: SectionDescriptor[] }
  | { kind: "malformed"; hint: string };

/** The view the controller drives — a real Monaco widget in the app, a fake in the test. */
export interface PanelView {
  show(content: PanelContent, at: PanelPosition): void;
  hide(): void;
}

function typeLabel(m: HoverMatch): string {
  if (m.kind === "block") return "input block";
  if (m.kind === "block-option") return `option of ${m.block}`;
  return "simple keyword";
}

function dedup(ds: SectionDescriptor[]): SectionDescriptor[] {
  const seen = new Set<string>();
  return ds.filter((d) => {
    if (!d) return false;
    const k = `${d.file}${d.breadcrumb.join("")}${d.title}${d.nth}`;
    return seen.has(k) ? false : (seen.add(k), true);
  });
}

export interface SelectionController {
  /** Read the current selection now and update the view (bypasses the debounce). */
  update(): void;
  /** Open the current hit's section in the drawer (the panel's "Open" action). */
  open(): void;
  /** Explain the current hit (word + line + its section) — the ADR-014 T1 action. Advice
   *  only; NEVER writes to the editor (there is no editor-mutating call in this path). */
  explain(): void;
  dispose(): void;
}

/**
 * The pure wire: subscribe to selection changes, debounce, resolve, and drive the view.
 * DOM-free (no `document`), so the wiring test can exercise it with a fake editor + view.
 */
export function createSelectionController(
  editor: MonacoEditor.ICodeEditor,
  view: PanelView,
  opts: { debounceMs?: number } = {},
): SelectionController {
  let timer: ReturnType<typeof setTimeout> | null = null;
  // The current hit's three explain fields (word + the line around it + its section descriptor).
  // Null when there is no resolvable section (miss / malformed / a descriptor-less record).
  let currentHit: { word: string; line: string; descriptor: SectionDescriptor } | null = null;

  const sub = editor.onDidChangeCursorSelection(() => {
    if (timer) clearTimeout(timer);
    timer = setTimeout(update, opts.debounceMs ?? 150); // settle after the drag, not during
  });

  function update() {
    const sel = editor.getSelection();
    const model = editor.getModel();
    if (!sel || !model || sel.isEmpty()) {
      currentHit = null;
      view.hide();
      return;
    }
    const selected = model.getValueInRange(sel);
    const r = resolveSelection(
      model.getValue(),
      sel.startLineNumber - 1,
      sel.startColumn - 1,
      sel.endLineNumber - 1,
      sel.endColumn - 1,
      selected,
    );
    const at: PanelPosition = { lineNumber: sel.startLineNumber, column: sel.startColumn };
    if (r.kind === "miss") {
      // qualified miss (not in the map, or an argument token) → silence, no panel.
      currentHit = null;
      view.hide();
      return;
    }
    if (r.kind === "malformed") {
      // internal space / a mid-token cut → a correctable FORMAT hint, not an answer.
      currentHit = null;
      view.show({ kind: "malformed", hint: "Select one keyword whole" }, at);
      return;
    }
    const descriptors = dedup(r.match.records.flatMap(recordSections));
    const descriptor = descriptors[0];
    const lineText = model.getValue().split("\n")[sel.startLineNumber - 1] ?? "";
    currentHit = descriptor ? { word: r.match.word, line: lineText, descriptor } : null;
    view.show({ kind: "hit", word: r.match.word, typeLabel: typeLabel(r.match), descriptors }, at);
  }

  function open() {
    if (currentHit) openManualSection(currentHit.descriptor);
  }
  function explain() {
    // Advice only — hands the three fields to the drawer channel; touches nothing in the editor.
    if (currentHit) requestExplain(currentHit);
  }
  function dispose() {
    if (timer) clearTimeout(timer);
    sub.dispose();
  }
  return { update, open, explain, dispose };
}

// ── The Monaco content-widget view (the DOM adapter; not unit-tested — jsdom-free) ──────

function crumb(d: SectionDescriptor): string {
  return (d.breadcrumb.length ? d.breadcrumb.join(" › ") + " › " : "") + d.title;
}

/** Register the selection panel on a live editor. Returns a disposable. */
export function registerSelectionLookup(
  monaco: Monaco,
  editor: MonacoEditor.ICodeEditor,
): IDisposable {
  const dom = document.createElement("div");
  dom.className = "selection-panel";

  // Whether a usable key exists (TASK 3: no key → no Explain button, not an error on click).
  // Refreshed from `api_key_status` on register and on each show, so adding a key in Settings
  // makes the button appear on the next selection without a reload.
  let keyUsable = false;
  const refreshKey = () => {
    invoke<KeySource>("api_key_status")
      .then((k) => {
        keyUsable = k.state === "stored-in-keyring" || k.state === "from-environment";
      })
      .catch(() => {
        keyUsable = false;
      });
  };
  refreshKey();

  let position: MonacoEditor.IContentWidgetPosition | null = null;
  let added = false;
  const widget: MonacoEditor.IContentWidget = {
    getId: () => "orca.selectionPanel",
    getDomNode: () => dom,
    getPosition: () => position,
    allowEditorOverflow: true, // overflow widget → fixedOverflowWidgets un-clips it (debugging/010)
  };

  const view: PanelView = {
    show(content, at) {
      dom.replaceChildren();
      if (content.kind === "malformed") {
        const hint = document.createElement("div");
        hint.className = "sp-hint";
        hint.textContent = content.hint;
        dom.appendChild(hint);
      } else {
        const head = document.createElement("div");
        head.className = "sp-head";
        head.textContent = `${content.word} — ${content.typeLabel}`;
        dom.appendChild(head);

        const actions = document.createElement("div");
        actions.className = "sp-actions";
        if (content.descriptors.length) {
          const open = document.createElement("button");
          open.className = "btn btn-sm sp-open";
          open.textContent =
            content.descriptors.length > 1
              ? `Open in manual (${content.descriptors.length}) →`
              : "Open in manual →";
          open.title = crumb(content.descriptors[0]);
          open.onclick = () => controller.open();
          actions.appendChild(open);
        }
        // "Explain with Claude" (ADR-014 T1). Appears only with a usable key AND a resolved
        // section — no key → absent, not an error on click. Advice shows in the drawer.
        const explainSlot = document.createElement("span");
        explainSlot.className = "sp-explain-slot";
        if (keyUsable && content.descriptors.length) {
          const explain = document.createElement("button");
          explain.className = "btn btn-sm sp-explain";
          explain.textContent = "Explain with Claude";
          explain.onclick = () => controller.explain();
          explainSlot.appendChild(explain);
        }
        actions.appendChild(explainSlot);
        dom.appendChild(actions);
        refreshKey(); // keep the flag fresh for the next selection
      }
      position = {
        position: { lineNumber: at.lineNumber, column: at.column },
        preference: [
          monaco.editor.ContentWidgetPositionPreference.ABOVE,
          monaco.editor.ContentWidgetPositionPreference.BELOW,
        ],
      };
      if (!added) {
        editor.addContentWidget(widget);
        added = true;
      } else {
        editor.layoutContentWidget(widget);
      }
    },
    hide() {
      if (added) {
        editor.removeContentWidget(widget);
        added = false;
      }
    },
  };

  const controller = createSelectionController(editor, view);
  return {
    dispose() {
      controller.dispose();
      if (added) editor.removeContentWidget(widget);
    },
  };
}
