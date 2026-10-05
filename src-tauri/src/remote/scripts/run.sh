
# ---- run — the trampoline for the uploaded job scripts (ADR-024 o item 14.1) -----------------
# Fed on ONE `bash -s` stdin (ADR-024 n item 11), then the values as a NUL list
#   <root> NUL <name> NUL <sha256> NUL <arg> NUL <arg> NUL …
# Every call of an uploaded job script (cancel, collect — the wrapper stays tsp's) goes through
# here, so no per-job value is ever re-parsed by the remote login shell: the values are read from
# stdin and handed on as argv. In this order:
#   1  <root> by the one path rule; <name> in the closed allow-list BUDGET (its value is the
#      script's time budget N, s); <sha> exactly 64 lowercase hex
#   2  <root>/bin/<name>-<sha>.sh, built here: absent → `not-installed`; otherwise it must be a
#      regular file (not a symlink) equal to its realpath whose sha256 is <sha>, else `refused`
#   3  `timeout -k 1 <N> bash <path> <arg>… </dev/null` — the script gets EOF on stdin; its stdout
#      and stderr go to temp files, each capped at CAP bytes (over the cap → an `error` record)
#   4  the reply carries the script's rc and both streams, length-framed; exit 0 whenever the
#      reply is complete, whatever the script's rc (the rc lives in the record)
# The reply (remote::run::parse_run_reply):
#   orcastudio-run 1
#   argc <n>, then `arg <len>` × n   the values, verbatim
#   refused <len> | not-installed | ran
#   rc <n>                           ┐
#   stdout <len>                     │ only after `ran`
#   stderr <len>                     ┘
#   end
# A read error is an `error <len>` record and exit 3.

export LC_ALL=C

# The closed allow-list and each script's budget N (s): the cancel sweep waits up to 5 s after its
# TERM; the collector reads /proc and a few `tsp -l`. Never `wrapper`: it would start ORCA
# outside tsp.
declare -A BUDGET=([cancel]=20 [collect]=15)
T_CHECK=5          # s: timeout -k 1 on realpath and sha256sum
CAP=400000         # bytes per captured stream: two of them plus the echo stay under 1 MiB

emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

die() {
    emit_text error "$1"
    exit 3
}

refuse() {
    emit_text refused "$1"
    printf 'end\n'
    exit 0
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
    local a root name sha path kind real line rc size
    printf 'orcastudio-run 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# < 3 )); then
        printf 'end\n'
        exit 2
    fi
    root=$1 name=$2 sha=$3
    shift 3
    valid_path "$root" || refuse "values: the root breaks the path rule"
    [[ $name =~ ^[a-z]+$ && -n ${BUDGET[$name]+set} ]] || refuse "values: $(printf '%q' "$name") is not an allowed script"
    [[ $sha =~ ^[0-9a-f]{64}$ ]] || refuse "values: the sha256 is not 64 lowercase hex digits"
    path=$root/bin/$name-$sha.sh
    T=$(mktemp -d </dev/null) || die "mktemp -d failed"
    trap 'rm -rf -- "$T" </dev/null' EXIT

    # 2. The script, by its sha. Only ENOENT is "not installed".
    if ! kind=$(stat -c %F -- "$path" 2>&1 </dev/null); then
        [[ $kind == "stat: cannot statx '$path': No such file or directory" ]] || die "stat $path: $kind"
        printf 'not-installed\nend\n'
        exit 0
    fi
    [[ $kind == "regular file" ]] || refuse "script: $path is a $kind, not a regular file"
    real=$(timeout -k 1 "$T_CHECK" realpath -e -- "$path" 2>/dev/null </dev/null && printf x) \
        || refuse "script: realpath $path failed"
    real=${real%x}
    [[ ${real%$'\n'} == "$path" ]] || refuse "script: $path is reached through a symlink (realpath ${real%$'\n'})"
    timeout -k 1 "$T_CHECK" sha256sum -- "$path" </dev/null >"$T/sha" 2>/dev/null || refuse "script: sha256sum $path failed"
    line=''
    { IFS=' ' read -r line _ || true; } <"$T/sha"
    [[ $line == "$sha" ]] || refuse "script: the sha256 of $path is not the one its name carries"

    # 3. Run it: no stdin, both streams captured, bounded in time.
    timeout -k 1 "${BUDGET[$name]}" bash "$path" "$@" </dev/null >"$T/out" 2>"$T/err"
    rc=$?
    for a in out err; do
        size=$(stat -c %s -- "$T/$a" </dev/null) || die "stat $T/$a failed"
        (( size <= CAP )) || die "the script's std$a is $size bytes, over $CAP"
    done
    printf 'ran\nrc %s\n' "$rc"
    emit stdout "$T/out"
    emit stderr "$T/err"
    printf 'end\n'
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
