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
