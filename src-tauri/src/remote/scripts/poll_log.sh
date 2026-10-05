
# ---- poll_log — one chunk of a job's output.out (ADR-024 o item 7) ---------------------------
# Fed on ONE `bash -s` stdin (ADR-024 n item 11), then the values as a NUL list
#   <job dir> NUL <offset> NUL <cap> NUL
# (remote::poll::PollLogArgs::values). The size is read FIRST and the bytes are taken from the
# first <size> bytes only, so the size header matches the bytes even while ORCA appends (probe
# 5.3a). `tail -c +K` past the end prints nothing with rc 0, so a shrunken file is told by the
# size alone (size < offset: no bytes, Rust reads it as a reset). Never the whole file (rule #5):
# at most <cap> bytes. The reply (remote::poll::parse_poll_reply):
#   orcastudio-log 1
#   argc 3, then `arg <len>` × 3   the values, verbatim
#   size <n>|-                     output.out's size, or - when it does not exist
#   bytes <len>                    then exactly <len> raw bytes and a newline
#   end
# A read error other than ENOENT is an `error <len>` record and exit 3.

export LC_ALL=C

MAX_DECIMAL=9223372036854775807   # offsets and caps stay within shell arithmetic

emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

die() {
    emit_text error "$1"
    exit 3
}

main() {
    local a job off cap f size err start
    local -a rc
    printf 'orcastudio-log 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# != 3 )); then
        printf 'end\n'
        exit 2
    fi
    job=$1
    valid_path "$job" || die "invalid job dir: $job"
    dec_norm "$2" && dec_le "$REPLY" "$MAX_DECIMAL" || die "offset is not a decimal: $2"
    off=$REPLY
    dec_norm "$3" && dec_le "$REPLY" "$MAX_DECIMAL" || die "cap is not a decimal: $3"
    cap=$REPLY
    f=$job/output.out
    T=$(mktemp -d </dev/null) || die "mktemp -d failed"
    trap 'rm -rf -- "$T" </dev/null' EXIT

    # The size first. Only ENOENT is "no output.out yet".
    if err=$(stat -c %s -- "$f" 2>&1 >"$T/size" </dev/null); then
        IFS= read -r size <"$T/size"
        dec_norm "$size" || die "stat $f printed a size that is not a decimal: $size"
        size=$REPLY
    elif [[ $err == "stat: cannot statx '$f': No such file or directory" ]]; then
        printf 'size -\nbytes 0\n\nend\n'
        exit 0
    else
        die "stat $f: $err"
    fi
    printf 'size %s\n' "$size"

    : >"$T/chunk"
    if (( size > off )); then
        start=$(( off + 1 ))
        head -c "$size" -- "$f" </dev/null 2>"$T/err" | tail -c "+$start" 2>>"$T/err" | head -c "$cap" >"$T/chunk" 2>>"$T/err"
        rc=("${PIPESTATUS[@]}")
        # The last head may stop reading early, so the first two may die of SIGPIPE (141).
        if [[ ! ${rc[0]} =~ ^(0|141)$ || ! ${rc[1]} =~ ^(0|141)$ || ${rc[2]} != 0 ]]; then
            die "read $f: rc ${rc[*]} $(head -c 200 -- "$T/err" </dev/null)"
        fi
    fi
    printf 'bytes %s\n' "$(stat -c %s -- "$T/chunk" </dev/null)"
    cat -- "$T/chunk" </dev/null || die "cat failed"
    printf '\nend\n'
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
