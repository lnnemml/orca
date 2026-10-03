#!/bin/bash
# Probe prototype of the ADR-024 per-job wrapper (NOT production code).
# Enqueued through task-spooler:  tsp orca-job-wrapper.sh <job_dir> <cpu_mask>
#
#   1. FIRST action: write .started (PID, PGID, SID, boot_id, start time)   — ADR-024 d
#   2. run ORCA by absolute path (rule #1), pinned to <cpu_mask> with OpenMPI's
#      own binding disabled (rule #8), stdout/stderr into the job dir (ADR-024 b)
#   3. LAST action: write .exit_code                                         — rule #6
#
# A signal that kills this shell before step 3 leaves no .exit_code (ADR-024 i).
set -u
job_dir=$1
mask=$2
cd "$job_dir" || exit 97

# /proc/self/stat fields 5 and 6 = pgrp, session (comm is "bash", no spaces).
read -r _ _ _ _ pgid sid _ < /proc/$$/stat
printf 'pid=%s\npgid=%s\nsid=%s\nboot_id=%s\nstarted_at=%s\n' \
  "$$" "$pgid" "$sid" "$(cat /proc/sys/kernel/random/boot_id)" "$(date -u +%FT%TZ)" \
  > .started.tmp && mv .started.tmp .started

# Probe-only diagnostics: what environment does a tsp task actually get?
{
  echo "PATH=$PATH"
  echo "mpirun=$(command -v mpirun)"
  echo "TMPDIR=${TMPDIR-<unset>}"
  echo "ppid=$PPID ppid_cmd=$(tr '\0' ' ' < /proc/$PPID/cmdline)"
  echo "cgroup=$(cat /proc/$$/cgroup)"
} > .wrapper_env

OMPI_MCA_hwloc_base_binding_policy=none taskset -c "$mask" \
  /opt/orca/orca input.inp > output.out 2> stderr.log
rc=$?

echo "$rc" > .exit_code.tmp && mv .exit_code.tmp .exit_code
exit "$rc"
