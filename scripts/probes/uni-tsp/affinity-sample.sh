#!/bin/bash
# Probe D helper (NOT production): one sample of where every THREAD of a job's session runs.
# Usage: affinity-sample.sh <job_dir> <expected_mask as a-b>
# Prints per job: thread count, distinct current CPUs (psr), distinct Cpus_allowed_list values,
# and every thread whose psr lies outside the expected range (a violation).
set -u
job_dir=$1; lo=${2%-*}; hi=${2#*-}
sid=$(sed -n 's/^sid=//p' "$job_dir/.started")
n=0; viol=0; declare -A cpus allowed
for p in $(ps -o pid= -s "$sid"); do
  for t in /proc/$p/task/*; do
    [ -r "$t/stat" ] || continue
    # field 39 = processor last run on; strip "pid (comm)" first since comm may contain spaces
    rest=$(sed 's/^.*) //' "$t/stat" 2>/dev/null) || continue
    psr=$(awk '{print $37}' <<<"$rest")
    al=$(sed -n 's/^Cpus_allowed_list:\s*//p' "$t/status" 2>/dev/null)
    comm=$(cat "$t/comm" 2>/dev/null)
    [ -n "$psr" ] || continue
    n=$((n+1)); cpus[$psr]=1; allowed[$al]=1
    if [ "$psr" -lt "$lo" ] || [ "$psr" -gt "$hi" ]; then
      viol=$((viol+1)); echo "  VIOLATION pid=$p tid=${t##*/} comm=$comm psr=$psr allowed=$al"
    fi
  done
done
echo "$(date -u +%T) $(basename "$job_dir"): threads=$n psr={$(printf '%s\n' "${!cpus[@]}" | sort -n | paste -sd,)} allowed={$(printf '%s;' "${!allowed[@]}")} violations=$viol"
