// Part A2 of unit 1c — constraint-value formatter parity (reader half).
//
// Reads <16-hex-u64><TAB><String(v)> and checks Rust `format!("{}", x)` against
// the JS `String(v)` for the same f64 (reconstructed from bits, never a decimal
// string). Reports divergences with samples.
//
//   rustc -O scripts/value_parity_reader.rs -o /tmp/vpr && /tmp/vpr <corpus>

use std::fs::File;
use std::io::{BufRead, BufReader};

fn main() {
    let path = std::env::args().nth(1).expect("usage: value_parity_reader <corpus>");
    let rdr = BufReader::new(File::open(&path).expect("open corpus"));

    let (mut n, mut div) = (0u64, 0u64);
    let mut samples: Vec<String> = Vec::new();

    for line in rdr.lines() {
        let line = line.unwrap();
        if line.is_empty() {
            continue;
        }
        let (hex, js) = line.split_once('\t').expect("tab");
        let x = f64::from_bits(u64::from_str_radix(hex, 16).expect("hex"));
        let rust = format!("{}", x);
        n += 1;
        if rust != js {
            div += 1;
            if samples.len() < 15 {
                samples.push(format!("bits=0x{hex}  String(v)={js:?}  rust{{}}={rust:?}"));
            }
        }
    }

    println!("constraint values compared : {n}");
    println!("String(v) vs rust {{}} div.  : {div}");
    for s in &samples {
        println!("  {s}");
    }
    println!(
        "\nVERDICT: {}",
        if div == 0 {
            "fmt_value = format!(\"{}\") matches String(v) across the constraint range"
        } else {
            "DIVERGENCE — STOP, report bits"
        }
    );
}
