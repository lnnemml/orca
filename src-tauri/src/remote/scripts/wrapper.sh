
# ---- wrapper.sh <job_dir> <mask> <orca_path> ------------------------------------------------
# Runs one ORCA job inside its job dir, launched by the slot's tsp (which makes it PID = PGID =
# SID, probe P2). The start sequence is ADR-024 (l), in this order and no other:
#   0  cd into the job dir            — on failure exit before .started: ORCA never runs elsewhere
#   1  publish .started, no-clobber   — temp file + `ln -T`, BEFORE the .cancelled check (d′ race);
#                                       a .started that already exists → refuse: exit 1, no ORCA,
#                                       neither marker touched (ADR-024 l, Part B detail 5)
#   1a read .started back and check it — on failure .exit_code = 97 (by rename), no ORCA
#   2  .cancelled present             — exit: no ORCA, no .exit_code
#   3  <job>/.tmp, env + pinned ORCA  — .tmp cannot be made → .exit_code = 96, no ORCA (detail 6);
#                                       TMPDIR in the job dir, taskset mask, OpenMPI binding off
#   4  publish .exit_code             — temp file + rename, never a half-written file

# Exit codes the wrapper writes itself (our own choice, not ORCA codes): its .started fails the
# read-back (step 1a); <job>/.tmp cannot be created (step 3).
SELF_CHECK_FAILED=97
TMP_DIR_FAILED=96

# publish NAME CONTENT — write a marker in the cwd (the job dir) by temp file + rename, so a
# reader sees either no NAME or all of CONTENT (ADR-024 l: every marker by rename).
publish() {
    local tmp=".$1.tmp.$$"
    if printf '%s' "$2" >"$tmp" && mv -fT -- "$tmp" "$1"; then
        return 0
    fi
    rm -f -- "$tmp"
    return 1
}

# publish_started CONTENT — publish .started without ever replacing one: `ln -T` is a single
# linkat(2), which fails with EEXIST if the name exists in any form (file, directory, dangling
# symlink; measured, coreutils 9.4). So test and creation are one atomic step and two wrappers can
# never both start in one job dir. Returns 0 (published), 1 (not published and a .started
# exists in any form — whatever failed, the wrapper must touch nothing) or 2 (not published and
# no .started exists — the step-1a check then reports it with 97).
publish_started() {
    local tmp=".started.tmp.$$" rc=0
    if ! { printf '%s' "$1" >"$tmp" && ln -T -- "$tmp" .started 2>/dev/null; }; then
        rc=2
        [[ -e .started || -L .started ]] && rc=1
    fi
    rm -f -- "$tmp"
    return "$rc"
}

wrapper_main() {
    if (( $# != 3 )); then
        echo "wrapper: usage: wrapper.sh <job_dir> <mask> <orca_path>" >&2
        exit 2
    fi
    local job_dir=$1 mask=$2 orca=$3 stat_line boot now expected rc
    if ! valid_path "$job_dir"; then
        echo "wrapper: job dir is not a valid absolute path: $job_dir" >&2
        exit 2
    fi
    if [[ ! $mask =~ ^[0-9]+([,-][0-9]+)*$ ]]; then
        # Validated before anything is written, like a bad job dir: a value starting with "-"
        # would be parsed by taskset as an option (ADR-024 l, Part B item 8).
        echo "wrapper: invalid core mask: $mask" >&2
        exit 2
    fi
    if [[ $orca != /* ]]; then
        # Rule #1: ORCA by its absolute path, or OpenMPI parallelisation silently fails.
        echo "wrapper: ORCA path is not absolute: $orca" >&2
        exit 2
    fi

    # Step 0.
    cd -- "$job_dir" || exit 1

    # Step 1. Own ids from the builtin read of /proc/$$/stat (probe 5.2c). A failed read leaves
    # empty values, which the step-1a check rejects.
    stat_line='' boot=''
    read -r stat_line </proc/$$/stat
    parse_stat_line "$stat_line"
    read -r boot </proc/sys/kernel/random/boot_id
    printf -v now '%(%s)T' -1
    printf -v expected 'pid=%s\npgid=%s\nsid=%s\nboot_id=%s\nstarttime=%s\nstarted_at=%s\n' \
        "$$" "$ST_PGRP" "$ST_SID" "$boot" "$ST_START" "$now"
    publish_started "$expected"
    if (( $? == 1 )); then
        echo "wrapper: refusing: $job_dir/.started already exists" >&2
        exit 1
    fi

    # Step 1a (rule #9): the marker on disk must parse and say what we meant to write.
    if ! started_parse_file .started \
        || [[ $S_PID != "$$" || $S_PGID != "$ST_PGRP" || $S_SID != "$ST_SID" \
            || $S_BOOT != "$boot" || $S_START != "$ST_START" || $S_AT != "$now" ]]; then
        echo "wrapper: .started self-check failed: ${S_WHY:-content differs}" >&2
        publish .exit_code "$SELF_CHECK_FAILED"$'\n'
        exit "$SELF_CHECK_FAILED"
    fi

    # Step 2.
    if [[ -e .cancelled || -L .cancelled ]]; then
        exit 0
    fi

    # Step 3. TMPDIR in the job dir keeps OpenMPI's session files inside it (rule #3); taskset
    # pins every ORCA process to the slot's mask and OpenMPI's own binding is off (rule #8).
    if ! mkdir -p -- .tmp; then
        # A TMPDIR outside the job dir would let OpenMPI litter escape it (rule #3).
        publish .exit_code "$TMP_DIR_FAILED"$'\n'
        exit "$TMP_DIR_FAILED"
    fi
    TMPDIR=$job_dir/.tmp HWLOC_COMPONENTS=-gl OMPI_MCA_hwloc_base_binding_policy=none \
        taskset -c "$mask" "$orca" input.inp >output.out 2>stderr.log
    rc=$?

    # Step 4.
    publish .exit_code "$rc"$'\n'
    exit "$rc"
}

wrapper_main "$@"
