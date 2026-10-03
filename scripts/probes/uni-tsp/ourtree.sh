#!/bin/bash
# Inventory of OUR processes on the shared `yats` account (probe helper, NOT production).
# Never matches by process name — the account runs legacy ORCA installs that may be someone's job.
# A process is "ours" iff at least one holds:
#   (1) it is a tsp daemon listening on a socket under $ROOT          (from `ss -xlp`)
#   (2) it descends from such a daemon                                 (ppid walk)
#   (3) its cwd is under $ROOT/jobs                                    (catches re-parented MPI ranks)
#   (4) its session id equals a sid recorded in some $ROOT/jobs/*/.started
# Usage: ourtree.sh          → table (pid ppid pgid sid psr etime why args)
#        ourtree.sh --pids   → bare PIDs, for a reviewed kill-by-PID
set -u
ROOT=/home/yats/.orcastudio/probe
self=$$

declare -A why
for pid in $(ss -xlpn 2>/dev/null | grep -F "$ROOT/" | grep -o 'pid=[0-9]*' | cut -d= -f2 | sort -u); do
  why[$pid]+="daemon,"
done
# (1b) tsp runner processes: each enqueued task keeps a forked tsp client that execs nothing
#      and is the job's parent — identified by TS_SOCKET under $ROOT in its environment.
for d in /proc/[0-9]*; do
  { tr '\0' '\n' < "$d/environ"; } 2>/dev/null | grep -q "^TS_SOCKET=$ROOT/" || continue
  [ "$({ tr '\0' ' ' < "$d/cmdline"; } 2>/dev/null | cut -d' ' -f1)" = tsp ] && why[${d#/proc/}]+="tsp,"
done

# (2) descendants of the daemons
mapfile -t pairs < <(ps -eo pid=,ppid=)
changed=1
while [ $changed = 1 ]; do
  changed=0
  for line in "${pairs[@]}"; do
    read -r p pp <<<"$line"
    if [ -n "${why[$pp]:-}" ] && [ -z "${why[$p]:-}" ]; then
      why[$p]+="child,"; changed=1
    fi
  done
done

# (3) cwd under $ROOT/jobs
for d in /proc/[0-9]*; do
  p=${d#/proc/}
  cwd=$(readlink "$d/cwd" 2>/dev/null) || continue
  [[ "$cwd" == "$ROOT/jobs"* ]] && why[$p]+="cwd,"
done

# (4) recorded session ids (skip our own session: this script runs inside an ssh session)
mysid=$(ps -o sid= -p $self | tr -d ' ')
for f in "$ROOT"/jobs/*/.started; do
  [ -f "$f" ] || continue
  s=$(sed -n 's/^sid=//p' "$f")
  [ -n "$s" ] && [ "$s" != "$mysid" ] || continue
  for p in $(ps -eo pid=,sid= | awk -v s="$s" '$2==s{print $1}'); do why[$p]+="sid,"; done
done

unset 'why[$self]'
for p in "${!why[@]}"; do [ -d /proc/$p ] || unset 'why[$p]'; done

if [ "${1:-}" = "--pids" ]; then
  printf '%s\n' "${!why[@]}" | sort -n
  exit 0
fi
printf '%-8s %-8s %-8s %-8s %-4s %-10s %-18s %s\n' PID PPID PGID SID PSR ELAPSED WHY ARGS
for p in $(printf '%s\n' "${!why[@]}" | sort -n); do
  info=$(ps -o ppid=,pgid=,sid=,psr=,etime= -p "$p" 2>/dev/null) || continue
  args=$(ps -o args= -p "$p" | cut -c1-90)
  read -r pp pg sd psr et <<<"$info"
  printf '%-8s %-8s %-8s %-8s %-4s %-10s %-18s %s\n' "$p" "$pp" "$pg" "$sd" "$psr" "$et" "${why[$p]%,}" "$args"
done
