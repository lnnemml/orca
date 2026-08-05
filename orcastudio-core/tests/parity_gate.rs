//! Permanent `#[ignore]` parity gate (unit 1c Part A/A2). The routine `cargo test`
//! exercises the adversarial values through the committed golden fixtures; THIS gate
//! re-checks `fmt_coord` against the JS reference over the whole ~1M-double corpus.
//! It is `#[ignore]` because the corpus is Node-generated (not committed). Run when
//! the Rust or Node toolchain changes:
//!
//!   node scripts/float-parity-corpus.mjs scripts/_float-parity-corpus.txt
//!   cargo test -p orcastudio-core --test parity_gate -- --ignored --nocapture
//!
//! See `wiki/architecture/float-formatting-parity.md`.

use orcastudio_core::emit::fmt_coord;
use std::fs;

#[test]
#[ignore = "needs the Node-generated corpus; run by hand after regenerating it"]
fn fmt_coord_matches_js_over_the_full_corpus() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../scripts/_float-parity-corpus.txt"
    );
    let text = fs::read_to_string(path)
        .expect("corpus missing — run: node scripts/float-parity-corpus.mjs scripts/_float-parity-corpus.txt");

    let (mut n, mut div) = (0u64, 0u64);
    let mut sample: Option<String> = None;
    for line in text.lines() {
        if line.is_empty() {
            continue;
        }
        let mut it = line.split('\t');
        let hex = it.next().unwrap();
        let _js_fixed = it.next().unwrap();
        let js_pad = it.next().unwrap(); // formatCoord = toFixed(8).padStart(14)
        let x = f64::from_bits(u64::from_str_radix(hex, 16).unwrap());
        n += 1;
        if fmt_coord(x) != js_pad {
            div += 1;
            if sample.is_none() {
                sample = Some(format!("bits=0x{hex} js={js_pad:?} fmt_coord={:?}", fmt_coord(x)));
            }
        }
    }
    println!("corpus {n} doubles, fmt_coord divergences {div}");
    assert_eq!(div, 0, "fmt_coord diverged {div}/{n}; first: {sample:?}");
}
