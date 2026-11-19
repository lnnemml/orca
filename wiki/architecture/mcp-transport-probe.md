# MCP transport probe (ADR-027 unit M0)

**Date:** 2026-11-19 · **By:** prober subagent · **Serves:**
[ADR-027](adr-027-mcp-orchestration-server.md), decisions 1, 2, 7 and 8, its Honest limits, and
Open questions (a), (c), (d).

A rule-#10 record: every fact below comes from a run on the dev laptop. The throwaway code is
outside the repository, in this session's scratchpad (`m0/toy/`, `m0/rmcp-probe/`). It is not kept,
so this page carries the commands and outputs that matter.

**Which claims rest on a retained output.** Most claims here were checked against the retained logs
by the DESIGN reviewer, and the host facts were re-run. Four were **observed by the prober, but no
output survives** (marked *observed* below):
- the two relay re-spawns in reconnect run 1;
- `rmcp`'s −32602 code;
- the `sleep_for 6` / `echo` ordering;
- the stdin half-close.

**Versions:**
- Claude Code 2.1.318;
- kernel 6.14.0-35-generic;
- Python 3.12.3;
- rustc/cargo 1.97.1;
- `rmcp` 3.11.0 (features `server,macros`, which pull in `transport-async-rw`);
- tokio 1.54.1.

**The setup:**
- **`toy_server.py <sock> <log>`** — a hand-written NDJSON JSON-RPC MCP server on a Unix socket.
  Its tools are `echo`, `sleep_for` and `big`.
- **`relay.py <sock>`** — a dumb stdio↔socket byte relay. It exits 1 on socket EOF, and exits 2 with
  one stderr line if it cannot connect.
- **The client** is the real `claude` CLI in scratch project folders.

## Results

| # | fact | verdict |
|---|---|---|
| 1 | relay end to end | CONFIRMED |
| 2 | 15-min tool call | CONFIRMED |
| 3 | 50,000-character threshold | CONFIRMED |
| 4 | deny rule | CONFIRMED (tools hidden, connection still made) |
| 5 | reconnect after a server restart | QUALIFIES ADR-027 |
| 6 | `rmcp` over a Unix stream | CONFIRMED (two behaviours to design for) |
| 7 | socket host facts | CONFIRMED, except a foreign uid (OPEN) |

### 1. Relay end to end

**Two configurations work without a prompt:**
- `claude -p --strict-mcp-config --mcp-config mcp.json …`, with
  `{"mcpServers":{"toy":{"type":"stdio","command":"python3","args":["…/relay.py","…/mcp.sock"]}}}`;
- a project `.mcp.json` in the working directory, in `-p` mode. Whether an **interactive** session
  asks before loading it is **Open**; M5 observes it.

**The sequence the server logged:** `server/discover` → `initialize` → `notifications/initialized` →
`tools/list` → `tools/call echo`. The model answered `echo:hello-m0 (server pid 562453)`.

**Quirks:**
- **`server/discover` comes first.** The client sends `server/discover` (protocol 2026-07-28)
  **before** `initialize`. A server must answer an unknown method with a JSON-RPC error and carry on:
  the toy returned −32601, and `rmcp` returns −32602 (*observed*).
- **MCP tools are deferred.** The model finds them with ToolSearch before it calls them.

### 2. A long tool call

`MCP_TOOL_TIMEOUT` was unset, the relay sent no keepalive, and the server sent no progress
notifications. The run was `sleep_for seconds=900`:
- **Server log:** `10:11:57 sleep 900 start` → `10:26:57 sleep done after 900.0s`.
- **Client:** `slept 900`, `is_error=false`, exit 0 at 10:27:00.

**The negative control** shows the gate can fail. `MCP_TOOL_TIMEOUT=5000`, `sleep_for 15` →
`MCP server "toy" tool "sleep_for" timed out after 5s`. No `notifications/cancelled` reached the
server. So ADR-027's 600 s cap on `wait_for_jobs` is far inside the client's default limit.

### 3. The 50,000-character threshold

- **49,000 characters:** inline (a 49,030-byte text block). 49,000 random alphanumerics, which are
  many tokens, also stay inline. So the rule counts characters, not tokens.
- **51,000 characters:** the model gets an error-style text, `Error: result (51,000 characters
  across 1 line) exceeds maximum allowed tokens. Output has been saved to
  ~/.claude/projects/<proj>/<session>/tool-results/mcp-toy-big-<ts>.txt`. The file holds all
  51,000 characters.

So an oversized result leaves the app's control and lands in `~/.claude`. ADR-027's caps keep every
result under the threshold.

### 4. The deny rule

The project's `.claude/settings.json` held `{"permissions":{"deny":["mcp__toy__*"]}}`:

| case | `tools/call` reaching the server |
|---|---|
| deny + `--allowedTools=mcp__toy__echo` | 0 ("No such tool available") |
| deny + `--permission-mode bypassPermissions` | 0 |
| deny + `--permission-mode auto` | 0 |
| no rule, allow-listed (control) | 1 |
| no rule, `bypassPermissions` (control) | 1 |

**The deny rule hides the tools from the model, but the client still connects.** It runs
`initialize` and `tools/list`, so the app sees a connection.

**Permission modes (Open question d), measured on `echo` only:**
- headless default mode, no allow rule: 0 calls ("…you haven't granted it yet");
- `auto`: the call went through without an allow rule;
- `bypassPermissions`: the call went through.

How the auto-mode classifier treats a spend-type tool is **not** measured.

**A shell gotcha:** `--allowedTools` is variadic and swallows the prompt. Write `--allowedTools=…`.

### 5. Reconnect

One `claude -p --input-format stream-json` session was driven over several turns.

**Run 1** (`kill -9` the server, call, then restart it):
- the relay logged `socket EOF -> exit 1`;
- the next call gave `MCP server "toy" is not connected`, after the client had re-spawned the relay
  twice, both times hitting `ECONNREFUSED` (*observed*);
- once the server was back, the calls about 8 s and 44 s later did **not** re-spawn the relay, and
  the error stayed.

**Run 2** (kill, restart within 2 s, then call): the next call re-spawned the relay, reached the new
server pid and succeeded.

**What happens mid-call.** Killing the server during `sleep_for 40` → the client gets
`Connection closed` at once.

**The status field cannot be trusted.** `mcp_servers` in the stream's `init` event keeps saying
`"connected"` while the server is dead, so it is not a usable post-condition.

**Conclusion.** The client re-launches a dead stdio server lazily, on the next tool call. If that one
attempt finds the server down, it does not retry for at least 44 s, and possibly not at all. **After
an app restart, the dependable recovery is a new client session.**

**Open:** a manual `/mcp` reconnect (interactive only), and the retry schedule beyond 44 s.

### 6. `rmcp` over a Unix stream (Open question a)

**It works.** `Probe::new().serve(tokio::net::UnixStream)` with `#[tool_router]` and
`#[tool_handler]` compiles unchanged, because the blanket `IntoTransport` covers
`AsyncRead + AsyncWrite`.

**Driven by the real client through the relay:** `echo` worked, and `big 49000` came back inline.
`stream.peer_cred()` returned `uid=1000 pid=Some(<relay pid>)`.

**Two behaviours ADR-027 must design for:**
- **One connection's requests run concurrently.** A raw test sent `sleep_for 6`, then `echo`, and
  the `echo` reply arrived first (*observed*).
- **A handler is not cancelled when the transport closes.** The relay was `kill -9`ed during
  `sleep_for 8`. The log read `connection ended: Closed`, but the handler ran on to
  `sleep_for(8) finished normally`. A stdin half-close behaves the same way (*observed*).

### 7. Socket host facts (Open question c)

**`$XDG_RUNTIME_DIR`** is `/run/user/1000`: mode `700`, owner `laptop`, tmpfs.

**The socket's own mode.** A socket bound under umask 022 comes out **0755**, so the app must set
`0600` itself.

**`connect` results:**

| case | result |
|---|---|
| socket mode 0600 | OK |
| socket mode 0000 (negative control) | `EACCES` |
| socket mode 0400 | `EACCES` |
| socket mode 0200 | OK (connect needs write permission) |
| directory mode 0000 | `EACCES` |
| stale file, listener gone | `ECONNREFUSED`, and the file stays |
| missing file | `ENOENT` |

**`SO_PEERCRED`** returned `pid,uid,gid = 561837 1000 1000`, the caller's own values.

**`flock(LOCK_EX|LOCK_NB)`:**
- a second process gets `EAGAIN`;
- after `kill -9` of the holder, a new process acquires the lock;
- **but** if the holder had forked a child (no exec), the lock stayed held after the holder's
  `kill -9`, until the child exited.

**`sun_path`:** 107 bytes binds, 108 fails. Rust's `UnixListener::bind` reports
`path must be shorter than SUN_LEN`. The real path `/run/user/1000/orcastudio/mcp.sock` is 34 bytes.

**Open: a foreign uid.** Denial of a different uid on a 0600 socket is not measured: there is no
second account, and no sudo. An `unshare -U` attempt was invalid, because the kernel uid stays the
same. The owner-bit enforcement above *is* measured.

## M2a facts (2026-11-19)

Recorded by the M2a Part A implementer. Each fact says how it was settled: **sourced** (read in the
crate source in `~/.cargo/registry/src/`, not run) or **run** (a test in `src-tauri/src/mcp/server/`,
on the dev laptop). Versions: Tauri 2.11.5, `tauri-runtime-wry` 2.11.4, `rmcp` 3.11.0, tokio 1.53.1
(the workspace pin, not M0's 1.54.1), `tokio-util` 0.7.19, `rusqlite` 0.32.1, rustc 1.97.1.

### 1. A programmatic `app.exit()` re-enters `ExitRequested` (sourced, not run)

The M2 amendment (item 6) left this to M2a (rule #10). The source settles all three questions.

- **It re-enters.** `AppHandle::exit` asks the runtime to exit, and falls back to a hard exit only if
  that request cannot be posted (`tauri-2.11.5/src/app.rs:573-580`):
  ```rust
  /// Exits the app by triggering [`RunEvent::ExitRequested`] and [`RunEvent::Exit`].
  pub fn exit(&self, exit_code: i32) {
    if let Err(e) = self.runtime_handle.request_exit(exit_code) {
      log::error!("failed to exit: {}", e);
      self.cleanup_before_exit();
      std::process::exit(exit_code);
    }
  }
  ```
  `request_exit` posts `Message::RequestExit(code)` through the event-loop proxy
  (`tauri-runtime-wry-2.11.4/src/lib.rs:2748-2754`), so the event runs later, **on the main thread**,
  whichever thread called `exit`.
- **It carries `Some(code)`, and `prevent_exit` is honoured.** The event loop handles the message so
  (`tauri-runtime-wry-2.11.4/src/lib.rs:4354-4366`):
  ```rust
      Message::RequestExit(code) => {
        let (tx, rx) = channel();
        callback(RunEvent::ExitRequested {
          code: Some(code),
          tx,
        });

        let recv = rx.try_recv();
        let should_prevent = matches!(recv, Ok(ExitRequestedEventAction::Prevent));

        if !should_prevent {
          *control_flow = ControlFlow::Exit;
        }
      }
  ```
  Tauri wraps it as `RunEvent::ExitRequested { code, api: ExitRequestApi { tx, code } }`
  (`tauri-2.11.5/src/app.rs:2552-2555`), and `prevent_exit` sends `Prevent` unless the code is the
  restart code (`tauri-2.11.5/src/app.rs:90-94`, with `RESTART_EXIT_CODE = i32::MAX` at `:77`):
  ```rust
  pub fn prevent_exit(&self) {
    if self.code != Some(RESTART_EXIT_CODE) {
      self.tx.send(ExitRequestedEventAction::Prevent).unwrap();
    }
  }
  ```
- **The consequence.** The exit flow's own `app.exit(0)` comes back as an `ExitRequested` that a
  `prevent_exit` **would** stop. That is why the handler passes through on the app-owned
  `exit_ready` alone (the pure `lifecycle::pass_through`), and not on "the first time" or on the
  `code`: any programmatic exit carries `Some`, so the code does not say whose exit it is.
- **Two edges, sourced.** If the proxy is gone, `exit` hard-exits with no `ExitRequested` at all
  (the `Err` arm above). And on the close-the-last-window path, `ExitRequested { code: None }` is sent
  only after the last window has been **destroyed** (`tauri-runtime-wry-2.11.4/src/lib.rs:4310-4324`:
  the `TaoWindowEvent::Destroyed` arm, when the window map is empty). So preventing it there keeps a
  process with no window alive until the flow's `app.exit`: during the MCP wait the user would see
  no window. That is what the source says; what the user actually sees stays for Part B's live gate.

### 2. std's `File::open` already sets `O_CLOEXEC` (run)

The M0 amendment (item 4) left this open. The std source is not installed for rustc 1.97.1
(`rust-src` component absent), so it is settled by a run, not by reading:
`lock_tests::std_file_open_already_sets_cloexec` opens a plain `File` and reads `FD_CLOEXEC` back
with `fcntl(F_GETFD)` — set. `lock_tests::a_child_process_does_not_inherit_the_lock_fd` runs
`ls -l /proc/self/fd/` in a child while the lock is held: the lock file is not among its fds. The
lock still opens with an explicit `O_CLOEXEC`, and its `FD_CLOEXEC` assertion is what guards (control
C5 in the M2a Part A log entry).

### 3. `rmcp` 3.11.0 implements `server/discover` (sourced and run)

- **Before `initialize`** (`rmcp-3.11.0/src/service/server.rs:565-627`): a request that lacks the
  2026-07-28 `_meta` keys (`io.modelcontextprotocol/protocolVersion` and
  `io.modelcontextprotocol/clientCapabilities`) gets `invalid_params` (−32602), and the handshake
  loop goes on. That is the −32602 M0 observed for Claude Code's `server/discover`. With those keys,
  `server/discover` is answered with a `DiscoverResult` (`handler/server.rs:456-463`).
- **After `initialize`**: an unknown method reaches `on_custom_request`, whose default is
  −32601 (`handler/server.rs:636-648`).
- **Run:** `server_tests::an_unknown_method_gets_an_error_and_the_connection_stays_open` sends all
  three, and the connection answers `tools/list` afterwards.
- **Sourced, not run:** a *notification* before `initialize` ends the handshake with
  `ExpectedInitializeRequest` (`service/server.rs:673-676`), which closes the connection.

### 4. `rmcp` reports a transport's end only after a response drain (sourced and run)

`waiting()` returns only after `rmcp` drains in-flight handler responses: up to 5 s for `Closed` and
2 s for `Cancelled` (`rmcp-3.11.0/src/service.rs:1834-1836`). Run: with four handlers in flight, a
hang-up was not visible through `waiting()` within 3 s. So the server sets a connection's `closed`
flag at the **read EOF** of its own stream wrapper (`listener::Transport`), not when `waiting()`
returns (control C16).

### 5. The serve path runs on Tauri's runtime (run)

`rmcp` spawns its serve loop and each handler with `tokio::spawn` (`service.rs:1387-1393`), so they
run on whichever runtime runs `serve`. The server is spawned with `tauri::async_runtime::spawn`, and
the tests create no runtime of their own, so the first bind-and-serve test
(`server_tests::the_first_bind_and_serve_runs_on_tauris_runtime_and_get_grant_answers`) ran on
Tauri's runtime with the workspace's tokio 1.53.1. It passed: the M2 amendment's runtime requirement
holds, with no finding.

### 6. `rusqlite`'s default busy timeout is 5 s (sourced and run)

`rusqlite-0.32.1/src/inner_connection.rs:119` calls `sqlite3_busy_timeout(db, 5000)` on every open.
Run: the reconcile-BUSY test took 5 s per failed attempt until it set 50 ms on its own connection.
So a reconciliation that meets another instance's write transaction would hold the lifecycle mutex
and `DbState` for up to 5 s before it reports `SQLITE_BUSY`. That is off the main thread by
construction (the tick thread, or `set_enabled` through `spawn_blocking`), but every synchronous UI
command that locks `DbState` would wait as long. So the reconciliation runs with a 250 ms busy
timeout and restores the connection's own afterwards (decided by Anton, M2a Part B).

**Run (M2a Part B):** `PRAGMA busy_timeout` on a fresh `init_db` connection reads **5000**, so the
pragma reads back what `rusqlite` set, and is what the restore uses
(`lifecycle_tests::a_busy_reconciliation_gives_up_within_the_short_timeout_and_restores_the_connections`,
which also measures a busy reconciliation giving up well inside 1.5 s).

### 7. `tokio-util` 0.7.19 is the locked version (sourced)

The M2 amendment cites `tokio-util-0.7.20`; the workspace lock holds 0.7.19, and adding the direct
dependency did not move it. Its `TaskTracker::close` says the same, "It does not prevent you from
spawning new tasks" (`tokio-util-0.7.19/src/task/task_tracker.rs:331`), with `reopen` at `:349`.

## Consequences for ADR-027

There is no hard contradiction. The qualifications are applied as the ADR's
"Amendment (2026-11-19) — M0 results", items 1–10. The measurement-driven ones are:
1. the server answers unknown methods (`server/discover`) with an error;
2. the socket mode is set explicitly and checked after bind;
3. recovery after an app restart is a new client session;
4. the lock fd must not leak into a child (CLOEXEC, no fork without exec);
5. `rmcp` per-connection concurrency is bounded, and correctness rests on the transactions;
6. `wait_for_jobs` observes the connection's end through an explicit flag;
7. the 50,000-character rule counts characters;
8. the server is registered only in the research folder's `.mcp.json`.

**Still open:**
- foreign-uid denial;
- the auto-mode classifier on a spend-type tool;
- `/mcp` reconnect and the retry schedule;
- loading a project `.mcp.json` in an interactive session;
- the Claude desktop app (out of Stage-1 scope).
