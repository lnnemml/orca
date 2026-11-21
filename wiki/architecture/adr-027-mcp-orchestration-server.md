# ADR-027: The MCP orchestration server — T3, Stage 1

**Status:** Accepted · 2026-11-19 (forks decided by Anton 2026-11-19; accepted by Anton the same day)
- DESIGN round 1: FAIL (4 HIGH + 12 MED + LOW). All findings applied.
- Four follow-up forks were decided by Anton the same day: the SMILES and molecule sources, the
  cancel scope, TightOpt, and data leaving the machine.
- DESIGN round 2: PASS WITH FINDINGS (3 MED + 8 LOW). All applied. Two of them were decided by
  Anton: the grant keeps a stored charge counter, and it does not limit the run type.
- DESIGN round 3: PASS WITH FINDINGS (1 MED + 4 LOW). All applied. Anton decided the MED: a job
  still in step B counts as in flight.

Opens tier **T3 orchestrate** of [ADR-014](adr-014-ai-integration-boundary.md) (2). It keeps every
invariant of ADR-014:
- **(1)** the AI never produces a number that lands in an input's coordinates;
- **(1a)** geometric constants are retrieved and never recalled. Its own text extends this to scan
  windows, step sizes and point counts;
- **(3)** methodology is a guard in code, not a prompt;
- **(4)** every compute-spending operation has a pollable path;
- **the charge/multiplicity amendment**: neither value is ever set silently by the AI.

ADR-014 is narrowed here, not edited. An amendment pointer is appended there, as was done for ADR-015.

## Context

### The goal

Anton's goal after Phase 5 is to do real research **together with an AI assistant driving the app**.
In his words (2026-11-19), the advantage is that the assistant does not sleep. It can watch the job
queue in real time, start the next jobs when results arrive, and help design the study from real
results.

He also framed it as an experiment. As far as he knows, nobody has done this. Stage 1 is the
baseline from which more autonomy will be tested later, with the agent's behaviour as the data.

### The worked example

Anton's plan for re-computing the sultone cyclization mechanism ("Перерозрахунок механізму циклізації
сультонів — план", 2026-09-30, a document outside the repo) shows what the assistant has to do. Its
stages are:

- **Stage 1 — minima.** A conformer search (GOAT) for eight species. Then Opt + Freq at
  ωB97X-3c + CPCM(DMF) for every conformer within 3 kcal/mol. The gate: no imaginary frequency in
  any minimum.
- **Stage 2 — transition states.** Relaxed scans with stated ranges (C···C 3.4 → 1.6 Å, C–OMe 1.45
  → 2.8 Å). Then NEB-TS where a scan has a maximum, OptTS with an exact Hessian, and IRC from every
  TS with both ends optimized.
- **Stages 3–5.** Competing pathways (E1cb, attack on S), K⁺ ion pairs in two solvents, and the
  Thorpe branch.
- **Stage 6.** Single points at ωB97M-V/ma-def2-TZVP + SMD on every stationary point, and a
  DLPNO-CCSD(T) benchmark.

The plan also lists the original thesis's errors. Two of them are method errors a guard can catch:
**"ωB97X-3c + def2-QZVP"** (a composite method with a foreign basis) and **ALPB used for DFT** (an
xTB solvation model).

**Most of the compute is chaining**: take a geometry from a finished job, write a new method header,
run the next job. Three kinds of decision bring in **a new geometric number**: a scan range, a
constraint value, and placing K⁺. ADR-014 (1a) forbids the model from recalling those. The split
between chaining and new numbers is the spine of this ADR. Bringing in **a new species** is a third
kind of decision, whose charge and spin someone has to confirm.

ADR-014 (2) gated T3 on Phase 4.5 because "there is nothing to orchestrate" while the Job is the
central object. Phase 4.5 is now core-complete (ROADMAP), and the example shows that chaining Jobs
already carries most of a real study. Orchestrating at the level of a reaction (si/re approaches,
scans, ΔΔE‡) is Stage 2 (decision 10).

### The code this rests on (anchors at `4f0062a`)

**The DB and the queues.**
- There is one DB connection, `DbState(pub Mutex<Connection>)` (`commands/settings.rs:14`).
- Transactions are deferred `unchecked_transaction`s, for example `ssh_backend.rs:419`. No
  `busy_timeout` is set.
- The local queue lives in SQLite and has one run slot (`JobRunner`, `local_backend.rs:64`).
- The remote poller is one thread (`poller/app.rs:116`).

**Job statuses and submission.**
- `models/job.rs:22–45` defines `Draft`, `Queued`, `Running`, `Cancelling`, `Completed`, `Parsed`,
  `Failed`, `Cancelled` and `Lost`.
- Only a `Draft` can be submitted, locally (`local_backend.rs:382–389`) and remotely
  (`ssh_backend.rs:420–425`).
- The remote `persist_submit` takes `&DbState` and **locks it itself** (`ssh_backend.rs:417–419`).
  So a caller cannot hold the lock across it.
- `submit_to_server` runs under a per-job in-flight guard (`remote_jobs.rs:120`, `:144`).
- A job becomes *pending* only on a slot refusal (`makes_pending`, `ssh_backend.rs:534`).
- A failed upload leaves the job `queued`, labelled "not on the server (retry or cancel)"
  (`ssh_backend.rs:276`). Recovery is a user Retry or a cancel (`poller/cancel.rs:14–15`).

**Pollable paths.**
- `get_job` (`commands/jobs.rs:746`; its core `get_job_conn` is `pub(crate)`).
- `read_job_results` (`results.rs:1341`, `pub`).
- `remote_job_state` (`remote_jobs.rs:249`).
- `xtb_optimize` and `crest_grow` are event-only (`xtb.rs:682`, `crest.rs:282`).

**Building inputs.**
- Rust has no input builder on any app path. The `* xyz` block is built in TypeScript.
- `orcastudio-core/src/emit.rs:58` `emit_coordinate_block` is a byte-identical twin of the TypeScript
  emitter. Only tests call it.
- The derived-input builders inherit charge and multiplicity through `sceneFromOrcaInput`
  (`src/scene/reopt.ts:59–62`). Rust has no counterpart.

**Checks that live only in TypeScript**, which a Rust-side caller would bypass:
- The constraint index must be in range. The human path **blocks both Create and Create & Run**
  (`NewJobScreen.tsx:1372–1376`).
- A scan needs `! Opt` (`scan.ts:367`); the human path blocks it at `NewJobScreen.tsx:1380`.
- The content must not be empty (`preflight.ts:25–30`).
- Electron parity is checked only for information (`parity.ts:48`).

**SMILES and molecules.**
- The sidecar's `/smiles-to-3d` **always returns multiplicity 1** (`sidecar/app/smiles.py:25`,
  `:60`).
- `molecules.multiplicity` defaults to 1 (`db.rs:216`).

**What reaches the UI.**
- `create_job` emits nothing. The Jobs screen reloads on `job:status` (`JobsScreen.tsx:192`).

**Network.**
- The Rust process has no inbound listener and is HTTP-client only (`ureq`).
- The webview calls the sidecar's routes; Rust does not.

**Several app instances can run at once.** The sidecar port is chosen dynamically "so multiple app
instances don't clash" (`sidecar.rs:5`).

**Geometry math** already exists in Rust: distance, angle and dihedral (`xtb::distance`,
`xtb::angle`, `xtb::dihedral`). The dihedral "matches ASE / `measure.ts`" (measured false in M2b: it
returns 360° − ASE; [mcp-transport-probe.md](mcp-transport-probe.md), "M2b facts").

### Facts about MCP clients

Each fact below is **sourced** from the vendor's documentation. Every quote was checked against the
live page on 2026-11-19. When written, nothing here had been run; M0 has since measured them
([mcp-transport-probe.md](mcp-transport-probe.md), and the M0 amendment at the end).

- **SSE is deprecated** — [code.claude.com/docs/en/mcp](https://code.claude.com/docs/en/mcp): *"The
  SSE (Server-Sent Events) transport is deprecated. Use HTTP servers instead, where available."*
- **The tool-call limit is hard** — same page: *"The per-server `timeout` is a hard wall-clock limit
  per tool call, and progress notifications from the server don't extend it."*
- **The default limit is about 28 hours** — same page: *"…or to its default of about 28 hours when
  that variable is unset."*
- **Large results go to a file** — same page: *"Claude Code saves a successful result with no image
  content to a file once it's longer than 50,000 characters, whatever its token count."* The page
  qualifies this: it applies to tools that do not declare `anthropic/maxResultSizeChars`, and to
  foreground calls.
- **Per-server permission rules** — [code.claude.com/docs/en/permissions](https://code.claude.com/docs/en/permissions):
  *"To match every tool from an MCP server, write `mcp__<server>__*`."*
- **Tool annotations are hints only** — [MCP spec 2025-06-18, schema](https://modelcontextprotocol.io/specification/2025-06-18/schema):
  *"Clients should never make tool use decisions based on ToolAnnotations received from untrusted
  servers."*
- **Remote connectors must be public** — [support.claude.com 11175166](https://support.claude.com/en/articles/11175166-get-started-with-custom-connectors-using-remote-mcp):
  *"This means your MCP server must be reachable over the public internet from Anthropic's IP
  ranges."* So a localhost HTTP server is not a connector for the desktop app.
- **Local HTTP servers must check `Origin`** — [MCP spec, transports](https://modelcontextprotocol.io/specification/2025-06-18/basic/transports):
  *"Servers **MUST** validate the `Origin` header on all incoming connections to prevent DNS
  rebinding attacks"*.

**Not established, and left to M0 or later:**
- that Claude Code talks to a stdio server the way this ADR assumes (M0 measures it);
- whether the `rmcp` crate can serve over a Unix socket (Open question a);
- how auto mode treats MCP calls (Open question d);
- whether the Claude desktop app works with the relay. The desktop app is out of Stage-1 scope.

## Decision

### (1) One server, inside the app process, off by default

The MCP server runs **inside the OrcaStudio process**. It shares `DbState`, `JobRunner` and the
poller with the UI, so inside one process there is no second writer. Several app instances can share
the DB file, and that is an existing condition (`sidecar.rs:5`). Across instances, ordering rests on
SQLite's file lock. So every transaction this ADR adds that reads and then writes a grant starts
`BEGIN IMMEDIATE`. A `SQLITE_BUSY` from another instance becomes a tool error the model may retry.
The server itself never retries.

The server is **off by default**. Settings → MCP turns it on, and the setting persists. While it is
off, no socket exists.

The handlers run on the server's threads, never on the GTK main thread, which keeps the
`tauri-core.md` threading rule. A handler holds the DB lock for one query or transaction at most.
It never holds it across ssh or a wait. At most **4 connections** are served at once. Each
connection has at most 4 requests in flight (M0 amendment, item 5, which replaces the earlier
"one at a time").

### (2) Transport: a stdio relay into a Unix socket that only Anton's user can open

**Not HTTP.** An HTTP port can be reached by every local process and every browser tab, so the spec's
`Origin` check and a bearer token would become the whole boundary. And the desktop app cannot use a
localhost HTTP server as a connector (sourced, above).

**The socket.**
- **Location.** The app holds an exclusive `flock` on `orcastudio/mcp.lock` in `$XDG_RUNTIME_DIR`.
  It is released **only at process exit** (qualified by the M0 amendment, item 4: the exit of the
  last process holding the fd), never when MCP is switched off. That way no other instance
  can take the lock and reconcile while a step B here is still running. Only the instance holding the lock may unlink a stale `mcp.sock` and bind.
  An instance that cannot take the lock does not serve, and its Settings → MCP says so. This rule
  replaces a check-then-act on the socket file, which two instances starting together could race.
- **Permissions.** The directory is created `0700` and the socket `0600`.
- **Peer check.** On every accept, the app reads `SO_PEERCRED` and drops a peer whose uid differs
  from its own. That check is defence in depth.
- **What still needed measuring** (now measured: M0 amendment, item 2). Whether Linux enforces the
  socket file's mode on `connect`, and the `XDG_RUNTIME_DIR` facts. M0 measures both, with a negative control:
  `chmod 000` must give `EACCES` (Open question c).
- **Fallback.** If `XDG_RUNTIME_DIR` is unset, the app uses the app data dir with the same modes. If
  that path would exceed the `sun_path` limit, MCP refuses to start and says why.
- **Exit.** A clean exit first closes the task tracker (the gate: M2 amendment, item 4) and waits for every `create_job` task (M0
  amendment, item 6; each is bounded by its transactions and its own ssh timeouts). Only then does it
  unlink the socket and release the lock. So no other instance can reconcile a job this process is still submitting.

**The relay.** The MCP client launches **`orcastudio --mcp`** (the binary's real name; M2 amendment, item 3), the same binary, which in this mode
never opens a window. It is a **dumb byte relay**: stdin → socket, socket → stdout. It holds no
logic, so no guard lives in it. If it cannot connect, it writes one line to stderr ("OrcaStudio is
not running, or its MCP server is off") and exits non-zero.

**Lifecycle.**

| event | effect |
|---|---|
| the client session ends | the relay sees stdin EOF and closes the socket. The server sets that connection's `closed` flag. A waiting or reading handler sees the flag on its next tick and ends; handlers are not cancelled (M0 amendment, items 6 and 10) |
| the app exits, or MCP is turned off | the listener and every connection close: the server sets each connection's `closed` flag and cancels its service (M0 amendment, item 10). Switching off keeps the `flock`. A `create_job` already past its charge step finishes its order of operations (decision 5): its task runs on, the tracker refuses new ones (the gate: M2 amendment, item 4), and a clean exit waits for them before releasing the lock ("Exit" above). The relay sees socket EOF, exits, and the client shows the server as disconnected |
| the app restarts | the dependable recovery is a new client session (measured, M0 amendment, item 3). Jobs, grants and the audit log live in SQLite and survive. Only the per-connection read counters are lost |

### (3) Three dispositions, chosen by a pure Rust classifier

Every `create_job` call ends in exactly one of three outcomes:

| disposition | what happens | who spends compute |
|---|---|---|
| **REFUSED** | nothing is created; the tool returns the reasons | nobody |
| **PROPOSED** | a `Draft` job with `origin = 'mcp'` and its reasons; Jobs shows it with a "Claude proposes" badge and the reasons | Anton, by pressing Run (the existing draft path) |
| **SUBMITTED** | the job is created and submitted to the requested target under the active grant | the grant |

The disposition comes from **`classify_mcp_job`**, a pure function. Its inputs are:
- the parsed header;
- the resolved source;
- the target;
- the memory verdict, computed before step A (decision 4);
- the active grant and its counters;
- the number of open proposals.

**The header grammar is closed.** Every line is one of: a `!` line, a `%` block through its `end`,
a `#` comment, or blank. Nothing else is accepted.

**REFUSED rows.** These mirror or exceed what the human path refuses (ADR-014's amendment: the AI
path refuses where the human path warns):

- **R1** — a line outside the grammar. This includes `$new_job` and any second job.
- **R2** — coordinates in the text: any `*` geometry block (`* xyz`, `* int`, `* gzmt`,
  `* xyzfile`), or a `%coords` block.
- **R3** — `%pal`, `%maxcore` or a `! PAL<n>` token. Compute shape belongs to the app (ADR-025;
  ADR-024 o26). ADR-025 records as unmeasured which of `! PALn` and `%pal` ORCA honours.
- **R4** — the source does not resolve, is not terminal-successful, or is an optimization with
  `converged == Some(false)` (`results.rs:252`).
- **R5** — the source's charge and multiplicity are parity-impossible.
- **R6** — a constraint atom index is out of range for the source. This mirrors
  `NewJobScreen.tsx:1394–1400`, where a draft "would be an un-runnable landmine".
- **R7** — a `%geom Scan` without an Opt-family token. This mirrors `scan.ts:367` and
  `NewJobScreen.tsx:1402–1406`.
- **R8** — 50 open MCP proposals already exist, so Jobs cannot fill with unrun drafts.
- **R9** — a NEB-family token (`NEB`, `NEB-TS`, `NEB-CI`). A NEB needs two endpoints, and
  `create_job` takes one geometry source. Anton runs a NEB from the app's builder (N2). This is a
  limit of the tool's shape, not a claim about ORCA.

**PROPOSED rows.** Every row that matches is listed, so Anton sees all the reasons at once:

- **P1** — the source is `smiles:` or `molecule:`. **A new species enters the study by Anton's
  click** (decided by Anton). The draft shows the charge and multiplicity with their source. For a
  SMILES source it adds that the sidecar always gives multiplicity 1. Anton reads them and either
  runs the draft or leaves it. No draft-edit path exists: a job's input is fixed at creation
  (`NewJobScreen.tsx:1397`). A different charge or spin means Anton builds that job himself in New
  Job. Every later job chained from a job Anton ran inherits its charge and multiplicity.
- **P2** — a block outside the allowlist, or a key outside its block's allowlist (decision 6). That
  covers every model-chosen geometric number, such as a `Scan` or a constraint value, and every file
  path.
- **P3** — a Freq-type job (`Freq` or `NumFreq` without an Opt-family token) whose source is not a
  converged optimization **at the same method signature**. That covers scan frames, NEB images,
  SMILES and molecules (decided by Anton: same level, no TightOpt requirement in Stage 1). It also
  covers an optimization run with `%geom` constraints or a scan, because a constrained optimization
  is not a stationary point (decided by Anton 2026-11-19, M1b). It also covers a `%geom` holding any
  key other than `Calc_Hess`, `Recalc_Hess` or `MaxIter`: the resolver cannot tell such a source is
  a stationary point, so it fails closed.
- **P4** — the memory preflight (`commands/memory.rs:76` `memory_estimate_with`) warns for the
  target after the app's own `%maxcore`, **or** the target's memory facts are unknown. In that case
  a single reason names the remedy for either target: re-run the connection test for a server, or
  check that this machine's `/proc/meminfo` and data directory can be read (M1b; exact text in
  `modules/mcp-server.md`).
- **P5** — the grant does not cover the job:
  - there is no active grant, or it has expired or been revoked;
  - the target is not in the grant;
  - the method signature is not one of the grant's levels;
  - the grant's `max_jobs` is used up;
  - its `max_in_flight` is reached.

**SUBMITTED** — no row matched.

**The model cannot change a disposition.** No tool argument says "submit anyway", and no tool
creates, extends or reads around a grant.

### (4) Geometry, charge and multiplicity come by reference; the app assembles the input

**What `create_job` takes:**
- a **header**: the grammar above;
- a **geometry source**;
- a **target**: Local or a server profile id;
- an optional group and title.

It has **no parameter for coordinates, charge or multiplicity**. ADR-014's provenance rule is the
tool's shape.

**Geometry sources:**
- `job:<id>` — that job's `final_geometry`;
- `job:<id>/scan:<k>` — scan frame k (`read_scan_geometries`);
- `job:<id>/neb:<k|ts>` — a NEB image or the TS (`read_neb_geometries`);
- `job:<id>/goat:<k>` — GOAT ensemble member k (`read_job_ensemble`);
- `molecule:<id>` — always P1;
- `smiles:<s>` — the sidecar's `/smiles-to-3d`, which Rust calls with `ureq` (always P1).

**Charge and multiplicity:**
- **From a job.** Taken from the source job's own `* xyz` line, by a **Rust port of
  `sceneFromOrcaInput`'s locator**. A shared-fixture parity test pins it to the TypeScript one.
- **From a molecule.** Its stored columns.
- **From SMILES.** The sidecar's charge, and multiplicity 1.
- **How it is recorded.** The response names the source, and the input carries an
  `# charge/mult from <source>` comment.

**`%maxcore` is set by the app, never silently:**
- **Local.** `local_fitted_maxcore_with`.
- **A server with known memory facts.** P from the 5.5 preflight.
- **A server with unknown memory.** 2000, the builder's server default (unit 5.5 Part B), and the job
  goes to P4.

The response reports it as "set by the app", and the stored input shows it.

`%maxcore` and the memory verdict for P4 are computed **before step A**, outside any lock. The reason
is that `memory_estimate_with` and `local_fitted_maxcore_with` lock `DbState` themselves
(`commands/memory.rs:85`, `:121`). The classifier receives the verdict. M1b owns this.

**How the input is assembled:**
1. The model's header.
2. The app's `%maxcore`.
3. The coordinate block from `emit_coordinate_block`.

**The post-condition (rule #9).** The assembled input is re-parsed and must give back the source's
atom count, element order, coordinates, charge and multiplicity. A mismatch is a REFUSED internal
error.

Atom indices are **0-based**, as in ORCA's `%geom` (`wiki/orca/constraints.md`). The tool
descriptions say so.

### (5) The grant, and the order in which a SUBMITTED job is charged and submitted

**What a grant is.** A grant is a row Anton creates in Settings → MCP (until M4 builds it, by hand through the same
Rust command: M3 amendment, item 2). Its fields are:
- **Targets.**
- **Levels** — method signatures, decision 6. Anton adds one either with "this job's level" or by
  pasting a `!` line and a solvation block.
- **`max_jobs`** and **`max_in_flight`.**
- **`expires_at`** — on the laptop's clock. No server time is involved, so ADR-024 (j) does not
  apply.
- **`created_at`** and **`revoked_at`.**

**One grant at a time.** At most one grant is unrevoked. An **expression** partial unique index
enforces this: `CREATE UNIQUE INDEX … ON mcp_grants((revoked_at IS NULL)) WHERE revoked_at IS NULL`.
The column form `ON mcp_grants(revoked_at) WHERE …` would enforce nothing, because NULLs are distinct
in a UNIQUE index (measured with SQLite 3.45.1 in DESIGN round 3). Creating a new grant revokes the
previous one in the same `BEGIN IMMEDIATE` transaction. The *active* grant is the unrevoked one whose `expires_at` is
in the future. The grant create and revoke commands are Rust, owned by M1b.

**How the counters are defined (decided by Anton).**
- **`max_jobs`** is checked against a stored counter, `mcp_grants.charged_jobs`.
  - Step A increments it.
  - Only a step-C refund decrements it.
  - Deleting jobs never re-opens the budget.
- **`max_in_flight`** counts **every** job that is not terminal and has a non-null `grant_id`, under
  any grant (decided by Anton). That includes a `Draft` in step B, so concurrent connections cannot
  overshoot the cap. A refunded draft has `grant_id = NULL` and does not count. In-flight work is
  load on the machines, so a new grant does not get around jobs still running under an old one.
- **Run type is not limited** (decided by Anton). A DFT GOAT costs one job, the same as an SP. The
  core-hour budget remains the follow-up.

**Order of operations for a SUBMITTED job.**
- **Step A** — a `BEGIN IMMEDIATE` transaction under the mutex. It classifies the job, checks the
  grant and its counters, and inserts the job as `Draft` with `origin = 'mcp'`, `grant_id = g` and
  `mcp_state = 'submitting'`. Then it commits, releases the lock and emits `job:status`.
- **Step B** — no lock is held. Step B calls the existing path: local `submit`, or remote
  `submit_to_server` under its in-flight guard. `persist_submit` takes the lock itself.
  - **The window.** While `mcp_state = 'submitting'`, the `submit_job` **command** refuses the job,
    and the UI shows "Claude is submitting" instead of Run. So Anton cannot submit it during the
    window. This is a guard in the command, not in the backends.
- **Step C** — a transaction that **re-reads the row** and always clears `mcp_state`. What else it
  does depends on the **row's state**, not on step B's outcome type:

  | row state at step C | step C |
  |---|---|
  | still `Draft` with `grant_id = g` | **refund**: set `grant_id = NULL` and decrement `charged_jobs`. Add the reason "submit refused: <message>". The job becomes a normal PROPOSED draft |
  | left `Draft` (queued, pending, running, or any later state) | no refund. This covers every outcome after the claim: success, pending (a slot refusal, the o23 path), "not on the server", `FailedAfterClaim` ("submit interrupted", which can only be cancelled), `Unknown`, and the `KillUserProcesses` refusal. The job stays charged. Recovery for the stuck cases is Anton's Retry, or a cancel by Anton or Claude. The panel and the audit log show it |
  | the row is gone | nothing to do. The counter keeps the charge |

- **The remote step B takes time.** It makes several ssh calls and an rsync, each bounded by its
  own timeout.
- **A crash between A and C.** A reconciliation turns every `Draft` with `mcp_state = 'submitting'`
  into PROPOSED: refunded, with the reason "interrupted before submit". Every non-`Draft` row with
  that flag only has the flag cleared.
  - **Only the lock holder runs it.** It runs only in the instance that holds the `mcp.lock` `flock`,
    right after it takes the lock and before it serves. An instance without the lock never touches
    `mcp_state`, so it cannot refund a job that the serving instance is still submitting.
  - **Retaking the lock.** An instance with MCP enabled that does not hold the lock retries it every
    30 s. Taking it starts the reconciliation and then serving. (Replaced by the M2 amendment, item 2: one
    30 s tick per instance, which also starts a switched-off holder.)
  - **Bounds.** The reconciliation runs once per acquisition and never retries a submit. (An
    acquisition counts as done only when its reconciliation commits; a failed one is retried on the
    next tick: M2 amendment, items 2 and 6.)

**Revoke.** A revoke is a `BEGIN IMMEDIATE` transaction. Whichever of a step A and a revoke commits
first wins: a job charged before the revoke proceeds, and none is charged after it. Revoking does
not cancel running jobs.

**When Anton runs a PROPOSED job.** It is not charged to any grant, and it keeps `origin = 'mcp'`.
Claude may cancel it (decided by Anton). The panel's "Cancel Claude's jobs" acts on every
`origin = 'mcp'` job in `queued`, `running` or `cancelling`, through `cancel_job_core` per job. A
`Draft` cannot be cancelled (`local_backend.rs:563–566`).

**Core-hours are deferred.** A core-hour budget needs a remote wall time from the server alone,
because ADR-024 (j) forbids comparing the laptop and server clocks. Stage 1 bounds compute by count
and concurrency instead. The core-hour budget is a named follow-up.

### (6) Method signature and block allowlists — conservative by default

Anything these rules do not recognize goes to Anton as **PROPOSED**. It is never silently admitted.
That is what makes them safe with a mediocre model, and it means they claim no new ORCA fact
(rule #10): every admitted key cites a run record, and an unknown key routes to a human.

**Method signature.**
- **How it is built.** It is the case-folded, sorted set of `!` tokens, minus a closed list of
  non-method tokens. The normalized text of an admitted `%cpcm` block is added.
- **The non-method list** is a Rust copy of `NON_METHOD` (`src/reactions/compare.ts:626`): run
  types, the `*Opt` presets, the SCF thresholds and print control. A parity test pins the two lists
  equal.
- **What it catches.** "ωB97X-3c def2-QZVP" cannot match a "ωB97X-3c" level, which was exactly the
  thesis's error.
- **No divergence risk.** This function is not the ΔΔ table's `methodSignature`. A divergence
  between them can only send a job to Anton; it cannot weaken this guard.

**Key allowlist per block.** A key is admitted only with a run record. Any other key, any value that
is a file path, and any block not listed here → P2.

| block | admitted keys | record |
|---|---|---|
| `%geom` | `Calc_Hess` | `orca/optts.md` (the DA OptTS run) |
| `%geom` | `MaxIter` | `orca/gotchas.md` (honoured from 6.1.1) |
| `%geom` | `Constraints` with **Cartesian** `{C n C}` entries only | `orca/constraints.md:108–111` |
| `%cpcm` | `smd`, `SMDsolvent` | `orca/solvation.md` (form 2) |

**What stays out until a probe records it.** Bond, angle and dihedral constraints, with or without a
value, are not admitted: only explicit-value entries were run, and an explicit value is a geometric
number. Nor are `Recalc_Hess`, `TS_Mode`, and every key of `%scf`, `%irc`, `%freq`, `%goat` and
`%basis`. ADR-014 (1a) counts step sizes as geometric constants, and an unrecorded key could set the
spin state or a file.

`! IRC`, `! Freq`, `! OptTS` and `! XTB GOAT` with no block need no key, so they are unaffected. Each
probe that records a key widens this list by a one-line ADR amendment.

**What this enforces of ADR-014 (3) in Stage 1:**
- **R4** — chain only from a converged source;
- **R5** — parity;
- **R7** — a scan needs Opt;
- **P3** — a frequency calculation at the optimization's level.

**What stays debt.** ADR-014 (3) names three guards: `! XTB GOAT` before a pathway, the
ΔΔE‡-without-thermochemistry warning, and TightOpt before Freq. The first two guard tools that Stage
1 does not expose. Anton decided that **TightOpt is not required in Stage 1**: P3's same-level rule
stands in for it.

### (7) The Stage-1 tool surface

| tool | kind | reuses | notes |
|---|---|---|---|
| `list_jobs` | read | `list_jobs_conn` | filters: status, origin, group; paged |
| `get_job` | read | `get_job_conn` | status, **header and atom count, never the coordinate block**, charge and multiplicity with their source, notices, error, `origin`, reasons, `grant_id`, `mcp_state` |
| `get_results` | read | `results::read_job_results` | a bounded summary with units in the field names (rule #11): energy (Eh), `converged`, imaginary-mode count and values (cm⁻¹), thermochemistry, scan profile, NEB summary |
| `get_geometry` | read | the source readers of (4) | xyz with charge and multiplicity and their source. Refused above 600 atoms, with a pointer to `measure` |
| `measure` | read | `xtb::distance`/`angle`/`dihedral` | distance, angle or dihedral by 0-based indices on a source. A number for the model to **quote**, not compute. Cross-checked against ASE on fixtures |
| `read_output` | read | the `read_job_output` tail/search | bounded, never the whole `output.out` (rule #5) |
| `search_manual` | read | the manual FTS search | hits with section ids: a source for ADR-014 (1a) retrieval |
| `list_run_targets` | read | profiles, `is_run_target`, the poller's reachability | includes "outside expected window" |
| `memory_estimate` | read | `memory_estimate_with` | |
| `list_groups` | read | groups core | |
| `get_grant` | read | grant row + counters | read only |
| `create_job` | write | decisions 3–5 | returns the disposition, job id and reasons |
| `cancel_job` | write | `cancel_job_core` | **only `origin = 'mcp'` jobs** (any other → refused) |
| `create_group` | write | groups core | spends nothing |
| `wait_for_jobs` | read | DB re-read | decision 8 |

**Not exposed in Stage 1:**
- delete, rename and retry;
- server-profile writes and connection tests;
- `reparse_job` and `read_orbital_cube` (synchronous I/O);
- the queue pause;
- `xtb_optimize` and `crest_grow` (no pollable path);
- grants and settings.

**Result size.** Every result stays under the 50,000-character threshold by these caps: paging, the
600-atom cap, the bounded output reads, and no coordinates in `get_job`. One client "session" is not
a concept the server tracks. "Claude's jobs" means every `origin = 'mcp'` job, whichever client
session created it.

### (8) Following a long job: polling plus one bounded long-poll

No decision relies on server-to-client notifications, because the client's per-call limit is hard
(sourced). **`get_job` is the poll.**

**`wait_for_jobs(known: {job_id: status}, timeout_s)`:**
- **It returns at once** if any named job's current status differs from the status the caller
  passed. So a change between the model's last read and the wait is never lost.
- **Otherwise it waits.** It re-reads the DB every 2 s, holding no lock while it sleeps, until a
  status differs or `timeout_s` passes. `timeout_s` is capped at 600.
- **It checks the connection.** Each tick, it reads the connection's `closed` flag and ends if it is
  set (how the flag is set: M0 amendment, items 6 and 10).

The repeat is bounded by the cap, and because each call is one client action.

**The limits, stated plainly:**
- **The laptop must be on**, with the app and the client running.
- **uni is unreachable 22:00–08:00** (ADR-024). At night the assistant sees uni jobs as unreachable
  and cannot advance a uni chain. Local chains do advance.
- **Server-side dependency chains** are a Stage-2 candidate.

### (9) What Anton sees, and what leaves the machine

**What Anton sees:**
- **Jobs.** Each MCP job carries an **`origin = 'mcp'`** badge ("Claude"). A PROPOSED job also shows
  its reasons and Run. A job in step B shows "Claude is submitting". Step A emits `job:status`, so the
  list refreshes.
- **Settings → MCP:**
  - the switch, the socket path, and whether this instance serves;
  - whether a client is connected;
  - the active grant with its usage, plus Revoke and "Cancel Claude's jobs";
  - **the audit log**.
- **The audit log** is an `mcp_audit` table. It holds every state-changing call, every refusal and
  every proposal (scope qualified by the M3 amendment, item 3: dispositions only, plus step C's outcome): time, tool, bounded argument text, disposition, reasons, `job_id` and `grant_id`.
  Read calls are counted per connection in memory, and those counts are lost on restart. The log is
  also **the data of the experiment**: how the agent behaves at a given level of autonomy.

**What leaves the machine (accepted by Anton).** T3 sends unpublished research to the model's
provider: geometries, headers, energies, output excerpts. T1 excluded geometry by construction
(ADR-014, the T1 amendment). T3 cannot, because chaining needs it. The Settings → MCP switch states
this in one line, next to the toggle.

### (10) Stage 2 — named, not designed

These items are not designed here. Each needs its own ADR, written from Stage-1 experience:
- reaction building through MCP: si/re approaches, scans whose range is **derived** from a measured
  geometry (ADR-014 (1a)(b)), pathway comparison and ΔΔE‡. Their logic is TypeScript today, so a
  Rust port or a bridge is the first fork;
- a ΔΔE‡ / energetic-span tool with ADR-014 (3)'s guards;
- server-side dependency chains;
- the core-hour budget;
- pollable `xtb_optimize` and `crest_grow`;
- each block key that a probe records;
- **wider-autonomy experiments**, for example scan ranges taken from an Anton-authored protocol the
  app can verify. Anton decides those one at a time, from the audit log.

### Honest limits

- **The OS boundary is the uid**, provided the socket mode is enforced. M0 measured the owner bits;
  denial of a foreign uid stays Open (M0 amendment, items 2 and 8). Every
  process running as Anton's user can open the socket, including the development Claude Code
  sessions on this repository. Inside the uid, the separation is:
  - the server is off by default;
  - grants are explicit, bounded and expiring;
  - this repository's `.claude/settings.json` gets a `deny` rule on `mcp__orcastudio__*`. Anton adds
    it himself (unit M4), because agents may not edit that file.

  The deny rule matches names; it is a guardrail, not a boundary.
- **The client's own permission prompts are a second line, never the first.** The authority boundary
  is the classifier and the grant. Tool annotations are hints (sourced) and are not used.
- **PROPOSED drafts cost nothing but clutter Jobs.** R8 caps them at 50 (a step-C refund may exceed it:
  M3 amendment, item 1).

## Consequences

- **Schema v24.** It is the next free version: v23 at `d1551db`, and neither sibling worktree changes
  it. It adds:
  - `jobs.origin TEXT NOT NULL DEFAULT 'user' CHECK (origin IN ('user','mcp'))`;
  - `jobs.mcp_reasons` (JSON, nullable);
  - `jobs.grant_id` (nullable FK);
  - `jobs.mcp_state` (nullable, `'submitting'`);
  - `mcp_grants`, with `charged_jobs` and the **expression** partial unique index of decision 5;
  - `mcp_audit`.

  `tauri dev` must be stopped and the DB backed up before the migration (memory).
- **`submit_job`** gains the `mcp_state` refusal of decision 5. That is a command guard; the backends
  do not change. The reconciliation of decision 5 is added; it runs on each acquisition of the lock (M2 amendment, item 6).
- **`wiki/modules/tauri-core.md`** gets a new MCP section. Its pollable-path debt now names
  `crest_grow`.
- **A new module page, `wiki/modules/mcp-server.md`**, written in M1a.
- **[ADR-014](adr-014-ai-integration-boundary.md)** gets the amendment pointer. **`ROADMAP.md`**
  lists units M0–M5.
- **No change** to ADR-024's state machine, to the backends, or to the UI's own Create/Run paths,
  apart from the `mcp_state` display.

## Open questions

M0 closed (a) and (c), apart from a foreign uid, and part of (d). The remainder is listed under
"Open questions after M0" in the M0 amendment.

- **(a) The server library.** Does `rmcp` serve over an arbitrary `AsyncRead + AsyncWrite` stream?
  If not, Stage 1 implements the JSON-RPC 2.0 methods it needs (`initialize`, `tools/list`,
  `tools/call`). M0 settles this.
- **(c) The socket host facts.** On the laptop:
  - Does `XDG_RUNTIME_DIR` exist, and what are its mode and owner?
  - Does `connect` on a `0000` socket fail with `EACCES`?
  - Does `SO_PEERCRED` return the peer's uid?
  - Is the `flock` exclusive between two processes, and is it released when its holder is killed?

  M0 measures these.
- **(d) The client side.** M0 measures:
  - whether Claude Code reconnects after the app restarts;
  - how auto mode and its classifier treat MCP calls;
  - whether a `deny` rule on `mcp__orcastudio__*` blocks them.

(b), TightOpt before Freq, was decided by Anton: not in Stage 1.

## Decomposition — Stage 1 units

Each unit follows the CLAUDE.md loop: Part A pure and tested, then STOP, verifier CODE, Anton, then
Part B. Every listed control must turn the suite red.

- **M0 — probe** (prober).
  - **What it measures:**
    - a toy relay plus a socket server against the real Claude Code;
    - a tool call blocking 15 minutes;
    - the 50,000-character threshold;
    - the `deny` rule;
    - reconnect after a server restart;
    - `rmcp` over a Unix stream (a);
    - the socket host facts, including `chmod 000` → `EACCES` and the `flock` exclusivity and release
      on kill (c).
  - **Where it is recorded:** `wiki/architecture/mcp-transport-probe.md`.
- **M1a — the classifier and the input assembly** (implementer `opus`; pure, Part A).
  - **What it contains:**
    - the header grammar;
    - `classify_mcp_job` with R1–R9 and P1–P5;
    - the method signature and its `NON_METHOD` parity test;
    - the block key allowlist;
    - the Rust charge/multiplicity locator and its parity test against `sceneFromOrcaInput`;
    - the input assembly with its re-parse post-condition;
    - parity.
  - **Controls:**
    - one per row;
    - "ωB97X-3c def2-QZVP" against a "ωB97X-3c" level, which must be PROPOSED;
    - `! PAL16` → REFUSED;
    - `%irc` with any key → PROPOSED;
    - a valueless `{B 3 17 C}` → PROPOSED;
    - out-of-range index → REFUSED;
    - `smiles:` → PROPOSED;
    - Freq from a scan frame → PROPOSED;
    - `! NEB-TS` → REFUSED;
    - a dropped re-parse check lets a reordered block through.
  - **Wiki:** `modules/mcp-server.md`.
- **M1b — the resolver, the schema and the grant core** (implementer `opus`).
  - **What it contains:**
    - the source resolver over the DB and the readers, and the sidecar for SMILES;
    - computing `%maxcore` and the memory verdict before step A;
    - schema v24, with `charged_jobs` and the partial unique index;
    - the grant create and revoke commands;
    - the `submit_job` `mcp_state` guard;
    - the lock-holder reconciliation.
  - **Controls:**
    - a non-converged source → REFUSED;
    - the v24 `CHECK`;
    - creating grant 2 leaves grant 1 active;
    - a raw second unrevoked `INSERT` succeeding, which is what a column-form index would allow;
    - deleting a charged job re-opens the budget;
    - `max_in_flight` ignoring an old grant's running job;
    - `submit_job` accepting a `submitting` job;
    - the reconciliation leaving a `submitting` Draft charged;
    - an instance without the lock reconciling.
- **M2 — the server, read-only.** Split into **M2a** (transport) and **M2b** (the read tools) by the
  M2 amendment, item 1, which governs where it differs from this bullet.
  - **What it contains:** the `flock`, the socket and its modes, `SO_PEERCRED`, the `--mcp` relay,
    the 4-connection cap, the lifecycle table, and the read tools including `measure` with its ASE
    cross-check.
  - **Controls:**
    - a foreign-uid peer accepted;
    - a second instance binding without the lock;
    - `get_job` returning coordinates;
    - `get_geometry` above 600 atoms;
    - `read_output` reading the whole file;
    - the M0-amendment obligations listed in its item 9.
- **M3 — the write tools.** Split into **M3a** (`create_job`) and **M3b** (the other write tools) by
  the M3 amendment, item 1, which governs where it differs from this bullet.
  - **What it contains:** `create_job` through the step A/B/C order, `cancel_job`, `create_group`,
    `wait_for_jobs`, the audit log, R8, and the `job:status` emit.
  - **Controls:**
    - revoke vs step A (a job charged after the revoke);
    - a step-B refusal not refunded;
    - two concurrent step As with `max_in_flight = 1`, where the second is not PROPOSED;
    - step C refunding a job that already left `Draft`;
    - `cancel_job` on a `user` job accepted;
    - `wait_for_jobs` ignoring `known`;
    - a refusal with no audit row;
    - the 51st proposal accepted;
    - a grant check outside `BEGIN IMMEDIATE`;
    - the M0-amendment obligations listed in its item 9.
- **M4 — the UI (Part B, live WebKitGTK gate).** The switch, the data line and "this instance serves"
  move to M2a Part B (M2 amendment, item 2).
  - Settings → MCP: the switch with the data line, "this instance serves", the grant editor with
    "this job's level", Revoke, "Cancel Claude's jobs", and the audit view.
  - In Jobs: the badge, the reasons and "Claude is submitting".
  - Anton adds the `.claude/settings.json` deny rule himself.
- **M5 — the live research gate (Anton).** A Claude Code session in a separate research folder.
  - Anton runs the PROPOSED GOAT from a SMILES.
  - Claude then chains Opt+Freq of the lowest conformers and an SP, Local and uni, under a grant.
  - At least one PROPOSED scan is run by Anton's click.
  - The run is recorded with the audit log.

## Amendment (2026-11-19) — M0 results

**Status:** Accepted by Anton on 2026-11-19. DESIGN review: PASS WITH FINDINGS (4 MED + 7 LOW), all
applied; the semaphore-full fork was decided by Anton.

The decision text above stands, **except where an item below says it replaces or qualifies a
sentence**; those sentences carry an inline pointer here. This amendment records what the M0 probe measured
([mcp-transport-probe.md](mcp-transport-probe.md); Claude Code 2.1.318, `rmcp` 3.11.0, kernel
6.14). It closes Open questions (a) and (c), apart from a foreign uid, and part of (d). Where a
measurement qualifies a decision, the rule below governs.

1. **Unknown methods (decision 2).** The client sends `server/discover` **before** `initialize`. The
   server answers any method it does not implement with a JSON-RPC error and keeps the connection.
   `rmcp` already does this (−32602).
2. **The socket's mode (decision 2).** Under umask 022 a bound socket comes out 0755
   (measured). So the app creates the directory and the socket with explicit modes, `0700` and
   `0600`. After bind it `stat`s both, and refuses to serve if either mode differs (rule #9).
   **Measured:** `connect` on a 0000 or 0400 socket fails with `EACCES`, and on 0600 it succeeds.
   **Open:** denial of a foreign uid on 0600 (no second account to test with). `SO_PEERCRED` stays
   as the check.
3. **Restarts (decision 2, lifecycle table, "the app restarts").**
   - **How the client recovers.** It re-launches a dead stdio server lazily, on the next tool call.
     If that attempt finds the app down, it did not retry for at least 44 s.
   - **Its status cannot be trusted.** The client keeps reporting the server as "connected" while it
     is dead.
   - **The rule.** After an app restart, the dependable recovery is **a new client session**. The
     research workflow and the tool descriptions say so. Jobs, grants and the audit log are
     unaffected.
   - **Open:** a manual `/mcp` reconnect, and the client's retry schedule beyond 44 s.
4. **The lock (decision 2).** `flock` is exclusive, and it is released when the holder is killed —
   **unless a child process inherited the fd**. Measured: a forked child kept the lock after its
   parent was `kill -9`ed. So the lock file is opened `O_CLOEXEC`, and nothing in the app forks
   without exec while holding it. Whether Rust's `File::open` sets it by default is **not
   measured**; M2's check settles it. "Released only at process exit" means the
   exit of the last process holding the fd. M2 checks that the fd carries `FD_CLOEXEC`.
5. **Concurrency on one connection (decision 1).** `rmcp` serves a connection's requests
   **concurrently** (measured). That replaces the sentence "each connection handles its requests
   one at a time":
   - each connection holds a semaphore of **4** in-flight requests;
   - **when it is full**, a fifth request gets an immediate tool error, "busy, retry" (decided by
     Anton). It never queues: an abandoned `wait_for_jobs` keeps its permit for up to 600 s, because
     a client timeout sends no `notifications/cancelled` (measured);
   - **a permit returns when its work ends**: when its handler returns, or, for `create_job`, when
     its task ends after step C (item 6). A connection's slot among the 4 frees only when its
     last permit has returned. So **16 handlers** is a real bound, even though handlers
     outlive their transport (measured);
   - correctness never rests on serial handling. Inside one process, the `DbState` mutex orders two
     step As; across instances, `BEGIN IMMEDIATE` does (decision 5). That holds on one connection
     exactly as on two.
6. **Hang-up (decision 8).** `rmcp` does **not** cancel a handler when the transport closes
   (measured: the handler ran to completion after the relay was killed). So each connection's serve
   task sets an `AtomicBool closed` when its transport ends. Each `wait_for_jobs` tick reads that
   flag and returns. Any future **waiting or reading** handler must check the same flag.
   **`create_job` never abandons on hang-up** once step A has committed. It finishes steps B and C,
   so no job is left at `mcp_state = 'submitting'` except after a crash.
   - **Why this holds by construction (decided by Anton).** The **whole** `create_job` (the
     memory verdict, step A, step B and step C) runs as **one task on the app's own runtime**, which
     the `rmcp` service does not own.
     - **The order.** The handler first checks the **task tracker** (the gate: M2 amendment, item 4). If the tracker is closed, the
       handler refuses with "MCP is shutting down". Otherwise it registers the task in the tracker,
       moves its semaphore permit into the task, spawns the task, and then only awaits the result,
       in order to return it.
     - **What this guarantees.** Nothing the task does depends on the handler surviving. The spawn
       happens **before** any DB write, so no cancellation point lies between step A's commit and
       steps B and C.
   - **Why that matters.** Neither a transport close (measured: it does not cancel handlers) nor a
     service cancel on switch-off or exit can abort the task, or come between its steps. What a
     service cancel does to a handler is **not measured**, and the construction makes it irrelevant:
     a handler cut off before the spawn has written nothing.
   - **Switch-off and exit.**
     - They first **close the tracker** (the gate: M2 amendment, item 4), so no new `create_job` task can start.
     - Then they set every connection's `closed` flag and cancel the services.
     - Then they wait for every tracked task. Each is bounded by its DB transactions and its own ssh
       timeouts.
     - A clean exit only then unlinks the socket and releases the lock (decision 2, "Exit"), so every
       job finishes.
   - **What a crash leaves.** A crash anywhere between step A's commit and step C's commit leaves a
     `submitting` row. The next lock holder's reconciliation covers every such row, and it can take
     the lock only after the crashed process, and any child holding the fd, is gone (item 4).
   - **The permit goes with the work.** The permit (item 5) lives in the task, so it returns when
     step C ends, not when the handler returns. A connection's slot frees only when its last permit
     has returned. So the 16 bound covers `create_job` work too.
7. **Large results (decision 7).** Over 50,000 characters, the client writes the result to
   `~/.claude/projects/…/tool-results/` and hands the model an error-style pointer. The rule counts
   characters, not tokens (measured). ADR-027's caps keep every result below this, and M2's controls
   pin the caps.
8. **Where the server is registered (Honest limits).** A `deny` rule on `mcp__<server>__*` hides
   the tools from the model in every mode tested (default, `auto`, `bypassPermissions`). **But the
   client still connects** and runs `initialize` and `tools/list`, so it would take one of the 4
   connection slots and show as "connected".
   - **The rule.** OrcaStudio's server is registered **only in the research folder's project
     `.mcp.json`**, never in user scope. Sessions in this repository then never launch the relay at
     all.
   - The repository's `deny` rule (M4) stays as the second layer.
   - In `auto` mode an MCP call without an allow rule went through (measured on `echo`). That
     confirms the client's gates are not the authority boundary. The auto-mode classifier on a
     spend-type tool stays Open.

9. **Who owns the new obligations, and their controls.** Each control must turn its unit's suite
   red.
   - **M2:** (carried into M2a by the M2 amendment, item 1)
     - an unknown method such as `server/discover` closing the connection instead of returning an
       error;
     - serving after a socket or directory mode differs from `0600`/`0700` post-bind;
     - the lock fd without `FD_CLOEXEC`;
     - a fifth concurrent request queued instead of refused "busy";
     - a connection slot freed while a permit is still held;
     - the tool descriptions not stating that a new client session is the recovery after an app
       restart.
   - **M3:** (split into M3a/M3b by the M3 amendment, item 1)
     - `wait_for_jobs` ignoring `closed`;
     - `create_job` abandoning step B or C when `closed` is set after step A;
     - any step of `create_job` running inside the handler instead of the tracked task;
     - a service cancel right after step A's commit leaving the job at `mcp_state = 'submitting'`;
     - a `create_job` task starting after the tracker has closed (the gate: M2 amendment, item 4);
     - switching MCP off between step A and step C leaving the job at `mcp_state = 'submitting'`;
     - switch-off or exit not setting every connection's `closed` flag;
     - a clean exit releasing the lock while a `create_job` task is still running;
     - the permit returned when the `create_job` handler returns instead of when step C ends.
10. **Ending connections (decision 2, lifecycle).** Closing a transport does not stop its handlers
    (measured). So when a client hangs up, the server sets that connection's `closed` flag. When MCP
    is switched off or the app exits, it also cancels each connection's service. Handlers then end:
    - a waiting or reading handler, at its next `closed` check;
    - a `create_job` handler returns once its task has run through step C (item 6). If the handler
      itself is cut off by the service cancel, the task still completes. Switch-off and exit close
      the tracker (the gate: M2 amendment, item 4) first and wait for the tasks.

**Open questions after M0:**
- (c′) denial of a foreign uid;
- (d′) the auto-mode classifier on a spend-type tool, the `/mcp` manual reconnect, and the
  client's retry schedule beyond 44 s.
- (e′) whether an **interactive** Claude Code session asks before loading the research folder's
  project `.mcp.json`. M0 measured `-p` mode only; M5 observes this.
- (f′) the packaged binary's name (M2 amendment, item 3): unmeasured until a release build exists.

Neither blocks M1a.

## Amendment (2026-11-19) — M2: the split, the switch, the binary name

**Status:** Accepted by Anton on 2026-11-19.
- The four forks below were decided by Anton on 2026-11-19, each on the orchestrator's lean: items
  1–3, and the two-instance rule in item 2.
- The rest follows from the code at `642d1af`, as mapped by the explorer.
- DESIGN round 1: FAIL (3 HIGH + 8 MED + 5 LOW), all applied; one fork from it was decided by Anton
  (item 2).
- DESIGN round 2: FAIL (1 HIGH + 3 MED + 4 LOW), all applied. The second FAIL was escalated to Anton,
  who decided to fix and run round 3, and decided the exit fork (item 6).
- DESIGN round 3: FAIL (1 HIGH + 1 MED + 5 LOW), all applied. The third FAIL was escalated to Anton,
  who decided to fix and run a narrow round 4 over the delta.
- DESIGN round 4 (the delta only): PASS WITH FINDINGS (1 MED + 4 LOW), all applied.

The decision text above stands, except where an item below says it replaces or qualifies a sentence.

1. **M2 is two units (decided by Anton).** Each follows the CLAUDE.md loop (Part A, STOP, verifier
   CODE, Anton, Part B), and every listed control must turn its suite red.
   - **M2a — the transport** (implementer `opus`, because of the concurrency).
     - **What it contains:**
       - **The lock.** The `flock` on the lock file, opened `O_CLOEXEC`, with the `FD_CLOEXEC`
         assertion.
       - **The socket.**
         - The path is resolved through `XDG_RUNTIME_DIR`. If that is unset or not absolute, the
           app data dir is used with the same modes (decision 2, "Fallback").
         - If the path is longer than `sun_path` allows, MCP refuses to start and says why.
         - The directory (`0700`) and the socket (`0600`) get explicit modes. Both are `stat`ed
           after bind (M0 amendment, item 2).
       - **`SO_PEERCRED`**, with the uid comparison as an injectable function.
       - **The `orcastudio --mcp` relay** (item 3).
       - **The caps.** At most 4 connections are served; a fifth connection is closed at once.
         Each connection holds a semaphore of 4 permits, and a fifth request gets "busy, retry"
         (M0 amendment, item 5). The 4-permit cap counts `tools/call` only; `initialize`,
         `tools/list` and `ping` take no permit (acknowledged by Anton, M2a Part A verification).
       - **The per-connection `closed` flag.**
       - **The admission gate and the tracker (item 4).** Created here, empty. M3 adds tasks.
       - **The lifecycle owner and the order of operations (item 6).** Switch-on, switch-off, exit and
         the one 30 s tick, every transition serialised by one lifecycle mutex.
       - **The reconciliation.** The lock holder calls `reconcile_submitting_conn`, then binds
         (decision 5, "Only the lock holder runs it").
       - **The switch** of item 2.
       - **One tool, `get_grant`**, over `current_grant_core`. It proves the stack end to end
         without M2b's projections.
       - **The runtime requirement (not a control).** The first bind-and-serve test runs on
         `tauri::async_runtime`, not on a test runtime (item 5).
       - **A test hook** between the gate's check and the spawn, so the interleave control can place
         a close there.
       - **The recovery sentence.** The "after an app restart, start a new client session" text is
         one shared constant, used in the server instructions and in every tool description
         (M0 amendment, item 3).
     - **Controls:**
       - **The lock and the reconciliation:**
         - a second instance binding without the lock;
         - an instance without the lock reconciling. This is already an M1b control; the wiring
           must keep it red.
         - binding or accepting before `reconcile_submitting_conn` returns;
         - exit running `terminate_on_exit` before the MCP wait has ended (a local step B could start
           ORCA after the kill), or the wait running on the main thread (item 6). Seam: the exit flow
           is a function over an injected handle and executor, so a test can drive it without a
           `RunEvent` and assert the thread and the order;
         - a reconcile error (for example `SQLITE_BUSY`) followed by serving. The required
           outcome: no serve, the reason shown, a new attempt on the next tick.
         - the lock fd without `FD_CLOEXEC`;
         - `mcp_set_enabled(false)`, or the tick reading "off", releasing the `flock`.
       - **The socket and the peer:**
         - serving after a mode differs from `0600`/`0700` post-bind;
         - an over-long `sun_path` bound instead of refused;
         - a foreign-uid peer accepted. The uid source is injected, since no second account
           exists (Open question c′).
       - **The caps:**
         - a fifth concurrent request queued instead of refused "busy";
         - a connection slot freed while a permit is still held;
         - a fifth connection served.
       - **The gate:**
         - work admitted after the gate is closed;
         - the gate's check and the spawn not done under one lock (item 4). The control
           interleaves a close between them.
       - **The tick and the switch:**
         - a non-holder that stops retrying after the row has been off and then on again;
         - the holder not starting to serve within one tick after another instance writes
           `mcp_enabled = "true"`;
         - the holder still serving one tick after the row reads "off";
         - a socket on disk once a stop has completed;
         - two concurrent tick steps (the background tick and `mcp_set_enabled`) with the row "on"
           giving more than one reconcile or more than one bind;
         - a row reading "on" while a stop is still running (`stopping_tasks > 0` or the unlink
           pending) reconciling or binding;
         - an old stop's unlink removing a newer socket;
         - exit releasing the lock while a task of an older tracker generation still runs;
         - a tick whose row read fails (for example `SQLITE_BUSY`) changing any state;
         - a tick or `mcp_set_enabled(true)` after exit began binding or opening the gate (bites by
           removing the `exiting` check). The control places the tick where no other guard makes it
           a no-op: after stop complete and before the lock release, and, for a second instance's
           view, after the release with the row "on";
         - `mcp_set_enabled` writing the row during exit;
         - a second user-initiated `ExitRequested` during the exit flow passing through (bites by
           keying the pass-through on "the first time" instead of `exit_ready`);
         - exit releasing the lock while a stop's unlink is still pending;
         - a second switch-on after a completed stop, on the same acquisition, reconciling again;
         - `mcp_enabled` writable through the generic `set_setting` (`commands/settings.rs:43`).
       - **The relay:**
         - writing anything but MCP bytes to stdout;
         - exiting 0, or writing no stderr line, when connect fails (decision 2);
         - not exiting on socket EOF (the lifecycle table).
       - **The protocol:**
         - an unknown method such as `server/discover` closing the connection instead of returning
           an error;
         - a tool description without the recovery sentence.
   - **M2b — the read tools** (implementer `opus`, because of the units and the geometry).
     - **What it contains:** `list_jobs`, `get_job`, `get_results`, `get_geometry`, `measure`,
       `read_output`, `search_manual`, `list_run_targets`, `memory_estimate` and `list_groups`,
       with the character caps of item 7.
     - **What stays in M3:** `wait_for_jobs`, as the decomposition already says.
     - **What M2b re-asserts:** each new tool's description carries M2a's recovery sentence.
     - **Controls:** M2's original ones (`get_job` returning coordinates, `get_geometry` above 600
       atoms, `read_output` reading the whole file), plus those of items 7 and 8.
2. **The switch before M4 (decided by Anton, including the two-instance rule).**
   - **Storage.** One `settings` row, `mcp_enabled`, holding `"true"` or `"false"`. A missing row
     means off. The row is in the **shared** DB, so it is one switch for every instance using that
     DB. `mcp_set_enabled` is its **only writer**: the generic `set_setting` refuses the key.
   - **The lock** is taken the first time this instance finds the row "on", whether at start, on a
     toggle or on a tick. It is then held until process exit, and never released on switch-off
     (decision 2).
   - **The tick.** Each instance runs one 30 s tick on a background thread. Each tick re-reads the
     row, and then:
     - a **holder** that reads "on" and is not serving starts to serve (the item 6 order);
     - a **holder** that reads "off" and is serving stops (the item 6 order);
     - a **non-holder** that reads "on" tries the lock. Taking it starts the item 6 order;
     - a tick whose row read fails (for example `SQLITE_BUSY` from another instance) changes nothing
       and is retried on the next tick;
     - while a stop is still running (`stopping_tasks > 0` or the unlink pending), a row reading
       "on" is a no-op; the next tick after the stop completes starts serving;
     - once exit has begun (the terminal `exiting` flag, item 6), every tick step and every
       `mcp_set_enabled` is a no-op: no row write, nothing started. The call returns a status with
       `reason = "exiting"`. Writing the shared row while exiting would silently change other
       instances.

     Every tick step, `mcp_set_enabled` (its row write included) and exit take **one lifecycle
     mutex** for the whole transition decision, so two starts or a start and a stop never run at
     once (item 6).

     This replaces decision 5's "an instance with MCP enabled that does not hold the lock retries it
     every 30 s". The tick covers that retry, and it also covers the holder that is switched off
     while another instance switches MCP on: that holder starts serving within one tick.
   - **Commands.**
     - `mcp_status() -> McpStatus`.
     - `mcp_set_enabled(on) -> McpStatus` writes the row, then runs the tick's step once, at once,
       instead of waiting up to 30 s.
     - Neither command runs on the GTK main thread (`tauri-core.md`, the threading rule): both are
       `async` commands whose work runs through `spawn_blocking`, as `test_server_profile` does
       (`commands/server_profiles.rs:594`), so the main thread never takes the lifecycle mutex
       (item 6). Switch-off returns once it has closed the gate, set the `closed` flags and cancelled
       the services. Waiting for the tracked tasks and unlinking the socket then run on the server's
       runtime, and the status reads "stopping (N tasks)" until they end.
     - Their I/O is the row write, the lock file, the reconciliation transaction, the bind and the
       `stat`s.
   - **`McpStatus`** has these fields:
     - `enabled` — the DB-wide row;
     - `holds_lock`;
     - `serving` — this instance only;
     - `stopping_tasks`;
     - `connections` — the number of connected clients;
     - `socket_path`;
     - `reason` — why this instance does not serve, for example "another OrcaStudio instance holds
       the MCP lock", the `sun_path` refusal, or "exiting".

     The status is **advisory**. Its fields may be momentarily inconsistent with one another (for
     example right after a toggle), and no decision reads it. If it is kept behind its own lock, that
     lock is a leaf (lifecycle → snapshot).
   - **M2a Part B** adds one row to Settings → MCP:
     - the switch;
     - decision 9's one-line statement of what leaves the machine, beside the toggle;
     - a line saying whether this instance serves, with the socket path, the number of connected
       clients, or the reason it does not serve.

     That is a small render gate. The rest of decision 9's Settings → MCP (the grant, the audit log,
     Revoke, "Cancel Claude's jobs") stays in M4. This item governs where M4's bullet still lists
     the switch, the data line or "this instance serves".
   - No environment variable or other second mechanism is added.
3. **The binary is `orcastudio` (decided by Anton).**
   - That is the cargo artifact's name (`src-tauri/Cargo.toml` `[package] name`). Decision 2's relay
     text is corrected in place.
   - In development the research folder's `.mcp.json` names `<repo>/target/debug/orcastudio`
     (workspace `target/`, measured to exist). The packaged binary's name is not measured and stays
     Open until a release build exists.
   - The relay branches in `main.rs` before `run()`. It therefore opens no window, initialises no
     GTK, and writes diagnostics only to stderr.
4. **The admission gate (replaces the M0 amendment's reliance on the tracker refusing work).**
   `tokio-util`'s `TaskTracker::close` "does not prevent you from spawning new tasks" (sourced:
   `tokio-util-0.7.20/src/task/task_tracker.rs:331`; a `reopen()` exists at `:340`). So the refusal
   is app-owned:
   - A `Mutex<bool>` "open" is held across both the check and `tracker.spawn`. `close` takes the
     same mutex.
   - `TaskTracker` is used only for `wait()`.
   - Switch-on after a completed switch-off makes a fresh tracker under that mutex. Every tracker
     generation is kept until its tasks have ended, and exit waits on all of them (item 6).
   - Wherever the M0 amendment says "the tracker refuses" or "close the tracker", read "the gate".
5. **Dependencies and the runtime.**
   - **New direct dependencies:**
     - `rmcp` 3.11.0 with features `server` and `macros`, as M0 measured (`mcp-transport-probe.md`);
     - `tokio`, adding `net`. Today it is transitive only, with `rt`, `rt-multi-thread`, `sync`,
       `fs` and `io-util`;
     - `tokio-util`, for `TaskTracker`.
   - **The runtime.** The server runs on Tauri's tokio runtime (`tauri::async_runtime`). M0 ran
     `rmcp` on its own runtime with tokio 1.54.1, while the workspace pins 1.53.1. So M2a's first
     serve test runs on Tauri's runtime (item 1), and a failure there is a finding, not a workaround.
   - **DB work.** A handler's DB work goes through `spawn_blocking`. It locks `DbState` for one query
     or transaction, and never holds the lock across an `.await` (decision 1).
6. **The order of operations (qualifies the lifecycle table).**
   - **One owner.** Every transition (a tick step, `mcp_set_enabled`, exit) runs under **one
     lifecycle mutex**, which serialises transitions. A start never overlaps a stop, and two starts
     never overlap. The mutex is held only for the decision and the short synchronous steps (row
     read, lock, reconcile, bind, `stat`s, gate open or close), never across the wait for tasks.
   - **Lock order.** lifecycle → gate, and lifecycle → `DbState`. The lifecycle mutex is never taken
     while `DbState` or the gate is held. Handlers take the gate and `DbState`, never the lifecycle
     mutex. `mcp_status` reads a snapshot without the lifecycle mutex. **No step under the lifecycle
     mutex waits on the main thread**, and the main thread never takes the lifecycle mutex.
   - **Acquisition.** Taking the `flock` begins an acquisition. Its reconciliation
     (`reconcile_submitting_conn`) runs **once per acquisition**, as decision 5 says, before the first
     bind. An acquisition counts as done only when that reconciliation commits. If it fails (for
     example `SQLITE_BUSY`), nothing is bound, the reason is shown, and the next tick retries it. A
     later switch-on on the same acquisition does not reconcile again. That is safe, because a
     completed stop leaves no `submitting` row of this instance: every tracked task ran through
     step C before the stop completed.
   - **Switch-on (holder, acquisition done, no stop running):** bind, `stat` both modes, open the
     gate, accept. While a stop is still running, a switch-on is a no-op, and the next tick retries.
   - **Switch-off:** close the gate, set every `closed` flag, cancel the services (synchronous, under
     the mutex). Then, off the main thread and outside the mutex: wait for the tracked tasks, unlink
     the socket **this start bound** (checked by its inode, so an older stop can never remove a newer
     socket), and only then mark the stop complete. The `flock` is **kept**.
   - **Exit (decided by Anton: `prevent_exit` and an asynchronous wait).**
     - `RunEvent::ExitRequested` (`lib.rs:226`) passes through only when the app-owned `exit_ready`
       flag is set. Otherwise it calls `prevent_exit` and sets the terminal **`exiting`** flag, an
       `AtomicBool`, with one `compare_exchange` and no mutex. That one operation is both the
       "already running" test and the terminal flag: only the request that wins it starts the exit
       flow, and a second request during the flow is absorbed.
     - Tick steps and `mcp_set_enabled` check `exiting` **after** taking the lifecycle mutex, so any
       transition that takes the mutex after the flag is set is a no-op (item 2). A transition
       already in flight when the flag is set is undone by the flow's switch-off, which takes the
       mutex after it.
     - Off the main thread, the flow then takes the lifecycle mutex, runs the switch-off order and
       releases the mutex. It then waits for **stop complete** (every tracker generation ended and
       the unlink done, including a stop that began before exit), and then runs the existing
       `terminate_on_exit` and the sidecar stop, in that order. Running the MCP wait first means a
       `create_job` task's local step B cannot start ORCA after the kill.
     - It then releases the lock, sets `exit_ready`, and calls `app.exit()`. How a programmatic
       `app.exit()` re-enters `ExitRequested` (Tauri 2.11.5) is not measured: M2a settles it from
       the crate source or a run before relying on it (rule #10). (Settled from the crate source,
       not run: [mcp-transport-probe.md](mcp-transport-probe.md), "M2a facts".) A second signal is sourced
       (`tauri-2.11.5/src/app.rs:226-228`, on `ExitRequested`'s `code`): "[`Option::None`] when the
       exit is requested by user interaction, [`Option::Some`] when requested programmatically via
       [`AppHandle#method.exit`] and [`AppHandle#method.restart`]."
     - What the user sees during the wait is **not measured**. On the close-the-last-window path
       there might be no window left. M2a records what actually happens.
     - In M2a the trackers are empty, so the wait is immediate. The mechanism and the order are
       built here all the same.
     - **The backstop (added in M2a Part B, decided by Anton 2026-11-19; the grace and `_exit` in
       its fix round).** A thread starts with the exit flow and ends the process in two cases:
       - the flow has not reached `exit` within **60 s**, or dies first: the backstop runs its own
         queue pause, kill and sidecar stop (bounded at 20 s), then ends the process (the pause becomes
         the terminal exit flag: M3 amendment, item 4);
       - the flow reached `exit`, but the process is still alive **10 s** later: it ends the process
         (the flow has already run the kill and the sidecar stop).

       The end is `libc::_exit(1)`, not `std::process::exit`, so no atexit handler runs that could
       deadlock against the GTK/WebKit loop. Without the backstop, a stuck flow would absorb every
       later close request, so the app could never close. A forced end counts as a **crash** for the
       reconciliation: a job left in step B stays `submitting`, and the next lock holder's
       reconciliation refunds it (decision 5). M3 revisits the 60 s against the ssh timeouts of a
       remote step B (M3 amendment, item 1: a fork brought to Anton).
     - **The local queue (decided by Anton, M2a Part B fix round).** The flow's first step, before
       the MCP wait, pauses the local queue for good (now the terminal exit flag, which `resume_queue`
       cannot clear: M3 amendment, item 4). Otherwise the job `terminate_on_exit` kills frees
       its slot, and its drive thread could start the next queued ORCA job during the kill's grace;
       that job's ranks would outlive the app. It holds whether or not MCP was ever switched on.

7. **Result caps (M0 amendment, item 7: "M2's controls pin the caps").** Each M2b tool has a
   character cap below 50,000 on its serialised result, and paging or truncation that says so in
   the result. The reused readers are not bounded in characters:
   - `read_job_output` caps at 10,000 **lines** (`commands/jobs.rs:1205`);
   - `search_manual`'s `limit` is unbounded (`commands/manual.rs:89–96`).

   **Control, one per tool:** its largest page or result exceeding 50,000 characters.
8. **What decision 7's "reuses" column needs in code** (explorer and DESIGN r1, `642d1af`). This does
   not change the decision. It records what M2b builds:
   - **`list_jobs`.** `list_jobs_conn` is private and has no filters or paging. M2b adds a
     filtered, paged query over `Job::COLUMNS`/`Job::from_row`.
   - **`get_job`.** `get_job_conn` returns the full `Job`, with `input_content` and `scene_json`.
     M2b serialises a **projection**, never `Job`. Control: any field of the projection carrying a
     coordinate line.
     `get_job`'s header is reconstructed from M1a's parsed structure, and raw header text is never served (decided by Anton after two verifier FAILs on filtering, M2b): `!` tokens are shown with every numeric-looking one hidden; allowlisted keys show their admitted values; `%geom Constraints` entries are shown with their targets, although the M1a SUBMITTED allowlist admits only `{C n C}`; a display-only list shows exactly one integer value for `%pal nprocs`, `%scf MaxIter`, `%neb NImages` and `%geom Recalc_Hess` (any other value hidden), and `%maxcore N` comes from the strict reader; every other key is named without values; geometry constructs are markers; every error the view builds (the header's, the charge and multiplicity's, a coordinate row's) is a kind and a line, while the job's `error` field is ORCA's stored text, guarded by the tripwire; and the rule-#9 tripwire replaces any field holding a coordinate-like row (a geometry line, which in text fields means a geometry opener `* xyz`/`* int`/`* gzmt`/`* xyzfile` in any case and spacing, also after ORCA's input-echo prefix, and in the header field any `*` line; or an element-or-label token followed by ≥3 numbers at any position on a line; a `{` or `=` line is skipped in the header field only) with a "withheld" marker and serves the rest. The ID fields are not guarded one by one: a coordinate-like ID field trips the backstop (`jobs::tripwire`, a walk of the whole serialised view) and refuses the whole `get_job` view; `list_jobs` guards its title and `error_excerpt` and has no backstop (its IDs are v4 UUIDs). The exact definitions of "numeric-looking", "label" and "numbers" are in [mcp-server.md](../modules/mcp-server.md), "The read tools", `get_job`.
   - **`get_results`.** `read_job_results` returns per-atom arrays. M2b builds the bounded summary,
     with units in the field names (rule #11).
   - **`measure`.** Results are `distance_ang`, `angle_deg` and `dihedral_deg` (rule #11).
     - **Refusals.** `xtb::distance`, `xtb::angle` and `xtb::dihedral` index without a range check
       (the functions themselves). For degenerate input they return plausible numbers: a distance of
       0.0, an angle of NaN, a dihedral of 0.0 for a zero axis or a collinear inner triple
       (`xtb::dihedral`). M2b refuses, never panics and never returns a number, for each of these
       cases, as `measure.ts` returns null (`src/scene/measure.ts:106–141`):
       - an index out of range;
       - a repeated index;
       - coincident atoms in a distance;
       - a zero-length vector in an angle;
       - a zero axis or a collinear inner triple in a dihedral.

       **Control:** one per case.
     - **The ASE cross-check.** It uses the existing rule-#10 record: the butane fixtures in
       `sidecar/tests/test_geometry.py` (ASE 3.29.0; anti 179.998°, gauche 67.523°, still passing
       on 2026-11-19).
       - That record pins only the dihedral. M2b extends the same pytest to pin ASE
         `get_distance(0,1)` and `get_angle(0,1,2)` on that fixture, which is a recorded run, and
         copies all three values into the Rust tests.
       - **Controls:** the Rust value differing from the ASE value, and the dihedral not folded to
         [0, 360).
     - **The dihedral (decided by Anton, M2b).** `measure` does not reuse `xtb::dihedral`'s formula, because `xtb::dihedral` returns 360° − ASE (measured on the butane fixture, M2b; [mcp-transport-probe.md](mcp-transport-probe.md), "M2b facts"); it ports ASE's formula.
   - **`list_run_targets`.** No per-profile reachability exists, and nothing computes "outside
     expected window".
     - **The window.** M2b computes "outside the window" with a pure function of the profile's
       `availability_window` (ADR-024 g: `HH:MM-HH:MM` in the laptop's local time, possibly
       wrapping midnight) and `now` as local minutes, passed in as a parameter. The local time comes
       from SQLite's `datetime('now','localtime')`, the one measured route (`db.rs:827–850`).
       Controls: a wrapping window (`22:00-08:00`) at 23:00 and at 07:59, and the end minute
       excluded.
     - **Reachability.** The poller's `last_contact` is per **job**, held in memory with no
       timestamp (`poller/remote_state.rs:203`, `remote/contact.rs:42`). So `list_run_targets`
       reports the window and the profile's validity, and reachability as `"unknown"`. Per-profile
       reachability is not built in Stage 1.
   - **Private helpers.** `list_groups_conn`, `list_server_profiles_conn` and `RunTargetStatus::of`
     become `pub(crate)`. That is visibility only.

## Amendment (2026-11-21) — M3: the split and the live check

**Status:** Accepted by Anton on 2026-11-21.
- Two forks (items 1 and 2) were decided by Anton on 2026-11-21, each on the orchestrator's lean.
- DESIGN round 1 was PASS WITH FINDINGS (6 MED + 6 LOW), all applied. It raised two more forks, also
  decided by Anton on the leans: the audit scope (item 3) and the exit flag (item 4).
- DESIGN round 2 was PASS WITH FINDINGS (3 MED + 7 LOW), all applied. The 60 s revisit becomes a
  fork for M3a (MED-3).

The decision text above stands, except where an item below says it replaces or qualifies a sentence.

1. **M3 is two units (decided by Anton).** Each follows the CLAUDE.md loop, and every listed control
   must turn its unit's suite red.
   - **M3a — `create_job`** (implementer `opus`: money, concurrency, chemistry consequences).
     - **What it contains:**
       - **One tracked task.** `create_job` is one task admitted through the M2 amendment's gate
         (item 4). It runs the memory verdict, step A, step B and step C around `classify_mcp_job`
         and `assemble` (decisions 3–5; M0 amendment, item 6).
       - **`SidecarSmiles`.** It is handed to the resolver and run off the main thread.
       - **The audit rows of item 3.**
       - **R8's open-proposal count.**
         - It is read in the same `BEGIN IMMEDIATE` transaction that inserts the draft, so two
           concurrent step As in the serving instance cannot both read 49 and insert 50 and 51.
         - A step-C refund may raise the count above 50: that is allowed and is not an error. It only
           makes the next `create_job` refused by R8 until proposals are run or deleted.
       - **The `job:status` emit**, after step A and after step C.
       - **The R4 source-charge reason.** It is mapped to a kind and a line, and never quotes the
         source `* xyz` line (verifier finding F8, M2b).
       - **Bounded arguments.**
         - The bounds cover the header, the title and every other string argument.
         - Every result stays under the 40,000-byte cap of the M2 amendment, item 7.
         - This closes M1a's note that "M3 bounds tool arguments". The implementer fixes the numbers and
           records them in `wiki/modules/mcp-server.md`; the controls bite on those numbers.
       - **The 60 s exit backstop** (M2 amendment, item 6), revisited against a remote step B. A remote
         step B can outlast it: `SUBMIT_TIMEOUT` and `CALL_TIMEOUT` are 60 s each (`ssh_backend.rs`).
         A forced end mid-step-B counts as a crash: the reconciliation refunds a Draft still at
         `submitting` to PROPOSED, although its submit may already have reached the server.
         - **Criterion:** the backstop's flow deadline is at least the worst case of a remote step B,
           i.e. the sum of its bounded ssh calls (× `CALL_TIMEOUT`), `SUBMIT_TIMEOUT` and the rsync
           bound, measured from the code.
         - **This is a fork, not an implementer's change.** The backstop's timings were decided by
           Anton. M3a computes the worst case, brings the result to Anton with a lean (keep 60 s,
           raise it, or bound step B by the remaining exit budget), and only then changes anything.
           A changed value gets a control. The outcome replaces the "M3 revisits the 60 s"
           sentences in item 6 and in `wiki/modules/mcp-server.md`.
       - **The exit flag of item 4.**
     - **Controls:**
       - **The decomposition's `create_job` controls:**
         - revoke vs step A;
         - a step-B refusal not refunded;
         - two concurrent step As with `max_in_flight = 1`, where the second is not PROPOSED;
         - step C refunding a job that already left `Draft`;
         - the 51st proposal accepted;
         - a grant check outside `BEGIN IMMEDIATE`.
       - **The M0 amendment's item-9 obligations for `create_job`:**
         - abandoning step B or C when `closed` is set after step A;
         - any step running inside the handler instead of the tracked task;
         - a service cancel right after step A's commit leaving `mcp_state = 'submitting'`;
         - a task starting after the gate has closed;
         - switching MCP off between step A and step C leaving `mcp_state = 'submitting'`;
         - a clean exit releasing the lock while a `create_job` task is still running;
         - the permit returned when the handler returns instead of when step C ends.
       - **The remaining item-9 obligation, "switch-off or exit not setting every connection's
         `closed` flag".** M2a's test covers one connection and has no recorded control on the
         switch-off arm. M3a extends it to **at least two connections**, and records the control:
         delete the `closed` store in the listener's `shut`, and the test goes red.
       - **Audit (item 3):**
         - a charge committed without its audit row;
         - a step-C refund leaving the log at SUBMITTED only;
         - a classifier REFUSED with no audit row.
       - **R8:** two concurrent PROPOSED calls at 49 open proposals, where both are inserted (a SUBMITTED
         job does not count toward R8).
       - **Arguments:** an over-long header accepted; a result over the cap.
       - **R4:** an R4 reason quoting the source line.
       - **Exit (item 4):**
         - `resume_queue` during exit lets a queued job be claimed;
         - a local step B that runs after the exit flow has begun starts ORCA, i.e. claims a slot.
   - **M3b — the other write tools** (implementer `opus`).
     - **What it contains:**
       - **The tools:** `cancel_job` (only `origin = 'mcp'` jobs), `create_group` and
         `wait_for_jobs` (decision 8, with `timeout_s` capped at 600).
       - **Audit rows** for `cancel_job` (a refusal included) and `create_group`. `wait_for_jobs`
         is a read, counted in memory only (decision 9).
       - **Bounded arguments:** the group name and the size of `wait_for_jobs`' `known` map.
       - **No grant needed.** No M3b tool needs an active grant: `create_group` spends nothing
         (decision 7), and a cancel stops spending (decision 5).
     - **Controls:**
       - `cancel_job` on a `user` job accepted;
       - `wait_for_jobs` ignoring `known`;
       - `wait_for_jobs` ignoring `closed`;
       - `timeout_s` above 600 honoured;
       - a `cancel_job` or `create_group` call with no audit row;
       - an over-long group name or `known` map accepted.
2. **The live check of the SUBMITTED path before M4 (decided by Anton).** The order stays M3 → M4.
   - **What this qualifies.** It qualifies decision 5's "a row Anton creates in Settings → MCP", until
     M4.
   - **How the grant is made.** Anton himself creates the grant through the same Rust command,
     `mcp_create_grant`, the only grant writer. He types it by hand in the `tauri dev` devtools
     console.
   - **What else must not create or change a grant.** No agent, test hook, script, environment
     variable or direct SQL creates or changes a grant in the app's database. No MCP tool can reach the command
     (decision 7).
   - **The arguments**, the values Anton types (`commands/mcp.rs`). The core enforces only its own
     refusals (a bad level, a count below 1, an expiry not after now, an unknown profile); the limits
     below are Anton's procedure, and he checks the returned `GrantView` against them before the check:
     - `targets: ["local"]`;
     - `levels: [<one "!" line>]`;
     - `max_jobs` 1–2;
     - `max_in_flight` 1;
     - `expires_at` as UTC `YYYY-MM-DD HH:MM:SS`, within the hour.
   - **Revocation.** Anton revokes the grant the same way (`mcp_revoke_grant`) after the check.
   - **When and from where.** The check runs in M3a Part B, before M3a's commit. The client is a
     Claude Code session in a research folder with a project `.mcp.json` (M0 amendment, item 8).
   - **The PROPOSED path** is checked without a grant (P5).
   - **Unmeasured (rule #10).** These facts are settled at the live check:
     - that devtools opens in `tauri dev` on WebKitGTK;
     - that its console can reach the Tauri `invoke` (`withGlobalTauri` is not set);
     - that the default capability admits the command;
     - the JavaScript argument spelling `invoke` expects (its default argument-name conversion).

     If any of them fails, stop and bring it to Anton. No other mechanism is added.
3. **The audit scope (decided by Anton).**
   - **What it records.** The audit log records **dispositions**, not tool errors:
     - each SUBMITTED, PROPOSED and REFUSED from the classifier;
     - step C's outcome (a refund to PROPOSED with its reason, or no refund with the job's state);
     - the reconciliation's refund after a crash ("interrupted before submit"), as an outcome row;
     - a REFUSED internal error (decision 4: a failed re-parse post-condition), as REFUSED.

     Outcome rows use the v24 columns: `disposition` `proposed` for a refund, NULL otherwise, with the
     reason or the job's state in `reasons`. No schema change is implied. `db.rs`'s "one row per MCP
     tool call" comment is updated in M3a.
     - the M3b rows of item 1.
   - **Where the SUBMITTED row is written.** Inside step A's transaction, so a charge without its
     audit row is impossible.
   - **What is not recorded.** A call that ends before classification is not a disposition and is not
     audited: a busy refusal, "MCP is shutting down", an argument error, `SQLITE_BUSY` at step A, or a
     resolver error before classification. It qualifies decision 9's "every refusal", which means every classifier
     refusal.
4. **The exit flag (decided by Anton).**
   - **The flag.** The exit flow's queue pause (M2 amendment, item 6) becomes a separate, terminal
     exit flag. `pause_for_exit`, called by both the flow and the backstop, sets the exit flag instead
     of the shared `paused` flag. `claim_next` checks it, and `resume_queue` cannot clear it.
   - **Pages M3a updates** (accurate today, stale after the change): `wiki/modules/execution-backends.md`
     (the exit pause), `wiki/modules/mcp-server.md` (the exit flow's queue pause),
     `wiki/modules/tauri-core.md` (exit), and the `JobRunner.paused` doc comment in `local_backend.rs`.
   - **Why.** It closes the case where a Resume click during an exit wait lasting up to 60 s (window
     still open, `prevent_exit`) clears the shared pause flag. The freed slot would then start the
     next queued ORCA job, and that job would outlive the app.
   - **What stays.** The user's pause and resume are unchanged.
