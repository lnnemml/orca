#!/usr/bin/env python3
"""Out-of-band fetcher for the ORCA manual sources (ADR-013).

This is an AUTHOR tool, run by hand once per ORCA version. It is NOT part of the
running application: not a Tauri command, not a sidecar endpoint (ADR-013 (2)).
Its only network target is the ORCA manual docs; it never follows links found in
document bodies (ADR-013 — a manifest walk, not a crawl).

Standard library only — no third-party dependencies (ADR-013 consequence).

The ORCA 6.1 manual is Sphinx + MyST built with ``html_copy_source``, so the
original Markdown ships at ``<base>_sources/<path>.md.txt``. We build the file
list by a DETERMINISTIC walk of the ``{toctree}`` graph starting from the root
``index``, and fetch ONLY paths that walk produces.

Scope: this script does ONLY what fetching needs — fetch, manifest, the toctree
walk (``parse_toctrees``, an allow-list), and ``objects.inv`` retrieval. Body
CONTENT analysis (ATX sectioning, keyword-markup classification, anchor rule) was
moved to Rust (ADR-013 (3): ``src-tauri/src/manual/``, unit 4.2), where it is
verified against the whole corpus by ``cargo test manual_corpus -- --ignored``.
The numbers those checks produce live in ``wiki/orca/manual-sources.md``.

Usage:
    python scripts/fetch-manual.py --manifest            # walk the toctree, report
    python scripts/fetch-manual.py --all                 # fetch every leaf + manifest.json
    python scripts/fetch-manual.py --objects-inv         # fetch the label->anchor map
"""

from __future__ import annotations

import argparse
import hashlib
import json
import posixpath
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
from datetime import datetime, timezone
from pathlib import Path

# --- Configuration ---------------------------------------------------------

DEFAULT_BASE = "https://www.faccts.de/docs/orca/6.1/manual/"
ORCA_VERSION = "6.1"

# A descriptive UA with the project name and a contact link — this is someone
# else's server; identify ourselves and say why.
USER_AGENT = (
    "OrcaStudio-manual-fetch/0.1 "
    "(+https://github.com/lnnemml/orca-studio; personal research use, ADR-013)"
)

REQUEST_PAUSE_S = 0.7          # polite gap between requests
MAX_RETRIES = 3               # on 5xx / timeout, with backoff
REQUEST_TIMEOUT_S = 30
HARD_REQUEST_CAP = 250        # a walk bug must not become a hammer

# toctree entries that are Sphinx-generated pages with NO .md.txt source. Not a
# lost branch — classified and named in the report so the author can confirm.
GENERATED_NO_SOURCE = {
    "genindex", "modindex", "py-modindex", "search",
    "bibliography", "html_versions",
}

# Representative sample that MUST be included when --sample is used: the two
# files that carry native "Keywords" sections (RI → RIJCOSX; solvation → the
# "Complete Keyword List for the %cpcm Block"), plus a Structure-and-Reactivity
# leaf (the research-program branch guarded by the double-slash normalization).
MANDATORY_SAMPLE = [
    "contents/essentialelements/RI",
    "contents/structurereactivity/optimizations_scans",
    "contents/essentialelements/solvationmodels",
]


# --- Fetcher ---------------------------------------------------------------

class RequestCapExceeded(RuntimeError):
    pass


@dataclass
class FetchResult:
    url: str
    status: int          # HTTP status (304 = not modified), or 0 for a network failure
    body: bytes | None
    error: str | None = None
    etag: str | None = None
    last_modified: str | None = None

    @property
    def ok(self) -> bool:
        return self.status == 200 and self.body is not None


class Fetcher:
    """Polite, capped HTTP GET with retry/backoff. Counts every attempt."""

    def __init__(self, cap: int = HARD_REQUEST_CAP):
        self.cap = cap
        self.attempts = 0
        self._first = True

    def get(self, url: str, if_none_match: str | None = None,
            if_modified_since: str | None = None) -> FetchResult:
        """GET url. With a conditional header (If-None-Match from a stored ETag, or
        If-Modified-Since from a stored Last-Modified), a 304 comes back as
        status=304 with no body and the caller reuses its cached copy. Measured:
        faccts.de sends Last-Modified but NOT ETag, so If-Modified-Since is the
        header that actually yields 304s here."""
        last_err = None
        for attempt in range(1, MAX_RETRIES + 1):
            if self.attempts >= self.cap:
                raise RequestCapExceeded(
                    f"hard request cap {self.cap} reached at {url!r} — "
                    "aborting to avoid hammering the server"
                )
            self.attempts += 1
            # Pause between requests (not before the very first).
            if not self._first:
                time.sleep(REQUEST_PAUSE_S)
            self._first = False
            headers = {"User-Agent": USER_AGENT}
            if if_none_match:
                headers["If-None-Match"] = if_none_match
            if if_modified_since:
                headers["If-Modified-Since"] = if_modified_since
            req = urllib.request.Request(url, headers=headers)
            try:
                with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT_S) as resp:
                    return FetchResult(
                        url=url, status=resp.status, body=resp.read(),
                        etag=resp.headers.get("ETag"),
                        last_modified=resp.headers.get("Last-Modified"),
                    )
            except urllib.error.HTTPError as e:
                # 304 Not Modified is a real, non-error answer to a conditional GET.
                if e.code == 304:
                    return FetchResult(url=url, status=304, body=None,
                                       etag=if_none_match, last_modified=if_modified_since)
                # 4xx is terminal (a 404 is a real answer); 5xx is retryable.
                if 500 <= e.code < 600 and attempt < MAX_RETRIES:
                    last_err = f"HTTP {e.code}"
                    time.sleep(REQUEST_PAUSE_S * (2 ** attempt))
                    continue
                return FetchResult(url=url, status=e.code, body=None, error=f"HTTP {e.code}")
            except (urllib.error.URLError, TimeoutError) as e:
                last_err = str(getattr(e, "reason", e))
                if attempt < MAX_RETRIES:
                    time.sleep(REQUEST_PAUSE_S * (2 ** attempt))
                    continue
                return FetchResult(url=url, status=0, body=None, error=last_err)
        return FetchResult(url=url, status=0, body=None, error=last_err or "unknown")


# --- toctree parsing -------------------------------------------------------

# A MyST directive fence opens with >=3 backticks OR >=3 colons, then {name}.
_FENCE_OPEN = re.compile(r"^(?P<fence>`{3,}|:{3,})\{(?P<name>[a-zA-Z0-9_-]+)\}")
# An entry may be "Title <path>" or a bare "path".
_TITLED_ENTRY = re.compile(r"^.*<(?P<path>[^>]+)>\s*$")
# RST toctree inside an {eval-rst} block.
_RST_TOCTREE = re.compile(r"^\s*\.\.\s+toctree::")


def _is_fence_close(line: str, fence_char: str, fence_len: int) -> bool:
    s = line.strip()
    return len(s) >= fence_len and set(s) == {fence_char}


def _clean_entry(raw: str) -> str | None:
    """Turn a raw toctree line into a path, or None if it is not an entry."""
    s = raw.strip()
    if not s:
        return None
    if s.startswith(":"):          # directive option (:maxdepth:, :hidden:, ...)
        return None
    m = _TITLED_ENTRY.match(s)
    if m:
        return m.group("path").strip()
    # A bare path: reject anything with whitespace (a stray prose line).
    if re.search(r"\s", s):
        return None
    return s


def parse_toctrees(text: str) -> tuple[list[str], int]:
    """Return (raw entries, eval_rst_block_count) from a document's Markdown.

    Handles backtick and colon {toctree} fences of any length, and RST
    ``.. toctree::`` directives inside {eval-rst} fences.
    """
    lines = text.splitlines()
    entries: list[str] = []
    eval_rst_blocks = 0
    i = 0
    n = len(lines)
    while i < n:
        line = lines[i]
        m = _FENCE_OPEN.match(line)
        if not m:
            i += 1
            continue
        fence = m.group("fence")
        fence_char, fence_len = fence[0], len(fence)
        name = m.group("name")
        # Collect the fenced block body.
        block: list[str] = []
        i += 1
        while i < n and not _is_fence_close(lines[i], fence_char, fence_len):
            block.append(lines[i])
            i += 1
        i += 1  # skip the closing fence
        if name == "toctree":
            for b in block:
                e = _clean_entry(b)
                if e:
                    entries.append(e)
        elif name == "eval-rst":
            eval_rst_blocks += 1
            entries.extend(_parse_rst_toctree(block))
    return entries, eval_rst_blocks


def _parse_rst_toctree(block: list[str]) -> list[str]:
    """Extract entries from ``.. toctree::`` directives inside an eval-rst block."""
    out: list[str] = []
    j = 0
    m = len(block)
    while j < m:
        if _RST_TOCTREE.match(block[j]):
            j += 1
            # Directive content is indented; options start with ':'.
            while j < m:
                s = block[j]
                if s.strip() == "":
                    j += 1
                    continue
                if not (s.startswith(" ") or s.startswith("\t")):
                    break  # dedent → directive ended
                stripped = s.strip()
                if stripped.startswith(":"):
                    j += 1
                    continue
                out.append(stripped)
                j += 1
        else:
            j += 1
    return out


def resolve_path(base_dir: str, entry: str) -> str:
    """Resolve a toctree entry relative to its container's directory.

    A leading '/' means source-root-relative. normpath collapses the '//' that
    really occurs in the root index ('structurereactivity//index_...') — the bug
    that would otherwise drop the whole Structure-and-Reactivity branch.
    """
    if entry.startswith("/"):
        p = entry[1:]
    else:
        p = posixpath.join(base_dir, entry) if base_dir else entry
    return posixpath.normpath(p)


def is_container(path: str) -> bool:
    """A toctree-bearing landing page. The ORCA manual names these ``index*``."""
    return posixpath.basename(path).startswith("index")


# --- Manifest --------------------------------------------------------------

@dataclass
class Entry:
    path: str
    kind: str                     # 'container' | 'leaf' | 'no-source'
    status: int | None = None     # HTTP status once fetched
    size: int | None = None
    eval_rst: int = 0


@dataclass
class Manifest:
    entries: dict[str, Entry] = field(default_factory=dict)
    order: list[str] = field(default_factory=list)      # leaves in walk order
    container_status: dict[str, FetchResult] = field(default_factory=dict)
    normalized: list[tuple[str, str]] = field(default_factory=list)  # (raw, norm) that changed

    def add(self, e: Entry) -> None:
        if e.path not in self.entries:
            self.entries[e.path] = e
            if e.kind == "leaf":
                self.order.append(e.path)


def source_url(base: str, path: str) -> str:
    return f"{base}_sources/{path}.md.txt"


def build_manifest(fetcher: Fetcher, base: str) -> Manifest:
    """Deterministic BFS over the toctree graph from the root ``index``."""
    man = Manifest()
    to_visit = ["index"]
    seen_containers: set[str] = set()
    root_eval_rst = 0

    while to_visit:
        cpath = to_visit.pop(0)
        if cpath in seen_containers:
            continue
        seen_containers.add(cpath)

        res = fetcher.get(source_url(base, cpath))
        man.container_status[cpath] = res
        man.add(Entry(path=cpath, kind="container",
                      status=res.status, size=len(res.body) if res.body else None))
        if not res.ok:
            # A container we cannot read is a NAMED failure, never a silent skip.
            print(f"  !! container fetch failed: {cpath}  ({res.error})", file=sys.stderr)
            continue

        text = res.body.decode("utf-8", errors="replace")
        raw_entries, eval_rst_n = parse_toctrees(text)
        if cpath == "index":
            root_eval_rst = eval_rst_n
        base_dir = posixpath.dirname(cpath)

        for raw in raw_entries:
            resolved = resolve_path(base_dir, raw)
            joined = posixpath.join(base_dir, raw[1:] if raw.startswith("/") else raw) if base_dir or raw.startswith("/") else raw
            if joined != resolved:
                man.normalized.append((joined, resolved))
            if posixpath.basename(resolved) in GENERATED_NO_SOURCE:
                man.add(Entry(path=resolved, kind="no-source"))
            elif is_container(resolved):
                man.add(Entry(path=resolved, kind="container"))
                to_visit.append(resolved)
            else:
                man.add(Entry(path=resolved, kind="leaf"))

    man.entries["index"].eval_rst = root_eval_rst
    return man


# --- Reporting -------------------------------------------------------------
#
# Content analysis of file bodies — ATX sectioning, keyword-markup classification,
# anchor prediction, the label→anchor cross-check — moved to Rust (ADR-013 (3):
# `src-tauri/src/manual/`, unit 4.2). It is verified there against the whole corpus
# by `cargo test manual_corpus -- --ignored`. The numbers those checks produce live
# in `wiki/orca/manual-sources.md`. This script keeps ONLY what fetching needs:
# fetch, manifest, the toctree walk (`parse_toctrees`, an allow-list), and
# `objects.inv` retrieval.

def report(man: Manifest, fetcher: Fetcher, base: str, sample_n: int) -> int:
    print("\n" + "=" * 72)
    print(f"ORCA {ORCA_VERSION} manual source measurement — base {base}")
    print("=" * 72)

    containers = [e for e in man.entries.values() if e.kind == "container"]
    leaves = [e for e in man.entries.values() if e.kind == "leaf"]
    nosrc = [e for e in man.entries.values() if e.kind == "no-source"]
    failed_containers = [p for p, r in man.container_status.items() if not r.ok]

    # (1) manifest sizes + container/leaf/no-source split, named failures.
    print(f"\n[1] MANIFEST")
    print(f"    total paths           : {len(man.entries)}")
    print(f"    containers (toctree)  : {len(containers)}  (all fetched to walk)")
    print(f"    leaves (source pages) : {len(leaves)}")
    print(f"    no-source (generated) : {len(nosrc)}  -> {sorted(e.path for e in nosrc)}")
    if failed_containers:
        print(f"    !! CONTAINER FETCH FAILURES (named): {failed_containers}")
    else:
        print(f"    container fetches     : all {len(containers)} returned 200")
    if man.normalized:
        print(f"    path normalizations (raw -> norm), incl. the '//' trap:")
        for raw, norm in man.normalized:
            print(f"        {raw}  ->  {norm}")
    else:
        print(f"    path normalizations   : none changed a path")

    # --- Sample fetch (in memory; Part A does not populate resources/manual) ---
    if sample_n <= 0:
        print("\n(no --sample: skipping source-format measurement)")
        return 1 if failed_containers else 0

    sample_paths: list[str] = []
    for p in MANDATORY_SAMPLE:
        if p in man.entries and man.entries[p].kind == "leaf":
            sample_paths.append(p)
        else:
            print(f"    !! mandatory sample not a leaf in manifest: {p}", file=sys.stderr)
    for p in man.order:
        if len(sample_paths) >= sample_n:
            break
        if p not in sample_paths:
            sample_paths.append(p)
    sample_paths = sample_paths[:max(sample_n, len(MANDATORY_SAMPLE))]

    fetched: dict[str, FetchResult] = {}
    sample_404: list[str] = []
    for p in sample_paths:
        r = fetcher.get(source_url(base, p))
        fetched[p] = r
        if not r.ok:
            sample_404.append(f"{p} ({r.error})")

    total_bytes = sum(len(r.body) for r in fetched.values() if r.body)
    ok_files = [p for p in sample_paths if fetched[p].ok]

    print(f"\n[SAMPLE] fetched {len(ok_files)}/{len(sample_paths)} in memory:")
    for p in sample_paths:
        r = fetched[p]
        sz = len(r.body) if r.body else 0
        print(f"    {r.status:>3}  {sz:>7}B  {p}")
    if sample_404:
        print(f"    !! sample 404s (named): {sample_404}")

    # (2) bytes + extrapolation.
    avg = total_bytes / max(len(ok_files), 1)
    print(f"\n[2] SIZE")
    print(f"    sample bytes (source) : {total_bytes}  over {len(ok_files)} files")
    print(f"    mean per file         : {avg:,.0f} B")
    print(f"    extrapolated corpus   : {len(leaves)} leaves x mean "
          f"~= {avg * len(leaves) / 1_048_576:,.1f} MiB (rough)")

    # (3) objects.inv presence (content parsing/sectioning is Rust's — 4.2).
    print(f"\n[3] objects.inv (authoritative label->anchor map; presence only)")
    inv = fetcher.get(base + "objects.inv")
    if inv.ok:
        print(f"    present: {len(inv.body)} bytes at {base}objects.inv "
              f"(fetch it with --objects-inv; parsing is Rust)")
    else:
        print(f"    NOT available ({inv.status} {inv.error})")

    print("\n" + "=" * 72)
    print(f"requests used: {fetcher.attempts}/{fetcher.cap}")
    print("Manifest/sample only. Body sectioning + anchor checks live in Rust "
          "(cargo test manual_corpus -- --ignored).")
    print("=" * 72)
    return 1 if (failed_containers or sample_404) else 0


# --- Part B: full fetch, manifest.json, post-conditions --------------------

def _now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def _looks_like_html(body: bytes) -> bool:
    head = body.lstrip()[:64].lower()
    return head.startswith(b"<!doctype") or head.startswith(b"<html")


def fetch_all(fetcher: Fetcher, base: str, out_dir: Path, force: bool) -> int:
    """Fetch every leaf into out_dir/<version>/<path>.md.txt, write manifest.json,
    then verify the result IN OUR TERMS (rule #9). Returns a process exit code."""
    version_dir = out_dir / ORCA_VERSION
    manifest_path = out_dir / "manifest.json"

    # Prior manifest → idempotency (skip a file whose ETag still matches).
    prior: dict[str, dict] = {}
    if manifest_path.exists() and not force:
        try:
            for rec in json.loads(manifest_path.read_text()).get("files", []):
                prior[rec["path"]] = rec
        except (json.JSONDecodeError, KeyError):
            print("  (prior manifest.json unreadable — refetching all)", file=sys.stderr)

    print("Building manifest (toctree walk)…", file=sys.stderr)
    man = build_manifest(fetcher, base)
    leaves = [e.path for e in man.entries.values() if e.kind == "leaf"]
    failed_containers = [p for p, r in man.container_status.items() if not r.ok]

    records: list[dict] = []
    print(f"Fetching {len(leaves)} leaves into {version_dir}/ …", file=sys.stderr)
    for path in leaves:
        url = source_url(base, path)
        local = version_dir / (path + ".md.txt")
        prior_rec = prior.get(path, {})
        prior_etag = prior_rec.get("etag")
        prior_lm = prior_rec.get("last_modified")
        have_local = local.exists()

        # Prefer ETag; fall back to Last-Modified (this server sends only the latter).
        res = fetcher.get(
            url,
            if_none_match=prior_etag if have_local else None,
            if_modified_since=prior_lm if (have_local and not prior_etag) else None,
        )

        if res.status == 304 and have_local:
            body = local.read_bytes()
            rec = dict(prior.get(path, {}))
            rec.update(action="reused", status=304, fetched_at=_now_iso())
            rec.setdefault("size", len(body))
            rec.setdefault("sha256", hashlib.sha256(body).hexdigest())
            rec["path"], rec["url"] = path, url
            rec["local"] = str(local.relative_to(out_dir))
            records.append(rec)
            continue

        if not res.ok:
            records.append({
                "path": path, "url": url, "local": str(local.relative_to(out_dir)),
                "status": res.status, "action": "failed", "error": res.error,
                "size": 0, "sha256": None, "etag": None, "last_modified": None,
                "fetched_at": _now_iso(),
            })
            print(f"  !! FAILED {res.status}: {path} ({res.error})", file=sys.stderr)
            continue

        body = res.body
        sha = hashlib.sha256(body).hexdigest()
        # Idempotent write: unchanged content (same hash) is not rewritten.
        if not (have_local and prior.get(path, {}).get("sha256") == sha):
            local.parent.mkdir(parents=True, exist_ok=True)
            local.write_bytes(body)
        records.append({
            "path": path, "url": url, "local": str(local.relative_to(out_dir)),
            "status": 200, "action": "downloaded", "size": len(body), "sha256": sha,
            "etag": res.etag, "last_modified": res.last_modified, "fetched_at": _now_iso(),
        })

    # --- Write manifest.json (goal: a 6.2 refresh is a DIFF, not a blind redownload).
    manifest_doc = {
        "orca_version": ORCA_VERSION,
        "base_url": base,
        "generated_at": _now_iso(),
        "counts": {
            "manifest_paths": len(man.entries),
            "leaves": len(leaves),
            "containers": sum(1 for e in man.entries.values() if e.kind == "container"),
            "no_source": sorted(e.path for e in man.entries.values() if e.kind == "no-source"),
        },
        "files": records,
    }
    out_dir.mkdir(parents=True, exist_ok=True)
    manifest_path.write_text(json.dumps(manifest_doc, indent=2) + "\n")

    return report_all(fetcher, out_dir, version_dir, records, leaves, failed_containers)


def report_all(fetcher: Fetcher, out_dir: Path, version_dir: Path,
               records: list[dict], leaves: list[str], failed_containers: list[str]) -> int:
    print("\n" + "=" * 72)
    print(f"ORCA {ORCA_VERSION} manual — FULL fetch (Part B)")
    print("=" * 72)

    downloaded = [r for r in records if r["action"] == "downloaded"]
    reused = [r for r in records if r["action"] == "reused"]
    failed = [r for r in records if r["action"] == "failed"]

    # --- Post-conditions IN OUR TERMS (rule #9) ---
    ok_records = [r for r in records if r["action"] in ("downloaded", "reused")]
    problems: list[str] = []
    html_masq: list[str] = []
    empty: list[str] = []
    for r in ok_records:
        p = out_dir / r["local"]
        if not p.exists() or p.stat().st_size == 0:
            empty.append(r["path"])
            continue
        body = p.read_bytes()
        if _looks_like_html(body):
            html_masq.append(r["path"])
    success = len(ok_records) - len(html_masq) - len(empty)
    expected = len(leaves)

    print(f"\n[FETCH] downloaded={len(downloaded)} reused={len(reused)} failed={len(failed)}")
    print(f"        requests used: {fetcher.attempts}/{fetcher.cap}")
    if failed:
        problems.append(f"{len(failed)} fetch failure(s): {[r['path'] for r in failed]}")
    if html_masq:
        problems.append(f"{len(html_masq)} HTML-masquerade file(s): {html_masq}")
    if empty:
        problems.append(f"{len(empty)} empty/missing file(s): {empty}")
    if success != expected:
        problems.append(f"success {success} != expected leaves {expected}")
    if failed_containers:
        problems.append(f"container fetch failures: {failed_containers}")

    print(f"\n[POST-CONDITIONS] (rule #9 — verified in our terms, not the server's 'done')")
    print(f"    every OK file is text, not an HTML error page : {'PASS' if not html_masq else 'FAIL'}")
    print(f"    every OK file is non-empty                    : {'PASS' if not empty else 'FAIL'}")
    print(f"    success count == manifest 200 leaves          : "
          f"{success} == {expected}  {'PASS' if success == expected else 'FAIL'}")

    # Exact total corpus size (a fetch statistic). ATX distribution, eval-rst,
    # label ASCII, and anchor checks are the Rust gate's job now (4.2) — see
    # `cargo test manual_corpus -- --ignored` and `wiki/orca/manual-sources.md`.
    total_bytes = sum((out_dir / r["local"]).stat().st_size
                      for r in ok_records if (out_dir / r["local"]).exists())
    print(f"\n[SIZE] {total_bytes} bytes = {total_bytes/1_048_576:.2f} MiB over {len(ok_records)} leaves")

    print("\n" + "=" * 72)
    if problems:
        print("RESULT: FAIL — " + " | ".join(problems))
        print("=" * 72)
        return 1
    print(f"RESULT: PASS — {expected} leaves fetched & verified; manifest.json written.")
    print("Reminder: resources/manual/* is gitignored (copyright, ADR-006) — do NOT commit it.")
    print("=" * 72)
    return 0


# --- objects.inv retrieval -------------------------------------------------

def fetch_objects_inv(fetcher: Fetcher, base: str, out_dir: Path) -> int:
    """Fetch `<base>objects.inv` once into resources/manual/<version>/objects.inv.
    The authoritative label->anchor map; PARSED in Rust (src/manual/objects_inv.rs),
    not here. Gitignored like the rest of the corpus (ADR-006)."""
    res = fetcher.get(base + "objects.inv")
    if not res.ok:
        print(f"objects.inv fetch failed: {res.status} {res.error}", file=sys.stderr)
        return 1
    dest = out_dir / ORCA_VERSION / "objects.inv"
    dest.parent.mkdir(parents=True, exist_ok=True)
    dest.write_bytes(res.body)
    print(f"objects.inv: {len(res.body)} bytes -> {dest}")
    print("Reminder: resources/manual/* is gitignored (copyright, ADR-006) — do NOT commit it.")
    return 0


# --- Source vs published-HTML sample (unit 4.15 gate) ----------------------
#
# The RENDER whitelist (category 3 = "hide, invisible in the real manual") was built
# from OUR list of construct TYPES (directives/math/code/xrefs, 4.12) — so `(label)=`
# anchor markers, which are none of those, were shown. That is Pattern 1 (a check that
# measures US, not the subject) once more. The cure is to ask the SUBJECT: the published
# HTML. This mode fetches a DIVERSE sample of pages' rendered HTML, extracts the main
# content (Furo `<article id="furo-main-content">`), and reports which SOURCE construct
# classes are ABSENT from the rendered text — the domain's own list of "invisible", and
# the reverse (visible in HTML but we hide). It only PROPOSES; the whitelist stays
# by-name and author-approved. Author-run, out-of-band (ADR-013 (2) intact).

from html.parser import HTMLParser  # noqa: E402  (stdlib; kept local to this mode)

# Diverse by chapter (the author's spec): essential/model/structure/quickstart/preface/
# spectroscopy/utilities/architecture, the shortest leaf, the largest leaf, and a container.
HTML_SAMPLE_PATHS = [
    "contents/essentialelements/solvationmodels",
    "contents/essentialelements/scf",
    "contents/essentialelements/RI",
    "contents/modelchemistries/mdci",
    "contents/modelchemistries/CASSCF",                # the largest leaf
    "contents/structurereactivity/optimizations",
    "contents/quickstartguide/troubleshooting",
    "contents/orcaarchitecture/conversionfactors",     # the shortest leaf
    "contents/preface/howtocite",                       # citations by nature
    "contents/spectroscopyproperties/CASSCFresp",
    "contents/utilitiesvisualization/orca_2json",
    "index",                                            # a container (index_*)
]

_VOID_TAGS = {"area", "base", "br", "col", "embed", "hr", "img", "input",
              "link", "meta", "param", "source", "track", "wbr"}


class _ArticleText(HTMLParser):
    """Collect the visible text of `<article id="furo-main-content">` — Furo's main
    content, without the nav/sidebar/footer. Skips <script>/<style>."""

    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.on = False
        self.depth = 0
        self.skip = 0
        self.parts: list[str] = []

    def handle_starttag(self, tag, attrs):
        if not self.on:
            if tag == "article" and dict(attrs).get("id") == "furo-main-content":
                self.on = True
                self.depth = 1
            return
        if tag in _VOID_TAGS:
            return
        self.depth += 1
        if tag in ("script", "style"):
            self.skip += 1

    def handle_endtag(self, tag):
        if not self.on or tag in _VOID_TAGS:
            return
        if tag in ("script", "style") and self.skip > 0:
            self.skip -= 1
        self.depth -= 1
        if self.depth == 0:
            self.on = False

    def handle_data(self, data):
        if self.on and self.skip == 0:
            self.parts.append(data)

    def text(self) -> str:
        return " ".join("".join(self.parts).split())


# Source-side construct detectors. Each yields the PAYLOAD whose presence we test in the
# rendered text (absent → invisible/transformed; present → visible verbatim).
_LABEL_LINE = re.compile(r"^\s*\(([^()]+)\)=\s*$", re.M)          # MyST anchor label line
_INDEX_DIR = re.compile(r"^\s*(?:`{3,}|:{3,})\{index\}\s*(.+)$", re.M)
_TABCOLS = re.compile(r"^\s*(?:`{3,}|:{3,})\{tabularcolumns\}\s*(.+)$", re.M)
_RAW_LATEX = re.compile(r"^\s*(?:`{3,}|:{3,})\{raw\}\s*latex\b", re.M)
_CITE = re.compile(r"\{cite[a-z:]*\}`([^`]+)`")                   # {cite}/{cite:t}/… keys
_NUMREF = re.compile(r"\{numref\}`([^`]+)`")
_INLINE_CODE = re.compile(r"(?<!\})`([^`]+)`")                    # ` … ` NOT a role arg


def _fence_iter(src: str):
    """Yield (line, in_orca_fence) — so a `(x)=` INSIDE a ```orca block is not a label."""
    in_fence = False
    fence_char = ""
    fence_len = 0
    for line in src.splitlines():
        s = line.strip()
        if not in_fence and (s.startswith("```") or s.startswith(":::")):
            fence_char, fence_len = s[0], len(s) - len(s.lstrip(s[0]))
            in_fence = True
            yield line, False  # the opener line itself is not prose
            continue
        if in_fence and s and set(s) == {fence_char} and len(s) >= fence_len:
            in_fence = False
            yield line, False
            continue
        yield line, in_fence


def html_sample(fetcher: Fetcher, base: str, out_dir: Path, n: int) -> int:
    paths = HTML_SAMPLE_PATHS[: n] if n and n < len(HTML_SAMPLE_PATHS) else HTML_SAMPLE_PATHS
    print("\n" + "=" * 72)
    print(f"ORCA {ORCA_VERSION} — SOURCE vs PUBLISHED HTML ({len(paths)} pages)")
    print("Asking the subject (Furo HTML), not our list of construct types.")
    print("=" * 72)

    # Aggregate over the sample: class -> [occurrences, payload-visible-in-rendered].
    # `reliable`: the payload is SYNTHETIC (a label/key/spec that never occurs in prose),
    # so "absent from rendered" truly means invisible. For NATURAL-word payloads (index
    # terms, code tokens) the substring test over-reports "visible" (the word is in the
    # prose anyway) — marked unreliable, decided by Sphinx builder semantics instead.
    RELIABLE = {"(label)= anchor", "{tabularcolumns}", "{cite} keys", "{numref} keys"}
    agg: dict[str, list[int]] = {
        "(label)= anchor": [0, 0],
        "{index}": [0, 0],
        "{tabularcolumns}": [0, 0],
        "{raw} latex": [0, 0],
        "{cite} keys": [0, 0],
        "{numref} keys": [0, 0],
        "inline `code`": [0, 0],
    }
    sample_ok = 0
    for p in paths:
        html_res = fetcher.get(f"{base}{p}.html")
        src_res = fetcher.get(source_url(base, p))
        if not (html_res.ok and src_res.ok):
            print(f"  !! skip {p}: html={html_res.status} src={src_res.status}")
            continue
        sample_ok += 1
        parser = _ArticleText()
        parser.feed(html_res.body.decode("utf-8", "replace"))
        rendered = parser.text().lower()
        src = src_res.body.decode("utf-8", "replace")

        def probe(name: str, payloads: list[str]) -> None:
            for pl in payloads:
                agg[name][0] += 1
                if pl and pl.lower() in rendered:
                    agg[name][1] += 1

        # anchor labels — only whole prose lines (not inside a ```orca fence)
        prose = "\n".join(line for line, inf in _fence_iter(src) if not inf)
        probe("(label)= anchor", [m.group(1) for m in _LABEL_LINE.finditer(prose)])
        probe("{index}", [m.group(1).split(";")[0].split(":")[-1].strip()
                          for m in _INDEX_DIR.finditer(src)])
        probe("{tabularcolumns}", [m.group(1).strip() for m in _TABCOLS.finditer(src)])
        # {raw} latex: presence, and whether a distinctive latex command leaks (it should not)
        if _RAW_LATEX.search(src):
            agg["{raw} latex"][0] += 1
            if "\\sisetup" in rendered or "\\begin{" in rendered:
                agg["{raw} latex"][1] += 1
        probe("{cite} keys", [k.strip() for m in _CITE.finditer(src)
                              for k in m.group(1).split(",")][:40])
        probe("{numref} keys", [m.group(1) for m in _NUMREF.finditer(src)][:40])
        probe("inline `code`", [m.group(1) for m in _INLINE_CODE.finditer(src)
                               if 2 <= len(m.group(1)) <= 40][:40])

    print(f"\n[SAMPLE] {sample_ok}/{len(paths)} pages fetched (html + source)")
    print(f"\n[CLASS]  payload PRESENT in rendered / total  (RELIABLE = synthetic payload)")
    for name, (tot, vis) in agg.items():
        frac = f"{vis}/{tot}" if tot else "0/0"
        if name not in RELIABLE:
            note = "prose/latex payload — substring test NOT decisive (Sphinx semantics)"
            print(f"    {name:<20} {frac:>10}   -> [{note}]")
            continue
        verdict = ("INVISIBLE (category-3 candidate)" if tot and vis <= max(2, tot // 50) else
                   "visible" if tot and vis == tot else
                   "partial")
        print(f"    {name:<20} {frac:>10}   -> {verdict}")

    # Corpus-wide frequency of each class (local .md.txt — the author has the corpus).
    version_dir = out_dir / ORCA_VERSION
    print(f"\n[CORPUS FREQUENCY] (local {version_dir}/*.md.txt)")
    if not version_dir.is_dir():
        print(f"    (no local corpus at {version_dir} — run --all first for these counts)")
    else:
        counts = {k: 0 for k in ("(label)= anchor", "{index}", "{tabularcolumns}",
                                 "{raw} latex", "{cite}", "{numref}", "{eq}")}
        for f in version_dir.rglob("*.md.txt"):
            t = f.read_text(encoding="utf-8", errors="replace")
            prose = "\n".join(line for line, inf in _fence_iter(t) if not inf)
            counts["(label)= anchor"] += len(_LABEL_LINE.findall(prose))
            counts["{index}"] += len(_INDEX_DIR.findall(t))
            counts["{tabularcolumns}"] += len(_TABCOLS.findall(t))
            counts["{raw} latex"] += len(_RAW_LATEX.findall(t))
            counts["{cite}"] += len(_CITE.findall(t))
            counts["{numref}"] += len(_NUMREF.findall(t))
            counts["{eq}"] += len(re.findall(r"\{eq\}`[^`]+`", t))
        for k, v in counts.items():
            print(f"    {k:<20} {v:>6}")

    print("\n" + "=" * 72)
    print("Candidates for category 3 (INVISIBLE) are PROPOSALS — the whitelist stays")
    print("by-name and author-approved. Requests used: "
          f"{fetcher.attempts}/{fetcher.cap}")
    print("=" * 72)
    return 0


# --- CLI -------------------------------------------------------------------

def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(
        description="Fetch ORCA manual sources (ADR-013). Body sectioning + anchor "
                    "checks live in Rust (src-tauri/src/manual/).")
    ap.add_argument("--base", default=DEFAULT_BASE, help="manual base URL")
    ap.add_argument("--manifest", action="store_true",
                    help="build the toctree manifest and report it")
    ap.add_argument("--sample", type=int, default=0, metavar="N",
                    help="also fetch N representative leaves (in memory) as a spot-check")
    ap.add_argument("--all", action="store_true",
                    help="full fetch of every leaf into --out, write manifest.json, verify")
    ap.add_argument("--force", action="store_true",
                    help="ignore the manifest.json cache and refetch every file")
    ap.add_argument("--objects-inv", action="store_true",
                    help="fetch objects.inv (the label->anchor map) into --out")
    ap.add_argument("--html-sample", type=int, default=0, metavar="N",
                    help="fetch N diverse pages' published HTML + source and report which "
                         "source constructs are absent from the rendered text (unit 4.15 gate)")
    ap.add_argument("--out", default=str(Path("resources/manual")),
                    help="output dir (default resources/manual)")
    args = ap.parse_args(argv)

    if not (args.manifest or args.all or args.objects_inv or args.html_sample):
        ap.print_help()
        return 2

    base = args.base if args.base.endswith("/") else args.base + "/"
    fetcher = Fetcher()
    try:
        if args.html_sample:
            return html_sample(fetcher, base, Path(args.out), args.html_sample)
        if args.objects_inv:
            return fetch_objects_inv(fetcher, base, Path(args.out))
        if args.all:
            return fetch_all(fetcher, base, Path(args.out), args.force)
        man = build_manifest(fetcher, base)
        return report(man, fetcher, base, args.sample)
    except RequestCapExceeded as e:
        print(f"ABORT: {e}", file=sys.stderr)
        return 3


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
