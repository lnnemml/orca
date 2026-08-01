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

/// Every keyword the APP ITSELF emits into an input — read from the code, not memory
/// (`src/input-builder/orca-options.ts`, `build-input.ts`, `templates/orca-templates.ts`,
/// `scene/constraints.ts`). Compound `!` tokens are split. Kind: `!` simple / `%` block
/// / `opt` block-option. THIS is the denominator of the coverage number.
const APP_EMITTED: &[(&str, &str)] = &[
    // job types (Opt Freq / OptTS Freq / NumFreq split)
    ("Opt", "!"), ("Freq", "!"), ("OptTS", "!"), ("NumFreq", "!"),
    // composite (3c) methods
    ("r2SCAN-3c", "!"), ("B97-3c", "!"), ("PBEh-3c", "!"), ("wB97X-3c", "!"), ("HF-3c", "!"),
    // functionals
    ("BP86", "!"), ("PBE", "!"), ("BLYP", "!"), ("TPSS", "!"), ("r2SCAN", "!"), ("M06-L", "!"),
    ("B3LYP", "!"), ("PBE0", "!"), ("TPSSh", "!"), ("M06-2X", "!"),
    ("wB97X-D4", "!"), ("CAM-B3LYP", "!"), ("wB97M-V", "!"), ("HF", "!"),
    // basis sets
    ("def2-SVP", "!"), ("def2-TZVP", "!"), ("def2-TZVPP", "!"), ("def2-QZVPP", "!"),
    ("def2-TZVPD", "!"), ("def2-SVPD", "!"),
    // aux bases (emitted by build-input / templates)
    ("def2/J", "!"), ("def2/JK", "!"),
    // dispersion
    ("D4", "!"), ("D3BJ", "!"), ("D3Zero", "!"), ("NL", "!"),
    // RI
    ("RIJCOSX", "!"), ("RI-JK", "!"), ("RI", "!"),
    // solvation models
    ("CPCM", "!"), ("SMD", "!"),
    // SCF convergence
    ("TightSCF", "!"), ("VeryTightSCF", "!"),
    // %-blocks the app writes
    ("%pal", "%"), ("%maxcore", "%"), ("%geom", "%"),
    // %geom sub-option
    ("Constraints", "opt"),
];

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

    // ---- A3: COVERAGE of what the app emits (the main number) ----
    let pool: HashSet<String> = corpus_distinct.iter().map(|t| norm_kw(t)).collect();
    let mut covered = 0;
    let mut missing: Vec<&str> = Vec::new();
    for (kw, _kind) in APP_EMITTED {
        if pool.contains(&norm_kw(kw)) {
            covered += 1;
        } else {
            missing.push(kw);
        }
    }
    println!("\n[A3] APP COVERAGE (the number that matters)");
    println!("    app-emitted keywords: {}", APP_EMITTED.len());
    println!("    with a candidate in the corpus pool: {}", covered);
    println!("    MISSING ({}): {:?}", missing.len(), missing);

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

    // Coverage post-condition preview: every app-emitted keyword must resolve.
    let pool: HashSet<String> = token_homes.keys().map(|t| norm_kw(t)).collect();
    let alias_of: HashMap<&str, &str> = ALIASES.iter().cloned().collect();
    let prose: HashSet<&str> = PROSE_CURATED.iter().cloned().collect();
    let (mut by_seed, mut by_alias, mut by_prose) = (0, 0, 0);
    let mut unresolved: Vec<&str> = Vec::new();
    for (kw, _) in APP_EMITTED {
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
    println!("\n[B3] APP COVERAGE post-condition (either 46/46 or the generator fails)");
    println!("    by home-seed: {by_seed}   by alias: {by_alias}   by prose-curation: {by_prose}");
    println!("    UNRESOLVED (would FAIL generation) ({}): {:?}", unresolved.len(), unresolved);

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

    // App `!`-simple tokens for type inference.
    let app_simple: HashSet<String> =
        APP_EMITTED.iter().filter(|(_, k)| *k == "!").map(|(t, _)| norm_kw(t)).collect();
    let type_of = |tok: &str| -> &'static str {
        if tok.starts_with('%') {
            "block"
        } else if app_simple.contains(&norm_kw(tok)) || title_home.contains(tok) {
            "simple"
        } else {
            "block-option"
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

    // Build entries.
    let mut entries: Vec<serde_json::Value> = Vec::new();
    let mut tokens: Vec<&String> = homes.keys().collect();
    tokens.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then(a.cmp(b)));
    for tok in tokens {
        let idxs = &homes[tok];
        let mut obj = serde_json::Map::new();
        obj.insert("keyword".into(), tok.clone().into());
        obj.insert("provenance".into(), "seeded".into());
        let mut ty = type_of(tok);
        if let Some(aliases) = curated_aliases.get(tok) {
            obj.insert("aliases".into(), serde_json::json!(aliases));
            obj.insert("provenance".into(), "curated".into()); // an alias is a curation act
            // If an alias is an app `!`-simple keyword (M06-L), the entry is simple.
            if aliases.iter().any(|a| app_simple.contains(&norm_kw(a))) {
                ty = "simple";
            }
        }
        obj.insert("type".into(), ty.into());
        if idxs.len() == 1 {
            obj.insert("section".into(), section_key(&all[idxs[0]], nth_of[idxs[0]]));
        } else {
            // AMBIGUOUS: many homes -> targets[]; hover must NOT pick first (next unit).
            let targets: Vec<serde_json::Value> =
                idxs.iter().map(|&i| section_key(&all[i], nth_of[i])).collect();
            obj.insert("targets".into(), serde_json::json!(targets));
        }
        entries.push(serde_json::Value::Object(obj));
    }

    // Curated prose-only entries (not in the structured seed).
    if let Some(i) = scf_tol {
        for kw in PROSE_CURATED {
            entries.push(serde_json::json!({
                "keyword": kw,
                "type": "simple",
                "provenance": "curated",
                "summary": "Tighten SCF convergence thresholds (prose-documented; no keyword table).",
                "section": section_key(&all[i], nth_of[i]),
            }));
        }
    }
    // Keep the whole file deterministically ordered (curated entries included).
    entries.sort_by(|a, b| {
        let ka = a["keyword"].as_str().unwrap();
        let kb = b["keyword"].as_str().unwrap();
        ka.to_lowercase().cmp(&kb.to_lowercase()).then(ka.cmp(kb))
    });

    // ---- HARD post-condition: every app-emitted keyword resolves, or FAIL. ----
    let resolvable: HashSet<String> = entries
        .iter()
        .flat_map(|e| {
            let mut ks = vec![norm_kw(e["keyword"].as_str().unwrap())];
            if let Some(al) = e.get("aliases").and_then(|a| a.as_array()) {
                ks.extend(al.iter().filter_map(|x| x.as_str()).map(norm_kw));
            }
            ks
        })
        .collect();
    let missing: Vec<&str> = APP_EMITTED
        .iter()
        .map(|(k, _)| *k)
        .filter(|k| !resolvable.contains(&norm_kw(k)))
        .collect();
    let ambiguous = entries.iter().filter(|e| e.get("targets").is_some()).count();

    let doc = serde_json::json!({
        "_comment": "SEEDED from the ORCA 6.1 manual structured markup (home mappings only) \
                     by `cargo test generate_keywords_json`; curate on top. \
                     Key = (file, breadcrumb, title, nth). See wiki/modules/manual-keywords.md.",
        "orca_version": version,
        "keywords": entries,
    });
    let out_dir = repo_root().join("src/manual");
    std::fs::create_dir_all(&out_dir).unwrap();
    let out_path = out_dir.join("keywords.json");
    let text = serde_json::to_string_pretty(&doc).unwrap() + "\n";
    std::fs::write(&out_path, &text).unwrap();

    println!("\n{:=<72}", "");
    println!("GENERATED {}", out_path.display());
    println!("    entries: {} ({} ambiguous -> targets[])", doc["keywords"].as_array().unwrap().len(), ambiguous);
    println!("    app coverage: {}/{} ({} missing)", APP_EMITTED.len() - missing.len(), APP_EMITTED.len(), missing.len());
    if !missing.is_empty() {
        println!("    MISSING: {missing:?}");
    }
    println!("{:=<72}", "");
    assert!(scf_tol.is_some(), "SCF Convergence Tolerances home not found — curation target moved");
    assert!(missing.is_empty(), "coverage post-condition FAILED: app keywords with no entry: {missing:?}");
}
