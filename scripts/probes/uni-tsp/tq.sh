#!/bin/bash
# tsp against one of OUR queues — never the account's default socket (probe helper).
# Usage: tq.sh <slot> [tsp args...]          e.g.  tq.sh 0 -l
#        tq.sh <slot> submit <job> <input> <mask>
#   submit: creates $ROOT/jobs/<job>/ with input.inp, enqueues the wrapper, prints the tsp id.
set -u
ROOT=/home/yats/.orcastudio/probe
slot=$1; shift
export TS_SOCKET=$ROOT/slot$slot.sock
export TMPDIR=$ROOT/tsp-tmp        # tsp's own output files + nothing else of ours lands in /tmp
mkdir -p "$TMPDIR"

if [ "${1:-}" = "submit" ]; then
  job=$2; input=$3; mask=$4
  dir=$ROOT/jobs/$job
  [ -e "$dir" ] && { echo "job dir exists: $dir" >&2; exit 1; }
  mkdir -p "$dir" && cp "$input" "$dir/input.inp"
  exec tsp -L "$job" "$ROOT/bin/orca-job-wrapper.sh" "$dir" "$mask"
fi
exec tsp "$@"
