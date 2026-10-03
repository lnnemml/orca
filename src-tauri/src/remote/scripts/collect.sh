
# ---- collect.sh <job_dir> [<socket>...] -----------------------------------------------------
# Emits one raw-fact snapshot of a job (ADR-024 l) on stdout, in the fixed order
#   boot_id → .started → wrapper stat + cmdline, SID stat, session members and their cwds →
#   per socket (the given slot sockets, then the .enqueued socket): /proc/net/unix evidence and
#   NoDaemon, or the `tsp -l` rows that mention the job dir, or a tsp error →
#   .exit_code, .cancelled, last 5 KiB of output.out → .started again → end
# as length-prefixed records; the format is specified in wiki/modules/remote-jobs.md and parsed
# by src-tauri/src/remote/wire.rs. Facts only: every verdict is Rust's (rule #9).
#
# `tsp` runs only on a socket that /proc/net/unix lists: on a dead socket it would silently start
# a daemon (probe 5.2b). The /proc reads happen only when .started parses and is from this boot.
# Any read error other than ENOENT/ESRCH ends the snapshot with an `error` record and exit 3 —
# never "absent" (a computing job must not look Lost).

TAIL_BYTES=5120   # local_backend::TAIL_BYTES (rule #5)

die() {
    local LC_ALL=C msg=$1
    printf 'error %s\n%s\n' "${#msg}" "$msg"
    exit 3
}

# record NAME FILE — a byte record: "NAME <len>\n<bytes>\n".
record() {
    local size
    size=$(stat -c %s -- "$2") || die "stat $2 failed"
    printf '%s %s\n' "$1" "$size"
    cat -- "$2" || die "cat $2 failed"
    printf '\n'
}

# record_text NAME TEXT — a byte record from a string.
record_text() {
    local LC_ALL=C
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

# file_record NAME PATH — PATH's bytes, or "NAME -" when it is absent (ENOENT/ESRCH).
file_record() {
    read_raw "$2" "$T/rec"
    case $? in
        0) record "$1" "$T/rec" ;;
        1) printf '%s -\n' "$1" ;;
        *) die "$R_ERR" ;;
    esac
}

collect_proc() {
    local pid
    file_record wrapper_stat "/proc/$S_PID/stat"
    file_record wrapper_cmdline "/proc/$S_PID/cmdline"
    file_record sid_stat "/proc/$S_SID/stat"
    session_pids "$S_SID" || die "$R_ERR"
    printf 'members %s\n' "${#SESSION_PIDS[@]}"
    for pid in "${SESSION_PIDS[@]}"; do
        printf 'member %s\n' "$pid"
        read_link "/proc/$pid/cwd" "$T/cwd"
        case $? in
            0) record cwd "$T/cwd" ;;
            1) printf 'cwd -\n' ;;
            *) die "$R_ERR" ;;
        esac
    done
}

# socket_fact SOCKET — the three-way fact for one socket.
socket_fact() {
    local sock=$1 line rc
    record_text socket "$sock"
    read_raw /proc/net/unix "$T/net_unix" || die "/proc/net/unix: ${R_ERR:-absent}"
    net_unix_listed "$T/net_unix" "$T/net_unix.ours" "$sock"
    rc=$?
    (( rc == 2 )) && die "$R_ERR"
    record net_unix "$T/net_unix.ours"
    if (( rc == 1 )); then
        printf 'nodaemon\n'
        return
    fi
    if ! TS_SOCKET=$sock tsp -l >"$T/tsp_l" 2>"$T/tsp_l.err"; then
        record_text tsp_error "tsp -l failed: $(head -c 200 -- "$T/tsp_l.err")"
        return
    fi
    local -a rows=()
    while IFS= read -r line; do
        [[ $line == *"$JOB"* ]] && rows+=("$line")
    done <"$T/tsp_l"
    printf 'rows %s\n' "${#rows[@]}"
    for line in "${rows[@]}"; do
        record_text row "$line"
    done
}

collect_main() {
    local job_kind sock boot_rc started_rc current proc=skipped
    local -a sockets=()
    export LC_ALL=C
    printf 'orcastudio-snapshot 1\n'
    (( $# >= 1 )) || die "usage: collect.sh <job_dir> [<socket>...]"
    JOB=$1
    shift
    valid_path "$JOB" || die "invalid job dir: $JOB"
    for sock in "$@"; do
        valid_path "$sock" || die "invalid socket path: $sock"
        sockets+=("$sock")
    done
    cd / || die "cd / failed"
    T=$(mktemp -d) || die "mktemp failed"
    trap 'rm -rf -- "$T"' EXIT

    # A missing job dir is an error, not "nothing started": it would read as NeverStarted.
    job_kind=$(stat -c %F -- "$JOB" 2>&1) || die "job dir: $job_kind"
    [[ $job_kind == directory ]] || die "job dir is a $job_kind"

    # boot_id → .started.
    read_raw /proc/sys/kernel/random/boot_id "$T/boot_id"
    boot_rc=$?
    (( boot_rc == 0 )) || die "boot_id: ${R_ERR:-absent}"
    record boot_id "$T/boot_id"
    read_raw "$JOB/.started" "$T/started"
    started_rc=$?
    case $started_rc in
        0) record started "$T/started" ;;
        1) printf 'started -\n' ;;
        *) die "$R_ERR" ;;
    esac

    # /proc and `ps -s`, only for a .started that parses and is from this boot (Anton, M7).
    if (( started_rc == 0 )) && started_parse_file "$T/started"; then
        IFS= read -r current <"$T/boot_id"
        [[ $current == "$S_BOOT" ]] && proc=collected
    fi
    printf 'proc %s\n' "$proc"
    [[ $proc == collected ]] && collect_proc

    # Sockets: the slot sockets, then the .enqueued one if it is not among them.
    read_raw "$JOB/.enqueued" "$T/enqueued"
    case $? in
        0) if enqueued_parse_file "$T/enqueued"; then
               [[ " ${sockets[*]} " == *" $E_SOCKET "* ]] || sockets+=("$E_SOCKET")
           else
               sockets+=("bad-enqueued")
           fi ;;
        1) ;;
        *) die "$R_ERR" ;;
    esac
    printf 'sockets %s\n' "${#sockets[@]}"
    for sock in "${sockets[@]}"; do
        if [[ $sock == bad-enqueued ]]; then
            # The queue entry cannot be located: an Error fact (row 10), never "no rows".
            record_text socket "$JOB/.enqueued"
            record_text socket_error "unparsable .enqueued: $E_WHY"
        else
            socket_fact "$sock"
        fi
    done

    # .exit_code, .cancelled, tail → .started again.
    file_record exit_code "$JOB/.exit_code"
    path_exists "$JOB/.cancelled"
    case $? in
        0) printf 'cancelled yes\n' ;;
        1) printf 'cancelled no\n' ;;
        *) die "$R_ERR" ;;
    esac
    local err
    if err=$(tail -c "$TAIL_BYTES" -- "$JOB/output.out" 2>&1 >"$T/tail"); then
        record tail "$T/tail"
    elif [[ $err == "tail: cannot open '$JOB/output.out' for reading: No such file or directory" ]]; then
        printf 'tail -\n'
    else
        die "tail $JOB/output.out: $err"
    fi
    file_record started "$JOB/.started"
    printf 'end\n'
}

collect_main "$@"
