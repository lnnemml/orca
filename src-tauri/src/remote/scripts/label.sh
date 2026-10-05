
# ---- label — the read-only label call (ADR-024 o item 3.4) ----------------------------------
# Fed on ONE `bash -s` stdin (ADR-024 n item 11), then the values as a NUL list
#   <job dir> NUL <recorded socket> NUL
# For a remote Queued job, before any collect: does the dir exist, which markers exist (in any
# form — `.submitting` is a dangling symlink), and which `tsp -l` rows of the recorded socket
# mention the job dir. Facts only; the label is Rust's (remote::submit::label). It writes nothing,
# and runs tsp only on a socket /proc/net/unix lists (a tsp call on a dead socket would start a
# daemon, probe 5.2b). The reply (remote::submit::parse_label_reply), facts in the o item 3.4
# order:
#   orcastudio-label 1
#   argc 2, then `arg <len>` × 2   the values, verbatim
#   dir yes|no                     no → nothing else is read
#   started yes|no                 ┐
#   exit_code yes|no               │
#   cancelled yes|no               │ only for `dir yes`
#   enqueued yes|no                │
#   submitting yes|no              │
#   net_unix <len>                 │ /proc/net/unix header + the lines naming the socket, then
#     nodaemon | rows <n> + n × row <len> | tsp_error <len>
#   end
# A read error other than ENOENT is an `error <len>` record and exit 3 — never "absent".

export LC_ALL=C

LABEL_MARKERS=(started exit_code cancelled enqueued submitting)

emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

die() {
    emit_text error "$1"
    exit 3
}

# emit NAME FILE — a byte record from a file's exact bytes.
emit() {
    local size
    size=$(stat -c %s -- "$2" </dev/null) || die "stat $2 failed"
    printf '%s %s\n' "$1" "$size"
    cat -- "$2" </dev/null || die "cat $2 failed"
    printf '\n'
}

main() {
    local a job sock kind marker line
    local -a rows=()
    printf 'orcastudio-label 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# != 2 )); then
        printf 'end\n'
        exit 2
    fi
    job=$1 sock=$2
    valid_path "$job" || die "invalid job dir: $job"
    valid_path "$sock" || die "invalid socket path: $sock"
    T=$(mktemp -d </dev/null) || die "mktemp -d failed"
    trap 'rm -rf -- "$T" </dev/null' EXIT

    # 1. The dir. Only ENOENT is "no dir"; a path that is not a directory is an error.
    if kind=$(stat -c %F -- "$job" 2>&1 </dev/null); then
        [[ $kind == directory ]] || die "job dir is a $kind"
        printf 'dir yes\n'
    elif [[ $kind == "stat: cannot statx '$job': No such file or directory" ]]; then
        printf 'dir no\nend\n'
        exit 0
    else
        die "stat $job: $kind"
    fi

    # 2–4. The markers, in any form.
    for marker in "${LABEL_MARKERS[@]}"; do
        path_exists "$job/.$marker"
        case $? in
            0) printf '%s yes\n' "$marker" ;;
            1) printf '%s no\n' "$marker" ;;
            *) die "$R_ERR" ;;
        esac
    done

    # The recorded socket: /proc/net/unix first; a failed tsp -l is an Error fact.
    read_raw /proc/net/unix "$T/net_unix" || die "/proc/net/unix: ${R_ERR:-absent}"
    net_unix_listed "$T/net_unix" "$T/net_unix.ours" "$sock"
    case $? in
        0) emit net_unix "$T/net_unix.ours" ;;
        1) emit net_unix "$T/net_unix.ours"
           printf 'nodaemon\nend\n'
           exit 0 ;;
        *) die "$R_ERR" ;;
    esac
    # Bounded: a hung tsp client is an Error fact (the classifier's), never a hung poller.
    if ! TS_SOCKET=$sock timeout -k 1 3 tsp -l </dev/null >"$T/tsp_l" 2>"$T/tsp_l.err"; then
        emit_text tsp_error "tsp -l failed or timed out: $(head -c 200 -- "$T/tsp_l.err" </dev/null)"
        printf 'end\n'
        exit 0
    fi
    while IFS= read -r line; do
        [[ $line == *"$job"* ]] && rows+=("$line")
    done <"$T/tsp_l"
    printf 'rows %s\n' "${#rows[@]}"
    for line in "${rows[@]}"; do
        emit_text row "$line"
    done
    printf 'end\n'
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
