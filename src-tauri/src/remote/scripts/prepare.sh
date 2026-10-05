
# ---- prepare — the read-only pre-upload call (ADR-024 o items 3.2, 13.1, 14.1) ----------------
# Fed on ONE `bash -s` stdin (ADR-024 n item 11), then the values as a NUL list
#   <root> NUL <job dir> NUL <wrapper sha256> NUL <cancel sha256> NUL <collect sha256> NUL
# Before anything is written for a submit or a withdraw: for <root>, <root>/jobs, the job dir,
# <root>/bin and <root>/tsp, whether each exists and, if it does, its kind (`stat -c %F`, which
# does not follow a symlink) and its realpath; and the same for each uploaded script
# <root>/bin/<name>-<sha>.sh (wrapper, cancel, collect), plus its sha256 when it is a regular file
# (nothing else is read: a fifo would block). Facts only — whether a shape is acceptable and which
# scripts must be uploaded is Rust's (remote::prepare::check_prepare). It writes nothing. The reply
# (remote::prepare::parse_prepare_reply):
#   orcastudio-prepare 1
#   argc 5, then `arg <len>` × 5   the values, verbatim
#   <name> absent|present          for root, jobs, job, bin, tsp, wrapper, cancel, collect, in this order
#     kind <len>                   ┐ only for `present`: the kind's bytes
#     realpath <len>|-             │ realpath -e's bytes, or - when it does not resolve
#     sha256 <hex>|-               ┘ scripts only: a regular file's sha256, else -
#   end
# A read error other than ENOENT is an `error <len>` record and exit 3 — never "absent".

export LC_ALL=C

T_SHA=5            # s: timeout -k 1 on each script's sha256sum

emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

die() {
    emit_text error "$1"
    exit 3
}

# describe NAME PATH — `NAME absent`, or `NAME present` + kind + realpath. Only ENOENT is absent.
# Sets KIND for the caller; returns 1 when absent.
describe() {
    local name=$1 p=$2 real
    KIND=''
    if KIND=$(stat -c %F -- "$p" 2>&1 </dev/null); then
        printf '%s present\n' "$name"
        emit_text kind "$KIND"
        # Byte-exact: `$(…)` would strip every trailing newline, realpath prints exactly one.
        if real=$(realpath -e -- "$p" 2>/dev/null </dev/null && printf x); then
            real=${real%x}
            emit_text realpath "${real%$'\n'}"
        else
            printf 'realpath -\n'
        fi
        return 0
    elif [[ $KIND == "stat: cannot statx '$p': No such file or directory" ]]; then
        printf '%s absent\n' "$name"
        return 1
    fi
    die "stat $p: $KIND"
}

main() {
    local a root job id name i line script
    local -a shas
    printf 'orcastudio-prepare 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# != 5 )); then
        printf 'end\n'
        exit 2
    fi
    root=$1 job=$2
    shas=("$3" "$4" "$5")
    valid_path "$root" || die "invalid root: $root"
    id=${job#"$root/jobs/"}
    if [[ $id == "$job" || -z $id || $id == */* ]] || ! valid_path "$job"; then
        die "the job dir is not <root>/jobs/<id>: $job"
    fi
    for i in 0 1 2; do
        [[ ${shas[i]} =~ ^[0-9a-f]{64}$ ]] || die "sha256 $((i + 1)) is not 64 lowercase hex digits"
    done
    T=$(mktemp -d </dev/null) || die "mktemp -d failed"
    trap 'rm -rf -- "$T" </dev/null' EXIT

    describe root "$root"
    describe jobs "$root/jobs"
    describe job "$job"
    describe bin "$root/bin"
    describe tsp "$root/tsp"
    i=0
    for name in wrapper cancel collect; do
        script=$root/bin/$name-${shas[i]}.sh
        i=$(( i + 1 ))
        describe "$name" "$script" || continue
        if [[ $KIND == "regular file" || $KIND == "regular empty file" ]]; then
            timeout -k 1 "$T_SHA" sha256sum -- "$script" </dev/null >"$T/sha" 2>"$T/sha.err" \
                || die "sha256sum $script failed: $(head -c 200 -- "$T/sha.err" </dev/null)"
            line=''
            { IFS=' ' read -r line _ || true; } <"$T/sha"
            [[ $line =~ ^[0-9a-f]{64}$ ]] || die "sha256sum printed no sha256 for $script"
            printf 'sha256 %s\n' "$line"
        else
            printf 'sha256 -\n'
        fi
    done
    printf 'end\n'
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
