# ADR-001: Tauri 2 + React/TypeScript as the desktop shell

**Status:** accepted · 2026-07-26

## Context
Need a Linux-first desktop GUI. Author's strongest stack is React/TypeScript (Next.js
production experience). Alternatives considered: Electron, PySide6/Qt, egui (pure Rust).

## Decision
Tauri 2 with React 18 + TypeScript (strict) + Vite.

## Rationale
- Reuses the author's highest-velocity skill set; UI iteration speed matters most
  for a solo project.
- Whole web visualization ecosystem becomes available: 3Dmol.js/Mol* for molecules and
  cube isosurfaces, Monaco for editing, recharts for plots. Qt equivalents are far weaker
  or require OpenGL work.
- Tauri vs Electron: ~10 MB binary vs ~150 MB, lower RAM, native Rust backend which we
  need anyway for process/SSH work.
- PySide6 rejected: one-runtime simplicity is attractive (cclib/RDKit in-process), but 3D
  visualization is painful and UI velocity would be much lower for this author.

## Consequences
- Rust learning curve for the core (acceptable: the Rust layer is deliberately thin).
- Chemistry libraries need a separate Python process → ADR-002.
- WebKitGTK is the rendering engine on Linux — test WebGL (3Dmol) early in Phase 2;
  if performance disappoints, Mol* or a WebGL flag fallback are the escape hatches.
