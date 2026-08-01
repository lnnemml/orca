//! Corpus gate for the manual sectioner (precedent: `orca_plot.rs`). Reads the
//! locally fetched `resources/manual/<version>/**.md.txt` + `objects.inv`, runs the
//! three post-conditions over all 126 leaves, and PRINTS the report whose numbers
//! unit 4.3 will turn into the FTS schema. Ignored by default:
//!
//!     cargo test manual_corpus -- --ignored --nocapture
//!
//! The plain `cargo test` run relies on the small in-code fixtures in `sections.rs`
//! and `objects_inv.rs` (no network, no corpus).

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection};

use super::index::collect_leaves;
use super::objects_inv;
use super::projection::search_projection;
use super::sections::{self, Section};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// The ORCA version whose corpus is on disk (from manifest.json; falls back to 6.1).
fn corpus_version(manual_dir: &Path) -> String {
    std::fs::read_to_string(manual_dir.join("manifest.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("orca_version").and_then(|x| x.as_str()).map(String::from))
        .unwrap_or_else(|| "6.1".to_string())
}

fn percentile(sorted: &[usize], p: f64) -> usize {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[test]
#[ignore]
fn manual_corpus() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus at {} — run scripts/fetch-manual.py --all", version_dir.display());
        return;
    }

    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    assert!(!leaves.is_empty(), "no .md.txt leaves found under {}", version_dir.display());

    println!("\n{:=<72}", "");
    println!("ORCA {version} manual — SECTIONER GATE ({} leaves)", leaves.len());
    println!("{:=<72}", "");

    // --- Sectionize every leaf; post-condition (a) line conservation per file. ---
    let mut all: Vec<Section> = Vec::new();
    let mut sections_per_file: Vec<usize> = Vec::new();
    let mut conservation_fail: Vec<String> = Vec::new();
    // Fenced vs prose bytes (report item 6).
    let mut prose_bytes: u64 = 0;
    let mut fenced_bytes: u64 = 0;

    for (file, text) in &leaves {
        let lines: Vec<&str> = text.lines().collect();
        let mask = sections::prose_mask(&lines);
        for (i, l) in lines.iter().enumerate() {
            if mask[i] {
                prose_bytes += l.len() as u64;
            } else {
                fenced_bytes += l.len() as u64;
            }
        }
        match sections::sectionize(file, text) {
            Ok(secs) => {
                sections_per_file.push(secs.len());
                all.extend(secs);
            }
            Err(e) => {
                conservation_fail.push(format!("{e}"));
                sections_per_file.push(0);
            }
        }
    }

    // --- (1) counts, level distribution, sections/file, breadcrumb depth ---
    let mut by_level: HashMap<u8, usize> = HashMap::new();
    for s in &all {
        *by_level.entry(s.level).or_insert(0) += 1;
    }
    let mut spf = sections_per_file.clone();
    spf.sort_unstable();
    let deepest_bc = all.iter().map(|s| s.breadcrumb.len()).max().unwrap_or(0);
    let bc_examples: Vec<String> = all
        .iter()
        .filter(|s| s.breadcrumb.len() == deepest_bc && deepest_bc > 0)
        .take(2)
        .map(|s| format!("{} > {}", s.breadcrumb.join(" > "), s.title))
        .collect();

    println!("\n[1] SECTIONS");
    println!("    total: {}", all.len());
    print!("    by level: ");
    for lvl in 1..=6u8 {
        if let Some(c) = by_level.get(&lvl) {
            print!("{}={} ", "#".repeat(lvl as usize), c);
        }
    }
    println!();
    println!(
        "    sections/file: min {} / median {} / max {}",
        spf.first().copied().unwrap_or(0),
        percentile(&spf, 0.5),
        spf.last().copied().unwrap_or(0),
    );
    println!("    deepest breadcrumb: {deepest_bc}");
    for ex in &bc_examples {
        println!("        e.g. {ex}");
    }

    // --- (2) body sizes + empty bodies ---
    let mut body_sizes: Vec<usize> = all.iter().map(|s| s.body.len()).collect();
    body_sizes.sort_unstable();
    let empty_bodies = all.iter().filter(|s| s.body.trim().is_empty()).count();
    println!("\n[2] BODY SIZE (bytes)");
    println!(
        "    median {} / p95 {} / max {}",
        percentile(&body_sizes, 0.5),
        percentile(&body_sizes, 0.95),
        body_sizes.last().copied().unwrap_or(0),
    );
    println!(
        "    empty-body sections (navigational): {empty_bodies} of {} ({:.1}%)",
        all.len(),
        100.0 * empty_bodies as f64 / all.len().max(1) as f64,
    );

    // --- (3) labels present / absent / slug collisions among the label-less ---
    let with_label = all.iter().filter(|s| !s.labels.is_empty()).count();
    let without = all.len() - with_label;
    let collisions = sections::title_slug_collisions(&all);
    println!("\n[3] LABELS");
    println!("    with >=1 label: {with_label}");
    println!("    without (anchor = title slug): {without}");
    println!("    label-less sections that COLLIDE on title-slug within a file: {collisions}");

    // --- (4) objects.inv + (b) anchors + (c) label binding ---
    let inv_path = version_dir.join("objects.inv");
    println!("\n[4] objects.inv + anchor/binding post-conditions");
    match std::fs::read(&inv_path).ok().and_then(|b| objects_inv::parse(&b).ok()) {
        None => {
            println!("    !! objects.inv missing/unparsable at {} — items 4 and (c) SKIPPED",
                     inv_path.display());
            println!("       (run scripts/fetch-manual.py --objects-inv)");
        }
        Some(entries) => {
            let rep = objects_inv::verify_against_inventory(&all, &entries);
            println!("    inventory entries: {} (std:label {})", rep.inv_entries, rep.inv_labels);
            println!("    entries not ours (other domains / auto ids): {}", rep.entries_not_ours);
            println!("    our labels found in objects.inv: {} / {}", rep.found_in_inv, rep.our_labels);
            if !rep.not_found.is_empty() {
                let mut prefixes: HashMap<String, usize> = HashMap::new();
                for l in &rep.not_found {
                    let pfx = l.split_once(':').map(|(p, _)| p).unwrap_or("(none)").to_string();
                    *prefixes.entry(pfx).or_insert(0) += 1;
                }
                println!("        not found by label prefix: {prefixes:?}");
            }
            let unchecked = rep.our_labels - rep.found_in_inv;
            println!("    (b) predict_anchor vs objects.inv: {} mismatch(es) out of {} checked; {} unchecked",
                     rep.anchor_mismatches.len(), rep.found_in_inv, unchecked);
            for m in rep.anchor_mismatches.iter().take(10) {
                println!("        {m}");
            }
            println!("    (c) label binding vs objects.inv: {} mismatch(es) out of {} checked; {} unchecked",
                     rep.binding_mismatches.len(), rep.found_in_inv, unchecked);
            for m in rep.binding_mismatches.iter().take(10) {
                println!("        {m}");
            }
            // Anchors are a post-condition (rule #9): any found label whose slug or file
            // disagrees with the authoritative inventory fails the gate, named above.
            assert!(rep.anchor_mismatches.is_empty(), "predict_anchor disagreed with objects.inv");
            assert!(rep.binding_mismatches.is_empty(), "label binding disagreed with objects.inv");
        }
    }

    // --- (5) line conservation summary ---
    println!("\n[5] LINE CONSERVATION (post-condition a)");
    println!(
        "    per-file: {} PASS / {} FAIL",
        leaves.len() - conservation_fail.len(),
        conservation_fail.len(),
    );
    for f in conservation_fail.iter().take(10) {
        println!("    !! {f}");
    }

    // --- (6) fenced vs prose byte fraction ---
    let total = prose_bytes + fenced_bytes;
    println!("\n[6] BYTES: prose vs fenced");
    println!(
        "    prose {} B ({:.1}%) / fenced {} B ({:.1}%) of {} B",
        prose_bytes,
        100.0 * prose_bytes as f64 / total.max(1) as f64,
        fenced_bytes,
        100.0 * fenced_bytes as f64 / total.max(1) as f64,
        total,
    );

    println!("\n{:=<72}", "");
    // The gate is red if the central post-condition ever fails.
    assert!(conservation_fail.is_empty(), "line conservation failed on {} file(s)", conservation_fail.len());
}

// --- Retrieval-quality gate (unit 4.3) ------------------------------------
//
// Builds TWO in-memory FTS5 indexes over the sectioned corpus — (A) raw body,
// (B) cleaned `search_projection` — and measures both with the SAME query set
// whose targets are fixed here BEFORE measuring (two are ROADMAP Phase-4
// acceptance criteria). The column choice is made by number, not taste.
//
//     cargo test retrieval_gate -- --ignored --nocapture

/// query, and the acceptable target sections as (file-id, any-of title keywords).
struct Query {
    q: &'static str,
    targets: &'static [(&'static str, &'static [&'static str])],
}

const QUERIES: &[Query] = &[
    // Two ROADMAP Phase-4 acceptance criteria:
    Query { q: "RIJCOSX", targets: &[("contents/essentialelements/RI", &["rijcosx"])] },
    Query { q: "how do I set up CPCM for water",
            targets: &[("contents/essentialelements/solvationmodels", &["cpcm"])] },
    // The rest, from CLAUDE.md + the template library:
    Query { q: "%geom Constraints", targets: &[("contents/structurereactivity/optimizations", &["constrain"])] },
    Query { q: "%geom Scan relaxed surface scan",
            targets: &[("contents/structurereactivity/optimizations_scans", &["scan"])] },
    Query { q: "%pal nprocs parallel", targets: &[("contents/essentialelements/parallel", &["parallel", "process"])] },
    Query { q: "%maxcore memory", targets: &[("contents/essentialelements/input", &["memory"])] },
    Query { q: "def2/J auxiliary basis", targets: &[("contents/essentialelements/basisset", &["auxiliary"])] },
    Query { q: "CPCM water", targets: &[("contents/essentialelements/solvationmodels", &["cpcm"])] },
    Query { q: "SMD solvation", targets: &[("contents/essentialelements/solvationmodels", &["smd"])] },
    Query { q: "TightOpt optimization thresholds",
            targets: &[("contents/structurereactivity/optimizations", &["threshold", "convergence", "optimization"])] },
    Query { q: "NumFreq numerical frequencies", targets: &[("contents/structurereactivity/frequencies", &["frequenc"])] },
    Query { q: "GOAT", targets: &[("contents/structurereactivity/goat", &["goat"])] },
    Query { q: "NEB-TS", targets: &[("contents/structurereactivity/neb", &["neb"])] },
    Query { q: "IRC intrinsic reaction coordinate",
            targets: &[("contents/structurereactivity/irc", &["reaction coordinate", "intrinsic", "irc"])] },
    Query { q: "imaginary frequency transition state",
            targets: &[("contents/structurereactivity/optimizations_TS", &["transition"]),
                       ("contents/structurereactivity/frequencies", &["frequenc"])] },
    Query { q: "orca_2json", targets: &[("contents/utilitiesvisualization/orca_2json", &["orca_2json", "json"])] },
    Query { q: "xtb GFN", targets: &[("contents/modelchemistries/semiempirical", &["xtb", "gfn", "semiempirical"])] },
];

fn build_fts(sections: &[super::sections::Section], project: bool) -> Connection {
    let conn = Connection::open_in_memory().unwrap();
    conn.execute_batch(
        "CREATE VIRTUAL TABLE fts USING fts5(
             file UNINDEXED, title, breadcrumb, body, tokenize='porter unicode61');",
    )
    .unwrap();
    {
        let mut stmt = conn
            .prepare("INSERT INTO fts(rowid, file, title, breadcrumb, body) VALUES (?1,?2,?3,?4,?5)")
            .unwrap();
        for (i, s) in sections.iter().enumerate() {
            let body = if project { search_projection(&s.body) } else { s.body.clone() };
            stmt.execute(params![(i as i64) + 1, s.file, s.title, s.breadcrumb.join(" > "), body])
                .unwrap();
        }
    }
    conn
}

/// Top-5 rowids (→ section indices) for a query, title-weighted bm25 ASC.
fn top5(conn: &Connection, match_q: &str) -> Vec<usize> {
    if match_q.is_empty() {
        return Vec::new();
    }
    let mut stmt = conn
        .prepare(
            "SELECT rowid FROM fts WHERE fts MATCH ?1
             ORDER BY bm25(fts, 0.0, 10.0, 5.0, 1.0) LIMIT 5",
        )
        .unwrap();
    stmt.query_map(params![match_q], |r| r.get::<_, i64>(0))
        .unwrap()
        .filter_map(Result::ok)
        .map(|x| (x - 1) as usize)
        .collect()
}

fn is_target(s: &super::sections::Section, q: &Query) -> bool {
    let title = s.title.to_lowercase();
    q.targets.iter().any(|(file, keys)| s.file == *file && keys.iter().any(|k| title.contains(k)))
}

fn measure(sections: &[super::sections::Section], project: bool, label: &str) -> (usize, usize) {
    let conn = build_fts(sections, project);
    let mut h1 = 0;
    let mut h5 = 0;
    println!("\n  [{label}]");
    for query in QUERIES {
        let hits = top5(&conn, &super::index::to_fts_match(query.q));
        let at1 = hits.first().is_some_and(|&i| is_target(&sections[i], query));
        let at5 = hits.iter().any(|&i| is_target(&sections[i], query));
        if at1 {
            h1 += 1;
        }
        if at5 {
            h5 += 1;
        }
        if !at5 {
            let got = hits.iter().take(3).map(|&i| format!("{}#{}", sections[i].file, sections[i].title)).collect::<Vec<_>>();
            println!("    MISS  {:<40} -> {:?}", query.q, got);
        } else if !at1 {
            let rank = hits.iter().position(|&i| is_target(&sections[i], query)).unwrap() + 1;
            println!("    @{rank:<3} {:<40} (in top-5, not first)", query.q);
        }
    }
    println!("    hit@1 {}/{}  hit@5 {}/{}", h1, QUERIES.len(), h5, QUERIES.len());
    (h1, h5)
}

#[test]
#[ignore]
fn retrieval_gate() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus at {}", version_dir.display());
        return;
    }
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    let mut sections: Vec<super::sections::Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = super::sections::sectionize(file, text) {
            sections.extend(secs);
        }
    }

    println!("\n{:=<72}", "");
    println!("RETRIEVAL GATE — {} sections, {} queries", sections.len(), QUERIES.len());
    println!("{:=<72}", "");

    let n = QUERIES.len() as f64;
    let (a1, a5) = measure(&sections, false, "A: raw body_md");
    let (b1, b5) = measure(&sections, true, "B: cleaned projection");

    let (a5f, b5f) = (a5 as f64 / n, b5 as f64 / n);
    println!("\n  SUMMARY  A raw: hit@1 {a1}/{} hit@5 {a5}/{}   |   B cleaned: hit@1 {b1}/{} hit@5 {b5}/{}",
             QUERIES.len(), QUERIES.len(), QUERIES.len(), QUERIES.len());

    // Exit criterion: if BOTH variants miss the 80% hit@5 bar, the problem is not
    // the column — stop and report (the gate goes red).
    let best5 = a5f.max(b5f);
    assert!(
        best5 >= 0.80,
        "hit@5 < 80% for BOTH variants (A {a5f:.0}%, B {b5f:.0}%) — column choice won't fix this; \
         the sectioning or index shape needs rethinking. STOP.",
    );

    // Choose by number; within noise (±1 query) prefer the simpler raw body (A).
    let choice = if (b5 as i64 - a5 as i64).abs() <= 1 {
        "A (raw body_md) — B is within noise, so take the simpler column"
    } else if b5 > a5 {
        "B (cleaned projection) — measurably better hit@5"
    } else {
        "A (raw body_md) — measurably better hit@5"
    };
    println!("  CHOICE: {choice}");
    println!("{:=<72}", "");
}

// --- Ingest gate: run the REAL build_index over the corpus (unit 4.3) --------
//
//     cargo test manual_ingest -- --ignored --nocapture

#[test]
#[ignore]
fn manual_ingest() {
    let manual_root = repo_root().join("resources/manual");
    let version = corpus_version(&manual_root);
    if !manual_root.join(&version).is_dir() {
        eprintln!("skipping: no corpus");
        return;
    }
    let mut conn = Connection::open_in_memory().unwrap();
    crate::db::create_manual_tables(&conn).unwrap();

    let report = super::index::build_index(&mut conn, &manual_root, &version)
        .expect("build_index (all content post-conditions run inside it)");

    println!("\n{:=<72}", "");
    println!("MANUAL INGEST — {report:#?}");
    println!("{:=<72}", "");

    assert_eq!(report.section_count, 1586, "section count matches the sectioner gate");
    assert_eq!(report.anchors_verified, 1068, "1068 anchors cross-checked against objects.inv");
    assert_eq!(report.null_anchors, 1586 - 1068, "the rest are UNDETERMINED (NULL), not guessed");

    // End-to-end search on the real index: the two ROADMAP acceptance queries.
    let rij = super::index::search_manual(&conn, "RIJCOSX", 5).unwrap();
    assert!(rij.iter().take(5).any(|h| h.file == "contents/essentialelements/RI"),
            "RIJCOSX finds the RI page in top-5");
    let cpcm = super::index::search_manual(&conn, "how do I set up CPCM for water", 5).unwrap();
    assert!(cpcm.iter().take(5).any(|h| h.file == "contents/essentialelements/solvationmodels"),
            "CPCM-for-water finds solvationmodels in top-5");

    // Idempotent: a second ingest replaces, not duplicates.
    let again = super::index::build_index(&mut conn, &manual_root, &version).unwrap();
    assert_eq!(again.section_count, report.section_count);
    let rows: i64 = conn.query_row("SELECT COUNT(*) FROM manual_sections", [], |r| r.get(0)).unwrap();
    assert_eq!(rows as usize, report.section_count, "re-ingest replaced, did not duplicate");
    println!("idempotent re-ingest OK: {rows} rows");
}

// --- keywords.json → DB bridge gate (unit 4.4 Part B) ------------------------
//
//     cargo test keywords_bridge -- --ignored --nocapture
//
// The post-condition specified in Part B and only checkable now that there is a
// consumer: EVERY section descriptor in keywords.json resolves to EXACTLY one
// manual_sections row (0 or a nth-out-of-range = error, never pick-first), and the
// map's orca_version matches the built index.

#[test]
#[ignore]
fn keywords_bridge() {
    let manual_root = repo_root().join("resources/manual");
    let version = corpus_version(&manual_root);
    if !manual_root.join(&version).is_dir() {
        eprintln!("skipping: no corpus");
        return;
    }
    let mut conn = Connection::open_in_memory().unwrap();
    crate::db::create_manual_tables(&conn).unwrap();
    super::index::build_index(&mut conn, &manual_root, &version).unwrap();

    let kw_path = repo_root().join("src/manual/keywords.json");
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&kw_path).unwrap()).unwrap();

    // Version agreement (a stale map must be caught, not silently resolved).
    assert_eq!(
        doc["orca_version"].as_str().unwrap(),
        version,
        "keywords.json orca_version != built index version"
    );

    let sections = doc["sections"].as_array().unwrap();
    let mut ids: HashSet<i64> = HashSet::new();
    let mut failures: Vec<String> = Vec::new();
    for s in sections {
        let file = s["file"].as_str().unwrap();
        let title = s["title"].as_str().unwrap();
        let nth = s["nth"].as_u64().unwrap() as usize;
        let breadcrumb: Vec<String> = s["breadcrumb"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| x.as_str().unwrap().to_string())
            .collect();
        match super::index::resolve_descriptor(&conn, &version, file, &breadcrumb, title, nth) {
            Ok(id) => {
                ids.insert(id);
            }
            Err(e) => failures.push(format!("{e}")),
        }
    }

    println!("\n{:=<72}", "");
    println!("KEYWORDS BRIDGE — {} descriptors", sections.len());
    println!("    resolved to distinct rows: {}", ids.len());
    println!("    failures: {}", failures.len());
    for f in failures.iter().take(10) {
        println!("      !! {f}");
    }
    println!("{:=<72}", "");
    // exactly one row each, and injective (no two descriptors → the same row).
    assert!(failures.is_empty(), "{} descriptor(s) did not resolve to exactly one row", failures.len());
    assert_eq!(ids.len(), sections.len(), "descriptors are not injective onto rows");
}

// --- Keyword-seed measurement gate (unit 4.4) --------------------------------
//
//     cargo test keyword_seed_measure -- --ignored --nocapture
//
// The seeding unit's hazard is a map that points at the WRONG section yet looks
// complete (a confident wrong hover, ADR-013's hit@1 finding). So this gate does
// NOT measure "how many keywords extracted" — it measures the STABLE-KEY property,
// the COVERAGE of what the app itself emits, and a cheap PRECISION proxy per entry.
// It writes nothing; it only prints the numbers wiki/orca/manual-sources.md records.

use regex::Regex;

/// A `%`-block name or a simple keyword token — rejects prose words, bare numbers,
/// and xyz element noise-with-coords. `%?[A-Za-z]…` with internal `-_./` runs.
fn kw_token_ok(t: &str) -> bool {
    let re = Regex::new(r"^%?[A-Za-z][A-Za-z0-9]*(?:[-_./][A-Za-z0-9]+)*$").unwrap();
    t.len() >= 2 && re.is_match(t)
}

/// Normalize for comparison: drop a leading `%`, lowercase. So the block table's
/// `pal` matches the app's `%pal`, and `RIJCOSX` matches `rijcosx`.
fn norm_kw(k: &str) -> String {
    k.trim_start_matches('%').to_lowercase()
}

/// First token of each content line inside every ` ```orca ` fenced block — the
/// annotated "name value # description" keyword-list form (`%cpcm` complete list).
/// Skips comment/`*`/coordinate-ish lines; keeps only keyword-shaped first tokens.
fn orca_block_keywords(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_orca = false;
    for line in body.lines() {
        let tl = line.trim_start();
        if tl.starts_with("```") {
            let lang = tl.trim_start_matches('`').trim();
            in_orca = if in_orca { false } else { lang.eq_ignore_ascii_case("orca") };
            continue;
        }
        if !in_orca {
            continue;
        }
        let s = line.trim();
        if s.is_empty() || s.starts_with('#') || s.starts_with('*') {
            continue;
        }
        if let Some(tok) = s.split_whitespace().next() {
            let tok = tok.trim_end_matches(',');
            if kw_token_ok(tok) {
                out.push(tok.to_string());
            }
        }
    }
    out
}

/// Backtick-wrapped keyword tokens from the first cell of a GFM/`:::{table}` pipe
/// row, and from `{list-table}` / `{flat-table}` row leaders (`* - `). Separator
/// rows are skipped. Covers the three non-code structured forms in one pass.
fn table_keywords(body: &str) -> Vec<String> {
    let btick = Regex::new(r"`([^`]+)`").unwrap();
    let mut out = Vec::new();
    for line in body.lines() {
        let s = line.trim();
        if let Some(inner) = s.strip_prefix('|') {
            let first = inner.split('|').next().unwrap_or("").trim();
            if first.chars().all(|c| c == '-' || c == ':' || c == ' ') {
                continue; // separator row
            }
            for cap in btick.captures_iter(first) {
                if kw_token_ok(&cap[1]) {
                    out.push(cap[1].to_string());
                }
            }
        } else if s.starts_with("* - ") {
            // flat-table / list-table row: take backtick tokens in the leader cell.
            for cap in btick.captures_iter(s) {
                if kw_token_ok(&cap[1]) {
                    out.push(cap[1].to_string());
                }
            }
        }
    }
    out
}

/// One expectation-inventory entry (the ONE home is `src/manual/keyword-inventory.json`,
/// shared with the TS coverage gate — no second list). `expect` = the type a hover needs
/// in the word's emit context; `gap` (a|b|c|d) is present only on words we KNOWINGLY do
/// not cover yet, with the closer that will. A word without `gap` is HARD.
struct InvEntry {
    keyword: String,
    expect: String,          // "simple" | "block" | "block-option"
    block: Option<String>,   // owning block for a block-option expectation
    gap: Option<String>,     // a|b|c|d — a declared, classified hole (not a failure)
}

/// Read the shared inventory. Author-run gate (like the corpus), so reading the repo
/// file is fine — and it means the Rust and TS gates cannot drift.
fn load_inventory() -> Vec<InvEntry> {
    let path = repo_root().join("src/manual/keyword-inventory.json");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read keyword-inventory.json"))
            .expect("parse keyword-inventory.json");
    v["keywords"]
        .as_array()
        .expect("keywords array")
        .iter()
        .map(|e| InvEntry {
            keyword: e["keyword"].as_str().unwrap().to_string(),
            expect: e["expect"].as_str().unwrap().to_string(),
            block: e.get("block").and_then(|x| x.as_str()).map(String::from),
            gap: e.get("gap").and_then(|x| x.as_str()).map(String::from),
        })
        .collect()
}

/// The input-builder's simple `!` tokens — a SEED type-inference hint (a token found in
/// the corpus that is one of these is recorded `simple`, else `block-option`). This is
/// distinct from the coverage inventory: it must NOT include aspirational domain/workflow
/// words (XTB, TightOpt, …) or the seed would reclassify them and change the file. So it
/// stays the input-builder set, not the inventory.
const SEED_SIMPLE_HINT: &[&str] = &[
    "Opt", "Freq", "OptTS", "NumFreq",
    "r2SCAN-3c", "B97-3c", "PBEh-3c", "wB97X-3c", "HF-3c",
    "BP86", "PBE", "BLYP", "TPSS", "r2SCAN", "M06-L", "B3LYP", "PBE0", "TPSSh", "M06-2X",
    "wB97X-D4", "CAM-B3LYP", "wB97M-V", "HF",
    "def2-SVP", "def2-TZVP", "def2-TZVPP", "def2-QZVPP", "def2-TZVPD", "def2-SVPD",
    "def2/J", "def2/JK", "D4", "D3BJ", "D3Zero", "NL", "RIJCOSX", "RI-JK", "RI",
    "CPCM", "SMD", "TightSCF", "VeryTightSCF",
];
fn app_simple_set() -> HashSet<String> {
    SEED_SIMPLE_HINT.iter().map(|t| norm_kw(t)).collect()
}

#[test]
#[ignore]
fn keyword_seed_measure() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus at {}", version_dir.display());
        return;
    }
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = sections::sectionize(file, text) {
            all.extend(secs);
        }
    }

    println!("\n{:=<72}", "");
    println!("KEYWORD-SEED MEASURE — {} sections", all.len());
    println!("{:=<72}", "");

    // ---- A1: stable key = (file, breadcrumb, title) unique across all sections? ----
    let mut key_counts: HashMap<(String, String, String), usize> = HashMap::new();
    for s in &all {
        let k = (s.file.clone(), s.breadcrumb.join(" > "), s.title.clone());
        *key_counts.entry(k).or_insert(0) += 1;
    }
    let collisions: Vec<(&(String, String, String), &usize)> =
        key_counts.iter().filter(|(_, &c)| c > 1).collect();
    println!("\n[A1] STABLE KEY (file, breadcrumb, title)");
    println!("    distinct keys: {} for {} sections", key_counts.len(), all.len());
    println!("    colliding keys: {}", collisions.len());
    for (k, c) in collisions.iter().take(20) {
        println!("        x{c}  {}  ::  {} > {}", k.0, k.1, k.2);
    }
    let triple_unique = collisions.is_empty();
    println!("    => bare (file, breadcrumb, title) is {}", if triple_unique { "UNIQUE" } else { "NOT UNIQUE" });
    // Chosen key adds an `nth` ordinal ONLY where the triple is ambiguous (document
    // order by line_start). Assert THAT key is total-unique — it is by construction.
    let mut ordinal: HashMap<(String, String, String), usize> = HashMap::new();
    let mut by_line = all.clone();
    by_line.sort_by_key(|s| (s.file.clone(), s.line_start));
    let mut augmented: HashSet<(String, String, String, usize)> = HashSet::new();
    for s in &by_line {
        let triple = (s.file.clone(), s.breadcrumb.join(" > "), s.title.clone());
        let n = ordinal.entry(triple.clone()).or_insert(0);
        augmented.insert((triple.0, triple.1, triple.2, *n));
        *n += 1;
    }
    let key_unique = augmented.len() == all.len();
    println!("    => (file, breadcrumb, title, nth) is {} ({} keys / {} sections)",
             if key_unique { "UNIQUE" } else { "NOT UNIQUE" }, augmented.len(), all.len());

    // ---- A2: candidate sources ----
    // (i) the "Keyword"-titled sections, split by structured form.
    let kw_sections: Vec<&Section> =
        all.iter().filter(|s| s.title.to_lowercase().contains("keyword")).collect();
    let mut kw_orca: Vec<String> = Vec::new();
    let mut kw_table: Vec<String> = Vec::new();
    for s in &kw_sections {
        kw_orca.extend(orca_block_keywords(&s.body));
        kw_table.extend(table_keywords(&s.body));
    }
    let kw_all: HashSet<String> = kw_orca.iter().chain(kw_table.iter()).cloned().collect();

    // (ii) the "List of Input Blocks" flat-table — the single richest %-block source.
    let blocks_section = all.iter().find(|s| s.title == "List of Input Blocks");
    let block_names: Vec<String> = blocks_section
        .map(|s| table_keywords(&s.body))
        .unwrap_or_default();
    let block_distinct: HashSet<String> = block_names.iter().cloned().collect();

    // (iii) corpus-wide structured extraction (the maximal seed pool).
    let mut corpus_tokens: Vec<String> = Vec::new();
    for s in &all {
        corpus_tokens.extend(orca_block_keywords(&s.body));
        corpus_tokens.extend(table_keywords(&s.body));
    }
    let corpus_distinct: HashSet<String> = corpus_tokens.iter().cloned().collect();

    println!("\n[A2] CANDIDATE SOURCES (keyword tokens, heuristic extraction)");
    println!("    \"Keyword\"-titled sections: {}", kw_sections.len());
    println!("      from ```orca blocks:  {} tokens ({} distinct)", kw_orca.len(),
             kw_orca.iter().collect::<HashSet<_>>().len());
    println!("      from pipe/list/flat tables: {} tokens ({} distinct)", kw_table.len(),
             kw_table.iter().collect::<HashSet<_>>().len());
    println!("      union distinct (these sections): {}", kw_all.len());
    println!("    \"List of Input Blocks\" flat-table: {} names ({} distinct)",
             block_names.len(), block_distinct.len());
    println!("    corpus-wide structured tokens: {} ({} distinct) <- maximal seed pool",
             corpus_tokens.len(), corpus_distinct.len());

    // ---- A3: COVERAGE of the inventory in the corpus pool (a coarse string probe) ----
    let inventory = load_inventory();
    let pool: HashSet<String> = corpus_distinct.iter().map(|t| norm_kw(t)).collect();
    let mut covered = 0;
    let mut missing: Vec<&str> = Vec::new();
    for e in &inventory {
        if pool.contains(&norm_kw(&e.keyword)) {
            covered += 1;
        } else {
            missing.push(&e.keyword);
        }
    }
    println!("\n[A3] INVENTORY IN CORPUS POOL (coarse — string, not type)");
    println!("    inventory keywords: {}", inventory.len());
    println!("    with a candidate in the corpus pool: {}", covered);
    println!("    absent from pool ({}): {:?}", missing.len(), missing);

    // ---- A4: precision proxy — literal occurrence in the target section ----
    // Extractor sanity: a token extracted from section S must occur literally in S
    // (else the extractor invented it). Expect ~100%; failures name the misfires.
    let mut checked = 0usize;
    let mut literal = 0usize;
    let mut worst: Vec<String> = Vec::new();
    for s in &all {
        let toks: HashSet<String> =
            orca_block_keywords(&s.body).into_iter().chain(table_keywords(&s.body)).collect();
        let hay = format!("{}\n{}", s.title, s.body).to_lowercase();
        for t in toks {
            checked += 1;
            if hay.contains(&t.to_lowercase()) {
                literal += 1;
            } else if worst.len() < 10 {
                worst.push(format!("{} !in {}#{}", t, s.file, s.title));
            }
        }
    }
    println!("\n[A4] PRECISION PROXY (token literally in its target section)");
    println!(
        "    home-extraction candidates: {} literal / {} checked ({:.1}%)",
        literal, checked, 100.0 * literal as f64 / checked.max(1) as f64
    );
    for w in &worst {
        println!("        suspicious: {w}");
    }

    println!("\n{:=<72}", "");
    // The hard post-condition: the CHOSEN key (triple + nth) must be total-unique.
    assert!(key_unique, "(file, breadcrumb, title, nth) is NOT unique — the file form must change");
}

// --- Refined seed extractors (unit 4.4, Part B) ------------------------------

/// Drop MyST role backtick-args (`{cite}`x``, `{numref}`tab:…``, `{ref}`…``) so
/// "first backtick token" reads the KEYWORD, not a citation/label key. Without
/// this, `| M06-L {cite}`m06l` | `M06L` |` would yield the citation `m06l`.
fn strip_roles(s: &str) -> String {
    let re = Regex::new(r"\{[a-zA-Z:+_-]+\}`[^`]*`").unwrap();
    re.replace_all(s, "").into_owned()
}

/// First keyword-shaped token of each content line inside ANNOTATED ` ```orca `
/// blocks only — the `name value # description` keyword-list form. A block counts
/// as annotated when ≥half its content lines carry an inline `#` (so example-input
/// blocks, which have none, are excluded). This is the home for a keyword list.
fn orca_annotated_keywords(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut in_orca = false;
    let mut block: Vec<String> = Vec::new();
    let flush = |block: &mut Vec<String>, out: &mut Vec<String>| {
        let content: Vec<&String> = block.iter().filter(|l| !l.trim().is_empty()).collect();
        if content.is_empty() {
            block.clear();
            return;
        }
        let annotated = content.iter().filter(|l| {
            let t = l.trim();
            // inline '#' after a leading token (not a full-line comment)
            !t.starts_with('#') && t.contains('#')
        }).count();
        if annotated * 2 >= content.len() {
            for l in &content {
                let t = l.trim();
                if t.starts_with('#') || t.starts_with('*') {
                    continue;
                }
                if let Some(tok) = t.split_whitespace().next() {
                    let tok = tok.trim_end_matches(',');
                    if kw_token_ok(tok) {
                        out.push(tok.to_string());
                    }
                }
            }
        }
        block.clear();
    };
    for line in body.lines() {
        let tl = line.trim_start();
        if tl.starts_with("```") {
            let lang = tl.trim_start_matches('`').trim();
            if in_orca {
                flush(&mut block, &mut out);
                in_orca = false;
            } else if lang.eq_ignore_ascii_case("orca") {
                in_orca = true;
            }
            continue;
        }
        if in_orca {
            block.push(line.to_string());
        }
    }
    out
}

/// The FIRST keyword-shaped backtick token in a table row (roles stripped first),
/// so the DFT functional table's second-column `M06L` is read even though column
/// one holds only a prose name. One token per pipe / list / flat row.
fn table_first_keyword(body: &str) -> Vec<String> {
    let btick = Regex::new(r"`([^`]+)`").unwrap();
    let mut out = Vec::new();
    for line in body.lines() {
        let s = line.trim();
        let is_pipe = s.starts_with('|')
            && !s.trim_matches('|').split('|').next().unwrap_or("").trim()
                .chars().all(|c| c == '-' || c == ':' || c == ' ');
        let is_leader = s.starts_with("* - ");
        if !is_pipe && !is_leader {
            continue;
        }
        let cleaned = strip_roles(s);
        if let Some(cap) = btick.captures_iter(&cleaned).find(|c| kw_token_ok(&c[1])) {
            out.push(cap[1].to_string());
        }
    }
    out
}

/// A single-token section title that is keyword-SHAPED (so `## RIJCOSX`, `## RI-JK`,
/// `## GOAT`, `## HF` count, but `## Keywords`, `## Theory`, `## Example` do not).
/// Keyword-shaped = has a digit / `-` / `/` / `%`, OR an internal uppercase, OR is
/// ALL-CAPS — i.e. NOT a plain Capitalized or lowercase English word.
fn looks_like_keyword_title(t: &str) -> bool {
    let plain_word = Regex::new(r"^[A-Z][a-z]+$|^[a-z]+$").unwrap();
    if plain_word.is_match(t) {
        return false;
    }
    let has_special = t.chars().any(|c| c.is_ascii_digit() || "-/%".contains(c));
    let internal_upper = t.chars().skip(1).any(|c| c.is_ascii_uppercase());
    let all_caps = t.chars().all(|c| !c.is_ascii_lowercase());
    has_special || internal_upper || all_caps
}

/// M06-L↔M06L, M06-2X↔M062X — the manual drops the hyphen. Aliases, NOT
/// normalization: dashes are significant in def2-SVP, NEB-TS, B3LYP-D4.
const ALIASES: &[(&str, &str)] = &[("M06-L", "M06L"), ("M06-2X", "M062X")];
/// App keywords that live ONLY in prose (measured A3) — closed by hand curation,
/// not by the seed extractor. They still MUST resolve for the coverage gate.
const PROSE_CURATED: &[&str] = &["TightSCF", "VeryTightSCF"];

#[test]
#[ignore]
fn keyword_seed_ambiguity() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus at {}", version_dir.display());
        return;
    }
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = sections::sectionize(file, text) {
            all.extend(secs);
        }
    }

    println!("\n{:=<72}", "");
    println!("KEYWORD-SEED AMBIGUITY (unit 4.4 Part B) — {} sections", all.len());
    println!("{:=<72}", "");

    // Home-seed: annotated ```orca + table-first-keyword, EXCLUDING "List of Input
    // Blocks" (its rows are {numref}-deferred — their useful target is the referenced
    // doc section, not this index). token -> set of home section indices.
    // Structural tokens that pass kw_token_ok but are not keywords (block enders).
    let stop: HashSet<&str> = ["end", "End", "END"].into_iter().collect();
    let mut token_homes: HashMap<String, HashSet<usize>> = HashMap::new();
    let mut deferred_numref: HashSet<String> = HashSet::new();
    for (i, s) in all.iter().enumerate() {
        if s.title == "List of Input Blocks" || s.title == "Simple Keyword Lines" {
            for t in table_first_keyword(&s.body) {
                deferred_numref.insert(t);
            }
            continue; // not a home mapping
        }
        // The appendix (change log / glossary / public) titles many change entries
        // with keyword names (`## GOAT`) — not documentation. Not a home.
        if s.file.starts_with("contents/appendix/") {
            continue;
        }
        // A section whose whole title IS a keyword token (`## RI-JK`, `## RIJCOSX`,
        // `## GOAT`) is that keyword's cleanest documentation home — but a plain
        // Capitalized English word (`## Keywords`, `## Theory`, `## Example`) is NOT
        // a keyword, so title-home is restricted to keyword-SHAPED titles.
        let title = s.title.trim();
        if !title.contains(char::is_whitespace)
            && kw_token_ok(title)
            && !stop.contains(title)
            && looks_like_keyword_title(title)
        {
            token_homes.entry(title.to_string()).or_default().insert(i);
        }
        for t in orca_annotated_keywords(&s.body)
            .into_iter()
            .chain(table_first_keyword(&s.body))
        {
            if !stop.contains(t.as_str()) {
                token_homes.entry(t).or_default().insert(i);
            }
        }
    }

    let total = token_homes.len();
    let mut ambiguous: Vec<(&String, usize)> =
        token_homes.iter().filter(|(_, h)| h.len() >= 2).map(|(k, h)| (k, h.len())).collect();
    ambiguous.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let amb_frac = 100.0 * ambiguous.len() as f64 / total.max(1) as f64;

    println!("\n[B1] AMBIGUITY — one keyword string, many home sections");
    println!("    distinct home-seed tokens: {total}");
    println!("    tokens mapping to >=2 sections: {} ({:.1}%)", ambiguous.len(), amb_frac);
    println!("    worst 10 (token -> #sections):");
    for (k, n) in ambiguous.iter().take(10) {
        let homes: Vec<String> = token_homes[*k].iter().take(4)
            .map(|&i| format!("{}#{}", all[i].file, all[i].title)).collect();
        println!("        {:<18} -> {} sections   e.g. {:?}", k, n, homes);
    }

    // {numref}-deferred (not seeded this part) — reported as a count, not a loss.
    println!("\n[B2] {{numref}}-DEFERRED (NOT seeded here — next measure's material)");
    println!("    distinct block names in \"List of Input Blocks\" (+index): {}", deferred_numref.len());

    // Coverage preview against the seed pool (coarse — string, not type; the real
    // type-aware gate lives in generate_keywords_json + coverage.test.ts).
    let inventory = load_inventory();
    let pool: HashSet<String> = token_homes.keys().map(|t| norm_kw(t)).collect();
    let alias_of: HashMap<&str, &str> = ALIASES.iter().cloned().collect();
    let prose: HashSet<&str> = PROSE_CURATED.iter().cloned().collect();
    let (mut by_seed, mut by_alias, mut by_prose) = (0, 0, 0);
    let mut unresolved: Vec<&str> = Vec::new();
    for e in &inventory {
        let kw = e.keyword.as_str();
        if pool.contains(&norm_kw(kw)) {
            by_seed += 1;
        } else if alias_of.get(kw).is_some_and(|a| pool.contains(&norm_kw(a))) {
            by_alias += 1;
        } else if prose.contains(kw) {
            by_prose += 1;
        } else {
            unresolved.push(kw);
        }
    }
    println!("\n[B3] INVENTORY IN SEED POOL (coarse string probe)");
    println!("    by home-seed: {by_seed}   by alias: {by_alias}   by prose-curation: {by_prose}");
    println!("    absent from pool ({}): {:?}", unresolved.len(), unresolved);

    println!("\n{:=<72}", "");
    // EXIT gate: if too much of the map is ambiguous, the record shape (single
    // `section` vs `targets[]`) is wrong and must change BEFORE generating ~1400
    // records. Report and STOP rather than emit a plausibly-wrong file.
    if amb_frac > 30.0 {
        println!(">>> AMBIGUITY {amb_frac:.1}% > 30% — STOP, discuss the file form before generating.");
    } else {
        println!(">>> ambiguity {amb_frac:.1}% <= 30% — safe to generate (targets[] for the ambiguous tail).");
    }
}

// --- keywords.json generator (unit 4.4, Part B, author-run) ------------------
//
//     cargo test generate_keywords_json -- --ignored --nocapture
//
// Emits src/manual/keywords.json from the corpus: the broad structured pool,
// HOME mappings only ({numref}-target entries are deferred). Rust owns manual
// text-to-structure (ADR-013); the frontend consumes the file. Curation is layered
// on top (provenance: curated). The HARD post-condition: all 46 app-emitted
// keywords resolve, or the generator panics naming the misses.

/// Section key written into keywords.json — `(file, breadcrumb, title)` + `nth`
/// ordinal only where the triple repeats (the mreom dup-H2). NOT line_start.
fn section_key(s: &Section, nth: usize) -> serde_json::Value {
    serde_json::json!({
        "file": s.file,
        "breadcrumb": s.breadcrumb,
        "title": s.title,
        "nth": nth,
    })
}

#[test]
#[ignore]
fn generate_keywords_json() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus at {}", version_dir.display());
        return;
    }
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = sections::sectionize(file, text) {
            all.extend(secs);
        }
    }

    // nth ordinal per (file, breadcrumb, title), document order.
    let mut order: Vec<usize> = (0..all.len()).collect();
    order.sort_by_key(|&i| (all[i].file.clone(), all[i].line_start));
    let mut seen: HashMap<(String, String, String), usize> = HashMap::new();
    let mut nth_of: Vec<usize> = vec![0; all.len()];
    for &i in &order {
        let s = &all[i];
        let k = (s.file.clone(), s.breadcrumb.join("\u{1}"), s.title.clone());
        let n = seen.entry(k).or_insert(0);
        nth_of[i] = *n;
        *n += 1;
    }

    // Home-seed: title-as-keyword + annotated ```orca + table-first-keyword,
    // EXCLUDING the {numref}-index sections. token -> sorted, deduped section idxs.
    let stop: HashSet<&str> = ["end", "End", "END"].into_iter().collect();
    let mut homes: HashMap<String, Vec<usize>> = HashMap::new();
    let mut title_home: HashSet<String> = HashSet::new();
    for (i, s) in all.iter().enumerate() {
        if s.title == "List of Input Blocks" || s.title == "Simple Keyword Lines" {
            continue;
        }
        if s.file.starts_with("contents/appendix/") {
            continue; // change log / glossary titles are not keyword documentation
        }
        let title = s.title.trim();
        if !title.contains(char::is_whitespace)
            && kw_token_ok(title) && !stop.contains(title) && looks_like_keyword_title(title)
        {
            homes.entry(title.to_string()).or_default().push(i);
            title_home.insert(title.to_string());
        }
        for t in orca_annotated_keywords(&s.body).into_iter().chain(table_first_keyword(&s.body)) {
            if !stop.contains(t.as_str()) {
                homes.entry(t).or_default().push(i);
            }
        }
    }
    for v in homes.values_mut() {
        v.sort_unstable();
        v.dedup();
    }

    // TYPE COMES FROM THE MANUAL, not from what our builder emits. `app_simple` is NO
    // LONGER consulted here — that made the seed's `type` a fact about OUR app (the
    // root of the mis-typing: `else` was a dumpster). It moves to the CURATED layer
    // below, attributed. So: `%`→block; a section whose TITLE is the token→simple;
    // otherwise `unknown` — resolved by the owner (a `block-option` needs a positive
    // owner; none → `undetermined`, a value, not a dumpster default).
    let curated_simple: HashSet<String> = app_simple_set();
    let type_of = |tok: &str| -> &'static str {
        if tok.starts_with('%') {
            "block"
        } else if title_home.contains(tok) {
            "simple"
        } else {
            "unknown"
        }
    };

    // Curation overlay (provenance: curated): the two prose-only SCF keywords, and
    // the M06 hyphen aliases. This is where seed-then-curate begins.
    let scf_tol = all.iter().position(|s| {
        s.file == "contents/essentialelements/scf" && s.title == "Convergence Tolerances"
    });
    let mut curated_aliases: HashMap<String, Vec<String>> = HashMap::new();
    for (app, seed) in ALIASES {
        curated_aliases.entry(seed.to_string()).or_default().push(app.to_string());
    }

    // ---- Owner derivation (union of two independent signals, with provenance) ----
    let path_of = |s: &Section| -> Vec<String> {
        let mut p = s.breadcrumb.clone();
        p.push(s.title.clone());
        p
    };
    // %-block home sections per file (block names lowercased).
    let mut block_sec: HashMap<String, Vec<(Vec<String>, String)>> = HashMap::new();
    let mut blocks_in_file: HashMap<String, HashSet<String>> = HashMap::new();
    for (tok, idxs) in &homes {
        if type_of(tok) == "block" {
            for &i in idxs {
                let s = &all[i];
                block_sec.entry(s.file.clone()).or_default().push((path_of(s), tok.to_lowercase()));
                blocks_in_file.entry(s.file.clone()).or_default().insert(tok.to_lowercase());
            }
        }
    }
    // A "List of related keywords" / "See also" section LISTS other blocks' keywords —
    // it references, it does not document. Both derivations there answer the wrong
    // question, so owner = null BY RULE (measured: 2 sections, mcd + nocv).
    let cross_ref = |s: &Section| -> bool {
        let t = s.title.to_lowercase();
        t.contains("related keyword") || t.contains("see also")
    };
    let struct_owner = |idx: usize| -> Option<String> {
        let s = &all[idx];
        let cand: Option<String> = (|| {
            let dblocks = blocks_in_file.get(&s.file)?;
            if dblocks.len() == 1 {
                return dblocks.iter().next().cloned();
            }
            let p = path_of(s);
            let cands = block_sec.get(&s.file)?;
            let mut anc: Vec<&(Vec<String>, String)> =
                cands.iter().filter(|(bp, _)| bp.len() <= p.len() && p[..bp.len()] == bp[..]).collect();
            anc.sort_by_key(|(bp, _)| bp.len());
            let deepest = anc.last().map(|(bp, _)| bp.len())?;
            let top: HashSet<&String> =
                anc.iter().filter(|(bp, _)| bp.len() == deepest).map(|(_, bn)| bn).collect();
            (top.len() == 1).then(|| top.iter().next().unwrap().to_string())
        })();
        // B1 VETO (rule #9, correlate two sources): a structural owner is accepted ONLY
        // when the section body NAMES it. An owner inferred purely from a breadcrumb
        // ancestor the text never mentions does not belong. (Measured: 522/814 targets
        // failed this; ~508 are simple keywords in name-tables, not block-options.)
        cand.filter(|o| s.body.to_lowercase().contains(&o.to_lowercase()))
    };
    let text_owner = |idx: usize| -> Option<String> {
        let pts = percent_tokens(&all[idx]);
        (pts.len() == 1).then(|| pts.into_iter().next().unwrap())
    };
    // (owner, owner_source) — TEXT priority, STRUCTURAL fill, else null (a value).
    let owner_of = |idx: usize| -> (Option<String>, Option<&'static str>) {
        if cross_ref(&all[idx]) {
            return (None, None);
        }
        if let Some(o) = text_owner(idx) {
            return (Some(o), Some("text"));
        }
        if let Some(o) = struct_owner(idx) {
            return (Some(o), Some("structural"));
        }
        (None, None)
    };
    let src_rank = |s: Option<&'static str>| match s { Some("text") => 2, Some(_) => 1, None => 0 };

    // ---- Build records; section refs kept as `all`-indices, interned later. ----
    let mut recs: Vec<(serde_json::Map<String, serde_json::Value>, Vec<usize>)> = Vec::new();
    let base = |tok: &str, ty: &str, prov: &str, aliases: Option<&Vec<String>>| {
        let mut o = serde_json::Map::new();
        o.insert("keyword".into(), tok.into());
        o.insert("type".into(), ty.into());
        o.insert("provenance".into(), prov.into());
        if let Some(a) = aliases {
            o.insert("aliases".into(), serde_json::json!(a));
        }
        o
    };
    let mut tokens: Vec<&String> = homes.keys().collect();
    tokens.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    for tok in tokens {
        let idxs = &homes[tok];
        let aliases = curated_aliases.get(tok);
        let mut ty = type_of(tok);
        let mut prov = "seeded";
        if let Some(al) = aliases {
            prov = "curated"; // an alias is a curation act
            if al.iter().any(|a| curated_simple.contains(&norm_kw(a))) {
                ty = "simple"; // an alias that is an app `!`-keyword (M06-L)
            }
        }
        if ty == "unknown" {
            // Resolve by OWNER: `MaxIter` splits into (%scf, MaxIter), (%casscf, MaxIter)
            // — each with a text- or veto-confirmed owner → `block-option`. A group with
            // NO owner is NOT a block-option (no positive evidence) → `undetermined`.
            let mut groups: HashMap<Option<String>, (Option<&'static str>, Vec<usize>)> =
                HashMap::new();
            for &i in idxs {
                let (owner, src) = owner_of(i);
                let e = groups.entry(owner).or_insert((None, Vec::new()));
                if src_rank(src) > src_rank(e.0) {
                    e.0 = src;
                }
                e.1.push(i);
            }
            let mut gk: Vec<Option<String>> = groups.keys().cloned().collect();
            gk.sort_by(|a, b| a.clone().unwrap_or_default().cmp(&b.clone().unwrap_or_default()));
            for owner in gk {
                let (src, gidxs) = groups.remove(&owner).unwrap();
                match owner {
                    Some(b) => {
                        let mut o = base(tok, "block-option", prov, aliases);
                        o.insert("block".into(), b.into());
                        o.insert("owner_source".into(),
                                 src.map(serde_json::Value::from).unwrap_or(serde_json::Value::Null));
                        recs.push((o, gidxs));
                    }
                    None => {
                        // No manual owner → UNDETERMINED (like anchor=NULL): a value, not
                        // a dumpster `block-option`/null. Reachable only by the unqualified
                        // path ("documented in N places"), never by a qualified lookup.
                        recs.push((base(tok, "undetermined", prov, aliases), gidxs));
                    }
                }
            }
        } else {
            recs.push((base(tok, ty, prov, aliases), idxs.clone()));
        }
    }
    // ---- CURATED overlay: the app-emitted simple keywords, moved OUT of type_of into
    // an attributed channel. The seed left them `undetermined` — homogeneous with their
    // table-mates (`def2-QZVPP` and `ma-def2-SVP` both undetermined). Curation now
    // asserts, WITH PROVENANCE, that the ones OUR builder emits are simple. Same bit as
    // before, but attributed (visible in the diff, arguable per word) instead of
    // masquerading as a manual measurement inside `type_of`. ----
    let simple_seeded: HashSet<String> = recs.iter()
        .filter(|(o, _)| o["type"] == "simple")
        .map(|(o, _)| norm_kw(o["keyword"].as_str().unwrap()))
        .collect();
    // Words already covered via a curated ALIAS (M06-L → M06L) need nothing here.
    let alias_covered: HashSet<String> = curated_aliases.values().flatten().map(|a| norm_kw(a)).collect();
    let mut curated_from_undet: Vec<String> = Vec::new(); // flipped an undetermined record
    let mut curated_added: Vec<String> = Vec::new();       // added alongside a block-option record
    let mut curated_no_record: Vec<String> = Vec::new();   // no home at all (a curation gap)
    for kw in SEED_SIMPLE_HINT {
        let n = norm_kw(kw);
        if simple_seeded.contains(&n) || alias_covered.contains(&n) {
            continue; // seeded simple (title), or already curated via an alias
        }
        // Prefer flipping the word's undetermined group (its table-mates stay undetermined).
        let mut flipped = false;
        for (o, _) in recs.iter_mut() {
            if o["type"] == "undetermined" && norm_kw(o["keyword"].as_str().unwrap()) == n {
                o.insert("type".into(), "simple".into());
                o.insert("provenance".into(), "curated".into());
                flipped = true;
                break;
            }
        }
        if flipped {
            curated_from_undet.push((*kw).to_string());
            continue;
        }
        // No undetermined group — the word was extracted only WITH an owner (block-option).
        // It is still a simple `!` keyword our builder emits; ADD a curated simple record at
        // the same home(s), leaving the (genuine) block-option record intact.
        let idxs: Vec<usize> = recs.iter()
            .filter(|(o, _)| norm_kw(o["keyword"].as_str().unwrap()) == n)
            .flat_map(|(_, r)| r.iter().copied())
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if idxs.is_empty() {
            curated_no_record.push((*kw).to_string()); // not extracted anywhere (e.g. CPCM)
        } else {
            recs.push((base(kw, "simple", "curated", None), idxs));
            curated_added.push((*kw).to_string());
        }
    }

    // Curated prose-only entries (not in the structured seed).
    if let Some(i) = scf_tol {
        for kw in PROSE_CURATED {
            let mut o = base(kw, "simple", "curated", None);
            o.insert("summary".into(),
                "Tighten SCF convergence thresholds (prose-documented; no keyword table).".into());
            recs.push((o, vec![i]));
        }
    }

    // ---- Normalize sections: one array of distinct section keys + int refs. ----
    let skey_tuple = |i: usize| -> (String, Vec<String>, String, usize) {
        (all[i].file.clone(), all[i].breadcrumb.clone(), all[i].title.clone(), nth_of[i])
    };
    let mut ordered: Vec<usize> =
        recs.iter().flat_map(|(_, r)| r.iter().copied()).collect::<HashSet<_>>().into_iter().collect();
    ordered.sort_by(|&a, &b| skey_tuple(a).cmp(&skey_tuple(b)));
    let idx_map: HashMap<usize, usize> =
        ordered.iter().enumerate().map(|(new, &old)| (old, new)).collect();
    let sections_json: Vec<serde_json::Value> =
        ordered.iter().map(|&i| section_key(&all[i], nth_of[i])).collect();

    // Finalize each record: remap refs to int indices; `section` (1) or `targets` (n).
    let mut entries: Vec<serde_json::Value> = Vec::new();
    for (mut o, refs) in recs {
        let mut ints: Vec<usize> = refs.iter().map(|i| idx_map[i]).collect();
        ints.sort_unstable();
        ints.dedup();
        if ints.len() == 1 {
            o.insert("section".into(), (ints[0] as u64).into());
        } else {
            // Many homes -> targets[] REFLECTS REALITY (one option, several docs), not
            // our inability to pick. Hover must NOT collapse this to the first.
            o.insert("targets".into(), serde_json::json!(ints));
        }
        entries.push(serde_json::Value::Object(o));
    }
    entries.sort_by(|a, b| {
        let (ka, kb) = (a["keyword"].as_str().unwrap(), b["keyword"].as_str().unwrap());
        let (ba, bb) = (a.get("block").and_then(|x| x.as_str()).unwrap_or(""),
                        b.get("block").and_then(|x| x.as_str()).unwrap_or(""));
        ka.to_lowercase().cmp(&kb.to_lowercase()).then(ka.cmp(kb)).then(ba.cmp(bb))
    });

    // ---- HARD post-conditions (rule #9) ----
    // (a) COVERAGE over the shared inventory (src/manual/keyword-inventory.json — the ONE
    // home, read by coverage.test.ts too). TYPE- and block-aware: a record resolves an
    // entry only if its type matches `expect` and, for a block-option, the owning block.
    // A record contributes (keyword, type, owner-block-or-"") for the join.
    let resolvable: HashSet<(String, String, String)> = entries.iter().flat_map(|e| {
        let ty = e["type"].as_str().unwrap().to_string();
        let blk = if ty == "block-option" {
            e.get("block").and_then(|x| x.as_str()).unwrap_or("").to_lowercase()
        } else {
            String::new()
        };
        let mut ks = vec![norm_kw(e["keyword"].as_str().unwrap())];
        if let Some(al) = e.get("aliases").and_then(|a| a.as_array()) {
            ks.extend(al.iter().filter_map(|x| x.as_str()).map(norm_kw));
        }
        ks.into_iter().map(move |k| (k, ty.clone(), blk.clone()))
    }).collect();
    let inventory = load_inventory();
    let inv_resolves = |e: &InvEntry| {
        let blk = if e.expect == "block-option" {
            e.block.clone().unwrap_or_default().to_lowercase()
        } else {
            String::new()
        };
        resolvable.contains(&(norm_kw(&e.keyword), e.expect.clone(), blk))
    };
    // HARD = entries without a `gap` tag: they MUST resolve. `gap` entries are declared,
    // classified holes (a|b|c|d) — reported, never a panic (fixing them is a separate unit).
    let resolved = inventory.iter().filter(|e| inv_resolves(e)).count();
    let missing: Vec<&str> = inventory.iter()
        .filter(|e| e.gap.is_none() && !inv_resolves(e))
        .map(|e| e.keyword.as_str())
        .collect();
    let gap_now_resolves: Vec<&str> = inventory.iter()
        .filter(|e| e.gap.is_some() && inv_resolves(e))
        .map(|e| e.keyword.as_str())
        .collect();
    let gaps_by = |c: &str| -> Vec<String> {
        inventory.iter().filter(|e| e.gap.as_deref() == Some(c))
            .map(|e| format!("{} [{}]", e.keyword, e.expect)).collect()
    };
    // (b) no dangling int ref — every section/target index is in range.
    let n_sec = sections_json.len();
    let dangling = entries.iter().any(|e| {
        let idxs: Vec<i64> = match (e.get("section"), e.get("targets")) {
            (Some(s), _) => vec![s.as_i64().unwrap()],
            (_, Some(t)) => t.as_array().unwrap().iter().map(|x| x.as_i64().unwrap()).collect(),
            _ => vec![-1],
        };
        idxs.iter().any(|&i| i < 0 || i as usize >= n_sec)
    });

    // report tallies
    let ambiguous = entries.iter().filter(|e| e.get("targets").is_some()).count();
    let bo = |e: &serde_json::Value| e["type"] == "block-option";
    let cnt_src = |v: &str| entries.iter().filter(|e| bo(e) && e["owner_source"] == v).count();
    let (o_text, o_struct) = (cnt_src("text"), cnt_src("structural"));
    let o_null = entries.iter().filter(|e| bo(e) && e["owner_source"].is_null()).count();

    let doc = serde_json::json!({
        "schema_version": 2,
        "_comment": "SEEDED from the ORCA 6.1 manual structured markup (home mappings only) by \
                     `cargo test generate_keywords_json`; curate on top. Records reference the \
                     `sections` array by index; block-options carry (block, owner_source). \
                     See wiki/modules/manual-keywords.md.",
        "orca_version": version,
        "sections": sections_json,
        "keywords": entries,
    });
    let out_dir = repo_root().join("src/manual");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_path = out_dir.join("keywords.json");
    let text = serde_json::to_string_pretty(&doc).unwrap() + "\n";
    std::fs::write(&out_path, &text).unwrap();

    println!("\n{:=<72}", "");
    println!("GENERATED {} ({} bytes)", out_path.display(), text.len());
    println!("    sections: {n_sec} (normalized; was {} target objects before)",
             recs_target_count(&doc));
    println!("    keyword records: {} ({} ambiguous -> targets[])", entries.len(), ambiguous);
    let n_undet = entries.iter().filter(|e| e["type"] == "undetermined").count();
    let n_curated = entries.iter().filter(|e| e["provenance"] == "curated").count();
    println!("    block-option owner_source: text {o_text} / structural {o_struct} / null {o_null}");
    println!("    types: undetermined {n_undet} (was `block-option`/null); curated records {n_curated}");

    // --- B2 measure: the `app_simple` knowledge, moved to the curated channel ---
    let ch_title = curated_simple.iter().filter(|k| simple_seeded.contains(*k)).count();
    println!("\n    [B2] app-builder simple keywords ({}) — where the type comes from now:",
             curated_simple.len());
    println!("        on a section TITLE (stay SEEDED, manual): {ch_title}");
    println!("        moved to CURATED (were app_simple only): flip-undetermined {} + add-beside-blockopt {} + alias {}",
             curated_from_undet.len(), curated_added.len(),
             curated_simple.iter().filter(|k| alias_covered.contains(*k)).count());
    println!("        no home at all (curation gaps): {curated_no_record:?}");
    println!("        curated-simple list (flipped): {curated_from_undet:?}");
    println!("        curated-simple list (added beside block-option): {curated_added:?}");

    // --- Inventory coverage, split by CHANNEL (seed vs curation) so the two don't merge ---
    let rec_prov: HashMap<(String, String, String), String> = entries.iter().flat_map(|e| {
        let ty = e["type"].as_str().unwrap().to_string();
        let blk = if ty == "block-option" {
            e.get("block").and_then(|x| x.as_str()).unwrap_or("").to_lowercase()
        } else { String::new() };
        let prov = e["provenance"].as_str().unwrap().to_string();
        let mut ks = vec![norm_kw(e["keyword"].as_str().unwrap())];
        if let Some(al) = e.get("aliases").and_then(|a| a.as_array()) {
            ks.extend(al.iter().filter_map(|x| x.as_str()).map(norm_kw));
        }
        ks.into_iter().map(move |k| ((k, ty.clone(), blk.clone()), prov.clone()))
    }).collect();
    let (mut via_seed, mut via_cur) = (0usize, 0usize);
    for e in inventory.iter().filter(|e| inv_resolves(e)) {
        let blk = if e.expect == "block-option" { e.block.clone().unwrap_or_default().to_lowercase() } else { String::new() };
        match rec_prov.get(&(norm_kw(&e.keyword), e.expect.clone(), blk)).map(|s| s.as_str()) {
            Some("curated") => via_cur += 1,
            _ => via_seed += 1,
        }
    }
    println!("\n    INVENTORY COVERAGE (type-aware): {resolved} of {} resolve — {via_seed} via SEED (manual), \
             {via_cur} via CURATION (ours, attributed)", inventory.len());
    println!("      gaps by closer:");
    println!("        (a) {{numref}} layer:     {:?}", gaps_by("a"));
    println!("        (b) curated (prose):    {:?}", gaps_by("b"));
    println!("        (c) second simple form: {:?}", gaps_by("c"));
    println!("        (d) not in corpus:      {:?}", gaps_by("d"));
    println!("{:=<72}", "");
    assert!(scf_tol.is_some(), "SCF Convergence Tolerances home not found — curation target moved");
    // Only HARD (non-gap) inventory words are a post-condition; a red/incomplete coverage
    // number is a REPORT (the gaps above), not a panic — that is the whole point of the unit.
    assert!(missing.is_empty(), "HARD coverage FAILED — non-gap inventory word unresolved: {missing:?}");
    assert!(gap_now_resolves.is_empty(),
            "a declared gap now resolves — remove its `gap` tag from keyword-inventory.json: {gap_now_resolves:?}");
    assert!(!dangling, "a section/target index is out of range — dangling reference");
    assert!(n_sec > 0, "no sections");
}

/// Sum of section references across records (for the before/after normalization report).
fn recs_target_count(doc: &serde_json::Value) -> usize {
    doc["keywords"].as_array().unwrap().iter().map(|e| {
        if e.get("section").is_some() { 1 } else { e["targets"].as_array().unwrap().len() }
    }).sum()
}

// --- Owner DIRECT-SIGNAL measure (unit 4.4, Part C) --------------------------
//
//     cargo test owner_signal_measure -- --ignored --nocapture
//
// The block-option owner should come from the LITERAL `%block` token in the
// option's home section (`%scf MaxIter 200 end` in an annotated ```orca block, or
// the heading) — text, not inference. This measures how strong that direct signal
// is: does the home section carry exactly one literal `%`-token (→ owner direct),
// several (→ needs a rule, not invented here), or none (→ null)?

/// Distinct literal `%block` tokens in a section's title + body.
fn percent_tokens(s: &Section) -> HashSet<String> {
    let re = Regex::new(r"%[A-Za-z][A-Za-z0-9]*").unwrap();
    let hay = format!("{}\n{}", s.title, s.body);
    re.find_iter(&hay).map(|m| m.as_str().to_lowercase()).collect()
}

/// Distinct `%block` tokens that OPEN an annotated ```orca block that also contains
/// `opt` as a first-token line — the finest signal (same code block as the option).
fn percent_in_option_block(body: &str, opt: &str) -> HashSet<String> {
    let mut owners = HashSet::new();
    let mut in_orca = false;
    let mut block: Vec<String> = Vec::new();
    let opt_l = opt.to_lowercase();
    let scan = |block: &[String], owners: &mut HashSet<String>| {
        let pcts: Vec<String> = block.iter().flat_map(|l| {
            Regex::new(r"%[A-Za-z][A-Za-z0-9]*").unwrap()
                .find_iter(l).map(|m| m.as_str().to_lowercase()).collect::<Vec<_>>()
        }).collect();
        let has_opt = block.iter().any(|l| {
            l.trim().split_whitespace().next().map(|t| t.trim_end_matches(',').to_lowercase())
                == Some(opt_l.clone())
        });
        if has_opt {
            for p in pcts {
                owners.insert(p);
            }
        }
    };
    for line in body.lines() {
        let tl = line.trim_start();
        if tl.starts_with("```") {
            let lang = tl.trim_start_matches('`').trim();
            if in_orca {
                scan(&block, &mut owners);
                block.clear();
                in_orca = false;
            } else if lang.eq_ignore_ascii_case("orca") {
                in_orca = true;
            }
            continue;
        }
        if in_orca {
            block.push(line.to_string());
        }
    }
    owners
}

#[test]
#[ignore]
fn owner_signal_measure() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus at {}", version_dir.display());
        return;
    }
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = sections::sectionize(file, text) {
            all.extend(secs);
        }
    }

    // Rebuild the home-seed exactly as the generator does, keeping the SOURCE token
    // per (section, token) so we can classify block-option targets.
    let stop: HashSet<&str> = ["end", "End", "END"].into_iter().collect();
    let app_simple: HashSet<String> = app_simple_set();
    let mut title_home: HashSet<String> = HashSet::new();
    // (token, section_idx) for every home mapping.
    let mut homes: Vec<(String, usize)> = Vec::new();
    for (i, s) in all.iter().enumerate() {
        if s.title == "List of Input Blocks" || s.title == "Simple Keyword Lines"
            || s.file.starts_with("contents/appendix/")
        {
            continue;
        }
        let title = s.title.trim();
        if !title.contains(char::is_whitespace)
            && kw_token_ok(title) && !stop.contains(title) && looks_like_keyword_title(title)
        {
            title_home.insert(title.to_string());
            homes.push((title.to_string(), i));
        }
        for t in orca_annotated_keywords(&s.body).into_iter().chain(table_first_keyword(&s.body)) {
            if !stop.contains(t.as_str()) {
                homes.push((t, i));
            }
        }
    }
    let type_of = |tok: &str| -> &'static str {
        if tok.starts_with('%') { "block" }
        else if app_simple.contains(&norm_kw(tok)) || title_home.contains(tok) { "simple" }
        else { "block-option" }
    };

    // Classify each block-option TARGET (option × home section) by the direct signal.
    let mut one = 0usize;      // exactly one literal %-token in the home section
    let mut many = 0usize;     // several -> needs a rule
    let mut none = 0usize;     // zero -> null
    let mut many_block_resolves = 0usize; // of `many`, the same-```orca-block signal is unique
    let mut sec_of: HashMap<usize, HashSet<String>> = HashMap::new();
    let mut bo_sections: HashSet<usize> = HashSet::new();
    for (tok, i) in &homes {
        if type_of(tok) != "block-option" {
            continue;
        }
        bo_sections.insert(*i);
        let pts = sec_of.entry(*i).or_insert_with(|| percent_tokens(&all[*i])).clone();
        match pts.len() {
            0 => none += 1,
            1 => one += 1,
            _ => {
                many += 1;
                // finer signal: %-token(s) in the SAME ```orca block as the option
                let inblk = percent_in_option_block(&all[*i].body, tok);
                if inblk.len() == 1 {
                    many_block_resolves += 1;
                }
            }
        }
    }
    let total = one + many + none;

    // Per distinct SECTION (what the prompt asked): how many %-tokens.
    let mut sec_one = 0usize;
    let mut sec_many = 0usize;
    let mut sec_none = 0usize;
    for i in &bo_sections {
        match sec_of.entry(*i).or_insert_with(|| percent_tokens(&all[*i])).len() {
            0 => sec_none += 1,
            1 => sec_one += 1,
            _ => sec_many += 1,
        }
    }

    println!("\n{:=<72}", "");
    println!("OWNER DIRECT-SIGNAL (literal %-token in the option's home section)");
    println!("{:=<72}", "");
    println!("\n[per block-option TARGET] total {total}");
    println!("    exactly one %-token  -> owner DIRECT & unique: {one} ({:.1}%)", 100.0 * one as f64 / total as f64);
    println!("    several %-tokens     -> needs a rule:          {many} ({:.1}%)", 100.0 * many as f64 / total as f64);
    println!("        (of those, the same-```orca-block signal is UNIQUE: {many_block_resolves})");
    println!("    zero %-tokens        -> null:                  {none} ({:.1}%)", 100.0 * none as f64 / total as f64);
    println!("    direct-signal owner coverage (one + block-resolved many): {} ({:.1}%)",
             one + many_block_resolves, 100.0 * (one + many_block_resolves) as f64 / total as f64);
    println!("\n[per distinct SECTION] {} sections yielded a block-option", bo_sections.len());
    println!("    one %-token: {sec_one} | several: {sec_many} | none: {sec_none}");
    println!("\n    vs the STRUCTURAL proxy measured earlier: 62.0% owner / 38.0% null.");
    println!("{:=<72}", "");
}

// --- Owner UNION + AGREEMENT measure (unit 4.4, Part C) ----------------------
//
//     cargo test owner_union_measure -- --ignored --nocapture
//
// Union of two independent derivations with provenance: TEXT (literal single
// %-token in the home section) takes priority; STRUCTURAL (unique %-block of the
// file, or unique deepest %-block ancestor) fills where text is silent; both
// silent -> null (a value, like anchor_source 'undetermined'). The load-bearing
// number is AGREEMENT on the intersection: if the two rarely disagree, the union
// is sound and the structural claim already in the wiki holds; if they disagree
// a lot, structural is unreliable and must be demoted.

#[test]
#[ignore]
fn owner_union_measure() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus at {}", version_dir.display());
        return;
    }
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = sections::sectionize(file, text) {
            all.extend(secs);
        }
    }

    let stop: HashSet<&str> = ["end", "End", "END"].into_iter().collect();
    let app_simple: HashSet<String> = app_simple_set();
    let mut title_home: HashSet<String> = HashSet::new();
    // dedup (token, section) so counts match the generated file's target set.
    let mut homes: HashSet<(String, usize)> = HashSet::new();
    for (i, s) in all.iter().enumerate() {
        if s.title == "List of Input Blocks" || s.title == "Simple Keyword Lines"
            || s.file.starts_with("contents/appendix/")
        {
            continue;
        }
        let title = s.title.trim();
        if !title.contains(char::is_whitespace)
            && kw_token_ok(title) && !stop.contains(title) && looks_like_keyword_title(title)
        {
            title_home.insert(title.to_string());
            homes.insert((title.to_string(), i));
        }
        for t in orca_annotated_keywords(&s.body).into_iter().chain(table_first_keyword(&s.body)) {
            if !stop.contains(t.as_str()) {
                homes.insert((t, i));
            }
        }
    }
    let type_of = |tok: &str| -> &'static str {
        if tok.starts_with('%') { "block" }
        else if app_simple.contains(&norm_kw(tok)) || title_home.contains(tok) { "simple" }
        else { "block-option" }
    };

    // Structural derivation inputs: %-block home sections per file (lowercased).
    let path_of = |s: &Section| -> Vec<String> {
        let mut p = s.breadcrumb.clone();
        p.push(s.title.clone());
        p
    };
    let mut block_sec: HashMap<String, Vec<(Vec<String>, String)>> = HashMap::new();
    let mut blocks_in_file: HashMap<String, HashSet<String>> = HashMap::new();
    for (tok, i) in &homes {
        if type_of(tok) == "block" {
            let s = &all[*i];
            block_sec.entry(s.file.clone()).or_default().push((path_of(s), tok.to_lowercase()));
            blocks_in_file.entry(s.file.clone()).or_default().insert(tok.to_lowercase());
        }
    }
    let struct_owner = |idx: usize| -> Option<String> {
        let s = &all[idx];
        let dblocks = blocks_in_file.get(&s.file)?;
        if dblocks.len() == 1 {
            return dblocks.iter().next().cloned();
        }
        let p = path_of(s);
        let cands = block_sec.get(&s.file)?;
        let mut anc: Vec<&(Vec<String>, String)> =
            cands.iter().filter(|(bp, _)| bp.len() <= p.len() && p[..bp.len()] == bp[..]).collect();
        anc.sort_by_key(|(bp, _)| bp.len());
        let deepest = anc.last().map(|(bp, _)| bp.len())?;
        let top: HashSet<&String> =
            anc.iter().filter(|(bp, _)| bp.len() == deepest).map(|(_, bn)| bn).collect();
        if top.len() == 1 {
            Some(top.into_iter().next().unwrap().clone())
        } else {
            None
        }
    };
    let text_owner = |idx: usize| -> Option<String> {
        let pts = percent_tokens(&all[idx]);
        if pts.len() == 1 { pts.into_iter().next() } else { None }
    };

    let mut total = 0usize;
    let (mut has_text, mut has_struct, mut has_union) = (0usize, 0usize, 0usize);
    let (mut both, mut agree) = (0usize, 0usize);
    let (mut src_text, mut src_struct, mut src_null) = (0usize, 0usize, 0usize);
    let mut disagreements: Vec<String> = Vec::new();
    for (tok, i) in &homes {
        if type_of(tok) != "block-option" {
            continue;
        }
        total += 1;
        let t = text_owner(*i);
        let st = struct_owner(*i);
        if t.is_some() { has_text += 1; }
        if st.is_some() { has_struct += 1; }
        if t.is_some() || st.is_some() { has_union += 1; }
        if let (Some(tt), Some(ss)) = (&t, &st) {
            both += 1;
            if tt == ss {
                agree += 1;
            } else if disagreements.len() < 10 {
                disagreements.push(format!("{:<16} @ {}#{}  text={} struct={}",
                    tok, all[*i].file, all[*i].title, tt, ss));
            }
        }
        match (t.is_some(), st.is_some()) {
            (true, _) => src_text += 1,       // text priority
            (false, true) => src_struct += 1,
            (false, false) => src_null += 1,
        }
    }

    println!("\n{:=<72}", "");
    println!("OWNER UNION + AGREEMENT — {total} block-option targets (deduped)");
    println!("{:=<72}", "");
    println!("\n[1] COVERAGE");
    println!("    text  resolves: {has_text} ({:.1}%)", pct(has_text, total));
    println!("    struct resolves: {has_struct} ({:.1}%)", pct(has_struct, total));
    println!("    UNION resolves:  {has_union} ({:.1}%)   null: {} ({:.1}%)",
             pct(has_union, total), total - has_union, pct(total - has_union, total));
    println!("\n[2] AGREEMENT on the intersection (THE number)");
    println!("    both resolve: {both} | agree: {agree} ({:.1}%) | disagree: {}",
             pct(agree, both.max(1)), both - agree);
    for d in &disagreements {
        println!("        {d}");
    }
    println!("\n[3] owner_source distribution (text priority)");
    println!("    text: {src_text} ({:.1}%) | structural: {src_struct} ({:.1}%) | null: {src_null} ({:.1}%)",
             pct(src_text, total), pct(src_struct, total), pct(src_null, total));
    println!("{:=<72}", "");
}

fn pct(n: usize, d: usize) -> f64 {
    100.0 * n as f64 / d.max(1) as f64
}

// --- Structural mis-typing overlap measure (unit 4.4 Part F) -----------------
//
//     cargo test structural_overlap_measure -- --ignored --nocapture
//
// ROOT (confirmed): the generator's `type_of` `else` branch is a dumpster — a token
// that is neither `%`-prefixed nor one of OUR builder/title-home simples defaults to
// `block-option` with NO manual signal. The structural proxy then hands it a breadcrumb
// owner. So the "mis-typed block-option" population is largely SIMPLE keywords (basis
// names, run-type tables). This measures how many of those 537 no-text-support targets
// are ALREADY documented elsewhere (a merge, not a new type), vs true orphans — the
// number that decides Part B's shape.

#[test]
#[ignore]
fn structural_overlap_measure() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus");
        return;
    }
    // Sectionise → body per descriptor (file, breadcrumb, title, nth), nth in doc order
    // (the SAME scheme the generator used to key sections).
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = sections::sectionize(file, text) {
            all.extend(secs);
        }
    }
    let mut order: Vec<usize> = (0..all.len()).collect();
    order.sort_by_key(|&i| (all[i].file.clone(), all[i].line_start));
    let mut nth_seen: HashMap<(String, String, String), usize> = HashMap::new();
    let mut body_by_key: HashMap<(String, String, String, usize), String> = HashMap::new();
    for &i in &order {
        let s = &all[i];
        let trip = (s.file.clone(), s.breadcrumb.join("\u{1}"), s.title.clone());
        let n = nth_seen.entry(trip.clone()).or_insert(0);
        body_by_key.insert((trip.0, trip.1, trip.2, *n), s.body.clone());
        *n += 1;
    }

    // Corpus-wide `!`-line tokens inside ```orca blocks (direct "this is a simple keyword"
    // signal, over the WHOLE corpus, not just the home section).
    let mut bang_tokens: HashSet<String> = HashSet::new();
    for (_f, text) in &leaves {
        let mut in_orca = false;
        for line in text.lines() {
            let tl = line.trim_start();
            if tl.starts_with("```") {
                in_orca = if in_orca { false } else { tl.trim_start_matches('`').trim().eq_ignore_ascii_case("orca") };
                continue;
            }
            if in_orca && tl.starts_with('!') {
                for t in tl.trim_start_matches('!').split_whitespace() {
                    bang_tokens.insert(norm_kw(t));
                }
            }
        }
    }

    // keywords.json
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo_root().join("src/manual/keywords.json")).unwrap()).unwrap();
    let secs = doc["sections"].as_array().unwrap();
    let key_of = |i: usize| -> (String, String, String, usize) {
        let s = &secs[i];
        (
            s["file"].as_str().unwrap().to_string(),
            s["breadcrumb"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect::<Vec<_>>().join("\u{1}"),
            s["title"].as_str().unwrap().to_string(),
            s["nth"].as_u64().unwrap() as usize,
        )
    };
    let title_of = |i: usize| secs[i]["title"].as_str().unwrap().to_string();
    let recs = doc["keywords"].as_array().unwrap();
    let idxs = |r: &serde_json::Value| -> Vec<usize> {
        if let Some(s) = r.get("section") { vec![s.as_u64().unwrap() as usize] }
        else { r["targets"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as usize).collect() }
    };
    let owner_in_body = |owner: &str, body: &str| body.to_lowercase().contains(&owner.to_lowercase());

    // Per-keyword: is there a SIMPLE record? a CONFIRMED block-option record?
    let mut has_simple: HashSet<String> = HashSet::new();
    let mut has_confirmed_bo: HashSet<String> = HashSet::new();
    for r in recs {
        let k = norm_kw(r["keyword"].as_str().unwrap());
        let ty = r["type"].as_str().unwrap();
        if ty == "simple" {
            has_simple.insert(k.clone());
        }
        if ty == "block-option" {
            let src = r["owner_source"].as_str();
            let owner = r["block"].as_str().unwrap_or("");
            let confirmed = src == Some("text")
                || (src == Some("structural")
                    && idxs(r).iter().any(|&i| body_by_key.get(&key_of(i)).is_some_and(|b| owner_in_body(owner, b))));
            if confirmed {
                has_confirmed_bo.insert(k);
            }
        }
    }

    // The 537: structural block-option TARGETS whose owner is NOT in the section body.
    let (mut c1a, mut c1b, mut c1c) = (0usize, 0usize, 0usize);
    let mut no_support = 0usize;
    let mut orphan_titles: HashMap<String, usize> = HashMap::new();
    let mut orphan_words: Vec<String> = Vec::new();
    let mut orphan_in_bang = 0usize;
    let mut orphan_word_set: HashSet<String> = HashSet::new();
    for r in recs {
        if r["type"].as_str() != Some("block-option") || r["owner_source"].as_str() != Some("structural") {
            continue;
        }
        let k = norm_kw(r["keyword"].as_str().unwrap());
        let owner = r["block"].as_str().unwrap_or("");
        for i in idxs(r) {
            let body_missing_owner = body_by_key.get(&key_of(i)).map(|b| !owner_in_body(owner, b)).unwrap_or(true);
            if !body_missing_owner {
                continue;
            }
            no_support += 1;
            if has_simple.contains(&k) {
                c1a += 1;
            } else if has_confirmed_bo.contains(&k) {
                c1b += 1;
            } else {
                c1c += 1;
                *orphan_titles.entry(title_of(i)).or_insert(0) += 1;
                if orphan_word_set.insert(k.clone()) {
                    orphan_words.push(r["keyword"].as_str().unwrap().to_string());
                    if bang_tokens.contains(&k) {
                        orphan_in_bang += 1;
                    }
                }
            }
        }
    }

    println!("\n{:=<72}", "");
    println!("STRUCTURAL MIS-TYPE OVERLAP — no-text-support structural targets");
    println!("{:=<72}", "");
    println!("    no-owner-in-body targets: {no_support}");
    println!("    1a already documented as type=simple elsewhere: {c1a}");
    println!("    1b already a CONFIRMED block-option elsewhere:  {c1b}");
    println!("    1c true orphans (no other record):             {c1c}");
    println!("    (1a+1b+1c = {} )", c1a + c1b + c1c);
    println!("\n    [1c] distinct orphan keywords: {}", orphan_word_set.len());
    println!("    [1c] orphan words also seen on a `!` line SOMEWHERE in corpus: {orphan_in_bang} / {}",
             orphan_word_set.len());
    let mut tt: Vec<(&String, &usize)> = orphan_titles.iter().collect();
    tt.sort_by(|a, b| b.1.cmp(a.1));
    println!("    [1c] top-10 source-section titles:");
    for (t, c) in tt.iter().take(10) {
        println!("        {c:4}  {t}");
    }
    println!("    [1c] 15 orphan words: {:?}", orphan_words.iter().take(15).collect::<Vec<_>>());
    println!("{:=<72}", "");
}

// --- B0: per-SECTION signal over the 508 orphans (unit 4.4 Part F/B0) --------
//
//     cargo test orphan_section_signal -- --ignored --nocapture
//
// The 8.8 % `!`-signal was per-WORD; the manual documents `!`-usage per-TABLE. So
// measure per source SECTION: does the section carry a `!`-line using one of ITS OWN
// extracted words (per-section simple signal), and — the reverse, mandatory — does the
// same section ALSO use its words as a block option (`%basis … <word>`)? A mixed table
// makes table-propagation illegal, and that must be a NUMBER, not an assumption of
// homogeneity.

#[test]
#[ignore]
fn orphan_section_signal() {
    let manual_dir = repo_root().join("resources/manual");
    let version = corpus_version(&manual_dir);
    let version_dir = manual_dir.join(&version);
    if !version_dir.is_dir() {
        eprintln!("skipping: no corpus");
        return;
    }
    let mut leaves: Vec<(String, String)> = Vec::new();
    collect_leaves(&version_dir, &version_dir, &mut leaves);
    leaves.sort_by(|a, b| a.0.cmp(&b.0));
    let mut all: Vec<Section> = Vec::new();
    for (file, text) in &leaves {
        if let Ok(secs) = sections::sectionize(file, text) {
            all.extend(secs);
        }
    }
    let mut order: Vec<usize> = (0..all.len()).collect();
    order.sort_by_key(|&i| (all[i].file.clone(), all[i].line_start));
    let mut nth_seen: HashMap<(String, String, String), usize> = HashMap::new();
    let mut body_by_key: HashMap<(String, String, String, usize), String> = HashMap::new();
    for &i in &order {
        let s = &all[i];
        let trip = (s.file.clone(), s.breadcrumb.join("\u{1}"), s.title.clone());
        let n = nth_seen.entry(trip.clone()).or_insert(0);
        body_by_key.insert((trip.0, trip.1, trip.2, *n), s.body.clone());
        *n += 1;
    }

    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(repo_root().join("src/manual/keywords.json")).unwrap()).unwrap();
    let secs = doc["sections"].as_array().unwrap();
    let key_of = |i: usize| -> (String, String, String, usize) {
        let s = &secs[i];
        (s["file"].as_str().unwrap().to_string(),
         s["breadcrumb"].as_array().unwrap().iter().map(|x| x.as_str().unwrap()).collect::<Vec<_>>().join("\u{1}"),
         s["title"].as_str().unwrap().to_string(),
         s["nth"].as_u64().unwrap() as usize)
    };
    let recs = doc["keywords"].as_array().unwrap();
    let idxs = |r: &serde_json::Value| -> Vec<usize> {
        if let Some(s) = r.get("section") { vec![s.as_u64().unwrap() as usize] }
        else { r["targets"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as usize).collect() }
    };
    let owner_in_body = |owner: &str, body: &str| body.to_lowercase().contains(&owner.to_lowercase());
    let mut has_simple: HashSet<String> = HashSet::new();
    let mut has_confirmed_bo: HashSet<String> = HashSet::new();
    for r in recs {
        let k = norm_kw(r["keyword"].as_str().unwrap());
        let ty = r["type"].as_str().unwrap();
        if ty == "simple" { has_simple.insert(k.clone()); }
        if ty == "block-option" {
            let owner = r["block"].as_str().unwrap_or("");
            let confirmed = r["owner_source"].as_str() == Some("text")
                || (r["owner_source"].as_str() == Some("structural")
                    && idxs(r).iter().any(|&i| body_by_key.get(&key_of(i)).is_some_and(|b| owner_in_body(owner, b))));
            if confirmed { has_confirmed_bo.insert(k); }
        }
    }
    // orphan (section index) -> its orphan words (normalized)
    let mut orphan_sec: HashMap<usize, HashSet<String>> = HashMap::new();
    for r in recs {
        if r["type"].as_str() != Some("block-option") || r["owner_source"].as_str() != Some("structural") { continue; }
        let k = norm_kw(r["keyword"].as_str().unwrap());
        let owner = r["block"].as_str().unwrap_or("");
        for i in idxs(r) {
            let missing = body_by_key.get(&key_of(i)).map(|b| !owner_in_body(owner, b)).unwrap_or(true);
            if missing && !has_simple.contains(&k) && !has_confirmed_bo.contains(&k) {
                orphan_sec.entry(i).or_default().insert(k.clone());
            }
        }
    }

    // Per section: does it carry a `!`-line using one of its OWN words (bang), and does
    // it use one of its OWN words INSIDE a `%…` block (blockuse)?
    let scan = |body: &str, words: &HashSet<String>| -> (bool, bool) {
        let (mut bang, mut blockuse) = (false, false);
        let mut in_orca = false;
        let mut block: Vec<String> = Vec::new();
        let flush = |block: &[String], bang: &mut bool, blockuse: &mut bool| {
            let has_pct = block.iter().any(|l| l.to_lowercase().contains('%'));
            for l in block {
                let tl = l.trim_start();
                if let Some(rest) = tl.strip_prefix('!') {
                    if rest.split_whitespace().any(|t| words.contains(&norm_kw(t))) { *bang = true; }
                } else if has_pct {
                    if tl.split_whitespace().any(|t| words.contains(&norm_kw(t))) { *blockuse = true; }
                }
            }
        };
        for line in body.lines() {
            let tl = line.trim_start();
            if tl.starts_with("```") {
                if in_orca { flush(&block, &mut bang, &mut blockuse); block.clear(); in_orca = false; }
                else if tl.trim_start_matches('`').trim().eq_ignore_ascii_case("orca") { in_orca = true; }
                continue;
            }
            if in_orca { block.push(line.to_string()); }
        }
        (bang, blockuse)
    };

    let (mut s_pure_bang, mut s_pure_block, mut s_mixed, mut s_neither) = (0usize, 0usize, 0usize, 0usize);
    let (mut o_pure_bang, mut o_pure_block, mut o_mixed, mut o_neither) = (0usize, 0usize, 0usize, 0usize);
    let mut title_of_bucket: HashMap<&str, usize> = HashMap::new();
    for (&i, words) in &orphan_sec {
        let body = body_by_key.get(&key_of(i)).cloned().unwrap_or_default();
        let (bang, blockuse) = scan(&body, words);
        let n = words.len();
        match (bang, blockuse) {
            (true, false) => { s_pure_bang += 1; o_pure_bang += n; *title_of_bucket.entry("pure-!").or_insert(0)+=1; }
            (false, true) => { s_pure_block += 1; o_pure_block += n; }
            (true, true)  => { s_mixed += 1; o_mixed += n; }
            (false, false)=> { s_neither += 1; o_neither += n; }
        }
    }
    let sections = orphan_sec.len();
    let orphans: usize = orphan_sec.values().map(|w| w.len()).sum();
    println!("\n{:=<72}", "");
    println!("ORPHAN PER-SECTION SIGNAL — {sections} source sections, {orphans} orphan words");
    println!("{:=<72}", "");
    println!("  sections: pure-! {s_pure_bang} | pure-block {s_pure_block} | mixed {s_mixed} | neither {s_neither}");
    println!("  orphans:  pure-! {o_pure_bang} | pure-block {o_pure_block} | mixed {o_mixed} | neither {o_neither}");
    println!("  => per-section `!` signal (pure-! ∪ mixed) covers {} of {} orphans",
             o_pure_bang + o_mixed, orphans);
    println!("  => clean simple candidates (pure-! only): {o_pure_bang}; MIXED (propagation illegal): {o_mixed}; no-signal: {o_neither}");
    println!("{:=<72}", "");
}
