# ADR-005: System ssh/rsync instead of SSH libraries

**Status:** accepted · 2026-07-26

## Context
Phase 5 remote execution needs SSH transport. Options: russh (Rust), paramiko/asyncssh
(Python sidecar), or shelling out to the system `ssh`/`rsync` binaries.

## Decision
Shell out to system `ssh` and `rsync` from the Rust core.

## Rationale
- Free, battle-tested support for `~/.ssh/config` aliases, ssh-agent, all key types,
  jump hosts, and ControlMaster connection multiplexing (one TCP session for many ops).
- The app never touches or stores credentials — delegation to the user's existing SSH
  setup is both simpler and safer.
- An SSH library would mean reimplementing config parsing, agent support, and host-key
  handling for zero user-visible benefit in a personal tool.

## Consequences
- Hard dependency on `openssh-client` + `rsync` (preinstalled on Mint; check at startup).
- Remote liveness = marker files (`.pid`, `.exit_code`) + byte-offset log polling,
  not a persistent channel → inherently disconnect-tolerant.
- Runner pattern: `nohup bash -c '<orca cmd>; echo $? > .exit_code' &` so jobs survive
  disconnects and laptop sleep.
