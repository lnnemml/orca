//! The single display component for the manual (a section indexes, a page shows). Both
//! `ManualScreen` (search result → page) and `ManualDrawer` (editor hover → page) render
//! THIS — there is no second render path. It shows the whole file, scrolls to the target
//! section, and highlights that section's line-bounds so the reader sees WHERE the match
//! came from and what surrounds it.
//!
//! Body rendering goes through `renderManualBody` (below) → the pure, tested `render.ts`
//! (`parseManualBody` + `tokenizeInline`). Three categories, each source char in exactly
//! one: (1) recognized & transformed (math→KaTeX, `` `code` ``→`<code>`, resolved
//! cross-refs→`<a>`), (2) unrecognized→verbatim (the preservation test), (3) three named
//! metadata directives→hidden. See `render.ts` and `render.test.ts`.

import {
  createElement,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";
import katex from "katex";

import {
  parseManualBody,
  tokenizeInline,
  xrefLabels,
  isHiddenDirective,
  isMissingInclude,
  type Block,
} from "./render";
import type { ManualPage } from "./types";

/** A resolved anchor target (from the Rust `resolve_manual_anchors`, which owns the
 *  slugify rule — we never re-normalize in JS). */
export interface AnchorTarget {
  file: string;
  section_id: number;
}

interface RenderOpts {
  /** Label → target, or null when UNDETERMINED (the link then stays verbatim text). */
  resolve?: (label: string) => AnchorTarget | null;
  /** Navigate to a resolved cross-reference (cross-page); same-page scroll is internal. */
  onNavigate?: (file: string, sectionId: number) => void;
}

/** Render a fragment of LaTeX with KaTeX. `throwOnError: false` → an unknown macro shows
 *  its SOURCE verbatim (category 2 fallback), never a broken page (asserted in tests).
 *  Fully offline: the fonts are bundled (CSS imported in `main.tsx`, 0 network refs). */
function renderMath(tex: string, display: boolean): ReactNode {
  let html: string;
  try {
    html = katex.renderToString(tex, { throwOnError: false, displayMode: display });
  } catch {
    // throwOnError:false renders parse errors as source, but stay defensive: a hard
    // failure falls back to the verbatim source rather than dropping it.
    return display ? `$$${tex}$$` : `$${tex}$`;
  }
  return (
    <span
      className={display ? "manual-math-display" : "manual-math"}
      dangerouslySetInnerHTML={{ __html: html }}
    />
  );
}

/** Render one prose block's inline tokens (category 1 transforms; everything else text). */
function renderInline(text: string, opts: RenderOpts, keyBase: string): ReactNode {
  return tokenizeInline(text).map((t, i) => {
    const key = `${keyBase}-${i}`;
    switch (t.kind) {
      case "text":
        return <span key={key}>{t.text}</span>;
      case "code":
        return (
          <code className="manual-code" key={key}>
            {t.text}
          </code>
        );
      case "math":
        return <span key={key}>{renderMath(t.tex, t.display)}</span>;
      case "cite":
        // {cite}`keys` → [keys] (category 1 — a VISIBLE citation in Sphinx; keep the keys).
        return (
          <span className="manual-cite" key={key}>
            [{t.keys}]
          </span>
        );
      case "xref": {
        const target = opts.resolve?.(t.label) ?? null;
        // Unresolved → verbatim source, NOT a dead click (same posture as a NULL anchor
        // and hover silence). Resolved → a link that navigates on click.
        if (!target) return <span key={key}>{t.raw}</span>;
        return (
          <a
            className="manual-xref"
            key={key}
            role="link"
            onClick={() => onNavigateTo(opts, target)}
            title={t.label}
          >
            {t.text}
          </a>
        );
      }
    }
  });
}

function onNavigateTo(opts: RenderOpts, target: AnchorTarget) {
  opts.onNavigate?.(target.file, target.section_id);
}

function renderBlocks(blocks: Block[], opts: RenderOpts, keyBase: string): ReactNode {
  return blocks.map((b, i) => {
    const key = `${keyBase}-${i}`;
    switch (b.kind) {
      case "fence":
        return (
          <pre className="manual-fence" key={key}>
            {b.text}
          </pre>
        );
      case "table":
        return (
          <pre className="manual-fence manual-table" key={key}>
            {b.text}
          </pre>
        );
      case "label":
        // Category 3 — a MyST anchor label line `(name)=`, invisible in the real manual.
        return null;
      case "prose":
        return (
          <div className="manual-prose" key={key}>
            {renderInline(b.text, opts, key)}
          </div>
        );
      case "directive": {
        // Category 3 — the three named, measured metadata directives: hidden.
        if (isHiddenDirective(b.name, b.arg)) return null;
        // Category 5 — external content not in our corpus: a visible ABSENCE MARKER, not
        // a bare path that reads as silent emptiness where the manual gave an input.
        if (isMissingInclude(b.name)) {
          return (
            <div className="manual-missing" key={key}>
              input example not loaded ({b.arg})
            </div>
          );
        }
        // Category 2 — a visible directive (note/warning/table/figure/…): markers kept
        // verbatim (nothing hidden but the named three), body rendered recursively so a
        // nested code fence or `$…$` inside a note still transforms.
        return (
          <div className={"manual-directive md-" + b.name.toLowerCase()} key={key}>
            <div className="manual-directive-marker">{b.open}</div>
            {renderBlocks(parseManualBody(b.inner), opts, key + "i")}
            {b.close ? <div className="manual-directive-marker">{b.close}</div> : null}
          </div>
        );
      }
    }
  });
}

/**
 * Render a body into React. With no `opts` every cross-reference stays verbatim, so this
 * is loss-free by default (the preservation test calls it this way). PageView passes a
 * `resolve` map + `onNavigate` so resolvable cross-references become links.
 */
export function renderManualBody(body: string, opts: RenderOpts = {}): ReactNode {
  return renderBlocks(parseManualBody(body), opts, "b");
}

export function PageView({
  page,
  targetSectionId,
  onNavigate,
}: {
  page: ManualPage;
  targetSectionId?: number | null;
  /** Cross-page cross-reference navigation (load another file). Optional: without it,
   *  cross-page links still render but do nothing; same-page scroll is always internal. */
  onNavigate?: (file: string, sectionId: number) => void;
}) {
  // One DOM node per section, keyed by id, for scroll + highlight. Sections tile the
  // file by line (the sectioner's line-conservation), so a section node spans exactly
  // its indexed [line_start, line_end] — which makes "highlight the bounds" simply
  // "highlight the node".
  const refs = useRef(new Map<number, HTMLElement | null>());

  // The anchor map for this page's cross-references. Resolution (the slugify rule) lives
  // in Rust (`predict_anchor`); we never re-normalize here. Filled once per page.
  const [anchors, setAnchors] = useState<Map<string, AnchorTarget>>(new Map());

  const lines = page.text.split("\n");
  const firstHeading = page.sections.length ? page.sections[0].line_start : lines.length;
  const preamble = firstHeading > 0 ? lines.slice(0, firstHeading).join("\n") : "";

  // Resolve every cross-reference label on the page in ONE batch call, then render
  // synchronously. Labels the resolver returns nothing for stay verbatim.
  useEffect(() => {
    let cancelled = false;
    const labels = xrefLabels(page.text);
    if (labels.length === 0) {
      setAnchors(new Map());
      return;
    }
    invoke<(AnchorTarget | null)[]>("resolve_manual_anchors", { labels })
      .then((targets) => {
        if (cancelled) return;
        const map = new Map<string, AnchorTarget>();
        labels.forEach((l, i) => {
          const t = targets[i];
          if (t) map.set(l, t);
        });
        setAnchors(map);
      })
      .catch(() => {
        if (!cancelled) setAnchors(new Map()); // resolve failure → all links verbatim
      });
    return () => {
      cancelled = true;
    };
  }, [page.file, page.text]);

  // Scroll the target section into view whenever the target (or the page) changes.
  useEffect(() => {
    if (targetSectionId == null) return;
    refs.current.get(targetSectionId)?.scrollIntoView({ block: "start" });
  }, [targetSectionId, page.file]);

  const scrollTo = (id: number) =>
    refs.current.get(id)?.scrollIntoView({ block: "start", behavior: "smooth" });

  const renderOpts: RenderOpts = {
    resolve: (label) => anchors.get(label) ?? null,
    onNavigate: (file, id) => {
      // Same-page cross-reference → scroll internally; cross-page → ask the host to load.
      if (file === page.file) scrollTo(id);
      else onNavigate?.(file, id);
    },
  };

  return (
    <article className="manual-page">
      <nav className="manual-breadcrumb mono" title={page.file}>
        {page.file}
      </nav>

      {/* In-page navigation: the file's headings as a table of contents. Pages reach
          ~209 KB / 160+ sections, so this is how the reader moves without endless scroll.
          Collapsed by default on big pages so it doesn't bury the content. */}
      {page.sections.length > 1 ? (
        <details className="manual-toc" open={page.sections.length <= 20}>
          <summary>{page.sections.length} sections on this page</summary>
          <ul>
            {page.sections.map((s) => (
              <li key={s.id} style={{ marginLeft: `${(Math.max(s.level, 1) - 1) * 12}px` }}>
                <button
                  className={"manual-toc-link" + (s.id === targetSectionId ? " active" : "")}
                  onClick={() => scrollTo(s.id)}
                >
                  {s.title}
                </button>
              </li>
            ))}
          </ul>
        </details>
      ) : null}

      <div className="manual-page-body">
        {preamble.trim() ? (
          <div className="manual-body manual-preamble">
            {renderManualBody(preamble, renderOpts)}
          </div>
        ) : null}
        {page.sections.map((s) => {
          const secLines = lines.slice(s.line_start, s.line_end + 1);
          const body = secLines.slice(1).join("\n"); // drop the heading line
          return (
            <section
              key={s.id}
              ref={(el) => {
                refs.current.set(s.id, el);
              }}
              id={`manual-sec-${s.id}`}
              className={"manual-page-section" + (s.id === targetSectionId ? " target" : "")}
            >
              {createElement(
                `h${Math.min(Math.max(s.level, 1) + 1, 6)}`,
                { className: "manual-heading" },
                s.title,
              )}
              {body.trim() ? (
                <div className="manual-body">{renderManualBody(body, renderOpts)}</div>
              ) : null}
            </section>
          );
        })}
      </div>
    </article>
  );
}
