# ADR-015: Anthropic API key — where the secret lives and where the call is made

**Status:** accepted · 2026-08-05
**Narrows:** [ADR-014](adr-014-ai-integration-boundary.md) — ADR-014 (2) describes the T1 tier's
authority as a *property* ("read-only, grounded on manual + parsed results"); this ADR makes T1 a
*construction* — the command accepts exactly three fields and has no wider reach. Same precedent as
ADR-012 narrowing ADR-002 and ADR-013 narrowing ADR-006: **ADR-014 is not edited** — it gains an
amendment pointing here.
**Precedent:** [ADR-005](adr-005-system-ssh.md) — we did **not** implement SSH; we shelled out to the
system `ssh` precisely so the app never holds credentials. Same principle applied to one secret we
*do* have to name: give it to the OS.

## Context

Phase 4's "Explain with Claude" (T1 of ADR-014) needs an Anthropic API key. Two questions the
ROADMAP left open: **where does the secret live at rest**, and **where is the network call made**.
Both have a failure mode that is *invisible at the moment it happens* — which is what makes them
worth an ADR rather than a line of code:

- **A key that crosses into the webview stays in the renderer's memory** and surfaces in the first
  diagnostic `console.log`. We placed exactly such logs **twice in two weeks** debugging the manual
  hover ([debugging/010](../debugging/010-hover-clipped-on-top-line.md)). A secret in renderer scope
  is one careless log away from the devtools console and any crash report.
- **A key written into `orcastudio.db` rides in every copy of that file** — the backup, the second
  machine, the archive mailed to a colleague. `orcastudio.db` is a file that gets *copied*; a plaintext
  secret inside it leaves with every copy, silently.

Neither loss is visible when it happens. That asymmetry — cheap to prevent, expensive and quiet to
discover — is the whole reason for the three decisions below.

## Decision

### (1) The key never crosses the boundary into the webview

The call to Anthropic is made by **Rust**. The frontend sends a request (the selection to explain)
and receives a response (the explanation text); it never sees the key. This is **not a separate act
of caution** — it is the direct consequence of two standing decisions:

- [ADR-009](adr-009-process-orchestration.md): external processes belong to Rust. Network egress is
  the same class of side-effecting orchestration.
- [ADR-013](adr-013-manual-indexing-ownership.md) (2) + `overview.md` §"Security / privacy posture":
  network egress is a **deliberate, enumerated** set of paths, not something scattered across the app.
  The posture already names "(b) the optional Anthropic 'explain' feature with the user's own key" as
  one of exactly two allowed egress points. This ADR *implements* that clause; it does not add a path.

A key handled only in Rust never enters renderer scope, so no frontend `console.log`, crash dump, or
devtools session can leak it. The webview is treated as untrusted for this secret **by construction**,
not by a discipline someone has to remember.

### (2) Storage is the system keyring (Secret Service), with an explicit environment fallback

The key is stored in and read from the OS credential store via the **`keyring` crate 4.1.6** (Linux
backend: pure-Rust `zbus-secret-service`; measured working on this host —
[keyring-availability.md](keyring-availability.md)). The precedent is ADR-005: hand the secret to the
OS and keep only a thin wrapper of our own.

**Why not `settings` in SQLite**, which would be the cheapest option and the most consistent with
[ADR-004](adr-004-sqlite-storage.md) (one store): because `orcastudio.db` is **a file that gets
copied** — backup, second machine, archive to a colleague — and a plaintext key leaves with every
copy. The threat is domestic, not hypothetical. A silent fallback to plaintext-in-the-DB is
**forbidden** under any condition (see the fallback below).

**The honest boundary, carried in the decision so it is not oversold.** The keyring protects the key
**on copy and at rest** — it is exactly the copied-file threat above that it defeats. It does **not**
protect against code running as the same user in an unlocked session: Secret Service will hand the
secret to whoever asks. We do not sell it as more than "not in the backup."

**The state model is structural, not a UI detail** (this is the correction the measurement forced —
the fallback's trigger is a *third* state that a two-state "have key / no key" model cannot express).
The Rust resolver reports exactly one of four states to the frontend, derived from the keyring
outcome, and **never returns the key itself** (optionally the last 4 characters, for recognition):

| state | derived from | meaning / UI |
|---|---|---|
| `stored-in-keyring` | `get_password` → `Ok` | key present; source = system keyring |
| `absent` | keyring usable, `Err(NoEntry)` | keyring works, no key stored → prompt the user to enter one |
| `from-environment` | keyring **unusable** *and* `ANTHROPIC_API_KEY` set | key taken from the env var; **say so in the UI** |
| `unavailable` | keyring **unusable** *and* env unset | **name both causes** ("Secret Service unavailable *and* `ANTHROPIC_API_KEY` not set") |

"keyring unusable" = `NoDefaultStore` (no backend — the measured genuine-absence signal) or a hard
access error (`NoStorageAccess` / `PlatformFailure`, e.g. a locked collection). The specific error
string is surfaced so the *cause* is named, not flattened to "unavailable". The environment fallback
is consulted **only** when the keyring is unusable — a working-but-empty keyring is `absent`, not
`from-environment`; the env var is a fallback for *unavailability*, not an override of an empty store.

**The fallback is visible, never silent** (NULL-anchor posture: name the absence, do not substitute
something worse without saying so). Secret Service unavailable → read `ANTHROPIC_API_KEY` from the
environment **and show that this is the source in the UI**. A silent write of the key to the DB as a
"fallback" is not permitted.

**Rejected alternative — `tauri-plugin-stronghold` (2.3.1, official).** Recorded because the official
plugin is the obvious future candidate and, without the reason written down, someone will return to
it. Stronghold is an **encrypted local file** under a password. That does not remove the threat of
(2) — it *relocates* it: the encrypted file still rides in the backup and the archive, and it adds a
*second* secret (the unlock password) to store somewhere. An encrypted copied file is still a copied
file. The keyring hands the secret to the OS's own store, which is not part of our copied data set.

### (3) What goes on the wire is explicit and minimal

The request context will naturally *want* to include the input file, and with it the geometry.
**Geometry is unpublished research work** — coordinates the author has not released. For T1 the wire
payload is: **the selected word, the line around it, and the section text.** **Not** the whole file,
**not** the coordinates. Any expansion of what is sent is a **separate, deliberate change with visible
consent** — not a default that grows quietly.

This is the same posture as ADR-014 (1): the AI reads what deterministic code emits; here we bound
*what it is even shown*.

## Consequence for ADR-014 (the structural closing of T1)

ADR-014 (2) described the tier boundaries as a **property** of each tier ("T1 = read-only"). The
review note against it was that a boundary described as a property, not built as one, relies on
everyone respecting it. This ADR closes that for T1: the explain command **accepts exactly the three
fields of (3)** and has no parameter for the file or the coordinates — the boundary is the command's
*type*, enforced by the compiler, not by intent. ADR-014 gains an **amendment** pointing here; its
decision text is unchanged.

## Consequences

- **New dependency:** `keyring = "4.1.6"` in `src-tauri/Cargo.toml` (pure-Rust Secret Service backend;
  no C `-dev` package). **No new HTTP dependency:** `ureq = "2"` is already present (used by
  `sidecar.rs`) and its default features carry rustls TLS — sufficient for `https://api.anthropic.com`.
- **Commands (tauri-core):** `set_api_key(key)`, `delete_api_key()`, `api_key_status() -> KeySource`
  (the four-state enum above), and a minimal `verify_api_key()` (a real, minimal Anthropic call behind
  the "Check" button, mirroring xtb's `Check`). **No command returns the key to the frontend.**
- **Network module (tauri-core):** a single outbound module for the Anthropic call — one egress point,
  timeout, a clear offline error (offline is a normal mode here, not a failure), model + API version in
  a commented constant, and **no logging of the key or the request body, even in debug**.
- **Settings UI (frontend):** a password field + Save / Delete / Check; the field is cleared
  immediately after Save (do not hold the value in state longer than needed); the **source of the key
  is shown explicitly** ("from the system keyring" / "from `ANTHROPIC_API_KEY`"). In the `unavailable`
  and `absent` states the **Check button is disabled** rather than issuing a network call that would
  report a misleading "could not reach Anthropic" when the real problem is that there is no key to
  check — different causes, different messages.
- `wiki/architecture/overview.md` §"Security / privacy posture" — the "(b) … with the user's own key"
  clause gains a pointer to this ADR for *how* the key is stored and *where* the call is made.
- **ROADMAP:** "Explain with Claude" is no longer marked *Optional* — CLAUDE.md states explanations
  are a first-class feature, not an extra.
- **Not built by this ADR:** the Explain button in the selection panel and the full explain payload are
  the **next** unit; this ADR ships storage + the minimal verify call only.

## Unverified path (named, per rule #9 ethos)

A **locked keyring** was not force-tested on this host (it would disrupt the user's real credential
store and can hang on a GUI unlock prompt — see [keyring-availability.md](keyring-availability.md)).
The structural expectation is `NoStorageAccess` / `PlatformFailure` — an `Err`, not a panic — handled
by the same "keyring unusable" branch as `NoDefaultStore`.
