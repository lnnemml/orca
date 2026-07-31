//! Corpus gate for the manual sectioner (precedent: `orca_plot.rs`). Reads the
//! locally fetched `resources/manual/<version>/**.md.txt` + `objects.inv`, runs the
//! three post-conditions over all 126 leaves, and PRINTS the report whose numbers
//! unit 4.3 will turn into the FTS schema. Ignored by default:
//!
//!     cargo test manual_corpus -- --ignored --nocapture
//!
//! The plain `cargo test` run relies on the small in-code fixtures in `sections.rs`
//! and `objects_inv.rs` (no network, no corpus).

use std::collections::HashMap;
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
