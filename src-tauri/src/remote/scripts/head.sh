#!/bin/bash
# OrcaStudio remote-job script (ADR-024 Decision l).
#
# This head is shared by wrapper.sh, cancel.sh and collect.sh: the build concatenates it in front
# of each body (src-tauri/src/remote/scripts.rs), so all three ship the same parsers byte for byte.
# Every per-job value arrives as a positional argument; nothing is ever evaluated as shell code.
set -u

U32_MAX=4294967295
U64_MAX=18446744073709551615

# dec_norm VALUE — VALUE must be ASCII digits; sets REPLY to it without leading zeros, so two
# normalised decimals are numerically equal iff they are equal as strings (any length).
dec_norm() {
    local LC_ALL=C v=$1
    [[ $v =~ ^[0-9]+$ ]] || return 1
    while [[ $v == 0?* ]]; do v=${v#0}; done
    REPLY=$v
}

# dec_le A B — A and B normalised decimals; true iff A <= B. No shell arithmetic, so no overflow.
dec_le() {
    local LC_ALL=C a=$1 b=$2
    if (( ${#a} != ${#b} )); then
        (( ${#a} < ${#b} ))
        return
    fi
    [[ $a == "$b" || $a < "$b" ]]
}

# valid_path PATH — an absolute path over [A-Za-z0-9._-] components, with no empty, "." or ".."
# component and no trailing "/". Job dirs, the root and socket paths must have this form: the cwd
# filter compares paths exactly and tsp rows are matched by whole token (ADR-024 l).
valid_path() {
    local LC_ALL=C p=$1
    [[ $p =~ ^(/[A-Za-z0-9._-]+)+$ ]] || return 1
    [[ $p/ != */./* && $p/ != */../* ]]
}

# parse_stat_line LINE — one /proc/<pid>/stat line. The fields after comm are taken from the text
# after the LAST ") " (comm may hold spaces and parens); field N is token N-2 (probe 5.2c).
# Sets ST_PID (1), ST_STATE (3), ST_PGRP (5), ST_SID (6), ST_START (22); returns 1 if malformed.
parse_stat_line() {
    local LC_ALL=C line=$1 rest
    local -a f
    ST_PID='' ST_STATE='' ST_PGRP='' ST_SID='' ST_START=''
    [[ $line == *" ("*") "* ]] || return 1
    rest=${line##*) }
    IFS=' ' read -r -a f <<<"$rest"
    (( ${#f[@]} >= 20 )) || return 1
    [[ ${f[0]} =~ ^[A-Za-z]$ ]] || return 1
    dec_norm "${line%% (*}" || return 1
    ST_PID=$REPLY
    ST_STATE=${f[0]}
    dec_norm "${f[2]}" || return 1
    ST_PGRP=$REPLY
    dec_norm "${f[3]}" || return 1
    ST_SID=$REPLY
    dec_norm "${f[19]}" || return 1
    ST_START=$REPLY
}

# kv_load FILE KEY... — a strict "key=value\n" marker: every listed KEY exactly once, no other
# line, every line newline-terminated, no NUL byte (the byte count must add up). Fills the
# associative array KV, or sets KV_WHY and returns 1.
declare -A KV=()
kv_load() {
    local LC_ALL=C f=$1 line key k known size bytes=0
    shift
    local -a lines=()
    KV=() KV_WHY=''
    [[ -f $f ]] || { KV_WHY="not a regular file"; return 1; }
    size=$(stat -c %s -- "$f" 2>/dev/null) || { KV_WHY="cannot stat"; return 1; }
    mapfile -t lines <"$f" 2>/dev/null || { KV_WHY="cannot read"; return 1; }
    for line in "${lines[@]}"; do
        bytes=$(( bytes + ${#line} + 1 ))
    done
    (( bytes == size )) || { KV_WHY="not newline-terminated text without NUL bytes"; return 1; }
    for line in "${lines[@]}"; do
        [[ $line == *=* ]] || { KV_WHY="line without '='"; return 1; }
        key=${line%%=*}
        known=1
        for k in "$@"; do
            [[ $key == "$k" ]] && known=0
        done
        (( known == 0 )) || { KV_WHY="unknown key '$key'"; return 1; }
        [[ -z ${KV[$key]+set} ]] || { KV_WHY="duplicate key '$key'"; return 1; }
        KV[$key]=${line#*=}
    done
    for key in "$@"; do
        [[ -n ${KV[$key]+set} ]] || { KV_WHY="missing key '$key'"; return 1; }
    done
}

# started_parse_file FILE — the .started marker, by the same rules as markers::parse_started:
# pid, pgid, sid (positive, fit u32), boot_id (lowercase 8-4-4-4-12 hex), starttime and
# started_at (decimal, fit u64). Sets S_PID S_PGID S_SID S_BOOT S_START S_AT (normalised), or
# S_WHY and returns 1.
started_parse_file() {
    local LC_ALL=C key
    S_PID='' S_PGID='' S_SID='' S_BOOT='' S_START='' S_AT='' S_WHY=''
    kv_load "$1" pid pgid sid boot_id starttime started_at || { S_WHY=$KV_WHY; return 1; }
    for key in pid pgid sid; do
        if ! dec_norm "${KV[$key]}" || [[ $REPLY == 0 ]] || ! dec_le "$REPLY" "$U32_MAX"; then
            S_WHY="$key is not a valid process id"
            return 1
        fi
        KV[$key]=$REPLY
    done
    for key in starttime started_at; do
        if ! dec_norm "${KV[$key]}" || ! dec_le "$REPLY" "$U64_MAX"; then
            S_WHY="$key is not a decimal u64"
            return 1
        fi
        KV[$key]=$REPLY
    done
    [[ ${KV[boot_id]} =~ ^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$ ]] \
        || { S_WHY="boot_id is not a lowercase UUID"; return 1; }
    S_PID=${KV[pid]} S_PGID=${KV[pgid]} S_SID=${KV[sid]}
    S_BOOT=${KV[boot_id]} S_START=${KV[starttime]} S_AT=${KV[started_at]}
}

# enqueued_parse_file FILE — the .enqueued marker 5.3's submit writes: "socket=<path>\nid=<n>\n"
# (any order, each once). Sets E_SOCKET and E_ID (normalised), or E_WHY and returns 1.
enqueued_parse_file() {
    E_SOCKET='' E_ID='' E_WHY=''
    kv_load "$1" socket id || { E_WHY=$KV_WHY; return 1; }
    valid_path "${KV[socket]}" || { E_WHY="socket is not a valid absolute path"; return 1; }
    dec_norm "${KV[id]}" || { E_WHY="id is not a decimal"; return 1; }
    E_SOCKET=${KV[socket]} E_ID=$REPLY
}

# The raw readers below tell "absent" from "error". Only ENOENT and ESRCH mean absent (the file
# or the process does not exist; a zombie's cwd is ENOENT, probe 5.2c). Anything else — EACCES,
# EISDIR, EIO, a message we do not recognise — is an error, never "absent" (ADR-024 l, rule #9).
# Messages are matched exactly in the C locale (coreutils 9.4, measured 2026-10-03); a message
# that differs fails closed, as an error. Each returns 0 (read), 1 (absent) or 2 (error, R_ERR).

# read_raw PATH DEST — copy PATH's bytes to DEST with cat.
read_raw() {
    local err
    R_ERR=''
    if err=$(LC_ALL=C cat -- "$1" 2>&1 >"$2"); then return 0; fi
    case $err in
        "cat: $1: No such file or directory" | "cat: $1: No such process") return 1 ;;
    esac
    R_ERR="read $1: ${err:-cat failed without a message}"
    return 2
}

# read_link PATH DEST — the raw target of a symlink (a /proc/<pid>/cwd), without a newline.
# Plain `readlink` prints nothing on EACCES (measured); -v makes it say why.
read_link() {
    local err
    R_ERR=''
    if err=$(LC_ALL=C readlink -n -v -- "$1" 2>&1 >"$2"); then return 0; fi
    case $err in
        "readlink: $1: No such file or directory" | "readlink: $1: No such process") return 1 ;;
    esac
    R_ERR="readlink $1: ${err:-readlink failed without a message}"
    return 2
}

# path_exists PATH — does PATH exist (any type, a dangling symlink included)?
path_exists() {
    local err
    R_ERR=''
    if err=$(LC_ALL=C stat -c %F -- "$1" 2>&1); then return 0; fi
    case $err in
        "stat: cannot statx '$1': No such file or directory") return 1 ;;
    esac
    R_ERR="stat $1: ${err:-stat failed without a message}"
    return 2
}

# session_pids SID — the PIDs of `ps -s SID` into SESSION_PIDS. procps-ng exits 1 both for "no
# such session" (empty stdout and stderr) and for errors (a message on stderr), so only the
# silent case is read as an empty session. Returns 0, or 2 with R_ERR. Needs the caller's
# private temp dir in $T (cancel.sh and collect.sh; the wrapper does not use it).
session_pids() {
    local out err rc line
    SESSION_PIDS=()
    R_ERR=''
    out=$(LC_ALL=C ps -o pid= -s "$1" 2>"$T/ps.err")
    rc=$?
    err=$(<"$T/ps.err")
    if (( rc != 0 )) && [[ -n $out || -n $err || $rc != 1 ]]; then
        R_ERR="ps -s $1: rc=$rc ${err}"
        return 2
    fi
    while IFS=' ' read -r line; do
        [[ -z $line ]] && continue
        dec_norm "$line" || { R_ERR="ps -s $1: not a pid: $line"; return 2; }
        SESSION_PIDS+=("$REPLY")
    done <<<"$out"
}

# net_unix_listed SRC DEST SOCKET — SRC is a copy of /proc/net/unix. A daemon listens on SOCKET
# iff some line's path column is SOCKET exactly (a stale socket file is not listed, probe 5.2b).
# DEST gets the header plus every such line: the evidence Rust re-checks. Returns 0 (listed),
# 1 (not listed) or 2 (the header is not the known one, R_ERR).
net_unix_listed() {
    local LC_ALL=C src=$1 dest=$2 sock=$3 header line listed=1
    local -a cols
    R_ERR=''
    { IFS= read -r header || true; } <"$src"
    read -r -a cols <<<"$header"
    if [[ ${cols[*]} != "Num RefCount Protocol Flags Type St Inode Path" ]]; then
        R_ERR="/proc/net/unix: unexpected header: $header"
        return 2
    fi
    printf '%s\n' "$header" >"$dest"
    while IFS= read -r line; do
        read -r -a cols <<<"$line"
        if (( ${#cols[@]} > 7 )) && [[ ${cols[*]:7} == "$sock" ]]; then
            printf '%s\n' "$line" >>"$dest"
            listed=0
        fi
    done < <(tail -n +2 -- "$src")
    return "$listed"
}

# own_session — this shell's own SID and process group, from the builtin read of /proc/$$/stat
# (`$(…)`/`cat` would report a child's PID, probe 5.2c). Sets OWN_SID and OWN_PGRP.
own_session() {
    local l
    read -r l </proc/$$/stat || return 1
    parse_stat_line "$l" || return 1
    OWN_SID=$ST_SID OWN_PGRP=$ST_PGRP
}
