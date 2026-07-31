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

use super::objects_inv;
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

/// Collect `(file_id, contents)` for every leaf. `file_id` is the path relative to
/// the version dir with `.md.txt` stripped and forward slashes — exactly the form
/// an `objects.inv` uri uses (`contents/essentialelements/RI`).
fn collect_leaves(dir: &Path, base: &Path, out: &mut Vec<(String, String)>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            collect_leaves(&p, base, out);
        } else if p.to_string_lossy().ends_with(".md.txt") {
            let rel = p.strip_prefix(base).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            let id = rel.strip_suffix(".md.txt").unwrap_or(&rel).to_string();
            if let Ok(text) = std::fs::read_to_string(&p) {
                out.push((id, text));
            }
        }
    }
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
