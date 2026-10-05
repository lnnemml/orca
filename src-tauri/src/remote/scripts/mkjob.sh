
# ---- mkjob — make a withdrawn job's dir and re-assert its shape (ADR-024 o items 2, 14.1) ------
# Fed on ONE `bash -s` stdin (ADR-024 n item 11), then the values as a NUL list
#   <root> NUL <job dir> NUL
# Withdraw publishes `.cancelled` into the job dir, which a job "not on the server" may not have.
# In one call: `mkdir -p <job dir>`, then — after the mkdir — the realpaths of the job dir and of
# its parent, as facts. Rust requires `<job>` and `<root>/jobs` (o item 1); a component reached
# through a symlink fails that, never silently passes. The reply (remote::run::parse_mkjob_reply):
#   orcastudio-mkjob 1
#   argc 2, then `arg <len>` × 2   the values, verbatim
#   job <len>                      realpath -e <job>
#   parent <len>                   realpath -e <job>/..
#   end
# Any failure is an `error <len>` record and exit 3.

export LC_ALL=C

emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

die() {
    emit_text error "$1"
    exit 3
}

# real NAME PATH — the record NAME with realpath -e PATH, byte-exact.
real() {
    local r
    r=$(realpath -e -- "$2" 2>/dev/null </dev/null && printf x) || die "realpath $2 failed"
    r=${r%x}
    emit_text "$1" "${r%$'\n'}"
}

main() {
    local a root job id
    printf 'orcastudio-mkjob 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# != 2 )); then
        printf 'end\n'
        exit 2
    fi
    root=$1 job=$2
    valid_path "$root" || die "invalid root: $root"
    id=${job#"$root/jobs/"}
    if [[ $id == "$job" || -z $id || $id == */* ]] || ! valid_path "$job"; then
        die "the job dir is not <root>/jobs/<id>: $job"
    fi
    mkdir -p -- "$job" </dev/null 2>/dev/null || die "mkdir -p $job failed"
    real job "$job"
    real parent "$job/.."
    printf 'end\n'
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
