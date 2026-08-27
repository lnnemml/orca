# ADR-022: In-session agentic development workflow

**Status:** accepted · 2026-08-27

## Context
For three weeks the project ran a two-tool loop: **Claude Web** as architect/reviewer wrote prompts
that Anton copy-pasted into **Claude Code**, which implemented. The loop worked but had two costs: a
manual relay (Anton shuttling prompts by hand) and a trust gap (the web reviewer accepted `cargo`
counts from Anton's machine because its container lacked `rustc`).

Claude Code now supports **subagents** — isolated Claude instances the main session spawns, each with
its own context window, tool set, permissions, and model (`.claude/agents/*.md`, committed to the
repo). This lets the *intra-session* architect → implement → verify loop run inside one tool, with
the objective checks running in the same environment as the code.

The question was whether to move the whole workflow in, and how to do it **without dissolving the
gates that have caught real defects** — Anton's chemistry sanity gate, his live WebKitGTK gate, and
his authority over design forks.

## Decision
Adopt an in-session agentic loop. Roles map as follows:

- **orchestrator-architect** = the **main Claude Code session**, not a spawned subagent and not a
  `--agent` persona. `--agent` replaces Claude Code's own system prompt entirely, discarding its
  built-in tool orchestration; instead the discipline is layered on via CLAUDE.md. The orchestrator
  holds the ROADMAP, decomposes, writes sub-task prompts, presents forks with a lean, spawns the
  workers, and commits — but **does not write code and does not review its own plan**.
- **prober** (`sonnet`) — settles third-party facts from real runs only (domain rule #10). Read-only
  on source; runs measurements.
- **explorer** (`haiku`) — read-only archaeology of the codebase and wiki; returns `file:line`
  anchors and reuse candidates; distinguishes ADR intent from implementation reality.
- **implementer** (`opus`) — lands one unit, STOP-AND-REPORT (Part A pure+tested → STOP → Part B
  wiring), reuse over rebuild, wiki in the same change. **Never commits**; leaves the working tree
  for review.
- **verifier** (`opus`, `isolation: worktree`) — independent, fresh-context push-time ritual; runs
  `tsc`/`vitest`/`cargo`/`pytest` for real; proves each negative control bites by breaking it in its
  throwaway worktree. Marks render/chemistry units REQUIRES LIVE GATE and hands them to Anton.

The loop: orchestrator probe → decompose → *(fork? Anton decides, ADR same session)* →
prober/explorer establish anchors → implementer Part A → **STOP** → verifier → *(Anton greenlight)* →
implementer Part B → verifier push-ritual → *(Anton live gate if render; chemistry gate if science)*
→ orchestrator commits on Anton's approval.

## Rationale
- **Independence comes from a fresh context + objective tooling, not from tool separation.** A
  read-only verifier that starts clean and runs the real tests is as independent as the old web
  reviewer — and strictly better, because it runs `cargo`/`vitest` in the code's own environment
  instead of accepting counts on trust. The orchestrator must therefore **never self-review**;
  review always routes through the separate verifier.
- **Subagents preserve the context boundaries the workflow relied on.** Each worker's verbose output
  stays in its own window; only its summary returns. The extraction/verification discipline is
  unchanged; only the relay is removed.
- **CLAUDE.md auto-loads into every non-Explore/Plan subagent**, so the 11 domain rules and the
  conventions reach every worker for free — the agent files stay thin, encoding only role discipline.
- **Model routing is a cost lever:** judgement roles (orchestrator, verifier, implementer) on
  `opus`; the `sonnet` prober interprets real tool output; the cheap `haiku` explorer does read-only
  search.

## Consequences
- **The human gates are structural, not optional.** The implementer cannot commit; the verifier
  cannot certify render or chemistry correctness. Anton remains the merge approver, the live
  WebKitGTK gate, the chemistry sanity gate, and the sole resolver of design forks. No agent closes
  these — by construction, not by convention.
- **Run the main session in default (manual) or plan mode, not auto mode.** In auto mode a
  subagent's `permissionMode` is ignored and edits can auto-apply, weakening the diff-review gate.
- This refines, and does not replace, the CLAUDE.md "Division of labor": Claude Web remains the
  cross-session strategic architect (this ADR was designed there); Claude Code now runs the
  intra-session loop agentically.
- Agent definitions live in `.claude/agents/` and are versioned with the code, so the workflow
  itself is reviewable and improvable in PRs like any other project artifact.
- New dependency on Claude Code's subagent feature set (subagents, `isolation: worktree`, per-agent
  model). If unavailable, the loop degrades gracefully to the prior two-tool relay.
