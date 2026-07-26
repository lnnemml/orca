# Module: Manual index (Phase 4)

**Status:** not started · Strategy in ADR-006

## Pipeline (sidecar, one-off per ORCA version)
fetch HTML docs → section splitter (heading tree) → rows:
`(id, version, breadcrumb, title, body_md, input_examples, source_anchor)` → FTS5 table.

## keywords.json
Curated map: `{ "RIJCOSX": { "summary": "...", "section_id": ... }, ... }`
Seed order: everything used by the Phase 1 template library → solvation → job types →
%blocks. Lives in repo (it's our own writing, not manual content).

## UI integration
- Search panel: FTS query, snippet highlighting, section rendering (markdown).
- Monaco hover provider: tokenize `!` line and `%block` names → keywords.json lookup.
- "Explain with Claude" (optional): POST keyword + current .inp + manual excerpt to
  Anthropic API with the user's key from settings.
