# 005-stale-sidecar-hmr.md — the app talks to a STALE sidecar, and says "Not Found"

**Date:** 2026-07-29 · **Area:** frontend ↔ sidecar lifecycle
**Symptom:** After adding a sidecar endpoint (e.g. `POST /geometry/set-internal` in 2.5.2c) and
continuing in the SAME `npm run tauri dev` window, calling it from the UI returns **HTTP 404 with
body `{"detail":"Not Found"}`** — the user sees a bare **"Not Found"** in the middle of a working
scenario, with no hint why. The endpoint exists in the code; the tests pass; a fresh app start fixes
it. Reproduced, not hypothesised.

**Root cause:** `npm run tauri dev` hot-reloads only the **frontend**. `SidecarManager::start` is
called once at Rust startup (`lib.rs`), and it launched uvicorn **without `--reload`**. So after you
add an endpoint, the hot-reloaded frontend already calls the new route, but the Python process is
still executing the code it loaded when the window opened → the route doesn't exist yet → 404. The
frontend had **no way to know** the sidecar was behind, and FastAPI's default 404 body is the
unhelpful `"Not Found"`.

Reproduce (before the fix):

```
# with the app running, hit an endpoint the running build doesn't have:
curl -s -o /dev/null -w "%{http_code}\n" http://127.0.0.1:<port>/geometry/set-torsion   # 404
curl -s http://127.0.0.1:<port>/geometry/set-torsion                                    # {"detail":"Not Found"}
curl -s http://127.0.0.1:<port>/openapi.json | jq '.paths | keys'                        # route absent
```

**Fix — three layers (2.5.2d-1):**

1. **Version handshake.** `sidecar/app/__init__.py` `__version__` bumps its **minor** on every
   endpoint add/change (the rule; recorded in `wiki/modules/sidecar.md`). The Rust core holds
   `EXPECTED_MIN_SIDECAR_VERSION` and, after `/health` answers, parses the reported `version` and
   compares it **component-wise as numbers** (`version_at_least` — string comparison is wrong:
   `"0.10.0" < "0.9.0"` lexically). Older → a new `Health::Stale` state (distinct from `Down`: the
   process is alive, just old). The status bar shows **"Sidecar STALE — restart the app"**
   prominently (a warning band, not a tooltip), with the running vs expected version.

2. **Human error message.** One client (`src/sidecar-client.ts`) wraps all three sidecar callers
   (EditPanel, import-file, smiles). The pure, tested `describeSidecarError` maps: **404** →
   *"the sidecar has no route <route> — it is running an older build. Restart the app."*; **422** →
   the `detail` verbatim (validation, already human); **5xx** → the `detail` prominently (a real
   post-condition breach); **network / not running** → *"the chemistry sidecar isn't running."*

3. **`--reload` in dev.** In a **debug build only** (`cfg!(debug_assertions)`), uvicorn is launched
   with `--reload --reload-dir app`, so a Python edit is picked up without restarting the window —
   the sidecar now behaves like the frontend HMR. The trap can still occur if you forget to bump the
   version *before* the reload catches up, but the handshake + message make it legible instead of
   silent.

**The `--reload` orphan risk (and why it's safe) — the debugging/004 pattern.** `--reload` runs a
supervisor that spawns a **worker child**; killing only the parent handle (`child.kill()`) would
orphan the worker, which keeps holding the port — exactly the shape of `debugging/004` (MPI ranks
escaping the process group). Fix: `start` puts the child in its **own process group**
(`CommandExt::process_group(0)`), and `stop`/`Drop` `killpg` the whole group (SIGTERM, short grace,
SIGKILL backstop). **Verified live:** under `--reload` the tree is `supervisor + resource_tracker +
worker`, all sharing the leader's pgid; `kill -TERM -<pgid>` reaped all three, `pgrep uvicorn` was
empty and the port released. Also verified the benefit: `touch app/geometry.py` →
`WatchFiles detected changes … Reloading` and a new worker, `/health` still serving.

**Lesson / rule:** a hot-reloading frontend and a **non**-hot-reloading backend drift apart
silently. When "it 404s but the code is right and a restart fixes it", suspect a stale process, not a
routing bug. Guard the seam with a **version handshake** (numeric compare, never string), turn the
backend's raw errors into human messages at **one** wrapper, and give the dev backend the same
hot-reload the frontend has — killing its whole process group so no worker is orphaned. This trap
would have recurred on 2.5.3, the constraint manager, and xTB.
