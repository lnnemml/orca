# ADR-006: Local ORCA manual indexing

**Status:** accepted · 2026-07-26

## Context
The ORCA manual (~1300 pp PDF / HTML docs) is the main learning barrier. Mission requires
in-app, context-sensitive access. The manual is copyrighted: fine to index locally for
personal use, must not be redistributed with the app.

## Decision
A one-off sidecar pipeline builds a **local** index on the user's machine: fetch HTML docs
→ split into sections (heading, body, keywords, input examples) → SQLite FTS5.
Three access layers in the UI:
1. Search panel (FTS5).
2. Monaco hover provider backed by a curated `keywords.json`
   (input keyword → section anchor + one-line summary).
3. Optional "Explain with Claude" (user's own Anthropic API key): keyword + user's current
   input + manual excerpt → plain-language explanation.

## Rationale
- FTS5 needs no extra infrastructure and is instant at this corpus size.
- The keyword map doubles as training data for input validation/linting later.
- The same index is greppable by Claude Code during development of the input builder.

## Consequences
- `keywords.json` is curated by hand and grows organically — seed it from the template
  library's keywords first.
- Index schema stores the source URL/section anchor so content can be refreshed when a new
  ORCA version ships.
- The app ships *without* any manual content; first-run setup builds the index.
