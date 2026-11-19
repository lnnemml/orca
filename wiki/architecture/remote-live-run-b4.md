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

**Rule:** until step 0 is recorded, **no Submit, Retry or Cancel** to uni *(Withdraw is gone since
5.4a B1; a Cancel is its successor)*. **The app stays closed until step 0 is recorded** *(amended
2026-10-18)*: since 5.4a a pending cancel is delivered automatically at the first status step that
reaches the server, and that delivery writes (ready/install, `mkjob`, `cancel.sh`). One is pending:
job `85a2d627` (Opt · r²SCAN-3c, never submitted; cancel requested 2026-10-18 13:39 UTC, left on
purpose by Anton). After step 0, opening the app delivers it through the not-on-server branch — a
live check of that path; record its outcome. The snapshot is taken by the prober directly.

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

### Facts (2026-11-17, prober, read-only as `anton`; M = measured, I = inferred)

- **No app layout on uni at all (M).** The profile's root (`remote_scratch_dir` = `~/.orcastudio`)
  holds only the old probe dirs (`probe-5.2/`, `probe-5.2b/`, `verify/`, dated 10-03…10-05) and
  `tsp/` with one stale `ts-out.*` and **no `slot*.sock`**. No `jobs/`, no `bin/` — the app's
  install and `mkjob` have never reached this host. `85a2d627`, `e900980b`, `f57c93ca` exist nowhere
  (`find`/`grep` over `~`, `/tmp`, `/var/tmp`). The DB agrees: these three are the only remote rows,
  all `queued`, none ever uploaded. So the step-1 open is the app's **first-ever install** on uni.
- **Not a power loss (M, `last -x -F`).** The outage began with a clean shutdown 2026-10-14 02:58;
  next boot 2026-10-29 10:41 (15 d 7 h gap); then boots 10-30 (×3), 11-01, 11-07, 11-12 15:43, every
  one preceded by a shutdown record. The box was up for most of 10-29…11-17; why the laptop saw it as
  offline is **not measured**. Something external powered it on after the clean shutdown (person,
  BIOS timer or WoL — not measured); "needed a physical visit" is neither confirmed nor refuted.
  Earlier history (07-15…08-27) has five boots with no shutdown record (crash/power loss) — the
  UPS question stays live.
- **Unattended boot (M):** `tailscaled` and `sshd` are `enabled` and active ~56 s after kernel boot,
  no login needed; `yats` gets a console session on `tty7` ~63 s after each boot (I: autologin).
- **Boot (M):** up since 2026-11-12 15:43 (server clock), `boot_id` `8e9bf2d8-d99c-42de-949a-a3c9946ae436`.
  No earlier real boot_id is recorded in the wiki, so there is nothing to compare it with.
- **Clock (M):** server − laptop = **+142.1 s ± 0.07** (5 back-to-back pairs, `LC_ALL=C`);
  `timedatectl`: not synchronized; RTC 6 s behind system clock. Never compare `started_at` across hosts.
- **tsp (M):** no tsp/orca/mpirun/wrapper process of any user; `/proc/net/unix` lists nothing
  under the root. `tsp` was not run.
- **Journal (M):** not readable by `anton` (groups `anton`, `users` only); `wtmp` is readable and
  was sufficient.
- **No writes (negative control, M):** `find ~/.orcastudio -newermt 2026-10-06` → 0 entries; `ps`
  identical before/after; the only new files under `~` are wireplumber state from the ssh login itself.

### Not measured

- When the node first reappeared from the laptop's side, and why it looked offline after 10-29.
- Whether the box powers itself on after an AC loss (BIOS setting / WoL).
- **The post-reboot state of a live job and a tsp socket** — no job or socket existed, so row 8
  (`Lost`) and the stale-socket case remain simulation-only. They can only be observed by a deliberate
  reboot with a job queued (Anton's call, not in B4 as planned).

## Step 1 — the two rows left by the outage

Both are `queued` with coordinates and failed at the read-only **prepare** step (ssh timeout), so
nothing was uploaded: "Ibuprofen opt+freq" (`e900980b…`) and "SP · r²SCAN-3c" (`f57c93ca…`). Expected
(inference, unverified): the label call says **"not on the server"** → Anton chooses Cancel (the
confirm moved there from Withdraw, o22.7) or Retry. This is also the live check of the not-on-server
path after a real outage. *(Step 0 showed none of the three rows reached the server and no app
install exists there; the first status step after opening the app is the app's first install on uni.)*

### Facts (2026-11-17, app opened by Anton; server read by the orchestrator, read-only)

- **First install (M):** at the first status step the app created `<root>/bin/` with `cancel-<sha>.sh`,
  `collect-<sha>.sh`, `wrapper-<sha>.sh` and `<root>/jobs/`; no tsp daemon was started by it.
- **Pending cancel 85a2d627 → not-on-server branch (M):** the server got `jobs/85a2d627…/` holding only an
  empty `.cancelled`; the row became `cancelled` ("the server confirms .cancelled and nothing of the job
  runs."), `cancel_requested_at`/`cancel_sent_at` cleared. The DB migrated v21→v22 on open.
- **The two outage rows (M):** both showed "Not on the server" with Cancel; Anton chose Retry submit for
  both. The first started the slot's tsp daemon (`tsp/slot0.sock`), wrote `.enqueued`/`.started` (the new
  `boot_id`) and runs; the second was **enqueued in tsp behind it** ("Server: queued") — a same-slot
  queue, not a 5.4c pending submit (pending needs a live blocker on the mask outside the slot's queue).
- **`%pal` stays 4 on a 24-CPU mask (M):** both inputs carry the input builder's default
  `%pal nprocs 4 end`; the remote path aligns downward only (ADR-024 o14.2), so 4 ranks run and 20 of the
  mask's 24 CPUs idle. Locally the same input is raised to the preset's rank count. Raised to Anton as a
  design fork.
- **Server memory (M, 2026-11-17T07:05Z, `free -h` + `/proc/meminfo`, two jobs' 4 ranks running):**
  `MemTotal` 65 795 232 kB (62 GiB), `MemAvailable` 62 327 852 kB (59 GiB), swap 119 GiB unused.
  Feeds ADR-025 item 7.
- **Title vs input (M):** the row titled "SP · r²SCAN-3c" holds `! r2SCAN-3c Opt Freq TightSCF` (33
  atoms) — the input was edited after the title was generated.

## Measurements

| # | What | Who / how | Feeds | Decision it settles |
|---|---|---|---|---|
| G1 | **Done-when gate**: submit a real job → close the laptop (suspend) → reopen hours later → the job is picked up, fetched, verified, parsed | Anton, app | ROADMAP 5.3 "Done when" | Unit 5.3 done |
| M1 | Fetch duration vs the 300 s bound: rsync-down + LIST of real finished job dirs (small SP, an opt+freq, the largest realistic output) | prober, raw rsync with the app's filter | o6 | keep `FETCH_TIMEOUT` 300 s or not; `.gbw` 1800 s stays unmeasured (no caller) |
| M2 | rsync rc on a **dropped link** (no RST): mid-download, the link disappears; rc, time to fail, leftovers, with and without the user-config keepalive | prober + Anton (`tailscale down` on the laptop is Anton's action) | o6, o12 open list | whether any rc needs its own handling; whether `rsync --timeout` / argv `ServerAliveInterval` is needed independent of the user config |
| M3 | `poll_log` 10 s bound and catch-up: a job whose output grows fast (> 256 KiB between polls); does the view catch up without gaps or duplicates | Anton, app (watch the log) | o item 7, o16.3 | `POLL_LOG_TIMEOUT`, cap |
| M4 | The emit under `LiveLog`'s mutex returns promptly (no stall of other steps) | app with timing: `ORCASTUDIO_TIMING=1 npm run tauri dev`, then grep `[timing]` in the dev terminal (stderr). One line per locked section (`live_log.open` / `.apply` / `.finish`; the drain is a run of `apply` steps and a `finish`) with `lock_wait_us`, `hold_us`, `emit_us`. Read the **max `hold_us` and max `lock_wait_us`** while M3's fast-growing log streams; `emit_us` close to `hold_us` means the emit dominates the hold. Prompt = tens to hundreds of µs, no `lock_wait_us` outliers. **Smoke check first:** before relying on the output in B4, open a remote job's view and expect one `[timing] live_log.open` line; none means the env var is not reaching the Rust process | o16.3 | keep emitting under the mutex or move it out |
| M5 | Quit mid-fetch: do the orphaned ssh/rsync survive, for how long, and is the local dir byte-identical to the server listing after the next (complete) fetch | Anton quits; prober watches `ps` locally and on uni | o18 | `ServerAliveInterval`/`rsync --timeout` in argv; o18 finality argument |
| M6 | ~15 jobs queued on one host: spurious `CheckFailed` from synchronized status steps; sshd `MaxStartups`/`MaxSessions` (read `/etc/ssh/sshd_config` if world-readable); cold vs warm ControlMaster | prober: 15 concurrent `ssh uni bash -s` (label-like, read-only); then Anton: 15 real queued jobs in the app | F-2 (`wiki/log.md`) | stagger the status steps only if it bites (Anton's rule) |
| M7 | Kill ORCA's main process mid-run under the wrapper: does `output.out` keep growing after `.exit_code` | prober, a small ORCA job via tsp + wrapper, `%pal` 4 | o18 finality residual | whether row 6 needs a job-session check before fetch finality |
| M8 | (optional) Does Tauri deliver an event emitted before a command's response ahead of that response | app instrumentation | ADR-024 open question (o19) | none today (o19.6 does not depend on it) |

**Order:** step 0 → step 1 → M6 raw burst, M1, M7 (prober, one session, no app writes) → G1 started
in the evening (suspend overnight) → M3, M5, M2 with Anton → M6 in the app. M4 runs with M3 (`ORCASTUDIO_TIMING=1`); M8 runs only if instrumented.
*Revised 2026-11-18 (Anton, ADR-024 o24.9):* M2, M3, M4, M5 and M8 are **waived** with their reasons
there; M6 in the app becomes the live check of the B4 fixes (o24.10); G1b stays (overnight suspend).

**Not in B4:** the 1800 s `.gbw` timeout (no production path); acting on `Lost`/`ReEnqueue` (5.4).

## G1a — submit, quit, power off, power on (2026-11-18)

G1 split in two: **G1a** is the cold restart (app quit + laptop powered off while the job runs), the
path "Done when" names; **G1b** is the suspend variant (the app process survives with stale ssh
connections), still open and wanted with a long sleep (an hour or overnight), since a short one may
not let the idle TCP connection die.

### Facts (Anton ran it in the app; prober compared both dirs read-only as `anton`; M = measured)

- **Job** `4b4972bc` "Ibuprofene", `! r2SCAN-3c Opt Freq TightSCF`, builder nprocs blank (ADR-025
  auto) → the uploaded input carries `%pal nprocs 24 end`; `output.out` "Program running with 24
  parallel MPI-processes" (M). Run time 8 min 41.9 s (`TOTAL RUN TIME`, = `.started` → `.exit_code`
  mtime, = `tsp -l` 521.98 s) (M). The same input on 4 ranks (row `f57c93ca`, 2026-11-17) took ~24 min.
- **Timeline** (laptop EET; server times corrected by the offset re-measured now, **+144.07 s**,
  three mid-point readings ±0.03 s, RTT ~0.1 s — it was +142.1 s on 2026-11-17, so the unsynced
  clock drifts ~2 s/day) (M); times to ±1 s (`last` resolution):

  | Event | Time |
  |---|---|
  | `.started` (server) / DB `started_at` | 09:19:25 / 09:19:32 |
  | laptop shutdown (`last -x`) | 09:19:54 |
  | `.exit_code` = 0 (server) | 09:28:08 |
  | laptop power-on (`last -x` reboot) | 09:34:17 |
  | DB `completed_at` (row parsed, no click) | 09:34:57 |

  The job ended 8 min 13 s after shutdown and 6 min 10 s before power-on: it finished with no app
  and no laptop. The row reached **parsed ~40 s after boot** (incl. Anton's `npm run tauri dev`) by
  the poller's resume-at-launch path, with no user action.
- **Fetch** byte-identical: all 11 local files (`.enqueued`, `.started`, `.exit_code`, `input.inp`,
  `input.xyz`, `input_trj.xyz`, `input.hess`, `input.property.txt`, `output.out`, `stderr.log`,
  `.tsp-out/ts-out.*`) match the server's in sha256, size and mtime (M). The dangling `.submitting
  -> x` claim symlink (o3.3) is on both sides with the same mtime, as on row `f57c93ca` — fetched by
  design, not a stale leftover (M; the prober's file listing skipped symlinks, the orchestrator
  listed it with `ls -la`). Nothing local-only (M).
  Server-only, excluded by the curated filter as designed: `input.gbw` (opt-in, o6),
  `input.densities`, `input.densitiesinfo`, `input.engrad`, `input.opt`, `input.bibtex` (M).
- **After:** nothing of the job running on uni; slot0 `tsp -l` shows it finished, E-level 0, queue
  empty; server boot_id unchanged since 11-12 (no server reboot in the run) (M).

### Not measured

- G1b (suspend), M1–M8. `output.out` carries the server's host name in ORCA's header; it is the
  user's own file and the app does not copy it anywhere, but it is a reason never to paste an
  `output.out` header into the wiki.

## 5.4a live — cancel of a running job (2026-11-17)

### Facts (Anton cancelled in the app; the orchestrator read the DB and the fetched dir, read-only)

- Row `e900980b` "Ibuprofen opt+freq" (4 ranks, pre-ADR-025) started on uni 09:11:18 (server
  label) and was cancelled ~19 min in, during the frequency step: `output.out` stops mid-"XC
  derivative integrals"; the server dir has `.cancelled` (09:30:55) and **no** `.exit_code` (M).
- The row is `cancelled` with "Cancelled: the server confirms .cancelled and nothing of the job runs;
  its files were downloaded as evidence."; the evidence (`output.out`, `input.property.txt`,
  `input_trj.xyz`, `input.xyz`, markers) is in the local job dir (M). `cancel_requested_at` /
  `cancel_sent_at` are NULL on the terminal row (M).
- On 2026-11-18 slot0's queue held no running task and no process of the job was found (G1a probe) (I:
  that the sweep left nothing; not checked right after the cancel).
- Not exercised: "Retry cancel" and stopped re-sends — they need a cancel that cannot reach the server;
  M2 is waived (o24.9), so this stays a hand check: Cancel on a throwaway job with the network off,
  then on.

## M6 raw burst, M1, M7 (2026-11-18)

### Facts (prober as `anton`; writes only under a probe dir on uni, removed after; M / I)

**M6 — 15 concurrent `ssh uni bash -s` (label-like, read-only), 3 reps each:**
- Client config (`~/.ssh/config`, `Host uni`): `ControlMaster auto`, `ControlPersist 10m`,
  `ServerAliveInterval 30`, `ServerAliveCountMax 4`; the app adds only `BatchMode=yes`,
  `ConnectTimeout=10` (`ssh.rs:44-61`; rsync `-e` reuses them, `sync.rs:135-137`) (M).
- Server `sshd_config` (+ `.d/`) is world-readable and sets neither `MaxStartups` nor `MaxSessions`;
  the defaults `10:30:100` / `10` then apply (M for unset; I for the defaults — the server's sshd
  version was not read; the observed drops fit them).
- **Cold** (no master): 2, 4, 2 of 15 calls fail with rc 255 (`kex_exchange_identification … reset by
  peer` / `Connection closed`) — the MaxStartups random-drop signature; 8 and 10 concurrent: 0
  failures. The rest race the master ("disabling multiplexing") and take ~1.5 s (M).
- **Warm** (master up): 0 failures in 45 calls; 5 of 15 per burst get "Session open refused by peer"
  (MaxSessions 10) and fall back to a fresh connection, ~1–2 s, rc 0, output intact (M).
- **The app does fan out:** each planned step runs on its own thread (`poller/app.rs` `spawn_step`),
  so ≥ 11 remote jobs due in one tick with no master (app start, or > 10 min idle) reproduce the cold
  burst → a spurious check failure for ~2–4 of them (I, from the code + the raw burst; the in-app
  15-job run is still to do).

**M1 — fetch vs `FETCH_TIMEOUT` 300 s (`ssh_backend.rs:921`), the app's own filter and argv
(`sync.rs:100-108,157-169`; no `--timeout`, no `-z`), into scratch dirs, 3 reps:**
- Largest real set on uni (`4b4972bc`, 11 files, 2.70 MB): rsync 4.2–10.2 s cold, 2.8–7.3 s warm;
  LIST 1.2 s cold / 0.3 s warm; re-fetch of an up-to-date dir 0.4–0.6 s (M). `f57c93ca` 2.70 MB and
  `903ea016` 1.37 MB: 1.4–4.5 s (M).
- The filter selects ~1.65 MB of `e900980b`'s 182 MB (scratch `.gbw`, densities, `*.tmp`, grids
  excluded) (M by the prober's dry run, no raw file kept; consistent with the 1.64 MB local dir).
- Synthetic 50 MB `output.out`: ~31.5–49.5 s, 1.06–1.66 MB/s (rsync stats), size-exact (M). The link is slow and
  noisy (~1–1.7 MB/s; cause not determined).
- Worst real fetch ≈ 30× under the bound; 300 s moves ~320 MB at the measured rate (I).

**M7 — `kill -TERM` of the ORCA driver only, under the installed wrapper (sha matches the source),
own tsp socket, r²SCAN-3c Opt, `%pal 4`, mask subset `0-3`:**
- The wrapper has no trap: `.exit_code` = `143` appears within the same second as the TERM, before
  the next 0.9 s sample (M).
- The 4 MPI ranks, `mpirun` and the intermediate `sh` **survive**, re-parented to pid 1 but still in
  the job's session (`ps -s <SID>`, what `collect.sh`/`cancel.sh` query) (M). They finished their
  module 5–7 s later; in a longer module they would live as long as the module (I).
- `output.out` **kept growing after `.exit_code`**: +27.5 KB over ~7 s, then constant; no
  `ORCA TERMINATED NORMALLY`, so rule #6 still classifies `Failed`, but a fetch at `.exit_code` would
  copy a truncated log (M). This is ADR-024 o18's "plausible, not seen" residual — now seen.
- tsp shows the task `finished` at once with E-Level `-113` (143 as a signed byte; `tsp -l`, M); the slot is free while the orphans still hold the pinned cores, so a queued job would start on
  them (M for tsp; I for the overlap — no second job was queued). The app does not parse tsp's
  E-Level (`remote/tsp.rs` reads rows only) (M, from source).

### Not measured

- M6 in the app (15 real queued jobs; now the B4 fixes' live check); M7 in a long module (Freq);
  G1b. M2, M3, M4, M5, M8 waived (ADR-024 o24.9).

## B4 fixes — ssh option precedence (2026-11-18)

### Facts (orchestrator, then reproduced by the DESIGN verifier; `ssh -G` resolves config, no connection; M)

- Laptop client: `ssh -V` → OpenSSH_9.6p1 (Ubuntu build).
- `ssh -G uni` → `serveraliveinterval 30`, `serveralivecountmax 4`, `controlmaster auto`,
  `controlpersist 600` (the `Host uni` block of `~/.ssh/config`).
- `ssh -G -o ServerAliveInterval=15 -o ServerAliveCountMax=3 uni` → `serveraliveinterval 15`,
  `serveralivecountmax 3`: command-line `-o` overrides the config file (feeds ADR-024 o24.5).
- Not measured: the keepalive's effect on a live, multiplexed call (the master's settings apply — I).

## Live checks of the B4 fixes and 5.4b (2026-11-18)

### Facts (Anton drove the app; the orchestrator read the DB, the fetched dirs and uni read-only; M)

- **o24.10 healthy wrapper line.** Job `a6b05a87` "test-healthy" (r²SCAN-3c Opt Freq, auto `%pal` →
  24 ranks) ran with the new wrapper; its fetched `.tsp-out/ts-out.*` holds exactly
  `wrapper: session empty at ORCA's return` — the healthy path published with no added wait (M; this
  settles o24.7's "healthy-zero is inferred" for one real job). The row reached `parsed`.
- **o25.10 check (1): wrapper killed → `lost`.** Job `d50caf4a` "Dexketoprofen" (a throwaway Anton
  made for it; Opt Freq, `%pal nprocs 15`), started 16:09:51 (laptop), its wrapper SIGKILLed while
  running; the row became terminal **`lost`** at 16:11:47 with "Lost: the job's wrapper died without an
  exit code; its files were downloaded as evidence." and the red LOST badge (Anton's screenshot) (M).
  On uni afterwards: no `orca`/`mpirun` process of the account (only the tsp daemon), no `.cancelled`,
  no `.exit_code`, `.tmp` removed (the `sweep` mode, not a cancel), tsp row `finished` with E-Level
  `-1`, `.tsp-out` empty (the wrapper died before printing) (M). The 9 fetched files (`.enqueued`,
  `.started`, `input.inp`, `input.property.txt`, `input_trj.xyz`, `input.xyz`, `output.out` cut in
  the DFT grid step, `stderr.log`, `.tsp-out/ts-out.*`) match the server's by sha256 (M); ORCA's
  `*.tmp` scratch stays on the server, as for every job.
- Not observed directly: the moment-by-moment sweep (the prober watching it was cut off by a power
  outage at the laptop); the outcome above is its end state.

## Live checks of 5.4b, 5.4c, 5.4a, M6 and 5.5 (2026-11-19)

### Facts (laptop times; the server clock runs ≈ +2 min 25 s ahead; Anton drove the app, the prober acted on uni as authorised by Anton; M = measured, I = inferred, by eye = Anton's screen)

- **5.5 live gate (Anton).** After the commit the real DB migrated v22 → v23 cleanly (93 jobs, 1 profile,
  `integrity_check` ok; backup `orcastudio.db.bak-2026-11-19`). The uni connection test recorded
  `mem_total_kib` 65 795 232 and `disk_avail_bytes` 78 360 924 160 at 08:21 (M). Anton: "everything
  works" — the New Job row, the builder default, the Settings line (by eye).
- **5.4b re-enqueue after `tsp -K` (o25.10 check 2).** Two throwaway jobs: "long" `0beee382…` running
  (A) and "short" `98b1f47d…` queued (B).
  - `tsp -K` at 08:25:32: the old daemon and B's runner were gone; A's runner and all 28 of A's
    processes stayed alive (P4 confirmed, M).
  - B's row got "Nothing of the job ever ran on the server (its queue entry was lost); it is re-enqueued
    once." 11–22 s later (M). B then waited ~9.6 min while A ran, and nothing started on A's cores (M).
    Anton saw the "Waiting for the slot…" notice in the UI (by eye).
  - 3 s after A's `.exit_code` the app claimed `.reenqueue-1`, wrote a new `.enqueued` (tsp id 0 — the
    new daemon renumbers from 0), and B started (M). B parsed (E −78.557 Eh, 41 s), A parsed
    (E −843.67 Eh, 12 min 36 s). One `.started`, one claim, no second wrapper, no leftover process (M).
  - Two findings. (1) tsp leaves `~/.orcastudio/tsp/slot0.sock.error` (A's surviving runner reporting
    ENDJOB to the dead daemon, "Broken pipe") beside the new socket — harmless to the app, which scans
    `/proc/net/unix`, not a glob (M). (2) The tsp id counter restarts at 0 with the new daemon (M).
- **M6 in the app (o24.10).** 12 live remote jobs (1 running, 11 queued), the ssh control master closed,
  the app restarted cold: a 0.2 s sampler of `ssh … -- uni bash -s` processes saw a **maximum of exactly
  8** concurrent (454 samples over 3 min; 8 at the launch burst): the cap bites (M, max 8 observed in
  454 samples). No row showed "Server unreachable" (Anton, by eye — not a measured count of lost calls).
- **5.4c pending submit (o23).** A foreign blocker `taskset -c 0 sleep 360` (own uid, pinned, meets the
  mask; `submit.sh`'s slot scan counts it). Anton's submit answered "slot busy: the core mask is held by
  pid … (pinned, cores 0 …)"; the row stayed `queued` with `submit_pending_since` frozen ~6 min and the
  job dir held only the upload (M). Anton saw the "Waiting for the slot since …" badge, no Retry, and
  the blockers' notice (by eye). 5–10 s after the blocker exited the re-check enqueued, the flag was
  cleared in the same step, and the job ran and parsed (E −155.934 Eh) (M).
- **5.4a Retry cancel with the link down (o24.9's hand check).** A running throwaway job (`c6303c88…`),
  `sudo tailscale down`, Cancel → the pending cancel recorded (`cancel_requested_at` set,
  `cancel_sent_at` NULL (M) and the unreachable state in the UI (Anton, by eye). After `tailscale up`
  the cancel was delivered: `.cancelled` on the server, the row `cancelled` with "…its files were
  downloaded as evidence." (M); no process left on uni (M).
- **Still open:** G1b (the overnight suspend); the ROADMAP ticks follow it.

## Drift found while planning (fix with the results)

- ADR-024 o18 residual "no `ServerAliveInterval`": true of the argv, not of the effective behaviour
  (user config). Whether the app should stop depending on the user config is M2/M5's decision.
  *Decided 2026-11-18 by ADR-024 o24.5: the app's argv carries its own keepalive.*
- ADR-024 o18 anchor `ssh.rs:225-228` is stale (the deadline return is now ~l.226–233).
- ROADMAP B4 names "LIST timeouts" and "1800 s with `.gbw`": LIST shares the 300 s download bound,
  and the `.gbw` branch has no production caller.
