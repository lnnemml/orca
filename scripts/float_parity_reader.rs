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

    let (mut n, mut fixed_div, mut pad_div, mut sign_zero, mut rounding) = (0u64, 0u64, 0u64, 0u64, 0u64);
    let mut sign_samples: Vec<String> = Vec::new();
    let mut round_samples: Vec<String> = Vec::new();

    for line in rdr.lines() {
        let line = line.unwrap();
        if line.is_empty() {
            continue;
        }
        let mut it = line.split('\t');
        let hex = it.next().unwrap();
        let js_fixed = it.next().unwrap();
        let js_pad = it.next().unwrap();

        let bits = u64::from_str_radix(hex, 16).expect("hex u64");
        let x = f64::from_bits(bits);
        let rust_fixed = format!("{:.8}", x);
        let rust_pad = format!("{:>14.8}", x);

        n += 1;
        if rust_fixed != js_fixed {
            fixed_div += 1;
            let sample = format!(
                "bits=0x{hex}  js.toFixed(8)={js_fixed:?}  rust{{:.8}}={rust_fixed:?}"
            );
            if is_sign_of_zero(js_fixed, &rust_fixed) {
                sign_zero += 1;
                if sign_samples.len() < 5 {
                    sign_samples.push(sample);
                }
            } else {
                rounding += 1;
                if round_samples.len() < 10 {
                    round_samples.push(sample);
                }
            }
        }
        if rust_pad != js_pad {
            pad_div += 1;
        }
    }

    println!("corpus doubles compared : {n}");
    println!("toFixed(8) divergences  : {fixed_div}  (sign-of-zero {sign_zero}, rounding {rounding})");
    println!("padded formatCoord div. : {pad_div}");
    if !sign_samples.is_empty() {
        println!("\n-- sign-of-zero samples --");
        for s in &sign_samples {
            println!("  {s}");
        }
    }
    if !round_samples.is_empty() {
        println!("\n-- ROUNDING samples (the dangerous class) --");
        for s in &round_samples {
            println!("  {s}");
        }
    }
    println!(
        "\nVERDICT: {}",
        if rounding == 0 {
            "no ROUNDING divergence — golden byte-identity is safe for the coordinate formatter (modulo the sign-of-zero rule below)"
        } else {
            "ROUNDING divergence present — STOP, golden byte-identity is NOT safe as-is"
        }
    );
}
