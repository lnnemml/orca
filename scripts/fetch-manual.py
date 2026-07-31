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

Part A (this unit): ``--manifest`` builds the manifest by walking the toctree;
``--sample N`` additionally fetches N representative leaves (in memory) and
reports measured facts about the source format. Part B (``--all``, manifest.json,
post-conditions) is a later unit and is intentionally not implemented yet.

Usage:
    python scripts/fetch-manual.py --manifest --sample 6
    python scripts/fetch-manual.py --manifest            # manifest only, no fetch
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
from collections.abc import Iterator
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


# Any fenced block opener — backtick CODE fences (```orca, ```python) AND colon
# directives (:::{table}), of any length. Wider than _FENCE_OPEN, which requires
# a {name} because parse_toctrees only cares about DIRECTIVE fences. This one is
# for hiding *all* fenced content from the heading/keyword scanners.
_FENCE_ANY = re.compile(r"^\s*(?P<fence>`{3,}|:{3,})(?P<info>.*)$")


def iter_prose_lines(text: str) -> Iterator[tuple[int, str]]:
    """Yield (index, line) for every line that is OUTSIDE a fenced block.

    THE single fence tracker for the heading/keyword scanners — the same
    open/close rule parse_toctrees uses (`_is_fence_close`, longer-outer /
    shorter-inner nesting), widened to every fence kind. Without it, ORCA input
    examples — which comment with '#' — get miscounted as ATX headings, and a
    '####' separator inside a ```orca block prematurely ends a section. One rule,
    one home: analyze_atx and classify_keywords_markup both go through here.
    """
    lines = text.splitlines()
    i, n = 0, len(lines)
    while i < n:
        line = lines[i]
        m = _FENCE_ANY.match(line)
        if m:
            fence = m.group("fence")
            fence_char, fence_len = fence[0], len(fence)
            i += 1  # consume the block body up to and including its close
            while i < n and not _is_fence_close(lines[i], fence_char, fence_len):
                i += 1
            i += 1  # skip the closing fence line
            continue
        yield i, line
        i += 1


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


def html_url(base: str, path: str) -> str:
    return f"{base}{path}.html"


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


# --- Source-format analysis (report items) ---------------------------------

_ATX = re.compile(r"^(#{1,6})\s+\S")
_LABEL = re.compile(r"^\(([^)]+)\)=\s*$")          # MyST target: (sec:a.b.c)=
_ID_ATTR = re.compile(r'id="([^"]+)"')


def analyze_atx(text: str) -> tuple[int, dict[int, int], int]:
    """Count ATX headings OUTSIDE fenced blocks (via iter_prose_lines), so ORCA
    '#'-comment lines inside ```orca examples are not miscounted as headings."""
    counts: dict[int, int] = {}
    total = 0
    deepest = 0
    for _idx, line in iter_prose_lines(text):
        m = _ATX.match(line)
        if m:
            lvl = len(m.group(1))
            counts[lvl] = counts.get(lvl, 0) + 1
            total += 1
            deepest = max(deepest, lvl)
    return total, counts, deepest


def predict_anchor(label: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", label.lower()).strip("-")


def find_labels(text: str) -> list[str]:
    return [m.group(1) for line in text.splitlines() if (m := _LABEL.match(line))]


def _keyword_forms(body_lines: list[str]) -> set[str]:
    """Which machine-readable form(s) a keyword section's body uses. Body is the
    ORIGINAL lines (fences intact) so a ```orca code block is visible."""
    blob = "\n".join(body_lines)
    forms: set[str] = set()
    # An annotated code block: a ```orca (or ```{...} orca) fence.
    if re.search(r"^\s*`{3,}[^\n]*\borca\b", blob, re.MULTILINE):
        forms.add("orca-codeblock")
    if "{list-table}" in blob:
        forms.add("list-table")
    if re.search(r"^\s*\|.*\|", blob, re.MULTILINE):
        forms.add("pipe-table")
    if re.search(r"^[^\s:|`].*\n:\s+\S", blob, re.MULTILINE):
        forms.add("def-list")
    if not forms:
        forms.add("prose" if blob.strip() else "(empty)")
    return forms


def classify_keywords_markup(text: str) -> list[tuple[str, set[str]]]:
    """For each 'Keywords' heading, which markup form(s) its body uses. Heading
    detection goes through iter_prose_lines (so a '####' inside a code block is
    not mistaken for a heading and does not truncate the section)."""
    lines = text.splitlines()
    heading_idxs = [idx for idx, line in iter_prose_lines(text) if _ATX.match(line)]
    heading_set = set(heading_idxs)
    out: list[tuple[str, set[str]]] = []
    for idx in heading_idxs:
        if "keyword" not in lines[idx].lower():
            continue
        # Body runs to the next PROSE heading (code-block headings excluded).
        later = [h for h in heading_idxs if h > idx]
        end = later[0] if later else len(lines)
        out.append((lines[idx].strip(), _keyword_forms(lines[idx + 1:end])))
    return out


def count_eval_rst(text: str) -> int:
    _, n = parse_toctrees(text)
    return n


# --- Reporting -------------------------------------------------------------

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

    # (3) ATX section counts across the sample.
    agg: dict[int, int] = {}
    total_sections = 0
    deepest_all = 0
    per_file_deepest: list[tuple[str, int, int]] = []
    for p in ok_files:
        text = fetched[p].body.decode("utf-8", errors="replace")
        t, counts, deepest = analyze_atx(text)
        total_sections += t
        deepest_all = max(deepest_all, deepest)
        per_file_deepest.append((p, t, deepest))
        for lvl, c in counts.items():
            agg[lvl] = agg.get(lvl, 0) + c
    print(f"\n[3] ATX SECTIONS (sample)")
    print(f"    total headings        : {total_sections}")
    print(f"    by level              : "
          + ", ".join(f"{'#'*lvl}={agg.get(lvl,0)}" for lvl in range(1, 7) if agg.get(lvl)))
    print(f"    deepest level         : {'#'*deepest_all} ({deepest_all})")
    for p, t, d in per_file_deepest:
        print(f"        {t:>3} headings, deepest {'#'*d:<6}  {p}")

    # (4) how are 'Keywords' sections marked?
    print(f"\n[4] 'KEYWORDS' SECTION MARKUP  (decides whether keywords.json can be seeded)")
    any_kw = False
    for p in ok_files:
        text = fetched[p].body.decode("utf-8", errors="replace")
        for heading, forms in classify_keywords_markup(text):
            any_kw = True
            print(f"    {p}")
            print(f"        {heading!r}  ->  {', '.join(sorted(forms))}")
    if not any_kw:
        print("    (no 'Keywords' heading found in the sample)")

    # (5) objects.inv presence + size (NOT parsed).
    print(f"\n[5] objects.inv (label->anchor map; presence + size only, not parsed)")
    inv = fetcher.get(base + "objects.inv")
    if inv.ok:
        print(f"    present: {len(inv.body)} bytes at {base}objects.inv")
        print(f"    -> an authoritative label->anchor map exists; slugify guessing can be retired")
    else:
        print(f"    NOT available ({inv.status} {inv.error}) — slugify rule stays a guess")

    # (6) eval-rst blocks in BODY (not root) — the MyST-parser review trigger.
    print(f"\n[6] eval-rst IN DOCUMENT BODIES  (ADR-013 (3) review trigger)")
    body_eval = []
    for p in ok_files:
        text = fetched[p].body.decode("utf-8", errors="replace")
        n = count_eval_rst(text)
        if n:
            body_eval.append((p, n))
    root_eval = man.entries["index"].eval_rst
    print(f"    root index eval-rst   : {root_eval} block(s) (expected — holds genindex/biblio)")
    if body_eval:
        print(f"    !! body eval-rst blocks: {body_eval}  <-- REPORT SEPARATELY, do not self-decide")
    else:
        print(f"    body eval-rst blocks  : 0 in sample (ATX-only sectioning holds so far)")

    # (7) anchor rule: (sec:a.b.c) -> #sec-a-b-c, checked against the HTML.
    print(f"\n[7] ANCHOR RULE  (sec:a.b.c  <->  #sec-a-b-c), verified against HTML")
    matched = 0
    checked = 0
    for p in ok_files:
        text = fetched[p].body.decode("utf-8", errors="replace")
        labels = find_labels(text)
        if not labels:
            continue
        html = fetcher.get(html_url(base, p))
        if not html.ok:
            print(f"    !! could not fetch HTML for {p} ({html.error})")
            continue
        ids = set(_ID_ATTR.findall(html.body.decode("utf-8", errors="replace")))
        for lab in labels:
            checked += 1
            pred = predict_anchor(lab)
            hit = pred in ids
            matched += 1 if hit else 0
            mark = "ok" if hit else "MISS"
            print(f"    [{mark}] {p}: ({lab})  ->  #{pred}")
    print(f"    matched {matched}/{checked} labels against real HTML ids")

    print("\n" + "=" * 72)
    print(f"requests used: {fetcher.attempts}/{fetcher.cap}")
    print("STOP — Part A gate. Part B (--all, manifest.json, post-conditions) "
          "awaits author approval.")
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

    # --- Read the on-disk corpus once for the four measurements. ---
    texts: dict[str, str] = {}
    for r in ok_records:
        p = out_dir / r["local"]
        if p.exists():
            texts[r["path"]] = p.read_text(encoding="utf-8", errors="replace")

    # (1a) TRUE ATX distribution + max level across ALL leaves.
    dist: dict[int, int] = {}
    deepest = 0
    deepest_files: list[str] = []
    for path, text in texts.items():
        _, counts, dpath = analyze_atx(text)
        for lvl, c in counts.items():
            dist[lvl] = dist.get(lvl, 0) + c
        if dpath > deepest:
            deepest = dpath
            deepest_files = [path]
        elif dpath == deepest:
            deepest_files.append(path)
    print(f"\n[1a] ATX DISTRIBUTION (all {len(texts)} leaves)")
    print("     " + ", ".join(f"{'#'*lvl}={dist.get(lvl,0)}" for lvl in range(1, 7) if dist.get(lvl)))
    print(f"     deepest ATX level: {'#'*deepest} ({deepest})  e.g. {deepest_files[:3]}")
    # TOC proves #### (level 4) exists (scf/basisset/DFT/CASSCF/mreom/mm/troubleshooting).
    if deepest < 4:
        print(f"     !! DISCREPANCY: TOC implies level-4 (####) sections exist "
              f"(e.g. 2.6.7.1.1 scf.md, 2.7.2.13.1 basisset.md) but ATX max is only "
              f"{'#'*deepest} — deep subsections may NOT be ATX headings. REPORTED, not silent.")

    # (1b) eval-rst blocks in BODY across ALL leaves (the ADR-013 (3) trigger).
    body_eval: list[tuple[str, int]] = []
    total_eval = 0
    for path, text in texts.items():
        n = count_eval_rst(text)
        if n:
            body_eval.append((path, n))
            total_eval += n
    print(f"\n[1b] eval-rst IN BODIES (all leaves) — ADR-013 (3) review condition")
    print(f"     total body eval-rst blocks: {total_eval}")
    if body_eval:
        print(f"     files: {sorted(body_eval)}")
        print(f"     -> non-zero: assess whether these carry SECTION structure before "
              f"trusting ATX-only sectioning (do not self-decide).")
    else:
        print(f"     -> ZERO across the whole corpus: ATX-only sectioning holds; "
              f"ADR-013 (3) stays closed.")

    # (1c) exact total corpus size.
    total_bytes = sum((out_dir / r["local"]).stat().st_size
                      for r in ok_records if (out_dir / r["local"]).exists())
    print(f"\n[1c] EXACT CORPUS SIZE")
    print(f"     {total_bytes} bytes = {total_bytes/1_048_576:.2f} MiB over {len(texts)} leaves")

    # (1d) are all labels ASCII? (predict_anchor collapses non-ASCII to '-').
    non_ascii: list[tuple[str, str]] = []
    total_labels = 0
    for path, text in texts.items():
        for lab in find_labels(text):
            total_labels += 1
            if not lab.isascii():
                non_ascii.append((path, lab))
    print(f"\n[1d] LABEL ASCII CHECK ({total_labels} labels)")
    if non_ascii:
        print(f"     !! {len(non_ascii)} NON-ASCII label(s) — predict_anchor would mangle them: "
              f"{non_ascii[:10]}")
    else:
        print(f"     all labels ASCII — predict_anchor's [^a-z0-9]+->'-' is lossless here")

    print("\n" + "=" * 72)
    if problems:
        print("RESULT: FAIL — " + " | ".join(problems))
        print("=" * 72)
        return 1
    print(f"RESULT: PASS — {expected} leaves fetched & verified; manifest.json written.")
    print("Reminder: resources/manual/* is gitignored (copyright, ADR-006) — do NOT commit it.")
    print("=" * 72)
    return 0


# --- Offline recount (reads disk, no network) + self-test ------------------

def _naive_atx(text: str) -> dict[int, int]:
    """The OLD, buggy count: every line, fences included. Kept only to measure
    the delta against the fence-aware count."""
    d: dict[int, int] = {}
    for line in text.splitlines():
        m = _ATX.match(line)
        if m:
            d[len(m.group(1))] = d.get(len(m.group(1)), 0) + 1
    return d


def analyze_disk(out_dir: Path) -> int:
    """Recount the corpus ALREADY on disk — no network. Reports the fence-aware
    ATX distribution vs the old naive one, files with >1 H1, and a keyword-markup
    reclassification over ALL leaves."""
    version_dir = out_dir / ORCA_VERSION
    files = sorted(version_dir.rglob("*.md.txt"))
    if not files:
        print(f"no .md.txt under {version_dir} — run --all first (no refetch here)", file=sys.stderr)
        return 2

    naive: dict[int, int] = {}
    fixed: dict[int, int] = {}
    fixed_total = 0
    deepest = 0
    deepest_files: list[str] = []
    multi_h1: list[tuple[str, int]] = []
    total_bytes = 0
    kw_forms: dict[str, int] = {}
    kw_rows: list[tuple[str, str, set[str]]] = []

    for p in files:
        rel = str(p.relative_to(version_dir))[:-len(".md.txt")]
        text = p.read_text(encoding="utf-8", errors="replace")
        total_bytes += p.stat().st_size
        for lvl, c in _naive_atx(text).items():
            naive[lvl] = naive.get(lvl, 0) + c
        t, counts, dpath = analyze_atx(text)
        fixed_total += t
        for lvl, c in counts.items():
            fixed[lvl] = fixed.get(lvl, 0) + c
        if dpath > deepest:
            deepest, deepest_files = dpath, [rel]
        elif dpath == deepest:
            deepest_files.append(rel)
        if counts.get(1, 0) > 1:
            multi_h1.append((rel, counts[1]))
        for heading, forms in classify_keywords_markup(text):
            kw_rows.append((rel, heading, forms))
            for f in forms:
                kw_forms[f] = kw_forms.get(f, 0) + 1

    print("=" * 72)
    print(f"ORCA {ORCA_VERSION} manual — OFFLINE RECOUNT ({len(files)} leaves on disk, no network)")
    print("=" * 72)

    naive_total = sum(naive.values())
    print(f"\n[ATX] fence-aware distribution (all {len(files)} leaves)")
    print("      " + ", ".join(f"{'#'*lvl}={fixed.get(lvl,0)}" for lvl in range(1, 7) if fixed.get(lvl)))
    print(f"      total headings: {fixed_total}   deepest: {'#'*deepest} ({deepest})  e.g. {deepest_files[:3]}")
    print(f"\n[ATX] OLD naive count (fences included), for the delta")
    print("      " + ", ".join(f"{'#'*lvl}={naive.get(lvl,0)}" for lvl in range(1, 7) if naive.get(lvl)))
    print(f"      total headings: {naive_total}")
    fake1 = naive.get(1, 0) - fixed.get(1, 0)
    print(f"\n[ATX] FALSE level-1 headings removed: {naive.get(1,0)} -> {fixed.get(1,0)}  "
          f"(= {fake1} were ORCA '#' comments inside code blocks)")
    print(f"      total false headings removed (all levels): {naive_total - fixed_total}")

    print(f"\n[H1] files with more than one level-1 heading (expect ~0; a Sphinx page has one)")
    if multi_h1:
        print(f"     {len(multi_h1)} file(s): {multi_h1}")
    else:
        print(f"     0 — every leaf has at most one H1 (consistent with one page = one section root)")

    print(f"\n[SIZE] {total_bytes} bytes = {total_bytes/1_048_576:.2f} MiB (unchanged — a cross-check)")

    print(f"\n[KEYWORDS] reclassified over ALL {len(files)} leaves ({len(kw_rows)} 'Keyword' headings)")
    for form, c in sorted(kw_forms.items(), key=lambda kv: -kv[1]):
        print(f"     {c:>3}  {form}")
    seed_forms = {f for f in kw_forms if f in ("orca-codeblock", "pipe-table", "list-table")}
    print(f"     machine-seedable forms present: {sorted(seed_forms)}")
    if seed_forms <= {"orca-codeblock", "pipe-table", "list-table"} and len(seed_forms) <= 2 \
            and "list-table" not in seed_forms:
        print(f"     -> the two-extractor conclusion HOLDS (table + ```orca code block).")
    else:
        print(f"     -> MORE than two seed forms present: {sorted(seed_forms)} — REPORT, do not self-decide.")
    return 0


_SELFTEST_FIXTURE = '''\
# Real Heading One

Some prose.

```orca
# comment line inside code
#### separator
Basis "def2-TZVP" # inline comment
%scf
  MaxIter 200   # another comment
end
```

## Real Heading Two

More prose with an inline `# not a heading` code span.
'''


def selftest() -> int:
    """Fence-tracking regression test — no network. The ```orca block carries a
    '# comment', a '#### separator' and an inline '#'; none may count."""
    total, counts, deepest = analyze_atx(_SELFTEST_FIXTURE)
    ok = True

    def check(cond: bool, msg: str) -> None:
        nonlocal ok
        print(f"  [{'ok' if cond else 'FAIL'}] {msg}")
        ok = ok and cond

    check(total == 2, f"exactly 2 headings counted (got {total})")
    check(counts == {1: 1, 2: 1}, f"one '#' and one '##' (got {counts})")
    check(deepest == 2, f"deepest is '##' — the '####' inside ```orca ignored (got {deepest})")
    prose = [line for _i, line in iter_prose_lines(_SELFTEST_FIXTURE)]
    check(not any("separator" in l for l in prose), "no code-block line leaked into prose")
    check(not any(l.startswith("# comment") for l in prose), "'# comment' not treated as prose/heading")
    print("SELFTEST:", "PASS" if ok else "FAIL")
    return 0 if ok else 1


# --- CLI -------------------------------------------------------------------

def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Fetch/measure ORCA manual sources (ADR-013).")
    ap.add_argument("--base", default=DEFAULT_BASE, help="manual base URL")
    ap.add_argument("--manifest", action="store_true",
                    help="build the toctree manifest and report")
    ap.add_argument("--sample", type=int, default=0, metavar="N",
                    help="also fetch N representative leaves (in memory) and measure format")
    ap.add_argument("--all", action="store_true",
                    help="full fetch of every leaf into --out, write manifest.json, verify")
    ap.add_argument("--force", action="store_true",
                    help="ignore the manifest.json cache and refetch every file")
    ap.add_argument("--analyze-only", action="store_true",
                    help="recount the corpus ALREADY on disk — no network")
    ap.add_argument("--selftest", action="store_true",
                    help="run the fence-tracking regression test — no network")
    ap.add_argument("--out", default=str(Path("resources/manual")),
                    help="output dir (default resources/manual)")
    args = ap.parse_args(argv)

    if args.selftest:
        return selftest()
    if args.analyze_only:
        return analyze_disk(Path(args.out))

    if not (args.manifest or args.all):
        ap.print_help()
        return 2

    base = args.base if args.base.endswith("/") else args.base + "/"
    fetcher = Fetcher()
    try:
        if args.all:
            return fetch_all(fetcher, base, Path(args.out), args.force)
        man = build_manifest(fetcher, base)
        return report(man, fetcher, base, args.sample)
    except RequestCapExceeded as e:
        print(f"ABORT: {e}", file=sys.stderr)
        return 3


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
