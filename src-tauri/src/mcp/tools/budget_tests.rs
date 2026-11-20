//! Tests for the shared size machinery (M2 amendment, item 7), the refusal text bound, and the
//! unit-name walk the other tests rely on.

use serde_json::json;

use super::test_support::unitless_numeric_keys;
use super::{clip, echo, page_limit, size_of_text, within_cap, ToolError, ECHO_MAX_BYTES, REFUSAL_MAX_BYTES, RESULT_CAP_BYTES};

/// The caps count UTF-8 bytes: what the client counts for non-ASCII text is unmeasured (M0 was
/// ASCII-only), and bytes are at least every candidate (characters, UTF-16 units).
#[test]
fn sizes_are_counted_in_utf8_bytes() {
    assert_eq!(size_of_text("abc"), 3);
    assert_eq!(size_of_text("Å°"), 4);
    assert_eq!(size_of_text("енергія"), 14);
    assert_eq!(size_of_text("😀"), 4);
}

/// A non-ASCII result under the cap in UTF-16 units (and in characters) but over it in bytes is
/// refused. NEGATIVE CONTROL (`ctl:cap_bytes`): count UTF-16 units instead → red.
#[test]
fn a_non_ascii_result_over_the_byte_cap_is_refused() {
    let text = "я".repeat(30_000); // 30,002 UTF-16 units with the quotes; 60,002 bytes.
    assert!(text.encode_utf16().count() + 2 < RESULT_CAP_BYTES);
    match within_cap(text, RESULT_CAP_BYTES) {
        Err(ToolError::TooLarge { bytes: 60_002, cap: RESULT_CAP_BYTES }) => {}
        other => panic!("[ctl:cap_bytes] over the cap in bytes must be refused: {:?}", other.map(|t| t.len())),
    }
}

/// The post-condition refuses a result over its cap, and serves one at it. NEGATIVE CONTROL
/// (`ctl:within_cap`): never refuse → red.
#[test]
fn a_result_over_the_cap_is_refused_never_served() {
    let text = "x".repeat(100);
    // `"xxx…"`: 100 bytes plus the two quotes.
    assert!(within_cap(text.clone(), 102).is_ok(), "a result at its cap is served");
    match within_cap(text, 101) {
        Err(ToolError::TooLarge { bytes: 102, cap: 101 }) => {}
        other => panic!("[ctl:within_cap] a result over its cap must be refused: {other:?}"),
    }
}

/// `clip` budgets the serialised bytes, escapes included: a field of control characters (6 bytes
/// each as `\u00XX`) is cut, not passed whole to be refused `TooLarge`. NEGATIVE CONTROL
/// (`ctl:clip_escapes`): count raw bytes → red.
#[test]
fn clip_counts_serialised_bytes_and_says_so() {
    assert_eq!(clip("abcdef", 6), ("abcdef".to_string(), false));
    assert_eq!(clip("abcdefg", 6), ("abc…".to_string(), true));
    let (cut, was_cut) = clip("енергія", 8);
    assert!(was_cut && cut.len() <= 8, "{cut}");
    let control = "\u{1}".repeat(1_000);
    let (cut, was_cut) = clip(&control, 600);
    let serialised = serde_json::to_string(&cut).unwrap();
    assert!(was_cut && serialised.len() <= 602, "[ctl:clip_escapes] {} bytes serialised", serialised.len());
}

#[test]
fn a_page_limit_is_clamped_to_one_through_the_maximum() {
    assert_eq!(page_limit(None, 20, 40), 20);
    assert_eq!(page_limit(Some(0), 20, 40), 1);
    assert_eq!(page_limit(Some(1000), 20, 40), 40);
}

/// Refusal text is bounded: echoed input to [`ECHO_MAX_BYTES`], the whole message to
/// [`REFUSAL_MAX_BYTES`]. NEGATIVE CONTROLS: drop the echo cut (`ctl:refusal_echo`) or the message
/// cut (`ctl:refusal_cap`) → red.
#[test]
fn refusal_text_never_grows_with_the_input() {
    let db = crate::mcp::test_db::TestDb::new("tools-refusal");
    let long = "x".repeat(10_000);
    // Echoed through `echo`: a status filter outside its set.
    let filter = super::jobs::JobFilter { status: Some(long.clone()), ..Default::default() };
    match super::jobs::list_jobs(&db.conn, &filter, None, None) {
        Err(ToolError::Refused(why)) => {
            assert!(!why.contains(&"x".repeat(ECHO_MAX_BYTES + 1)), "[ctl:refusal_echo] {} bytes", why.len())
        }
        other => panic!("refused: {other:?}"),
    }
    // Echoed by a reused reader (the resolver's "not found: job …"), bounded by the message cut.
    match super::geometry::get_geometry(&db.conn, &format!("job:{long}")) {
        Err(ToolError::Refused(why)) => assert!(why.len() <= REFUSAL_MAX_BYTES, "[ctl:refusal_cap] {} bytes", why.len()),
        other => panic!("refused: {other:?}"),
    }
    assert_eq!(echo("short"), "short");
}

/// The unit walk sees numbers at any depth of nested arrays. NEGATIVE CONTROL (`ctl:walk_nested`):
/// only flat arrays count as numeric → red.
#[test]
fn the_unit_walk_flags_nested_numeric_arrays() {
    let v = json!({"positions": [[0, 1, 2]], "xyz_ang": [[0.0, 1.0]], "name": "x", "rows": [{"energy_eh": 1.0}]});
    assert_eq!(unitless_numeric_keys(&v, &[]), vec![".positions".to_string()], "[ctl:walk_nested]");
    let flat = json!({"freqs": [1.0, 2.0], "count": 3});
    assert_eq!(unitless_numeric_keys(&flat, &[]), vec![".count".to_string(), ".freqs".to_string()]);
}
