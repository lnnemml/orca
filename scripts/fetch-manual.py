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
import posixpath
import re
import sys
import time
import urllib.error
import urllib.request
from dataclasses import dataclass, field
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
    status: int          # HTTP status, or 0 for a network-level failure
    body: bytes | None
    error: str | None = None

    @property
    def ok(self) -> bool:
        return self.status == 200 and self.body is not None


class Fetcher:
    """Polite, capped HTTP GET with retry/backoff. Counts every attempt."""

    def __init__(self, cap: int = HARD_REQUEST_CAP):
        self.cap = cap
        self.attempts = 0
        self._first = True

    def get(self, url: str) -> FetchResult:
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
            req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
            try:
                with urllib.request.urlopen(req, timeout=REQUEST_TIMEOUT_S) as resp:
                    return FetchResult(url=url, status=resp.status, body=resp.read())
            except urllib.error.HTTPError as e:
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
    counts: dict[int, int] = {}
    total = 0
    deepest = 0
    for line in text.splitlines():
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


def classify_keywords_markup(text: str) -> list[tuple[str, str]]:
    """For each heading containing 'keyword', how is the section's body marked?"""
    lines = text.splitlines()
    out: list[tuple[str, str]] = []
    for idx, line in enumerate(lines):
        m = _ATX.match(line)
        if not m or "keyword" not in line.lower():
            continue
        # Look at the section body until the next heading.
        body: list[str] = []
        for follow in lines[idx + 1:]:
            if _ATX.match(follow):
                break
            body.append(follow)
        blob = "\n".join(body)
        if "{list-table}" in blob:
            kind = "{list-table} directive"
        elif re.search(r"^\s*\|.*\|", blob, re.MULTILINE):
            kind = "pipe table"
        elif re.search(r"^[^\s:].*\n:\s+\S", blob, re.MULTILINE):
            kind = "definition list"
        elif blob.strip():
            kind = "plain text / prose"
        else:
            kind = "(empty body)"
        out.append((line.strip(), kind))
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
        for heading, kind in classify_keywords_markup(text):
            any_kw = True
            print(f"    {p}")
            print(f"        {heading!r}  ->  {kind}")
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


# --- CLI -------------------------------------------------------------------

def main(argv: list[str]) -> int:
    ap = argparse.ArgumentParser(description="Fetch/measure ORCA manual sources (ADR-013).")
    ap.add_argument("--base", default=DEFAULT_BASE, help="manual base URL")
    ap.add_argument("--manifest", action="store_true",
                    help="build the toctree manifest and report")
    ap.add_argument("--sample", type=int, default=0, metavar="N",
                    help="also fetch N representative leaves (in memory) and measure format")
    ap.add_argument("--all", action="store_true",
                    help="[Part B — not yet implemented] full fetch into resources/manual/")
    ap.add_argument("--out", default=str(Path("resources/manual")),
                    help="output dir (Part B)")
    args = ap.parse_args(argv)

    if args.all:
        print("--all is Part B (full fetch + manifest.json + post-conditions); "
              "not implemented in this unit. Run --manifest --sample first.", file=sys.stderr)
        return 2

    if not args.manifest:
        ap.print_help()
        return 2

    base = args.base if args.base.endswith("/") else args.base + "/"
    fetcher = Fetcher()
    try:
        man = build_manifest(fetcher, base)
    except RequestCapExceeded as e:
        print(f"ABORT: {e}", file=sys.stderr)
        return 3
    return report(man, fetcher, base, args.sample)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
