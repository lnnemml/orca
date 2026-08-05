// Part A of Phase 4.2 unit 1c — the float-formatting parity PROBE (reader half).
//
// Reads the corpus written by scripts/float-parity-corpus.mjs. Each line is
//   <16-hex-u64><TAB><js toFixed(8)><TAB><js formatCoord = toFixed(8).padStart(14)>
// The double is reconstructed EXACTLY from its bit pattern (f64::from_bits) — never
// from a decimal string — and formatted with Rust `{:.8}` / `{:>14.8}`, then
// compared byte-for-byte against the JS output. Counts and samples divergences,
// split into "sign-of-zero" (a value that rounds to zero, differing only by a '-')
// and "rounding" (everything else — the dangerous class for golden byte-identity).
//
// Standalone on purpose: Part A runs BEFORE the orcastudio-core crate exists.
//   rustc -O scripts/float_parity_reader.rs -o /tmp/fpr && /tmp/fpr <corpus>

use std::fs::File;
use std::io::{BufRead, BufReader};

/// The coordinate formatter that `orcastudio-core::emit::fmt_coord` implements —
/// byte-identical to JS `n.toFixed(8).padStart(14)` (mergeToAtomLines). Three parts
/// (unit 1c Part A2, architect's rule):
///  1. signed zero: map -0.0 -> +0.0 (JS toFixed drops the sign; Rust keeps it).
///     `x == 0.0` is true for BOTH ±0.0 and nothing else, so -1e-12 stays
///     "-0.00000000" (it is not zero).
///  2. exact 8th-decimal halves (x = odd/512): JS rounds half AWAY, Rust {:.8}
///     rounds half to EVEN. Detect with y = |x|*512 — multiplication by a power of
///     two never rounds, so no false positives; tie iff y is an odd integer (guarded
///     below 2^53). On a tie, render away-from-zero: m = floor(|x|*1e8)+1 (|x|*1e8
///     is a half-integer < 2^53, exact; +1 after floor = the larger n).
///  3. everything else: plain {:.8}. Then padStart(14).
fn fmt_coord(x: f64) -> String {
    let x = if x == 0.0 { 0.0 } else { x }; // (1) signed zero
    let ax = x.abs();
    let y = ax * 512.0; // exact (power-of-two scaling)
    let is_tie = y < 9_007_199_254_740_992.0 /* 2^53 */ && y.fract() == 0.0 && (y as u64) % 2 == 1;
    let core = if is_tie {
        let m = (ax * 1e8).floor() as u64 + 1; // away from zero
        let sign = if x.is_sign_negative() { "-" } else { "" };
        format!("{}{}.{:08}", sign, m / 100_000_000, m % 100_000_000)
    } else {
        format!("{:.8}", x)
    };
    format!("{:>14}", core) // padStart(14)
}

fn strip_leading_minus(s: &str) -> &str {
    s.strip_prefix('-').unwrap_or(s)
}

/// A "sign-of-zero" difference: the two strings are equal after removing a single
/// leading '-', and the numeric magnitude is zero (all digits 0). This isolates
/// the -0.0 class (Rust keeps the sign, JS toFixed drops it) from real rounding.
fn is_sign_of_zero(a: &str, b: &str) -> bool {
    let (na, nb) = (strip_leading_minus(a.trim()), strip_leading_minus(b.trim()));
    na == nb && na.chars().all(|c| c == '0' || c == '.')
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: float_parity_reader <corpus>");
    let f = File::open(&path).expect("open corpus");
    let rdr = BufReader::new(f);

    // Two comparisons against the JS reference (padStart(14) formatCoord):
    //  * BARE {:>14.8}  — the negative control: how many the naive formatter gets
    //    wrong. Split into sign-of-zero vs rounding(tie) so the two classes are
    //    named separately.
    //  * fmt_coord      — the shipping rule: MUST be 0.
    let (mut n, mut bare_div, mut bare_sign, mut bare_round, mut coord_div) =
        (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut sign_samples: Vec<String> = Vec::new();
    let mut round_samples: Vec<String> = Vec::new();
    let mut coord_samples: Vec<String> = Vec::new();

    for line in rdr.lines() {
        let line = line.unwrap();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split('\t');
        let hex = it.next().unwrap();
        let _js_fixed = it.next().unwrap();
        let js_pad = it.next().unwrap(); // formatCoord = toFixed(8).padStart(14)

        let bits = u64::from_str_radix(hex, 16).expect("hex u64");
        let x = f64::from_bits(bits);
        let bare = format!("{:>14.8}", x);
        let fixed_rule = fmt_coord(x);

        n += 1;
        if bare != js_pad {
            bare_div += 1;
            let sample = format!("bits=0x{hex}  js={js_pad:?}  bare{{:>14.8}}={bare:?}");
            // classify on the unpadded cores
            if is_sign_of_zero(js_pad.trim(), bare.trim()) {
                bare_sign += 1;
                if sign_samples.len() < 5 {
                    sign_samples.push(sample);
                }
            } else {
                bare_round += 1;
                if round_samples.len() < 10 {
                    round_samples.push(sample);
                }
            }
        }
        if fixed_rule != js_pad {
            coord_div += 1;
            if coord_samples.len() < 10 {
                coord_samples.push(format!(
                    "bits=0x{hex}  js={js_pad:?}  fmt_coord={fixed_rule:?}"
                ));
            }
        }
    }

    println!("corpus doubles compared        : {n}");
    println!("BARE {{:>14.8}} vs JS formatCoord : {bare_div}  (sign-of-zero {bare_sign}, rounding/tie {bare_round})");
    println!("fmt_coord vs JS formatCoord     : {coord_div}");
    if !sign_samples.is_empty() {
        println!("\n-- BARE sign-of-zero samples --");
        for s in &sign_samples {
            println!("  {s}");
        }
    }
    if !round_samples.is_empty() {
        println!("\n-- BARE rounding/tie samples (the dangerous class the naive formatter gets WRONG) --");
        for s in &round_samples {
            println!("  {s}");
        }
    }
    if !coord_samples.is_empty() {
        println!("\n-- fmt_coord FAILURES (should be none) --");
        for s in &coord_samples {
            println!("  {s}");
        }
    }
    println!(
        "\nVERDICT: {}",
        if coord_div == 0 {
            "fmt_coord is byte-identical to JS across the corpus; the bare formatter is NOT (count above) — that gap is why fmt_coord exists"
        } else {
            "fmt_coord DIVERGES — rule is wrong, STOP"
        }
    );
}
