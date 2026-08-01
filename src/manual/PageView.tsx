//! The single display component for the manual (a section indexes, a page shows). Both
//! `ManualScreen` (search result → page) and `ManualDrawer` (editor hover → page) render
//! THIS — there is no second render path. It shows the whole file, scrolls to the target
//! section, and highlights that section's line-bounds so the reader sees WHERE the match
//! came from and what surrounds it.
//!
//! Body rendering goes through `renderManualBody` (below) → the pure, tested
//! `parseManualBody` (`render.ts`). That is the ONE block renderer: fences/tables in a
//! monospace `<pre>`, everything else verbatim — the loss-free rule is unchanged.

import { createElement, useEffect, useRef, type ReactNode } from "react";

import { parseManualBody } from "./render";
import type { ManualPage } from "./types";

/**
 * Render a body into blocks. Each block's exact source text becomes a text node — a
 * fence/table in a `<pre>` (monospace, indentation preserved), everything else verbatim
 * in a `white-space: pre-wrap` block. Nothing is dropped (see `render.ts` /
 * `render.test.ts`).
 */
export function renderManualBody(body: string): ReactNode {
  return parseManualBody(body).map((b, i) =>
    b.kind === "fence" ? (
      <pre className="manual-fence" key={i}>
        {b.text}
      </pre>
    ) : b.kind === "table" ? (
      <pre className="manual-fence manual-table" key={i}>
        {b.text}
      </pre>
    ) : (
      <div className="manual-prose" key={i}>
        {b.text}
      </div>
    ),
  );
}

export function PageView({
  page,
  targetSectionId,
}: {
  page: ManualPage;
  targetSectionId?: number | null;
}) {
  // One DOM node per section, keyed by id, for scroll + highlight. Sections tile the
  // file by line (the sectioner's line-conservation), so a section node spans exactly
  // its indexed [line_start, line_end] — which makes "highlight the bounds" simply
  // "highlight the node".
  const refs = useRef(new Map<number, HTMLElement | null>());

  const lines = page.text.split("\n");
  const firstHeading = page.sections.length ? page.sections[0].line_start : lines.length;
  const preamble = firstHeading > 0 ? lines.slice(0, firstHeading).join("\n") : "";

  // Scroll the target section into view whenever the target (or the page) changes.
  useEffect(() => {
    if (targetSectionId == null) return;
    refs.current.get(targetSectionId)?.scrollIntoView({ block: "start" });
  }, [targetSectionId, page.file]);

  const scrollTo = (id: number) =>
    refs.current.get(id)?.scrollIntoView({ block: "start", behavior: "smooth" });

  return (
    <article className="manual-page">
      <nav className="manual-breadcrumb mono" title={page.file}>
        {page.file}
      </nav>

      {/* In-page navigation: the file's headings as a table of contents. Pages reach
          ~48 KB / 160+ sections, so this is how the reader moves without endless scroll.
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
          <div className="manual-body manual-preamble">{renderManualBody(preamble)}</div>
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
              {body.trim() ? <div className="manual-body">{renderManualBody(body)}</div> : null}
            </section>
          );
        })}
      </div>
    </article>
  );
}
