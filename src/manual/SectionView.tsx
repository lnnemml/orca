//! The rendered view of ONE manual section. A **standalone component** (not baked into
//! the screen) on purpose: the next unit's Monaco hover opens a section WITHOUT pulling
//! the author out of the editor, i.e. this same component will live in a drawer. So it
//! takes a `ManualSection` and renders it; it owns no fetching, no screen chrome.

import type { ReactNode } from "react";

import { parseManualBody } from "./render";
import type { ManualSection } from "./types";

/**
 * Render a body into blocks. Each block's exact source text becomes a text node — a
 * fence in a `<pre>` (monospace, indentation preserved), everything else verbatim in a
 * `white-space: pre-wrap` block. Nothing is dropped (see `render.ts` / `render.test.ts`).
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

export function SectionView({ section }: { section: ManualSection }) {
  const trail = section.breadcrumb.length ? section.breadcrumb.join(" › ") : section.file;
  return (
    <article className="manual-section">
      <nav className="manual-breadcrumb mono" title={section.file}>
        {trail}
      </nav>
      <h2 className="manual-section-title">{section.title}</h2>
      {section.body_md.trim() ? (
        <div className="manual-body">{renderManualBody(section.body_md)}</div>
      ) : (
        <div className="empty">This section is a heading only (no body).</div>
      )}
    </article>
  );
}
