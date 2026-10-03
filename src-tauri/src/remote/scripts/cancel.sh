
# ---- cancel.sh cancel|check <job_dir> <root> ------------------------------------------------
# Cancels one job (ADR-024 l, "Cancel script, revised"), queued or running, the same script for
# both. `check` evaluates the same predicates and prints them without writing or signalling
# anything: the tests compare it with the Rust predicates (shell/Rust liveness parity).
#
#   1  publish .cancelled (temp file + rename); from now on the wrapper will not start ORCA
#   2  queued:  `tsp -r <id>` only if a daemon listens on the .enqueued socket (/proc/net/unix,
#               tsp is never run to look) and `tsp -l` has a row with that id, in state
#               `queued`, carrying the job dir as a whole token
#   3  running: only within the boot that wrote .started —
#               TERM to the wrapper's group iff the wrapper is alive and ours and leads its group;
#               then the sweep, iff the wrapper is alive and ours or the job session is non-empty:
#               SID-reuse guard, never our own session, only `ps -s <sid>` members whose cwd is
#               the job dir; TERM+CONT, bounded wait, KILL
#   4  remove <job>/.tmp, whatever happened above
#
# It never cds into the job dir (absolute paths only) and starts in /, so it never matches the
# cwd filter itself. It never runs `tsp -k`, never signals by tsp id, never kills by name. A fact
# it cannot read (anything but ENOENT/ESRCH) stops it before any further signal: exit 3.

SWEEP_WAIT_STEPS=50   # x 0.1 s: how long the sweep waits after TERM before KILL

fail() {
    printf 'error %s\n' "$1"
    echo "cancel: $1" >&2
    exit 3
}

# publish_cancelled — create the empty .cancelled marker by temp file + rename (ADR-024 l).
publish_cancelled() {
    local tmp="$JOB/.cancelled.tmp.$$"
    if : >"$tmp" && mv -fT -- "$tmp" "$JOB/.cancelled"; then
        return 0
    fi
    rm -f -- "$tmp"
    return 1
}

# is_ours CMDLINE_FILE — ADR-024 l "ours": argv[0] = bash, argv[1] = <root>/bin/wrapper-<lowercase
# hex>.sh, argv[2] = this job's dir. The same rule as classify::is_our_wrapper.
is_ours() {
    local LC_ALL=C sha
    local -a argv=()
    mapfile -d '' -t argv <"$1"
    (( ${#argv[@]} >= 3 )) || return 1
    [[ ${argv[0]} == bash && ${argv[2]} == "$JOB" ]] || return 1
    [[ ${argv[1]} == "$ROOT/bin/wrapper-"*.sh ]] || return 1
    sha=${argv[1]#"$ROOT/bin/wrapper-"}
    sha=${sha%.sh}
    [[ $sha =~ ^[0-9a-f]+$ ]]
}

# read_stat PID — read and parse /proc/PID/stat. Returns 0 (ST_* set), 1 (no such process); a
# read error or a malformed line is fatal.
read_stat() {
    read_raw "/proc/$1/stat" "$T/stat"
    case $? in
        0) ;;
        1) return 1 ;;
        *) fail "$R_ERR" ;;
    esac
    local line
    IFS= read -r line <"$T/stat"
    parse_stat_line "$line" || fail "/proc/$1/stat does not parse"
    [[ $ST_PID == "$1" ]] || fail "/proc/$1/stat is for pid $ST_PID"
}

# job_session — the job session (ADR-024 l) into JOB_SESSION: empty if the SID is reused (a
# process at that number with a different start time), otherwise the members of `ps -s <sid>`
# whose cwd is exactly the job dir; a member without a readable cwd (ENOENT/ESRCH: exited or a
# zombie) is not in it. Sets SID_REUSED. The same rule as classify::sid_reused + job_session.
job_session() {
    local pid cwd
    JOB_SESSION=()
    SID_REUSED=no
    if read_stat "$S_SID" && [[ $ST_START != "$S_START" ]]; then
        SID_REUSED=yes
        return
    fi
    session_pids "$S_SID" || fail "$R_ERR"
    for pid in "${SESSION_PIDS[@]}"; do
        read_link "/proc/$pid/cwd" "$T/cwd"
        case $? in
            0) # Byte-exact: `read -d ''` keeps trailing newlines that `$(<…)` would strip.
               cwd=''
               IFS= read -r -d '' cwd <"$T/cwd"
               [[ $cwd == "$JOB" ]] && JOB_SESSION+=("$pid") ;;
            1) ;;
            *) fail "$R_ERR" ;;
        esac
    done
}

# assess — the facts every decision uses. Sets STARTED (ok|absent|corrupt), BOOT (current|stale),
# ALIVE, OURS, LEADER (the wrapper leads its process group), SID_REUSED, OWN (yes|no), and
# JOB_SESSION. ALIVE/OURS/LEADER/SID_REUSED/OWN are "-" unless STARTED=ok and BOOT=current.
assess() {
    local current
    STARTED=absent BOOT=- ALIVE=- OURS=- LEADER=- SID_REUSED=- OWN=-
    JOB_SESSION=()
    read_raw "$JOB/.started" "$T/started"
    case $? in
        0) if started_parse_file "$T/started"; then STARTED=ok; else STARTED=corrupt; fi ;;
        1) return ;;
        *) fail "$R_ERR" ;;
    esac
    [[ $STARTED == ok ]] || return

    read_raw /proc/sys/kernel/random/boot_id "$T/boot_id" || fail "boot_id: ${R_ERR:-absent}"
    IFS= read -r current <"$T/boot_id"
    if [[ $current != "$S_BOOT" ]]; then
        # Start times count ticks since boot: with another boot nothing of ours is left (round 5
        # LOW-4).
        BOOT=stale
        return
    fi
    BOOT=current

    ALIVE=no OURS=no LEADER=no
    if read_stat "$S_PID"; then
        # Alive: not a zombie and the start time .started recorded (probe 5.2c).
        [[ $ST_STATE != Z && $ST_START == "$S_START" ]] && ALIVE=yes
        [[ $ST_PGRP == "$S_PGID" && $S_PGID == "$S_PID" ]] && LEADER=yes
        read_raw "/proc/$S_PID/cmdline" "$T/cmdline"
        case $? in
            0) is_ours "$T/cmdline" && OURS=yes ;;
            1) ;;
            *) fail "$R_ERR" ;;
        esac
    fi
    own_session || fail "cannot read own /proc/$$/stat"
    OWN=no
    [[ $S_SID == "$OWN_SID" || $S_PGID == "$OWN_PGRP" ]] && OWN=yes
    job_session
}

# queued_cancel — step 2.
queued_cancel() {
    local line id_ok rows=0
    local -a tok
    read_raw "$JOB/.enqueued" "$T/enqueued"
    case $? in
        0) ;;
        1) echo "queued: skip no .enqueued"; return ;;
        *) fail "$R_ERR" ;;
    esac
    if ! enqueued_parse_file "$T/enqueued"; then
        echo "queued: skip corrupt .enqueued ($E_WHY)"
        return
    fi
    read_raw /proc/net/unix "$T/net_unix" || fail "/proc/net/unix: ${R_ERR:-absent}"
    net_unix_listed "$T/net_unix" "$T/net_unix.ours" "$E_SOCKET"
    case $? in
        0) ;;
        1) echo "queued: skip no daemon on $E_SOCKET"; return ;;
        *) fail "$R_ERR" ;;
    esac
    if ! TS_SOCKET=$E_SOCKET tsp -l >"$T/tsp_l" 2>"$T/tsp_l.err"; then
        echo "queued: skip tsp -l failed"
        return
    fi
    while IFS= read -r line; do
        read -r -a tok <<<"$line"
        (( ${#tok[@]} >= 2 )) || continue
        id_ok=no
        dec_norm "${tok[0]}" && [[ $REPLY == "$E_ID" ]] && id_ok=yes
        [[ $id_ok == yes && ${tok[1]} == queued ]] || continue
        [[ " ${tok[*]} " == *" $JOB "* ]] || continue
        rows=$(( rows + 1 ))
    done <"$T/tsp_l"
    if (( rows != 1 )); then
        echo "queued: skip no verified queued row for id $E_ID"
        return
    fi
    # The row may start between -l and -r; what -r does to a running row is not measured
    # (round 3 LOW-F). Accepted window: .cancelled already stops the wrapper.
    TS_SOCKET=$E_SOCKET tsp -r "$E_ID" >/dev/null 2>&1
    echo "queued: tsp -r $E_ID"
}

# signal_each SIGNAL PID... — signal individual PIDs; a PID that is already gone is fine.
signal_each() {
    local sig=$1 pid
    shift
    for pid in "$@"; do
        kill "-$sig" "$pid" 2>/dev/null
    done
}

# running_cancel — step 3.
running_cancel() {
    local step
    if [[ $STARTED != ok ]]; then
        echo "running: skip .started $STARTED"
        return
    fi
    if [[ $BOOT != current ]]; then
        echo "running: skip .started is from another boot"
        return
    fi
    if [[ $OWN == yes ]]; then
        # Never signal our own session or group (probe-era guard, ADR-024 i/l).
        echo "running: skip own session"
        return
    fi
    if [[ $ALIVE == yes && $OURS == yes && $LEADER == yes ]]; then
        kill -TERM -- "-$S_PGID" 2>/dev/null
        echo "group: TERM -$S_PGID"
    else
        echo "group: skip alive=$ALIVE ours=$OURS leader=$LEADER"
    fi
    if [[ $SID_REUSED == yes ]]; then
        echo "sweep: skip sid $S_SID reused"
        return
    fi
    if [[ $ALIVE != yes || $OURS != yes ]] && (( ${#JOB_SESSION[@]} == 0 )); then
        echo "sweep: skip nothing of the job runs"
        return
    fi
    echo "sweep: TERM+CONT ${JOB_SESSION[*]}"
    signal_each TERM "${JOB_SESSION[@]}"
    signal_each CONT "${JOB_SESSION[@]}"
    for (( step = 0; step < SWEEP_WAIT_STEPS; step++ )); do
        job_session
        (( ${#JOB_SESSION[@]} == 0 )) && break
        sleep 0.1
    done
    if [[ $SID_REUSED != yes ]] && (( ${#JOB_SESSION[@]} > 0 )); then
        echo "sweep: KILL ${JOB_SESSION[*]}"
        signal_each KILL "${JOB_SESSION[@]}"
    fi
}

cancel_main() {
    if (( $# != 3 )) || [[ $1 != cancel && $1 != check ]]; then
        echo "cancel: usage: cancel.sh cancel|check <job_dir> <root>" >&2
        exit 2
    fi
    local mode=$1
    JOB=$2 ROOT=$3
    valid_path "$JOB" || { echo "cancel: invalid job dir: $JOB" >&2; exit 2; }
    valid_path "$ROOT" || { echo "cancel: invalid root: $ROOT" >&2; exit 2; }
    export LC_ALL=C
    cd / || exit 2
    T=$(mktemp -d) || fail "mktemp failed"
    trap 'rm -rf -- "$T"' EXIT
    [[ -d $JOB ]] || fail "job dir is not a directory: $JOB"

    if [[ $mode == check ]]; then
        assess
        printf 'started=%s\nboot=%s\nalive=%s\nours=%s\nleader=%s\nsid_reused=%s\nown_session=%s\nsession=%s\n' \
            "$STARTED" "$BOOT" "$ALIVE" "$OURS" "$LEADER" "$SID_REUSED" "$OWN" "${JOB_SESSION[*]}"
        exit 0
    fi

    publish_cancelled || fail "cannot publish $JOB/.cancelled"
    echo "cancelled: marker published"
    queued_cancel
    assess
    running_cancel
    # Unconditionally (ADR-024 l, Part B item 7; Decision i step 4): a wrapper that starts later
    # sees .cancelled at step 2, before it creates .tmp at step 3.
    rm -rf -- "$JOB/.tmp"
    echo "tmp: removed $JOB/.tmp"
    echo "done"
}

cancel_main "$@"
