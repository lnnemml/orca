# Remote live run on uni — unit 5.3 B4 (plan; results land here)

**Status:** plan, written 2026-10-17 while the uni server was down (Tailscale node offline since
~2026-10-14, most likely a power outage at the site). Each section below gets its `### Facts` /
`### Not measured` once it is run, like [task-spooler-uni-probe.md](task-spooler-uni-probe.md) and
[remote-sync-probe.md](../orca/remote-sync-probe.md). This is a rule-#10 record for ADR-024's
B4 decisions (o6, o16, o18, F-2) and two of its open questions ((b) restart, UPS).

Everything runs as **`anton` over the `uni` alias**; never `uni-admin` (ADR-024 k, the managed
`deny` rules). No network identifiers in this page (uni-server.md policy).

## Code facts the plan rests on (HEAD `0257e5a`, explorer map 2026-10-17)

- **ssh argv** `ssh -o BatchMode=yes -o ConnectTimeout=10 -- <host> bash -s` (`remote/ssh.rs`
  `ssh_bash_argv`); no ControlMaster/ServerAlive in the argv, **but the user's `~/.ssh/config` for
  `uni` adds `ControlMaster auto`, `ControlPersist 600`, `ServerAliveInterval 30`,
  `ServerAliveCountMax 4`** (`ssh -G uni`). The app's effective keepalive therefore comes from the
  user config, not from the code — o18's residual text ("no ServerAliveInterval") is true of the
  argv only. Per-call timeouts: label / prepare / install / submit / mkjob 60 s, collect 60 s × up to
  2, `poll_log` 10 s, download **and** LIST 300 s (one shared `FETCH_TIMEOUT`).
- **rsync** down: `rsync -a --checksum <filter> -e '<ssh argv>'`, no `--timeout`. Every non-zero rc
  is one fetch strike; only rc 12 is measured (remote-sync-probe §7). The 1800 s `.gbw` branch has
  **no production caller** (the poller passes `FetchPolicy::SMALL_ONLY`), so B4 cannot exercise it.
- **Poller**: status step every 15 s, log step every 2 s, one thread per due step, **no per-host
  cap**; jobs planned in the same tick stay in phase (bursts every 15 s, inferred). Worst-case status
  step 780 s (label 60 + collect 2×60 + rsync 300 + LIST 300). `POLL_LOG_MAX_BYTES` 256 KiB; a chunk
  at the cap makes the next log step due at the next tick (catch-up). `LiveLog::apply` emits under
  its mutex.
- **Exit (o18)**: only the local ORCA job and the sidecar are killed; remote ssh/rsync children are
  left to the OS in their own process groups and lose their parent-enforced timeout.
- **Markers / classifier**: `.started` carries the boot's `boot_id`; with `.started` present and a
  different `boot_id` the classifier says **`Lost`** (row 8); `ReEnqueue` needs no `.started`
  (row 11). In 5.3 both are only *shown* (the row stays `queued`/`running`); acting on them is 5.4.
- **tsp safety**: `label.sh` (and the collect) call `tsp -l` only on a socket that `/proc/net/unix`
  lists, so the app's read-only checks do not start a tsp daemon. A **submit/Retry does**.
  `TS_SAVELIST` is set nowhere, so a restart loses the in-memory queue by design.

## Step 0 — post-outage snapshot (first, before anything writes)

**Rule:** until step 0 is recorded, **no Submit, Retry or Withdraw** to uni. The app may stay open
(its label/collect calls are read-only and start no daemon), but the snapshot is taken by the prober
directly.

Record (read-only, as `anton`):
1. `uptime`, `/proc/sys/kernel/random/boot_id` (compare with the last recorded boot_id in
   task-spooler-uni-probe.md / remote-sync-probe.md), clock offset vs the laptop (NTP is not synced).
2. How it went down: `last -x reboot shutdown | head` and `journalctl --list-boots` /
   `journalctl -b -1 -n 50` **if** readable without root; a missing "shutdown" record before the
   new boot = power loss, not a clean shutdown → feeds the **UPS** open question. Whether the box
   came back unattended (it did not stop at POST) — feeds open question **(b)**'s reboot risk.
3. tsp after the restart: any `tsp` daemon process of `anton`; which `<root>/tsp/slot*.sock` files
   exist and whether `/proc/net/unix` lists them (a stale socket file with no listener is the
   expected post-boot state, unmeasured); do **not** run `tsp` on an unlisted socket.
4. `<root>/jobs/`: every job dir, its markers (`.started` boot_id vs the new one, `.exit_code`,
   `.enqueued`, `.cancelled`) — what the classifier would now say (`Lost` / `ReEnqueue`), and that
   the dirs are intact (no truncated files from the power loss).
5. Did `tailscaled` come back by itself (it evidently did not until power returned; record when the
   node reappeared).

## Step 1 — the two rows left by the outage

Both are `queued` with coordinates and failed at the read-only **prepare** step (ssh timeout), so
nothing was uploaded: "Ibuprofen opt+freq" (`e900980b…`) and "SP · r²SCAN-3c" (`f57c93ca…`). Expected
(inference, unverified): the label call says **"not on the server"** → Anton chooses Withdraw (the
o20.3 confirm) or Retry. This is also the live check of the not-on-server path after a real outage.

## Measurements

| # | What | Who / how | Feeds | Decision it settles |
|---|---|---|---|---|
| G1 | **Done-when gate**: submit a real job → close the laptop (suspend) → reopen hours later → the job is picked up, fetched, verified, parsed | Anton, app | ROADMAP 5.3 "Done when" | Unit 5.3 done |
| M1 | Fetch duration vs the 300 s bound: rsync-down + LIST of real finished job dirs (small SP, an opt+freq, the largest realistic output) | prober, raw rsync with the app's filter | o6 | keep `FETCH_TIMEOUT` 300 s or not; `.gbw` 1800 s stays unmeasured (no caller) |
| M2 | rsync rc on a **dropped link** (no RST): mid-download, the link disappears; rc, time to fail, leftovers, with and without the user-config keepalive | prober + Anton (`tailscale down` on the laptop is Anton's action) | o6, o12 open list | whether any rc needs its own handling; whether `rsync --timeout` / argv `ServerAliveInterval` is needed independent of the user config |
| M3 | `poll_log` 10 s bound and catch-up: a job whose output grows fast (> 256 KiB between polls); does the view catch up without gaps or duplicates | Anton, app (watch the log) | o item 7, o16.3 | `POLL_LOG_TIMEOUT`, cap |
| M4 | The emit under `LiveLog`'s mutex returns promptly (no stall of other steps) | app with timing; needs a temporary debug timing log or devtools — **decide before the run** | o16.3 | keep emitting under the mutex or move it out |
| M5 | Quit mid-fetch: do the orphaned ssh/rsync survive, for how long, and is the local dir byte-identical to the server listing after the next (complete) fetch | Anton quits; prober watches `ps` locally and on uni | o18 | `ServerAliveInterval`/`rsync --timeout` in argv; o18 finality argument |
| M6 | ~15 jobs queued on one host: spurious `CheckFailed` from synchronized status steps; sshd `MaxStartups`/`MaxSessions` (read `/etc/ssh/sshd_config` if world-readable); cold vs warm ControlMaster | prober: 15 concurrent `ssh uni bash -s` (label-like, read-only); then Anton: 15 real queued jobs in the app | F-2 (`wiki/log.md`) | stagger the status steps only if it bites (Anton's rule) |
| M7 | Kill ORCA's main process mid-run under the wrapper: does `output.out` keep growing after `.exit_code` | prober, a small ORCA job via tsp + wrapper, `%pal` 4 | o18 finality residual | whether row 6 needs a job-session check before fetch finality |
| M8 | (optional) Does Tauri deliver an event emitted before a command's response ahead of that response | app instrumentation | ADR-024 open question (o19) | none today (o19.6 does not depend on it) |

**Order:** step 0 → step 1 → M6 raw burst, M1, M7 (prober, one session, no app writes) → G1 started
in the evening (suspend overnight) → M3, M5, M2 with Anton → M6 in the app → M4/M8 if instrumented.

**Not in B4:** the 1800 s `.gbw` timeout (no production path); acting on `Lost`/`ReEnqueue` (5.4).

## Drift found while planning (fix with the results)

- ADR-024 o18 residual "no `ServerAliveInterval`": true of the argv, not of the effective behaviour
  (user config). Whether the app should stop depending on the user config is M2/M5's decision.
- ADR-024 o18 anchor `ssh.rs:225-228` is stale (the deadline return is now ~l.226–233).
- ROADMAP B4 names "LIST timeouts" and "1800 s with `.gbw`": LIST shares the 300 s download bound,
  and the `.gbw` branch has no production caller.
