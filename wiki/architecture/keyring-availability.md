# Measured: system keyring availability on this host

**Measured 2026-08-05** on the dev machine (Linux Mint / Cinnamon). Rule #10: whether a
keyring backend works here is a third-party fact accepted only from a run, not from docs.
This page records the probe that gates [ADR-015](adr-015-api-key-storage.md). The probe lived
in a throwaway `/tmp/keyring-probe` cargo project — `src-tauri/Cargo.toml` was **not** touched.

## Host environment (observed)

- `gnome-keyring-daemon --foreground --components=pkcs11,secrets` running (pid 1552).
- D-Bus name **`org.freedesktop.secrets`** owned by that daemon; session bus at
  `unix:path=/run/user/1000/bus`.
- `libsecret-1.so.0` present — but, as the crate choice below shows, **not needed**.

## Crate options (from a real crates.io resolve, with versions)

| crate | version | what it is | verdict |
|---|---|---|---|
| **`keyring`** | **4.1.6** | mature all-in-one (hwchen/keyring-rs). Default Linux backend `zbus-secret-service` is **pure Rust** | **chosen** |
| `tauri-plugin-keyring` | 0.1.0 | thin community wrapper over the same `keyring` crate | pre-1.0 indirection |
| `tauri-plugin-keyring-store` | 0.2.0 | community | pre-1.0 |
| `tauri-plugin-keychain` | 2.0.2 | community | indirection |
| `tauri-plugin-stronghold` | 2.3.1 | **official** Tauri plugin — IOTA Stronghold, an **encrypted local file** under a password | **rejected**: still a file that rides in the backup, just encrypted (see ADR-015) |

`keyring 4.1.6` default features already include `zbus-secret-service-keyring-store`; the
dependency tree pulls `secret-service 5.1.0` → `zbus 5.18` (pure Rust) with **no libsecret /
libdbus C dependency** — so no `-dev` package is required in the build.

## Probe results (service name `orcastudio`)

Happy path (`set` → `get` → `delete` → `get`):

```
[1] Entry::new OK
[2] set_password OK
[3] get_password OK, matches_written=true      ← byte-equality post-condition (rule #9)
[4] delete_credential OK
[5] get after delete -> Error::NoEntry          ← correct "key absent" signal
```

## The measurement artifact: `env -u DBUS_SESSION_BUS_ADDRESS` does NOT simulate absence

The obvious way to simulate a missing Secret Service — unset `DBUS_SESSION_BUS_ADDRESS` —
**lies**. zbus still finds the session bus via `$XDG_RUNTIME_DIR/bus` (`/run/user/1000/bus`
exists regardless of the env var), connects to gnome-keyring, and returns `NoEntry`. A naive
test would have concluded "absence → NoEntry", sent the code down the *key-absent* branch
instead of the *service-absent* branch, and the fallback would **never** have fired — a defect
invisible on this host (which always has Secret Service) and only visible on a foreign user's
machine that lacks it. The measurement was aimed at the adjacent condition.

Genuine absence, forced by pointing at a dead socket:

| run | how | result |
|---|---|---|
| `env -u DBUS_SESSION_BUS_ADDRESS` | unset the env var | `NoEntry` — **artifact**, bus found via `$XDG_RUNTIME_DIR/bus` |
| `DBUS_SESSION_BUS_ADDRESS=unix:path=/nonexistent/...` | bogus bus address | **`Error::NoDefaultStore`** at `Entry::new` — genuine "no backend", clean `Err`, no panic |

**`NoDefaultStore` ≠ `NoEntry`, and that distinction is load-bearing** (ADR-015 (2)): it is the
difference between "the keyring works but holds no key" (→ ask the user to enter one) and "there
is no keyring backend at all" (→ fall back to the environment variable and say so). Without the
distinction the UI cannot tell the truth about *why* it has no key.

## `keyring::Error` surface (from `keyring-core 1.0.0`)

`NoEntry`, `NoDefaultStore`, `NoStorageAccess(PlatformError)`, `PlatformFailure(PlatformError)`,
`Ambiguous(Vec<Entry>)`, `Invalid`, `BadEncoding`, `BadDataFormat`, `BadStoreFormat`, `TooLong`,
`NotSupportedByStore`. Every failure is a structured `Err`; **no path panics**.

## Not tested on this host (named, with reason)

**A locked keyring was not force-tested.** Locking gnome-keyring's login collection would (a)
pop a GUI unlock prompt on next access — which can hang an automated run — and (b) leave the
user's **real** stored passwords locked until re-entry: a disruptive test on a working machine,
not worth the candle. Structurally the `keyring` backend auto-unlocks a collection before
reading and, on unlock failure, returns `NoStorageAccess` / `PlatformFailure` — again an `Err`,
not a panic, consistent with the two `Err` paths observed live. For the fallback decision a
locked keyring is grouped with "keyring unusable" (→ environment / unavailable), while the
specific error string is what the UI shows so the cause is named.
