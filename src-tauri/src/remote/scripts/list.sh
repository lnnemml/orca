
# ---- list — the server's side of the download post-condition (ADR-024 o item 6) -------------
# Fed on ONE `bash -s` stdin (ADR-024 n item 11), then the values as a NUL list
#   <job dir> NUL <pattern> NUL <pattern> NUL …
# where the patterns are the download filter's leaf patterns (remote::sync::download_patterns —
# the one list, sent, never restated here). Every top-level entry of the job dir is reported
# (never `.tsp-out`, which is tsp's and can still change); one a pattern selects is described:
# a regular file by its sha256, a symlink by its target — never followed (the `.submitting`
# claim is a dangling symlink). Nothing below the top level is read: the download's final
# `--exclude=*` keeps rsync out of every directory but `.tsp-out/`. Rust re-checks every
# selected/unselected verdict against its own filter. The reply (remote::sync::parse_list_reply):
#   orcastudio-list 1
#   argc <n>, then `arg <len>` × n      the values, verbatim
#   entries <n>                         then n times:
#     entry <len>                       a top-level name, then one of
#       sha256 <hex>                    selected regular file
#       link <len>                      selected symlink: its target bytes
#       dir                             selected directory (not entered)
#       other                           selected, but not a file, symlink or directory
#       unselected                      no pattern matches (not read)
#   end
# A read error is an `error <len>` record and exit 3.

export LC_ALL=C

emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

die() {
    emit_text error "$1"
    exit 3
}

main() {
    local a job pat rec kind name selected hash target
    local -a recs patterns
    printf 'orcastudio-list 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# < 1 )); then
        printf 'end\n'
        exit 2
    fi
    job=$1
    shift
    patterns=("$@")
    valid_path "$job" || die "invalid job dir: $job"
    T=$(mktemp -d </dev/null) || die "mktemp -d failed"
    trap 'rm -rf -- "$T" </dev/null' EXIT

    find "$job" -mindepth 1 -maxdepth 1 -printf '%y %f\0' </dev/null >"$T/entries" 2>"$T/err" \
        || die "listing $job failed: $(head -c 200 -- "$T/err" </dev/null)"
    mapfile -d '' -t recs <"$T/entries"
    local -a out=()
    for rec in "${recs[@]}"; do
        kind=${rec%% *} name=${rec#* }
        [[ $name == .tsp-out ]] && continue
        selected=no
        for pat in "${patterns[@]}"; do
            # Unquoted on the right: a glob over the leaf name, as rsync reads the same text.
            # shellcheck disable=SC2053
            [[ $name == $pat ]] && { selected=yes; break; }
        done
        if [[ $selected == no ]]; then
            out+=("$name" unselected)
            continue
        fi
        case $kind in
            f) # One file at a time on stdin: the name never reaches sha256sum's output.
               hash=$(sha256sum <"$job/$name" 2>"$T/err") || die "sha256sum $job/$name: $(head -c 200 -- "$T/err" </dev/null)"
               hash=${hash%% *}
               [[ $hash =~ ^[0-9a-f]{64}$ ]] || die "sha256sum $job/$name printed $hash"
               out+=("$name" "sha256 $hash") ;;
            l) target=$T/target.${#out[@]}
               read_link "$job/$name" "$target" || die "${R_ERR:-$job/$name vanished}"
               out+=("$name" "link:$target") ;;
            d) out+=("$name" dir) ;;
            *) out+=("$name" other) ;;
        esac
    done

    printf 'entries %s\n' "$(( ${#out[@]} / 2 ))"
    for (( a = 0; a < ${#out[@]}; a += 2 )); do
        emit_text entry "${out[a]}"
        case ${out[a+1]} in
            link:*) target=${out[a+1]#link:}
                    printf 'link %s\n' "$(stat -c %s -- "$target" </dev/null)"
                    cat -- "$target" </dev/null || die "cat $target failed"
                    printf '\n' ;;
            *) printf '%s\n' "${out[a+1]}" ;;
        esac
    done
    printf 'end\n'
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
