
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
#   4  the session wait               — ADR-024 o24.7: under a TERM trap, wait for the session's
#                                       other processes to end (grace, TERM+CONT, KILL), each
#                                       action printed on stdout (tsp's .tsp-out)
#   5  publish .exit_code             — ORCA's rc, temp file + rename, never a half-written file

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

# ---- step 4, the session wait (ADR-024 o24.7) ----------------------------------------------
# A killed ORCA driver leaves its MPI ranks, mpirun and a shell running in the session, still
# pinned to the slot's cores and still writing output.out (B4 M7), and tsp starts the next job
# the moment the wrapper exits. So .exit_code is published only once the session is empty. Every
# action prints one line on stdout, always before the publish: a fetch may follow it at once.

SESSION_GRACE=10      # s the members get to end on their own (o24.7)
SESSION_TERM_WAIT=5   # s after TERM+CONT, before KILL
SESSION_KILL_WAIT=1   # s after KILL for the kernel to finish them, before survivors are named
SESSION_STEP=0.2      # every wait sleeps in steps this short: bash runs a trap only after a
STEPS_PER_S=5         # foreground command returns, so a per-PID TERM must not wait out a long sleep

# session_leader — true iff the wrapper leads its own session and group: $$ = the SID = the PGID
# that .started recorded (step 1a checked them against /proc/$$/stat; tsp's setsid, probe P2).
# Only then does the wrapper list or signal its session; otherwise the session is someone else's.
session_leader() {
    [[ $S_SID == "$$" && $S_PGID == "$$" ]]
}

# session_members — one snapshot `ps -o pid=,ppid=,stat=,comm= -s <SID>`, the SID .started
# recorded (called only once session_leader holds). Sets MEMBERS (pids) and MEMBERS_TEXT
# ("pid stat comm, …") to every process of the session except the wrapper, its own descendants
# within the snapshot (this `ps`, a `sleep`, a subshell) and zombies. Every other member counts,
# whatever its cwd: a pinned process holds cores wherever it runs. The listing must contain the
# wrapper itself (rule #9). Returns 0, or 1 with LIST_ERR.
session_members() {
    local out rc line i changed
    local re='^ *([1-9][0-9]*) +([0-9]+) +([^ ]+) *(.*)$'
    local -a pid=() ppid=() stat=() comm=()
    local -A mine=()
    MEMBERS=() MEMBERS_TEXT='' LIST_ERR=''
    out=$(LC_ALL=C ps -o pid=,ppid=,stat=,comm= -s "$S_SID" 2>&1)
    rc=$?
    if (( rc != 0 )); then
        LIST_ERR="ps -s $S_SID failed: rc=$rc ${out//$'\n'/ }"
        return 1
    fi
    while IFS= read -r line; do
        if [[ ! $line =~ $re ]]; then
            LIST_ERR="ps -s $S_SID: not a process line: '$line'"
            return 1
        fi
        pid+=("${BASH_REMATCH[1]}") ppid+=("${BASH_REMATCH[2]}")
        stat+=("${BASH_REMATCH[3]}") comm+=("${BASH_REMATCH[4]}")
    done <<<"$out"
    if [[ " ${pid[*]} " != *" $$ "* ]]; then
        LIST_ERR="ps -s $S_SID: the wrapper itself is not listed"
        return 1
    fi
    mine[$$]=1
    changed=1
    while (( changed )); do
        changed=0
        for i in "${!pid[@]}"; do
            if [[ -z ${mine[${pid[i]}]+set} && -n ${mine[${ppid[i]}]+set} ]]; then
                mine[${pid[i]}]=1
                changed=1
            fi
        done
    done
    for i in "${!pid[@]}"; do
        [[ -n ${mine[${pid[i]}]+set} || ${stat[i]} == Z* ]] && continue
        MEMBERS+=("${pid[i]}")
        MEMBERS_TEXT+="${MEMBERS_TEXT:+, }${pid[i]} ${stat[i]} ${comm[i]}"
    done
}

# session_wait_for SECONDS — re-list every SESSION_STEP until the session is empty or SECONDS have
# passed. Returns 0 (empty; WAITED = the seconds slept, one decimal), 1 (members remain) or 2
# (ps failed, LIST_ERR).
session_wait_for() {
    local step steps=$(( $1 * STEPS_PER_S )) tenths
    for (( step = 1; step <= steps; step++ )); do
        sleep "$SESSION_STEP"
        session_members || return 2
        if (( ${#MEMBERS[@]} == 0 )); then
            tenths=$(( step * 10 / STEPS_PER_S ))
            WAITED="$(( tenths / 10 )).$(( tenths % 10 ))"
            return 0
        fi
    done
    return 1
}

# say LINE — print one line on stdout, unless SESSION_QUIET is set: the TERM trap runs the same
# sequence after its own publish and prints nothing there (every line precedes .exit_code).
SESSION_QUIET=0
say() {
    (( SESSION_QUIET )) || printf '%s\n' "$1"
}

# session_round "SIGNAL..." SECONDS — re-list right before signalling (as cancel.sh does), send
# each SIGNAL to each member by its own PID (never to a group), then wait up to SECONDS. Returns 0
# if members remain (MEMBERS current), 1 if the wait is over (a line says why).
session_round() {
    local sig pid label=${1// /+}
    if ! session_members; then
        say "wrapper: $LIST_ERR; publishing now"
        return 1
    fi
    if (( ${#MEMBERS[@]} == 0 )); then
        say "wrapper: session empty before $label"
        return 1
    fi
    say "wrapper: $label $MEMBERS_TEXT"
    for sig in $1; do
        for pid in "${MEMBERS[@]}"; do
            kill "-$sig" "$pid" 2>/dev/null
        done
    done
    session_wait_for "$2"
    case $? in
        0) say "wrapper: session empty ${WAITED} s after $label"; return 1 ;;
        2) say "wrapper: $LIST_ERR; publishing now"; return 1 ;;
    esac
    return 0
}

# session_grace — SESSION_GRACE_S: SESSION_GRACE, or ORCASTUDIO_TEST_SESSION_GRACE when that is a
# decimal below it. The tests' only knob: it can only shorten the wait, and production never sets
# it (the wrapper inherits tsp's environment, hence the specific name).
session_grace() {
    SESSION_GRACE_S=$SESSION_GRACE
    if dec_norm "${ORCASTUDIO_TEST_SESSION_GRACE-}" && (( ${#REPLY} <= 2 && REPLY < SESSION_GRACE )); then
        SESSION_GRACE_S=$REPLY
    fi
}

# session_wait — step 4: none left → publish at once; otherwise wait up to the grace for them to
# end on their own (letting them end keeps output.out complete as evidence), then TERM+CONT, up to
# SESSION_TERM_WAIT, then KILL. A survivor of KILL (D state) does not block the publish: it is
# named. A `ps` error, or a session the wrapper does not lead, publishes at once with a note.
session_wait() {
    if ! session_leader; then
        echo "wrapper: session wait skipped: the wrapper (pid $$) does not lead its session (sid $S_SID, pgid $S_PGID); publishing now"
        return
    fi
    if ! session_members; then
        echo "wrapper: session wait skipped: $LIST_ERR; publishing now"
        return
    fi
    if (( ${#MEMBERS[@]} == 0 )); then
        echo "wrapper: session empty at ORCA's return"
        return
    fi
    say "wrapper: ORCA returned $SESSION_RC; session members left: $MEMBERS_TEXT"
    session_escalate
}

# session_escalate — the members are listed and some are left: wait up to the grace for them to
# end, then TERM+CONT and up to SESSION_TERM_WAIT, then KILL and up to SESSION_KILL_WAIT. Bounded:
# at most grace + 5 + 1 s. Shared by the normal path and the TERM trap's hold.
session_escalate() {
    session_grace
    say "wrapper: waiting up to $SESSION_GRACE_S s for them to end"
    session_wait_for "$SESSION_GRACE_S"
    case $? in
        0) say "wrapper: session empty after ${WAITED} s"; return ;;
        2) say "wrapper: $LIST_ERR; publishing now"; return ;;
    esac
    session_round "TERM CONT" "$SESSION_TERM_WAIT" || return
    session_round KILL "$SESSION_KILL_WAIT" || return
    say "wrapper: survivors after KILL, publishing anyway: $MEMBERS_TEXT"
}

# session_term — the TERM trap of the session wait, installed right after ORCA returns and
# cleared at the normal publish (no trap while ORCA runs: a cancel then kills the wrapper and
# nothing is published). Single-shot: a cancel sends TERM twice (the group TERM, then cancel.sh's
# per-PID sweep) and bash would re-enter a running handler. It publishes ORCA's rc — a clean result
# is never discarded by a late cancel (classifier row 2) — and then keeps the tsp slot while the
# session empties, by the same bounded sequence as the normal path (grace, TERM+CONT, KILL; ADR-024
# o24.14), silently: its one line, printed before the publish, announces the hold. A cancel ends it
# sooner (cancel.sh KILLs what remains after its own 5 s, the wrapper included).
session_term() {
    trap '' TERM
    session_grace
    echo "wrapper: TERM during the session wait: publishing ORCA's rc $SESSION_RC, holding the slot until the session is empty (at most $(( SESSION_GRACE_S + SESSION_TERM_WAIT + SESSION_KILL_WAIT )) s more)"
    publish .exit_code "$SESSION_RC"$'\n'
    SESSION_QUIET=1
    if session_leader && session_members && (( ${#MEMBERS[@]} > 0 )); then
        session_escalate
    fi
    exit "$SESSION_RC"
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

    # Step 4, under the TERM trap from here to the publish.
    SESSION_RC=$rc
    trap session_term TERM
    session_wait

    # Step 5. The publish itself ignores TERM (the trap would publish a second time); after it
    # TERM is back to its default, so a TERM after the normal publish prints nothing.
    trap '' TERM
    publish .exit_code "$rc"$'\n'
    trap - TERM
    exit "$rc"
}

wrapper_main "$@"
