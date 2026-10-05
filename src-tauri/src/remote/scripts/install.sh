
# ---- install — the root's bin/ and tsp/, and the uploaded scripts by their sha (ADR-024 l, o 13.1, 14.1, 14.3)
# Fed on ONE `bash -s` stdin (ADR-024 n item 11), then the values as a NUL list
#   <root> NUL then, for wrapper, cancel and collect in this order: <sha256> NUL <bytes> NUL
# Run only when the prepare call found <root>/bin or <root>/tsp missing, or a script not hashing
# right. In this order:
#   1  the values' form
#   2  mkdir -p <root>/bin <root>/tsp; the root, bin and tsp must then each be a directory, not a
#      symlink, equal to its realpath. tsp/ holds the slot sockets; nothing else creates it. What
#      real tsp does when the socket's directory is missing is NOT measured (rule #10; probe 5.2b
#      had the parent present) — the B4 live run records it (ADR-024 o item 14.3).
#   3  for each script: <root>/bin/<name>-<sha>.sh already a regular file (not a symlink) whose
#      sha256 is <sha>: left as it is, `kept`. Otherwise its bytes go to a UNIQUE temp name in
#      <root>/bin/ (mktemp, so two profiles aliasing one host cannot clobber each other), the temp's
#      sha256 must be <sha>, and it is renamed over the name (mv -fT: a running script keeps its
#      own inode, never a `cp`/`>` over a script that may be running — probe P3): `installed`.
# Whether the result is right is not this script's word: Rust runs the prepare call again and
# requires every script to hash right (the post-condition, rule #9). The reply
# (remote::prepare::parse_install_reply):
#   orcastudio-install 1
#   argc 7, then `arg <len>` × 7   the values, verbatim (the bytes too, n item 6d)
#   wrapper installed|kept
#   cancel installed|kept
#   collect installed|kept
#   end
# Any failure is an `error <len>` record and exit 3; a temp file it created is removed.

export LC_ALL=C

T_SHA=5            # s: timeout -k 1 on each sha256sum

emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

die() {
    emit_text error "$1"
    exit 3
}

# sha_of FILE — its sha256 into REPLY, or die.
sha_of() {
    local line=''
    timeout -k 1 "$T_SHA" sha256sum -- "$1" </dev/null >"$T/sha" 2>"$T/sha.err" \
        || die "sha256sum $1 failed: $(head -c 200 -- "$T/sha.err" </dev/null)"
    { IFS=' ' read -r line _ || true; } <"$T/sha"
    [[ $line =~ ^[0-9a-f]{64}$ ]] || die "sha256sum printed no sha256 for $1"
    REPLY=$line
}

# real_dir DIR — DIR is a directory, not a symlink, and its realpath is DIR itself.
real_dir() {
    local real
    [[ -d $1 && ! -L $1 ]] || die "$1 is not a directory"
    real=$(realpath -e -- "$1" 2>/dev/null </dev/null && printf x) || die "realpath $1 failed"
    real=${real%x}
    [[ ${real%$'\n'} == "$1" ]] || die "$1 is reached through a symlink (realpath ${real%$'\n'})"
}

# place NAME SHA BYTES — step 3 for one script; prints `NAME installed|kept`.
place() {
    local name=$1 sha=$2 content=$3 final
    final=$root/bin/$name-$sha.sh
    if [[ -f $final && ! -L $final ]]; then
        sha_of "$final"
        if [[ $REPLY == "$sha" ]]; then
            printf '%s kept\n' "$name"
            return
        fi
    fi
    TMPW=$(mktemp -- "$root/bin/.$name-$sha.XXXXXX" </dev/null) || die "mktemp in $root/bin failed"
    printf '%s' "$content" >"$TMPW" || die "writing $TMPW failed"
    sha_of "$TMPW"
    [[ $REPLY == "$sha" ]] || die "the uploaded $name bytes hash to $REPLY, not $sha"
    mv -fT -- "$TMPW" "$final" </dev/null 2>"$T/mv.err" \
        || die "mv $TMPW $final failed: $(head -c 200 -- "$T/mv.err" </dev/null)"
    TMPW=''
    printf '%s installed\n' "$name"
}

main() {
    local a i
    local -a names=(wrapper cancel collect)
    printf 'orcastudio-install 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done
    if (( $# != 7 )); then
        printf 'end\n'
        exit 2
    fi
    root=$1
    valid_path "$root" || die "invalid root: $root"
    for i in 2 4 6; do
        [[ ${!i} =~ ^[0-9a-f]{64}$ ]] || die "the ${names[i / 2 - 1]} sha256 is not 64 lowercase hex digits"
    done
    T=$(mktemp -d </dev/null) || die "mktemp -d failed"
    TMPW=''
    trap 'rm -rf -- "$T" </dev/null; [[ -n $TMPW ]] && rm -f -- "$TMPW" </dev/null' EXIT

    mkdir -p -- "$root/bin" "$root/tsp" </dev/null 2>"$T/mkdir.err" \
        || die "mkdir -p $root/bin $root/tsp failed: $(head -c 200 -- "$T/mkdir.err" </dev/null)"
    real_dir "$root"
    real_dir "$root/bin"
    real_dir "$root/tsp"

    place wrapper "$2" "$3"
    place cancel "$4" "$5"
    place collect "$6" "$7"
    printf 'end\n'
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
