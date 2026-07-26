# Project Log

Append-only. Entry format: `## [YYYY-MM-DD] type | Title`
where `type ∈ {session, decision, ingest, lint, milestone}`. Newest entries at the bottom.

---

## [2026-07-26] milestone | Project scaffold created

Initial scaffold generated from architecture planning sessions (Claude web):

- Repository structure defined: Tauri/React frontend, Rust core, Python sidecar, wiki.
- Six founding ADRs written (stack, sidecar, execution backends, storage, SSH strategy,
  manual integration).
- Roadmap drafted: Phases 0–6, each ending in a usable increment.
- Wiki system initialized per the LLM-wiki pattern (CLAUDE.md = schema).

Key founding decisions to remember:
1. ExecutionBackend abstraction from day one — remote SSH execution is a first-class,
   optional path, not a bolt-on.
2. The app is a learning instrument, not just a launcher: live convergence plots and
   integrated manual are core features.
3. Mission test for every feature: "does it lower the barrier for a terminal-shy chemist?"

Next: Phase 0 — ORCA 6 installation + environment verification, then Tauri scaffold.
