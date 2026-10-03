
# ---- conntest.sh — the connection test (ADR-024 n items 8 and 11) ------------------------------
# Fed on ONE `bash -s` stdin: this script, then the profile's values as a NUL-separated list
#   <orca path> NUL <root> NUL <core mask, empty when unset> NUL
# The shape is load-bearing (probe 5.1c):
# - bash reads its script from the pipe one command at a time, so the read loop on the LAST line
#   would take any later script line as a value. Nothing may follow that line but the NUL list.
# - Before the last line there are only definitions and commands that do not read stdin.
# - Every child command gets </dev/null: a child that reads stdin swallows the rest of the script
#   and the values, silently, with rc 0.
# The output is raw facts as records — the format is in wiki/modules/server-profiles.md and is
# parsed by src-tauri/src/connection_test.rs. Every verdict is Rust's (rule #9); the script echoes
# the values it received, so Rust can check the transport before it reads any fact.

export LC_ALL=C

KILL_USER_PROCESSES=(busctl get-property org.freedesktop.login1 /org/freedesktop/login1
                     org.freedesktop.login1.Manager KillUserProcesses)

# emit_text NAME TEXT — a byte record from a string: "NAME <len>\n<bytes>\n" (LC_ALL=C: bytes).
emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

# fail MESSAGE — an `error` record, and stop.
fail() {
    emit_text error "$1"
    exit 3
}

# emit NAME FILE — a byte record from a file's exact bytes.
emit() {
    local size
    size=$(stat -c %s -- "$2" </dev/null) || fail "stat $2 failed"
    printf '%s %s\n' "$1" "$size"
    cat -- "$2" </dev/null || fail "cat $2 failed"
    printf '\n'
}

# check NAME CMD... — run CMD, stdout and stderr captured apart. Emits "NAME <rc>", then the
# records `out` and `err`. Sets LAST_RC.
check() {
    local name=$1
    shift
    "$@" </dev/null >"$T/out" 2>"$T/err"
    LAST_RC=$?
    printf '%s %s\n' "$name" "$LAST_RC"
    emit out "$T/out"
    emit err "$T/err"
}

# check_merged NAME CMD... — the same with stderr merged into stdout, captured whole (ORCA's
# `--version`: the version line is in the middle of a 183-line banner; never piped into head).
check_merged() {
    local name=$1
    shift
    "$@" </dev/null >"$T/out" 2>&1
    LAST_RC=$?
    : >"$T/err"
    printf '%s %s\n' "$name" "$LAST_RC"
    emit out "$T/out"
    emit err "$T/err"
}

# skip NAME — a check the script did not run.
skip() {
    printf '%s skipped\n' "$1"
}

main() {
    local orca root a
    printf 'orcastudio-conntest 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# != 3 )); then
        printf 'end\n'
        exit 2
    fi
    orca=$1 root=$2
    T=$(mktemp -d </dev/null) || fail "mktemp -d failed"
    trap 'rm -rf -- "$T" </dev/null' EXIT

    # The root (n item 8): created if absent, inside the profile user's own tree; never touched
    # unless it has the one path form (ADR-024 l item 4).
    if valid_path "$root"; then
        check mkdir mkdir -p -- "$root"
        check realpath realpath -e -- "$root"
        check findmnt findmnt -no FSTYPE --target "$root"
    else
        skip mkdir
        skip realpath
        skip findmnt
    fi
    check busctl "${KILL_USER_PROCESSES[@]}"
    check id id -nG
    check nproc nproc
    if [[ $orca == /* ]]; then
        check orca_x test -x "$orca"
        if (( LAST_RC == 0 )); then
            check_merged orca "$orca" --version
        else
            skip orca
        fi
    else
        skip orca_x
        skip orca
    fi
    check ompi ompi_info --version
    printf 'end\n'
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
