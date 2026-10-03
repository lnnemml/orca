# ADR-022: In-session agentic development workflow

**Status:** accepted · 2026-08-27 · amended 2026-10-03

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
- **implementer** (`opus`; *model superseded 2026-10-03 — `sonnet`, per-invocation `opus` for
  high-risk units, see amendment §2*) — lands one unit, STOP-AND-REPORT (Part A pure+tested → STOP → Part B
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
  *(Qualified 2026-10-03, see amendment §8: a subagent gets CLAUDE.md as it was when the main session
  **started**, not the current file.)*
- *(Routing superseded 2026-10-03, see amendment §2.)* **Model routing is a cost lever:** judgement roles (orchestrator, verifier, implementer) on
  `opus`; the `sonnet` prober interprets real tool output; the cheap `haiku` explorer does read-only
  search.

## Consequences
- **The human gates are structural, not optional.** The implementer cannot commit; the verifier
  cannot certify render or chemistry correctness. Anton remains the merge approver, the live
  WebKitGTK gate, the chemistry sanity gate, and the sole resolver of design forks. No agent closes
  these — by construction, not by convention.
- *(Superseded 2026-10-03, see amendment §5.)* **Run the main session in default (manual) or plan mode, not auto mode.** In auto mode a
  subagent's `permissionMode` is ignored and edits can auto-apply, weakening the diff-review gate.
- *(Superseded 2026-10-03, see amendment §1.)* This refines, and does not replace, the CLAUDE.md "Division of labor": Claude Web remains the
  cross-session strategic architect (this ADR was designed there); Claude Code now runs the
  intra-session loop agentically.
- Agent definitions live in `.claude/agents/` and are versioned with the code, so the workflow
  itself is reviewable and improvable in PRs like any other project artifact.
- New dependency on Claude Code's subagent feature set (subagents, `isolation: worktree`, per-agent
  model). If unavailable, the loop degrades gracefully to the prior two-tool relay. *(Fallback
  superseded 2026-10-03, see amendment §1: with Web out of the loop, the fallback is to run the roles
  sequentially in the main session — still never self-reviewing a plan — not a two-tool relay.)*

## Amended 2026-10-03 — fully in-session loop, model routing, DESIGN review, auto mode

**Status after amendment:** accepted. The superseded Consequences bullets above are marked inline.

**1. Claude Web leaves the loop.** The orchestrator (the main Claude Code session) is now also the
**cross-session strategic architect**. It keeps `ROADMAP.md` and `wiki/log.md` as the memory between
sessions, decomposes phases, and writes delegation prompts in a fixed shape: main risk first → steps
citing rules → checks with negative controls → wiki → commit → STOP-AND-REPORT. Design forks still
go to Anton with a lean. Claude Web is optional, only for an external review at a phase boundary.
Rationale: the relay was already gone (2026-08-27). What Web still contributed — independent design
review — is now a structural role (point 3), not a separate tool.

**2. Model routing — cost vs risk.** Frontmatter uses **aliases only**:

| Agent | Model |
|---|---|
| explorer | `haiku` |
| prober | `sonnet` |
| implementer | `sonnet` |
| verifier | `opus` |

The `claude-opus-4-8` pins are removed, because an alias follows the current model while a pin
silently ages. The implementer drops from `opus` to `sonnet`: most units are mechanical extractions
and wiring, with tests and an `opus` verifier behind them. For **high-risk** units — state machine,
concurrency, reconciliation, DB migrations, parsers with chemical consequences — the orchestrator
passes the **per-invocation `model: opus`**. Documented resolution order: invocation parameter →
frontmatter → `CLAUDE_CODE_SUBAGENT_MODEL` → main session model
(https://code.claude.com/docs/en/sub-agents). Judgement stays on `opus` where errors are expensive
and silent (verification, high-risk implementation); throughput work runs on cheaper models.

**3. Verifier DESIGN mode.** The verifier gains a second mode: a checklist review of every ADR or ADR
change **before acceptance**, report only (`.claude/agents/verifier.md`). Why it is needed: **design
defects are not caught by tests** — there is no code yet to test. The October 2026 review rounds
found, in order:
- a conclusion contradicting its own data (NEB "non-deterministic" at identical 42/129 attempts);
- in-memory queue state with no restart story;
- a cancel/start race;
- a local "cancelled" flag that bypassed the server source of truth;
- an unbounded re-enqueue.

Each became a checklist item. Without this mode, removing the external reviewer would silently
remove the only design review.

**4. Verifier worktree semantics, and the commit binding (sourced from the docs; a correction).** A
subagent with `isolation: worktree` branches **from the repository's default branch** unless
`worktree.baseRef` is `"head"`, and checks out **only tracked files**
(https://code.claude.com/docs/en/worktrees, "Choose the base branch"; "Isolate subagents with
worktrees"). So the verifier never saw the implementer's **uncommitted** change, and by default not
even local unpushed commits. Fix — a **mandatory** protocol (CLAUDE.md, `.claude/agents/verifier.md`):
1. `.claude/settings.json` sets `"worktree": {"baseRef": "head"}`.
2. In a **throwaway index** (`GIT_INDEX_FILE=<tmp>`: `read-tree HEAD` → `add -A`), the orchestrator
   builds the patch (`diff --cached --binary HEAD`, untracked files included) and the **expected tree
   hash** (`write-tree`). The main index is not touched.
3. The verifier checks its base commit, applies the patch, and requires `git add -A` + `git write-tree`
   to equal the expected hash, both at the start and again after its negative-control
   break-and-restore. It **reports the tree hash it verified**.
4. **Immediately before `git commit`**, the orchestrator recomputes `git add -A` + `git write-tree` in
   the main checkout and compares it with the verified hash. Any difference means the working tree
   changed after verification → **verify again**.

Why a tree hash and not `--stat` (DESIGN review, HIGH-2): `--stat` compares line counts, not bytes.
A stat built from the patch itself only proves the patch applied, not that it matches the tree that
gets committed. `write-tree` is a content hash of exactly the tree `git commit` will record, so equal
hashes mean equal bytes.

**5. Auto mode is allowed — with structural gates.** This supersedes the "run the main session in
default (manual) or plan mode" Consequence. Documented facts this rests on:
- *"When the main conversation is in `bypassPermissions`, `acceptEdits`, or auto mode, the subagent
  runs in that same mode and Claude Code ignores the `permissionMode` you set."*
  (https://code.claude.com/docs/en/sub-agents; also permission-modes, "How auto mode handles
  subagents": "Any `permissionMode` in the subagent's frontmatter is ignored.") → **no gate may
  depend on a subagent's `permissionMode`.**
- *"Rules are evaluated in order: deny, then ask, then allow … a matching ask rule prompts even when a
  more specific allow rule also matches"* (https://code.claude.com/docs/en/permissions).
- In auto mode, *"Actions matching your allow, ask, or deny rules resolve immediately"* before the
  classifier, and *"Explicit ask rules still force a prompt"*
  (https://code.claude.com/docs/en/permission-modes).
- The same decision order applies to subagents' actions (ibid.).
- Compound commands are split on `&&`, `||`, `;`, `|`, `&`, newlines; *"Deny and ask rules apply when
  any subcommand matches them"* (permissions).

Hence the gates are permission rules, which neither the classifier nor a subagent can bypass. *(The
rule set below replaces the first draft — `*git commit*`-only `ask` — after DESIGN review MED-3
showed commit-creating commands and spellings that draft missed.)*
- **`ask`** — every commit-creating or publishing git command, written so that wrappers (`bash -c`,
  env prefixes, `git -C <dir>`), quoting (`git 'commit'`) and inline aliases
  (`-c alias.ci=commit`) still match:
  - `Bash(*git*commit*)`, `*git*push*`, `*git*merge*`, `*git*cherry-pick*`, `*git*revert*`,
    `*git*rebase*`, `*git*pull*`;
  - `Bash(*git am*)`, `*git * am *`, quoted `am`;
  - `Bash(*git*alias.*)`.
- **`deny`** — privilege escalation: `sudo` in any position (`sudo*`, `* sudo*`, `*/sudo*`, quoted,
  `(sudo`), plus `su`, `pkexec`, `doas`.
- **`deny`** — **any command naming the admin ssh alias** (`Bash(*uni-admin*)`).
- **`deny`** — ssh-style logins as `yats`: `*yats@*`, `*User=yats*`, `*User yats*`, `-l yats` and its
  spellings.

**6. Managed settings — the gate file is not editable by the gated (DESIGN review HIGH-1; Anton chose
managed + a project layer).** A project `.claude/settings.json` can be changed by the agents it
gates, and the change applies at once. In auto mode, a write to `.claude/` is routed to the
classifier, not to a human (permission-modes, "Protected paths"), and settings hot-reload
(https://code.claude.com/docs/en/settings, *"reloads them when they change … including edits to
`permissions`"*). A gate that lives in such a file is a classifier judgement, not structure.

Sourced facts the fix rests on:
- Linux path **`/etc/claude-code/managed-settings.json`** (+ optional `managed-settings.d/`);
  *"Read at startup and reloaded when a file changes"*
  (https://code.claude.com/docs/en/managed-settings).
- *"no other level, including command line arguments, can override a managed permission rule"*; an
  `allow` saved locally *"doesn't outrank an `ask` rule from a project or managed file"*
  (https://code.claude.com/docs/en/permissions; settings).
- *"If the operating system denied the read, for example on a root-only file, every session starts
  without that source's policies"* (managed-settings) → the file must be **root-owned but
  world-readable (0644)**; a 0600 file would silently disable every gate.
- A settings file created mid-session is loaded only *"if its folder existed when the session
  started"* (settings). `/etc/claude-code/` did not exist → **restart the session after
  installing**. *Measured 2026-10-03 (Claude Code 2.1.288):* the managed file **was** applied in the
  running session **without** a restart, even though `/etc/claude-code/` was created mid-session (see
  the managed-layer controls below). A run beats the docs, but a restart after installing stays the
  safe default — the docs promise nothing else.

Decision:
- The `ask`/`deny` lists above live in **managed settings**, installed by Anton with `sudo`. **The OS
  boundary is file ownership plus the sudo password the agent does not have.** The `sudo` deny rule
  is only a text guardrail on top of that: it is bypassable like any Bash rule, e.g. `S=sudo; $S`.
  - *Partly measured 2026-10-03:* `sudo install …` run through `!` in the session shell (no tty)
    failed with *"a terminal is required to read the password … a password is required"*. So there is
    no `NOPASSWD` for those commands, and no usable ticket in that shell at that moment.
  - *Still open:* can a ticket that Anton has just created in another terminal be reused from the
    agent's tty-less Bash?

  So Anton **installs from a separate terminal, never via `!` inside the session, and ends with
  `sudo -k`**. (*Measured:* the `!sudo …` run reached sudo's password check despite the `Bash(sudo*)`
  deny, so the permission rules did not apply to a command Anton ran with `!`.)
- **Reference copy** of the managed file — `/etc/claude-code/managed-settings.json`, root:root 0644,
  in directory `/etc/claude-code` root:root 0755. Its sha256 is
  `dfc4d453dda14dc072451329f3c6cf856ff52e520c49840cc70ae8c10755189e` (the scratchpad staging copy
  is agent-writable, so Anton compares the hash before installing):

  ```json
  {"permissions": {
    "ask":  ["Bash(*git*commit*)", "Bash(*git*push*)", "Bash(*git*merge*)",
             "Bash(*git*cherry-pick*)", "Bash(*git*revert*)", "Bash(*git*rebase*)",
             "Bash(*git*pull*)", "Bash(*git am*)", "Bash(*git * am *)", "Bash(*git*'am'*)",
             "Bash(*git*\"am\"*)", "Bash(*git*alias.*)"],
    "deny": ["Bash(sudo*)", "Bash(* sudo*)", "Bash(*/sudo*)", "Bash(*'sudo*)", "Bash(*\"sudo*)",
             "Bash(*(sudo*)", "Bash(*pkexec*)", "Bash(*doas *)", "Bash(su)", "Bash(su *)",
             "Bash(* su *)", "Bash(*uni-admin*)", "Bash(*yats@*)", "Bash(*User=yats*)",
             "Bash(*User yats*)", "Bash(*-l yats*)", "Bash(*-lyats*)", "Bash(*-l 'yats'*)",
             "Bash(*-l \"yats\"*)"]}}
  ```

  This is the same list as the project file, minus its three `Edit(...)` denies. (The hash is of the
  pretty-printed staging file, not of this compact rendering.)
- The same lists are **mirrored** in `.claude/settings.json` as a project layer, so a clone without the
  managed file still gets them.
- The project layer adds `deny` on `Edit(/.claude/settings.json)`, `Edit(/.claude/settings.local.json)`
  and `Edit(~/.claude/settings.json)`.
  - **Sourced:** `Write(path)` rules are *"never consulted"*. `Edit` rules cover all built-in edit
    tools, plus Bash file commands *"such as `cat`, `head`, `tail`, `sed`, and `tee`"* and `>`
    redirections, but not *"a Python or Node script that opens files itself"* (permissions, "Read and
    Edit").
  - **Measured 2026-10-03:** a `cp` onto `.claude/settings.json`, and even a `cp` reading from it, was
    **denied** in this auto-mode session. Whether the `Edit` rule or the auto-mode classifier denied it
    is **undetermined** — the refusal text is the same.

  A script can still write the file, so the project layer alone would not be enough. Managed settings
  are what make the gate structural.
- **Managed is the source of truth for the gates.** The project copy must stay identical (DESIGN item 9).
- **Not adopted:** `allowManagedPermissionRulesOnly`. Sourced effect: it *"ignores `allow`, `ask`, and
  `deny` rules in user, project, local, and `--settings` files, ignores `--allowedTools`, hides the
  always-allow choices … and stops saving new rules"* (settings-reference). That would drop all ~130
  local `allow` rules (constant prompts in auto mode) and the project's `Edit` denies, while adding
  nothing to the gates: managed `ask`/`deny` already outrank every lower `allow`.

*Negative controls.*
- *Project layer (2026-10-03, this session):* `ssh uni-admin true`, `bash -c "ssh uni-admin true"`,
  `rsync … uni-admin:…` and `sudo -n true` were all **denied**; `sudo -n true` was denied despite an
  existing local `allow`, as documented. The positive control `ssh uni whoami` → `anton` is still
  allowed. A no-op `sed -i` and an `Edit` of `.claude/settings.json` were both **denied**.
- *Managed layer* — after Anton installs the file and **restarts** the session. (The 2026-10-03
  results below were taken **without** a restart; see the mid-session-load note above.) Because the project
  file mirrors the managed rules, "still denied" alone cannot tell the layers apart. So the managed
  layer is proven by:
  1. `/status` (or `claude doctor`) listing the managed source as loaded, with no read failure;
  2. **temporarily removing** the commit `ask` and the admin-alias `deny` from the project file — this
     is Anton's edit, since the project file is `Edit`-denied to agents — and showing that `git commit
     --dry-run` still prompts and `ssh <admin-alias> true` is still denied; then restoring the file
     byte-for-byte (sha256).

  **Results, 2026-10-03** — installed by Anton from a separate terminal: `/etc/claude-code`
  root:root 0755, `managed-settings.json` root:root 0644, sha256 `dfc4d453…189e` (= reference).
  1. **OS boundary:** the agent's `touch /etc/claude-code/managed-settings.json` → `Permission denied`.
  2. **Managed `ask` — proven.** `Bash(*git*commit*)` was removed from the project file. No other
     project, local or user `ask` rule matched `git commit --dry-run -m "managed ask control"`; the
     local file has no `ask` list. The local file does contain `allow` `Bash(git commit *)`, which would
     otherwise have run the command **silently**. The command **prompted** Anton (confirmed by him), so
     the prompt can only have come from the managed `ask` rule. This proves the managed layer was
     loaded.
  3. **Managed `deny` — consistent, not discriminating.** With the admin-alias rule removed from the
     project file, `ssh` to the admin alias was **denied**. In auto mode an unmatched command goes to
     the classifier, which might also block it, and the refusal text is the same. So this is
     *consistent with* the managed `deny`, but the layer is **undetermined** — the same caveat as the
     `cp` measurement above. To discriminate, re-run in default (manual) mode, where an unmatched
     command prompts instead of being denied. Not done.
  4. **Restore:** the project file was restored byte-for-byte; its sha256 equals the committed value
     `95e90074…1082b`. The first restore attempt failed: Anton copied a placeholder (`uni-…admin`)
     from the orchestrator's instructions, and the sha256 check caught it. Lesson: give humans exact
     rule text, never a disguised placeholder.

  `/status` was not captured.

*Known limits (honest) — the threat model is a cooperative agent making a mistake, not an adversary:*
- The Bash rules match **command text**. The docs: a deny or ask rule *"covers the invocation Claude
  usually produces and isn't a security boundary around the program"* (permissions). Forms that slip
  past, found by reasoning in DESIGN review round 2 and not executed:
  - split quoting or escapes: `git com''mit`, `git c\ommit`;
  - variables: `S=sudo; $S …`, `A=uni-; ssh ${A}admin`;
  - `echo commit | xargs git`;
  - plumbing that creates or publishes commits without the words: `git send-pack`,
    `git fast-import`, `git fetch . topic:main`;
  - `ssh -l "$U" uni`;
  - a `Host` stanza appended to `~/.ssh/config`;
  - script files.

  Managed settings close the bypass for the **gate file**, but not for the commands themselves.
- Auto mode's classifier **allows by default** *"Pushing to any branch of the repository you're
  working in"* (permission-modes). A push the patterns miss gets no second check.
- **A mod can override the merge-approval `ask` — even with managed settings** (sourced,
  permissions "Extend permissions with hooks"): a mod handling `tool.check` *"answers after the rules
  and the `PreToolUse` hooks have decided, and its answer can replace theirs"*, including *"the mod can
  approve a call that an ask rule would prompt for"*. In auto mode such a call *"runs without a
  classifier check"*. Managed **deny** rules *"hold over the mod by default"*, but managed **ask**
  rules do not. Commit/push approval is an `ask`, so **installing a mod that handles `tool.check`
  silently removes the central human gate**. Measured 2026-10-03: no such mod is installed — the
  plugins are `vercel-plugin` and `rust-analyzer-lsp`, and none has a `tool.check` handler. Rule:
  **no mod is installed without Anton re-running this check.** Option for Anton (fork, not adopted
  here): the managed-settings controls over mods ("Manage mods for your organization", not yet read).
- **Hooks** are shell commands that Claude Code itself runs, outside the Bash tool. That the Bash
  `ask`/`deny` rules do not gate them is **our inference**. The docs say only that hook *decisions*
  cannot override the rules: *"PreToolUse hook decisions don't bypass permission rules. Claude Code
  evaluates deny and ask rules regardless of what a PreToolUse hook returns"* (permissions, "Extend
  permissions with hooks"). Settings files reload live, so an agent that manages to
  write one (a script; see the cp measurement above) would gain an execution channel.
  - Project scope has no hooks today.
  - **Plugin scope does:** `vercel-plugin` 0.24.0 runs hooks; a `SubagentStart` hook injected
    context into the round-3 verifier (measured).
  - Option: `allowManagedHooksOnly` in the managed file — *"Restrict hook execution to hooks your
    organization deploys"* (settings-reference). It would **also switch off the plugin's hooks**,
    unless managed settings force-enable that plugin. **Design fork for Anton — not adopted here.**
- The admin alias uses the **same key** as `uni`. The real boundary for admin access would be a
  separate, passphrase-protected key, not loaded into the agent, or the Bash sandbox. **Design fork
  for Anton — not done.**
- The patterns over-match on purpose. `git merge-base`, `git stash push` and `git log --grep commit`
  prompt. Any Bash command whose text names the admin alias — even a `grep` of the wiki — is denied,
  so agents search for it with the Read tool or a regex that does not spell the alias out. (The Grep
  tool is not always available to subagents — measured in round 2.)

**7. Gates that remain prose (DESIGN review LOW-12, stated honestly).** These depend on the
orchestrator's judgement or a prompt, not on a mechanism:
- giving high-risk implementer units `model: opus` — the orchestrator decides what is high-risk;
- the verifier's model — a per-invocation `model` overrides the frontmatter (sub-agents,
  model-resolution order);
- the verifier's DESIGN-mode "no edits" rule — it keeps `Edit` and `Bash` for CODE mode;
- the "two FAILs in a row → escalate to Anton" bound (CLAUDE.md);
- the tree-hash comparison before commit and the `HEAD^{tree}` assertion after it (§4, CLAUDE.md) —
  mandatory steps the orchestrator runs, not a hook. A `git commit -a` / pathspec commit, a
  still-running agent's `git add`, or a future index-rewriting pre-commit hook would make the
  committed tree differ. The post-commit assertion **detects** that; it does not prevent it.
  **Design fork for Anton:** commit the verified tree directly (`git commit-tree <hash>`) to prevent
  it instead.

They are acceptable because a miss shows up downstream (in the verifier report, or at the `ask`
prompt for the commit). None of them is a gate.

**8. What a subagent actually starts with — CLAUDE.md sourced + measured; agent definitions an open
question.**
- **CLAUDE.md (sourced + measured):** *"A subagent in its own worktree takes the instruction files it
  starts with from your main conversation, not from its worktree"* (worktrees). Round 2 measured it:
  the verifier received the **session-start** CLAUDE.md (`ce610f7`-era: "vocabulary is these seven",
  "run … not auto"), although HEAD and the working tree had moved on. **In-session CLAUDE.md edits do
  not reach subagents until the main session restarts.**
- **Agent definitions (open question):** the docs say *"the next delegation uses the updated
  definition, with no restart needed"* (sub-agents). Measured on 2026-10-03:
  - round 1 got the **pre-edit** `verifier.md` body about 100 s after an edit;
  - round 2 got an **intermediate** body (DESIGN mode present, tree-hash binding absent) at least
    3 min after a later edit;
  - the round-2 verifier's tools lacked `Grep`/`Glob`, which its frontmatter lists.

  A run beats the docs (rule #10), and the mechanism is unexplained.

Consequences:
- **every delegation restates in its prompt the mode, protocol and rules it relies on**;
- **after editing CLAUDE.md or an agent definition, the main session is restarted before a delegation
  that depends on the edit**;
- a prompt cannot carry frontmatter (`tools`, `model`, `isolation`), so those are re-checked by asking
  the agent to report its tool list.

**Human gates unchanged:** merge approval (now enforced by managed `ask` rules, provided no
`tool.check` mod is installed — §6), the live WebKitGTK gate, the chemistry gate, design forks.

## Open questions

**Recorded 2026-10-03 — gate-hardening forks, deferred.** Five items left open by the §6–§7
amendment and the managed-layer probe. Each is a design fork for Anton; none is decided or
implemented here, and the gates in §5–§6 stand as they are until one is.

1. **Admin-access boundary.** The admin ssh alias uses the same key as `uni`, so only the text
   `deny` rules keep agents off it (§6, known limits). Options: a separate passphrase-protected key
   that is never loaded into the agent's ssh-agent, or the Bash sandbox. Open measurement in the
   same area: can a sudo ticket Anton just created in another terminal be reused from the agent's
   tty-less Bash (§6)?
2. **`allowManagedHooksOnly`.** It would close the hook execution channel (§6, hooks), but it also
   switches off `vercel-plugin`'s hooks unless managed settings force-enable that plugin. Related
   loose end: the managed **deny** layer is still undetermined; it needs the discriminating re-run in
   default (manual) mode (§6, result 3).
3. **`git commit-tree <verified hash>`** instead of today's detect-after (§7): commit exactly the
   tree the verifier reported, so a `-a`/pathspec commit or an index-rewriting hook cannot change
   it, rather than only being caught by the post-commit `HEAD^{tree}` assertion.
4. **Mods.** A `tool.check` mod can approve what a managed `ask` would prompt for (§6). Today this
   holds only by the prose rule "no mod without Anton re-checking". Option: the managed-settings
   controls over mods ("Manage mods for your organization" — not yet read, so not sourced).
5. **Broad `allow` rules in `.claude/settings.local.json`** (Anton's file; agents have `Edit`
   denied on it). Allow rules resolve before the auto-mode classifier, so these run with no
   second check:
   - interpreter wildcards — `Bash(node *)`, `Bash(.venv/bin/python *)`,
     `Bash(sidecar/.venv/bin/python -c ' *)`: a script can write any file the user can, including
     the project and local settings files that the `Edit` deny does not cover against scripts (§6);
   - work-discarding git — `Bash(git checkout *)`, `Bash(git stash *)`, `Bash(git branch *)`;
   - `Bash(git commit *)`, `Bash(git merge *)` — harmless while the managed `ask` outranks them, but
     they would silently re-open the gate if the `ask` ever failed to load;
   - `Bash(sudo -n true)` — already overridden by the `deny`, dead weight.
   - and others of the same classes, e.g. package installs and run scripts that execute arbitrary
     code (`Bash(.venv/bin/pip install *)`, `Bash(npm install *)`, `Bash(npm run *)`,
     `Bash(npm create *)`), two more `git commit -m …` patterns, and name-based kills
     (`Bash(pkill -KILL -x orca)`, `Bash(pkill -KILL -x mpirun)`) — the "never `pkill` by name" class
     ADR-024 k cites. The list above is a sample, not an inventory.

   Option: prune to read-only and test commands; the wildcards go back to the classifier.
