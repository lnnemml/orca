
# ---- submit — the one atomic submit call (ADR-024 o items 3.3 and 9) -------------------------
# Fed on ONE `bash -s` stdin (ADR-024 n item 11): this script, then the values as a NUL list
#   <job dir> NUL <root> NUL <slot socket> NUL <slot mask> NUL <ORCA path> NUL <wrapper sha256> NUL
#   and two values per uploaded file: <name> NUL <sha256> NUL
# (built by remote::submit::SubmitArgs::values; ADR-024 o item 13.1: the wrapper is named by its
# sha only — the script builds <root>/bin/wrapper-<sha>.sh itself, so the enqueued argv can never
# point outside <root>/bin). Before the read loop on the LAST line there are only definitions;
# nothing but the NUL list may follow it.
#
# The steps, in this order; each refusal stops before any later step runs:
#   0  the values' form                                  — before the lock
#   1  the account lock: flock -w 20 on fd 9             — timeout: "lock busy", listing the
#                                                          processes that have the lock file open
#   2  KillUserProcesses is exactly "b false" (anything else: `refused-kup`, o item 13.3);
#      realpath <job> == <job> and realpath <job>/.. == <root>/jobs; <root>/bin/wrapper-<sha>.sh is
#      a regular file equal to its realpath (neither it nor <root>/bin a symlink) whose sha256 is <sha>
#   3  no marker (.started .enqueued .exit_code .cancelled .submitting, in any form) and no tsp
#      row holding the job dir on the slot socket (tsp only on a socket /proc/net/unix lists)
#   4  the uploaded files (outside .tsp-out/) are exactly the expected names with their sha256
#   5  the slot check: every own process or queued row on cores of the mask is accounted for
#   6  mkdir -p <job>/.tsp-out (a real directory, not a symlink)
#   7  the claim: ln -sT x <job>/.submitting — no-clobber. Every refusal before it wrote no claim.
#   8  enqueue with TMPDIR=<job>/.tsp-out, publish .enqueued (temp file + rename), then the
#      post-condition: the socket is listed verbatim in /proc/net/unix
# The reply (remote::submit::parse_submit_reply): the header, the echoed values, then exactly
# one of `refused <len>` or `refused-kup <len>` (nothing claimed), `enqueued <id>`,
# `failed-after-claim <len>`, then end. `refused-kup`'s bytes are the busctl evidence:
#   rc <n>\nstdout <len>\n<stdout bytes>\nstderr <len>\n<first stderr line>\n
#
# The lock fd: every tsp call (`-l` too) and every child that may outlive the script runs with
# 9>&-; a daemon started with the fd open would hold the account lock for its whole life (probe
# 5.3c). Every child that can block while the lock is held runs under `timeout -k 1`; the budgets
# below add up to 37 s with the kill-after, under the 40 s of o item 3.3.1, so the holder is
# bounded on the server. The short file operations (stat, cat, mkdir, ln, mv) are not wrapped:
# they touch only the root, which is local ext4 (measured on uni, 2026-10-03), not a network mount
# that could hang.

export LC_ALL=C

LOCK_WAIT=20       # s, flock -w: how long a second submit waits for the account lock
# Each `timeout -k "$T_KILL" N` below bounds one child while the lock is held: TERM after N s,
# KILL 1 s later, so a child that ignores TERM costs N + 1 s at most. The nine add up to
# 28 s + 9 × 1 s = 37 s, under the 40 s of o item 3.3.1.
T_KILL=1
T_BUSCTL=2
T_REALPATH=2
T_WRAPPER_REALPATH=1
T_WRAPPER_SHA=2
T_TSP_LIST=2
T_FIND=2
T_SHA=6
T_SCAN=8           # the scan's own `tsp -l` calls run inside it, each under timeout -k 1 2
T_ENQUEUE=3
T_HOLDERS=2        # the "lock busy" path only, where the lock is not held
MAX_FILES=1000     # remote::sync::MAX_UPLOAD_FILES
MAX_SOCKET=100     # remote::MAX_SOCKET_PATH_BYTES
MARKERS=(.started .enqueued .exit_code .cancelled .submitting)
KILL_USER_PROCESSES=(busctl get-property org.freedesktop.login1 /org/freedesktop/login1
                     org.freedesktop.login1.Manager KillUserProcesses)

# emit_text NAME TEXT — a byte record from a string: "NAME <len>\n<bytes>\n".
emit_text() {
    printf '%s %s\n%s\n' "$1" "${#2}" "$2"
}

# refuse REASON — nothing was claimed: the job may be submitted again.
refuse() {
    emit_text refused "$1"
    printf 'end\n'
    exit 0
}

# refuse_kup RC STDOUT_FILE STDERR_FILE — KillUserProcesses is not exactly "b false" (rc 0): its
# own outcome, carrying the evidence, so Part B never has to read a refusal's text (o item 13.3).
# Nothing was claimed.
refuse_kup() {
    local err
    err=$(first_line "$3")
    {
        printf 'rc %s\n' "$1"
        printf 'stdout %s\n' "$(stat -c %s -- "$2" </dev/null)"
        cat -- "$2" </dev/null
        printf '\n'
        printf 'stderr %s\n%s\n' "${#err}" "$err"
    } >"$T/kup.evidence"
    printf 'refused-kup %s\n' "$(stat -c %s -- "$T/kup.evidence" </dev/null)"
    cat -- "$T/kup.evidence" </dev/null
    printf '\nend\n'
    exit 0
}

# failed_after_claim REASON — .submitting stays; only the label call decides now (o item 3.4).
failed_after_claim() {
    emit_text failed-after-claim "$1"
    printf 'end\n'
    exit 0
}

# first_line FILE — the first line of FILE (at most 200 bytes), for a reason text.
first_line() {
    local line=''
    { IFS= read -r line || true; } <"$1" 2>/dev/null
    printf '%s' "${line:0:200}"
}

# lock_openers LOCK — "<pid> <cmdline>; …" for every process (other than this one) that has LOCK
# open: the holder, any other submit waiting for it, a daemon that leaked fd 9 (probe 5.3c). It
# cannot tell which one holds the lock and does not claim to (o item 13.4). Only our own
# processes' fd dirs are readable, which is the account the lock serialises.
lock_openers() {
    local lock=$1 rec path target pid text='' line
    local -a recs argv
    local -A pids=()
    timeout -k "$T_KILL" "$T_HOLDERS" find /proc/[0-9]*/fd -mindepth 1 -maxdepth 1 -printf '%p\t%l\0' \
        </dev/null >"$T/fds" 2>/dev/null 9>&-
    mapfile -d '' -t recs <"$T/fds"
    for rec in "${recs[@]}"; do
        path=${rec%%$'\t'*} target=${rec#*$'\t'}
        [[ $target == "$lock" ]] || continue
        pid=${path#/proc/}
        pid=${pid%%/*}
        [[ $pid == "$$" || $pid == "$BASHPID" ]] && continue
        pids[$pid]=1
    done
    for pid in "${!pids[@]}"; do
        argv=()
        { mapfile -d '' -t argv <"/proc/$pid/cmdline"; } 2>/dev/null
        line="${argv[*]}"
        text+="${text:+; }$pid ${line:0:200}"
    done
    printf '%s' "${text:-no process found (the holder may have exited)}"
}

# ---- the slot check (o item 9) --------------------------------------------------------------
# Runs as its own `bash -c` child under `timeout` (its /proc reads must not hold the lock
# unbounded), so these functions are passed to it by `declare -f`. Prints one `block <text>`
# line per blocker and `done`, or `error <text>` and exits 3.

# cpu_set LIST — the CPUs of a list ("0-3,8") into the sparse array CPUS. Returns 1 for a list
# it cannot read; the caller then fails closed.
cpu_set() {
    local LC_ALL=C list=$1 part a b
    local -a parts
    CPUS=()
    [[ $list =~ ^[0-9]+(-[0-9]+)?(,[0-9]+(-[0-9]+)?)*$ ]] || return 1
    IFS=, read -r -a parts <<<"$list"
    for part in "${parts[@]}"; do
        a=${part%-*} b=${part#*-}
        (( ${#a} <= 4 && ${#b} <= 4 )) || return 1
        a=$((10#$a)) b=$((10#$b))
        (( a <= b )) || return 1
        for (( ; a <= b; a++ )); do CPUS[a]=1; done
    done
}

# intersects LIST — do LIST's CPUs meet the slot mask (MASK)? An unreadable list does.
intersects() {
    local cpu
    cpu_set "$1" || return 0
    for cpu in "${!CPUS[@]}"; do
        [[ -n ${MASK[cpu]+set} ]] && return 0
    done
    return 1
}

# row_fields LINE — a `tsp -l` row whose command runs our wrapper. Parsed from the command side
# (the Output column is "(file)" for a queued row and a path otherwise, probe 5.3c): the job dir
# and the mask are the two tokens after `<…>/bin/wrapper-<hex>.sh`. Sets RW_ID RW_STATE RW_JOB
# RW_MASK; returns 1 for any other line (the header, a command that is not our wrapper).
row_fields() {
    local LC_ALL=C i
    local -a tok
    read -r -a tok <<<"$1"
    [[ ${tok[0]-} =~ ^[0-9]+$ ]] || return 1
    for (( i = 2; i + 2 < ${#tok[@]}; i++ )); do
        if [[ ${tok[i]} =~ ^/.*/bin/wrapper-[0-9a-f]+\.sh$ ]]; then
            RW_ID=${tok[0]} RW_STATE=${tok[1]} RW_JOB=${tok[i+1]} RW_MASK=${tok[i+2]}
            return 0
        fi
    done
    return 1
}

# candidate PID KIND JOB_TOKEN CORES — a blocker unless its cores miss the mask or its job token
# is the job dir of a running row of this slot's daemon.
candidate() {
    intersects "$4" || return 0
    [[ -n $3 && -n ${ACCOUNTED[$3]+set} ]] && return 0
    if [[ -n $3 ]]; then
        printf 'block pid %s (%s, cores %s, job dir %q)\n' "$1" "$2" "$4" "$3"
    else
        printf 'block pid %s (%s, cores %s, cwd not readable)\n' "$1" "$2" "$4"
    fi
}

# slot_scan JOB SOCKET MASK SLOT_ROWS — SLOT_ROWS is the file of the slot daemon's `tsp -l`, or
# "-" when no daemon listens on the slot socket (then nothing is accounted for).
slot_scan() {
    set -u
    export LC_ALL=C
    local job=$1 sock=$2 mask=$3 slot_rows=$4 own='' d pid k v rest uid cpus state cwd line p rows
    local -a argv cols
    local -A seen=()
    declare -a CPUS=() MASK=()
    declare -A ACCOUNTED=()

    cpu_set "$mask" || { printf 'error the slot mask %s is not a CPU list\n' "$mask"; exit 3; }
    for k in "${!CPUS[@]}"; do MASK[k]=1; done

    # The full set: this shell's own allowed list (o item 9); unreadable → fail closed.
    { while IFS=$'\t' read -r k v; do
        [[ $k == Cpus_allowed_list: ]] && own=$v
      done </proc/$$/status; } 2>/dev/null
    [[ -n $own ]] || { printf 'error cannot read this shell'"'"'s Cpus_allowed_list\n'; exit 3; }

    # Accounted for: the job dirs of this slot daemon's running rows. Its queued rows are its own
    # queue and never block it.
    if [[ $slot_rows != - ]]; then
        while IFS= read -r line; do
            row_fields "$line" && [[ $RW_STATE == running ]] && ACCOUNTED[$RW_JOB]=1
        done <"$slot_rows"
    fi

    # Own processes. A read that fails means the process is gone (ENOENT races, probe 5.3b).
    for d in /proc/[0-9]*; do
        pid=${d#/proc/}
        [[ $pid == "$$" ]] && continue
        uid='' cpus='' state=''
        { while IFS=$'\t' read -r k v rest; do
            case $k in
                Uid:) uid=$v ;;
                Cpus_allowed_list:) cpus=$v ;;
                State:) state=${v%% *} ;;
            esac
          done <"$d/status"; } 2>/dev/null
        [[ -n $uid && -n $cpus && $uid == "$UID" ]] || continue
        # A wrapper of any root of this account, matched element by element (never a grep of
        # the joined cmdline: a scanning shell's own text holds the wrapper's name).
        argv=()
        { mapfile -d '' -t argv <"$d/cmdline"; } 2>/dev/null
        if (( ${#argv[@]} >= 4 )) && [[ ${argv[0]} == bash && ${argv[1]} =~ ^/.*/bin/wrapper-[0-9a-f]+\.sh$ ]]; then
            candidate "$pid" wrapper "${argv[2]}" "${argv[3]}"
        fi
        # A pinned process, whatever its cwd: a zombie or an unreadable cwd counts only when pinned.
        if [[ $cpus != "$own" ]]; then
            # Byte-exact: `$(…)` would strip a trailing newline from the cwd.
            if cwd=$(readlink -n -- "$d/cwd" 2>/dev/null </dev/null && printf x); then
                cwd=${cwd%x}
            else
                cwd=''
            fi
            [[ $state == Z ]] && cwd=''
            candidate "$pid" pinned "$cwd" "$cpus"
        fi
    done

    # Queued work of every other live, own slot socket (the `<dir>/tsp/slot<N>.sock` layout).
    { IFS= read -r line
      read -r -a cols <<<"$line"
      if [[ ${cols[*]} != "Num RefCount Protocol Flags Type St Inode Path" ]]; then
          printf 'error /proc/net/unix: unexpected header\n'
          exit 3
      fi
      while IFS= read -r line; do
          read -r -a cols <<<"$line"
          (( ${#cols[@]} == 8 )) && seen[${cols[7]}]=1
      done; } </proc/net/unix
    for p in "${!seen[@]}"; do
        [[ $p != "$sock" && $p =~ ^/.+/tsp/slot[0-9]+\.sock$ ]] || continue
        valid_path "$p" && [[ -O $p ]] || continue
        if ! rows=$(TS_SOCKET=$p timeout -k 1 2 tsp -l </dev/null 2>/dev/null 9>&-); then
            printf 'error tsp -l on %s failed\n' "$p"
            exit 3
        fi
        while IFS= read -r line; do
            row_fields "$line" && [[ $RW_STATE == queued ]] || continue
            intersects "$RW_MASK" || continue
            printf 'block queued row %s on %s (job dir %q, mask %s)\n' "$RW_ID" "$p" "$RW_JOB" "$RW_MASK"
        done <<<"$rows"
    done
    printf 'done\n'
}

main() {
    local a job root sock mask orca wrapper wsha id name sha rc i line content lock slot_rows
    local -a lines files_found recs blockers
    local -A expected=() found=() nonregular=() hashed=()
    local -a missing=() extra=() differing=() paths=()

    printf 'orcastudio-submit 1\n'
    printf 'argc %s\n' "$#"
    for a in "$@"; do
        emit_text arg "$a"
    done

    # Step 0: the values' form, before the lock (nothing here touches the server).
    (( $# >= 6 && ($# - 6) % 2 == 0 )) || refuse "values: expected 6 plus 2 per file, got $#"
    job=$1 root=$2 sock=$3 mask=$4 orca=$5 wsha=$6
    shift 6
    valid_path "$root" || refuse "values: the root breaks the path rule"
    id=${job#"$root/jobs/"}
    if [[ $id == "$job" || -z $id || $id == */* ]] || ! valid_path "$job"; then
        refuse "values: the job dir is not <root>/jobs/<id>"
    fi
    valid_path "$sock" || refuse "values: the socket breaks the path rule"
    (( ${#sock} <= MAX_SOCKET )) || refuse "values: the socket path is longer than $MAX_SOCKET bytes"
    [[ $mask =~ ^[0-9]+([,-][0-9]+)*$ ]] || refuse "values: the core mask is not a CPU list"
    [[ $orca == /* ]] || refuse "values: the ORCA path is not absolute"
    [[ $wsha =~ ^[0-9a-f]{64}$ ]] || refuse "values: the wrapper sha256 is not 64 lowercase hex digits"
    wrapper=$root/bin/wrapper-$wsha.sh
    (( $# / 2 <= MAX_FILES )) || refuse "values: more than $MAX_FILES files"
    while (( $# > 0 )); do
        name=$1 sha=$2
        shift 2
        valid_path "/$name" || refuse "values: file name $(printf '%q' "$name") breaks the path rule"
        [[ $sha =~ ^[0-9a-f]{64}$ ]] || refuse "values: the sha256 of $name is not 64 lowercase hex digits"
        [[ -z ${expected[$name]+set} ]] || refuse "values: $name is listed twice"
        expected[$name]=$sha
    done

    T=$(mktemp -d </dev/null) || refuse "mktemp -d failed"
    trap 'rm -rf -- "$T" </dev/null 9>&-' EXIT

    # Step 1: the account lock, outside every root, so profiles aliasing one host serialise.
    [[ -n ${HOME-} ]] || refuse "lock: HOME is not set"
    lock="$HOME/.orcastudio-submit.lock"
    if ! { exec 9>"$lock"; } 2>/dev/null; then
        refuse "lock: cannot open $lock"
    fi
    flock -w "$LOCK_WAIT" 9 </dev/null
    rc=$?
    (( rc == 1 )) && refuse "lock busy: lock file open in: $(lock_openers "$lock")"
    (( rc == 0 )) || refuse "lock: flock exited $rc"

    # Step 2: KillUserProcesses (n item 7), the realpath shapes (o item 1), the wrapper's bytes.
    timeout -k "$T_KILL" "$T_BUSCTL" "${KILL_USER_PROCESSES[@]}" </dev/null >"$T/kup" 2>"$T/kup.err" 9>&-
    rc=$?
    content=''
    IFS= read -r -d '' content <"$T/kup"
    # Exactly the 8 bytes "b false\n": `read -d ''` stops at a NUL, so the size is checked too.
    if (( rc != 0 )) || [[ $content != $'b false\n' ]] || [[ $(stat -c %s -- "$T/kup" </dev/null) != 8 ]]; then
        refuse_kup "$rc" "$T/kup" "$T/kup.err"
    fi
    timeout -k "$T_KILL" "$T_REALPATH" realpath -e -- "$job" "$job/.." </dev/null >"$T/realpath" 2>"$T/realpath.err" 9>&-
    rc=$?
    mapfile -t lines <"$T/realpath"
    if (( rc != 0 || ${#lines[@]} != 2 )) || [[ ${lines[0]} != "$job" || ${lines[1]} != "$root/jobs" ]]; then
        refuse "realpath: the job dir is not $job under $root/jobs (rc $rc: ${lines[*]} $(first_line "$T/realpath.err"))"
    fi
    [[ -f $wrapper && ! -L $wrapper ]] || refuse "wrapper: $wrapper is not a regular file"
    timeout -k "$T_KILL" "$T_WRAPPER_REALPATH" realpath -e -- "$wrapper" </dev/null >"$T/wrealpath" 2>"$T/wrealpath.err" 9>&-
    rc=$?
    line=''
    { IFS= read -r line || true; } <"$T/wrealpath"
    (( rc == 0 )) && [[ $line == "$wrapper" ]] || refuse "wrapper: $wrapper is reached through a symlink (realpath: $line)"
    timeout -k "$T_KILL" "$T_WRAPPER_SHA" sha256sum -- "$wrapper" </dev/null >"$T/wsha" 2>"$T/wsha.err" 9>&-
    rc=$?
    line=''
    { IFS=' ' read -r line _ || true; } <"$T/wsha"
    (( rc == 0 )) && [[ $line == "$wsha" ]] || refuse "wrapper: the sha256 of $wrapper is not the one its name carries"

    # Step 3: no marker, and no row of the slot daemon holding this job dir.
    for name in "${MARKERS[@]}"; do
        path_exists "$job/$name"
        case $? in
            0) refuse "marker: $job/$name exists" ;;
            1) ;;
            *) refuse "marker: $R_ERR" ;;
        esac
    done
    read_raw /proc/net/unix "$T/net_unix" || refuse "/proc/net/unix: ${R_ERR:-absent}"
    net_unix_listed "$T/net_unix" "$T/net_unix.slot" "$sock"
    case $? in
        0) slot_rows=$T/slot_rows
           TS_SOCKET=$sock timeout -k "$T_KILL" "$T_TSP_LIST" tsp -l </dev/null >"$slot_rows" 2>"$T/slot_rows.err" 9>&-
           rc=$?
           (( rc == 0 )) || refuse "slot socket: tsp -l on $sock exited $rc $(first_line "$T/slot_rows.err")"
           while IFS= read -r line; do
               [[ " $line " == *" $job "* ]] && refuse "row: a tsp row on $sock already holds $job: $line"
           done <"$slot_rows" ;;
        1) slot_rows=- ;;
        *) refuse "$R_ERR" ;;
    esac

    # Step 4: the upload post-condition (rule #9) — the set of files and every sha256.
    timeout -k "$T_KILL" "$T_FIND" find "$job" -mindepth 1 -path "$job/.tsp-out" -prune -o -printf '%y %P\0' \
        </dev/null >"$T/files" 2>"$T/files.err" 9>&-
    rc=$?
    (( rc == 0 )) || refuse "upload: listing $job failed (rc $rc) $(first_line "$T/files.err")"
    mapfile -d '' -t recs <"$T/files"
    for line in "${recs[@]}"; do
        case ${line%% *} in
            d) ;;
            f) found[${line#* }]=1 ;;
            *) nonregular[${line#* }]=1 ;;
        esac
    done
    for name in "${!expected[@]}"; do
        if [[ -n ${found[$name]+set} ]]; then
            paths+=("$job/$name")
        elif [[ -n ${nonregular[$name]+set} ]]; then
            differing+=("$name")
        else
            missing+=("$name")
        fi
    done
    for name in "${!found[@]}" "${!nonregular[@]}"; do
        [[ -n ${expected[$name]+set} ]] || extra+=("$(printf '%q' "$name")")
    done
    if (( ${#paths[@]} > 0 )); then
        timeout -k "$T_KILL" "$T_SHA" sha256sum -- "${paths[@]}" </dev/null >"$T/sha" 2>"$T/sha.err" 9>&-
        rc=$?
        (( rc == 0 )) || refuse "upload: sha256sum exited $rc $(first_line "$T/sha.err")"
        while IFS= read -r line; do
            # Every name passed the path rule, so sha256sum prints it unescaped.
            [[ $line =~ ^([0-9a-f]{64})\ \ (.*)$ ]] || refuse "upload: unexpected sha256sum line"
            hashed[${BASH_REMATCH[2]#"$job/"}]=${BASH_REMATCH[1]}
        done <"$T/sha"
        for name in "${!expected[@]}"; do
            [[ -n ${found[$name]+set} ]] || continue
            [[ ${hashed[$name]-} == "${expected[$name]}" ]] || differing+=("$name")
        done
    fi
    if (( ${#missing[@]} + ${#extra[@]} + ${#differing[@]} > 0 )); then
        refuse "upload: missing [${missing[*]}], extra [${extra[*]}], differing [${differing[*]}]"
    fi

    # Step 5: the slot check, bounded like every other child.
    timeout -k "$T_KILL" "$T_SCAN" bash -c "$(declare -f valid_path cpu_set intersects row_fields candidate slot_scan)"'
slot_scan "$@"' slot-scan "$job" "$sock" "$mask" "$slot_rows" </dev/null >"$T/scan" 2>"$T/scan.err" 9>&-
    rc=$?
    mapfile -t lines <"$T/scan"
    (( rc == 124 )) && refuse "slot check: timed out after $T_SCAN s"
    (( rc == 0 )) || refuse "slot check: ${lines[*]:-exited $rc} $(first_line "$T/scan.err")"
    (( ${#lines[@]} > 0 )) && [[ ${lines[-1]} == done ]] || refuse "slot check: unexpected output"
    blockers=()
    for line in "${lines[@]:0:${#lines[@]}-1}"; do
        [[ $line == "block "* ]] || refuse "slot check: unexpected line: $line"
        blockers+=("${line#block }")
    done
    if (( ${#blockers[@]} > 0 )); then
        refuse "slot busy: mask $mask is held by $(IFS=';'; printf '%s' "${blockers[*]}")"
    fi

    # Step 6: tsp's output dir (m item 1); not a marker, harmless if anything below fails.
    mkdir -p -- "$job/.tsp-out" </dev/null 2>"$T/mkdir.err" 9>&-
    [[ -d $job/.tsp-out && ! -L $job/.tsp-out ]] || refuse "tsp-out: cannot create $job/.tsp-out $(first_line "$T/mkdir.err")"

    # Step 7: the claim — no-clobber, so of two claimers exactly one wins (probe 5.3c).
    ln -sT x "$job/.submitting" </dev/null 2>"$T/claim.err" 9>&- \
        || refuse "claim: $(first_line "$T/claim.err")"

    # Step 8: enqueue. From here on the claim stays, whatever happens.
    TMPDIR=$job/.tsp-out TS_SOCKET=$sock timeout -k "$T_KILL" "$T_ENQUEUE" tsp bash "$wrapper" "$job" "$mask" "$orca" \
        </dev/null >"$T/enqueue" 2>"$T/enqueue.err" 9>&-
    rc=$?
    (( rc == 0 )) || failed_after_claim "enqueue: tsp exited $rc $(first_line "$T/enqueue.err")"
    content=''
    IFS= read -r -d '' content <"$T/enqueue"
    [[ $content =~ ^[0-9]+$'\n'?$ ]] || failed_after_claim "enqueue: tsp printed $(printf '%q' "${content:0:200}"), not a job id"
    dec_norm "${content%$'\n'}"
    id=$REPLY
    if ! { printf 'socket=%s\nid=%s\n' "$sock" "$id" >"$job/.enqueued.tmp.$$" \
            && mv -fT -- "$job/.enqueued.tmp.$$" "$job/.enqueued"; } </dev/null 2>/dev/null 9>&-; then
        rm -f -- "$job/.enqueued.tmp.$$" </dev/null 9>&-
        failed_after_claim "enqueued as tsp id $id, but $job/.enqueued could not be published"
    fi
    # The post-condition of (l): the socket verbatim in /proc/net/unix, so a truncated listing
    # can never fake NoDaemon later.
    read_raw /proc/net/unix "$T/net_unix" || failed_after_claim "enqueued as tsp id $id; /proc/net/unix: ${R_ERR:-absent}"
    net_unix_listed "$T/net_unix" "$T/net_unix.after" "$sock"
    (( $? == 0 )) || failed_after_claim "enqueued as tsp id $id, but $sock is not listed verbatim in /proc/net/unix"
    printf 'enqueued %s\nend\n' "$id"
    exit 0
}

args=(); while IFS= read -r -d '' a; do args+=("$a"); done; main "${args[@]}"; exit
