//! The preflight's pure core (ADR-024 o26.9, o26.13.8). Each gate names the mutation that turns it
//! red; the mutations are run by `run_55_controls.sh` (the unit's control script).

use super::*;

/// memory.md: uni `MemTotal: 65795232 kB` (line 1 of `head -5 /proc/meminfo`, kB = KiB).
const UNI_MEM_KIB: u64 = 65_795_232;
/// memory.md: the laptop's `MemTotal: 15988408 kB`.
const LAPTOP_MEM_KIB: u64 = 15_988_408;

fn local(n: u32) -> RankRule<'static> {
    RankRule::Local { preset_nprocs: n }
}

fn remote(mask: &str) -> RankRule<'_> {
    RankRule::Remote { core_mask: mask }
}

// ---- read_maxcore ------------------------------------------------------------------------------

/// CONTROL: take the first value when there are two (`[i, ..] =>` in `read_maxcore`) → red.
#[test]
fn read_maxcore_reads_one_directive_and_refuses_anything_else() {
    assert_eq!(read_maxcore("! HF\n* xyz 0 1\nH 0 0 0\n*\n"), Ok(None), "absent");
    assert_eq!(read_maxcore("! HF\n%maxcore 3000\n"), Ok(Some(3000)));
    assert_eq!(read_maxcore("! HF\n%MaxCore 2500\n"), Ok(Some(2500)), "any case");
    assert_eq!(read_maxcore("! HF\n  %MAXCORE   1200   # per rank\n"), Ok(Some(1200)), "indent, a trailing comment");
    assert_eq!(read_maxcore("! HF\n# %maxcore 3000\n"), Ok(None), "a commented-out line is no directive");
    assert_eq!(read_maxcore("! HF\n# %maxcore 3000\n%maxcore 2000\n"), Ok(Some(2000)), "only the live line counts");
    for (bad, why) in [
        ("%maxcore 3000\n%maxcore 2000\n", "two lines"),
        ("%maxcore 3000\n%MAXCORE 3000\n", "two lines, even equal"),
        ("%maxcore\n", "no value"),
        ("%maxcore 3000 end\n", "an end"),
        ("%maxcore 3e3\n", "not an integer"),
        ("%maxcore -1\n", "negative"),
        ("%maxcore +5\n", "a sign"),
        ("%maxcore 0\n", "zero"),
        ("%maxcore3000\n", "glued"),
        ("%maxcore 99999999999999999999999\n", "overflow"),
    ] {
        let got = read_maxcore(bad);
        assert!(matches!(&got, Err(e) if e.starts_with("cannot read the input's memory setting")), "{why}: {got:?}");
    }
}

// ---- apply_maxcore -----------------------------------------------------------------------------

/// CONTROLS: a duplicate line (the `[i]` arm inserting instead of replacing) → red; an insert inside
/// the `%pal` block (inserting after the last `%` line, or after the first line that is not `!`) →
/// red.
#[test]
fn apply_maxcore_replaces_or_inserts_after_the_first_bang_line_or_at_the_top() {
    // Replace the single existing line, nothing else changes.
    assert_eq!(apply_maxcore("! HF\n%MaxCore 4000\n* xyz 0 1\n*\n", 2300).unwrap(), "! HF\n%maxcore 2300\n* xyz 0 1\n*\n");
    // A commented-out line is never "replaced": the insert goes after the `!` line, the comment stays.
    assert_eq!(apply_maxcore("! HF\n# %maxcore 3000\n", 1600).unwrap(), "! HF\n%maxcore 1600\n# %maxcore 3000\n");
    // After the FIRST `!` line, before a `%pal` block that follows it.
    let with_pal = "! r2SCAN-3c\n! Opt\n%pal\n  nprocs 4\nend\n* xyz 0 1\nH 0 0 0\n*\n";
    assert_eq!(
        apply_maxcore(with_pal, 2300).unwrap(),
        "! r2SCAN-3c\n%maxcore 2300\n! Opt\n%pal\n  nprocs 4\nend\n* xyz 0 1\nH 0 0 0\n*\n"
    );
    // No `!` line: at the top.
    assert_eq!(apply_maxcore("%pal nprocs 4 end\n* xyz 0 1\n*\n", 900).unwrap(), "%maxcore 900\n%pal nprocs 4 end\n* xyz 0 1\n*\n");
    // The trailing-newline state is kept.
    assert_eq!(apply_maxcore("! HF", 1000).unwrap(), "! HF\n%maxcore 1000");
    // Exactly one directive afterwards, reading the value (the post-condition).
    for input in ["! HF\n%maxcore 4000\n", "! HF\n", "* xyz 0 1\n*\n", ""] {
        let out = apply_maxcore(input, 1700).unwrap();
        assert_eq!(read_maxcore(&out), Ok(Some(1700)), "{input:?}");
        assert_eq!(out.lines().filter(|l| l.to_ascii_lowercase().contains("%maxcore")).count(), 1, "{input:?}");
    }
    // Two lines: no deterministic choice, refused; zero is refused.
    assert!(apply_maxcore("%maxcore 1\n%maxcore 2\n", 1700).is_err());
    assert!(apply_maxcore("! HF\n", 0).is_err());
}

// ---- The estimate ------------------------------------------------------------------------------

/// m explicit or ORCA's 4096; the overheads. CONTROL: drop `+ RANK_OVERHEAD_MIB` or
/// `+ JOB_OVERHEAD_MIB` in `estimate` → red.
#[test]
fn the_estimate_is_n_times_m_plus_100_plus_140() {
    let e = estimate("! r2SCAN-3c Freq\n%maxcore 2000\n", local(8)).unwrap();
    assert_eq!(e, Estimate { ranks: 8, maxcore_mib: 2000, maxcore_from_input: true, estimate_mib: 8 * 2100 + 140 });
    let e = estimate("! r2SCAN-3c Freq\n", local(8)).unwrap();
    assert_eq!(e, Estimate { ranks: 8, maxcore_mib: 4096, maxcore_from_input: false, estimate_mib: 8 * 4196 + 140 });
    assert!(estimate("%maxcore 1\n%maxcore 2\n", local(8)).is_err(), "cannot read → no estimate");
    assert!(estimate("%maxcore 18446744073709551000\n", local(8)).is_err(), "an overflow is no estimate");
}

/// Local n is the preset's count, whatever the input's `%pal` says (align_pal_nprocs rewrites it
/// both ways). CONTROL: use the input's `%pal` locally (`read_pal_nprocs` in the `Local` arm) → red.
#[test]
fn local_ranks_are_the_presets_even_with_a_pal_in_the_input() {
    assert_eq!(ranks("! HF\n%pal nprocs 4 end\n", local(8)), Ok(8));
    assert_eq!(ranks("! HF\n%pal nprocs 16 end\n", local(8)), Ok(8));
    assert_eq!(ranks("! HF\n%pal nprocs x end\n", local(12)), Ok(12), "even an unreadable %pal is rewritten locally");
}

/// Remote n = min(explicit, the mask's distinct CPUs), or the mask without `%pal` (o14.2, through
/// the submit's own align_remote_pal). CONTROL: no min (`n.min(mask_cpus)` → `n`) → red.
#[test]
fn remote_ranks_are_the_pal_capped_by_the_mask() {
    assert_eq!(ranks("! HF\n", remote("0-23")), Ok(24));
    assert_eq!(ranks("! HF\n%pal nprocs 48 end\n", remote("0-23")), Ok(24));
    assert_eq!(ranks("! HF\n%pal nprocs 4 end\n", remote("0-23")), Ok(4));
    assert_eq!(ranks("! HF\n%pal nprocs 4 end\n", remote("0-3,2-5")), Ok(4));
    assert!(ranks("! HF\n%pal nprocs x end\n", remote("0-23")).is_err(), "an unreadable %pal is refused, as the submit does");
}

/// `PALn` with or without `%pal` → the larger. CONTROL: ignore the keyword (`ranks` returns
/// `backend`) → red.
#[test]
fn a_pal_keyword_makes_n_the_larger() {
    assert_eq!(ranks("! HF PAL16\n", local(8)), Ok(16));
    assert_eq!(ranks("! HF pal4\n", local(8)), Ok(8));
    assert_eq!(ranks("! HF PAL32\n%pal nprocs 4 end\n", remote("0-23")), Ok(32));
    assert_eq!(ranks("! HF\n! PAL8 Opt\n", remote("0-3")), Ok(8), "on any ! line");
    assert_eq!(ranks("! HF # PAL32\n", local(8)), Ok(8), "a comment is no keyword");
    assert_eq!(ranks("! HF PALX PAL\n", local(8)), Ok(8));
}

/// F2 (verifier, Part A): several `PALn` tokens → the largest, on one line or across `!` lines.
/// CONTROL: `.min()` in `read_pal_keyword` → red.
#[test]
fn several_pal_keywords_take_the_largest() {
    assert_eq!(ranks("! HF PAL4 PAL16\n", local(1)), Ok(16));
    assert_eq!(ranks("! HF PAL16 PAL4\n", local(1)), Ok(16));
    assert_eq!(ranks("! HF PAL2\n! Opt PAL8\n", local(1)), Ok(8));
}

// ---- Thresholds and P --------------------------------------------------------------------------

/// The uni builder default (24 × 2000 → 49.4 GiB) does not warn at 0.9 × 62.75 GiB; a hand input
/// without `%maxcore` (24 × 4096 → 98.5 GiB) does. CONTROL: `RAM_SHARE` 0.75 → red.
#[test]
fn uni_default_fits_and_a_hand_input_without_maxcore_warns() {
    let builder = estimate("! r2SCAN-3c Opt\n%maxcore 2000\n", remote("0-23")).unwrap();
    assert_eq!(builder.estimate_mib, 50_540);
    assert!(!ram_warns(builder.estimate_mib, UNI_MEM_KIB));
    let hand = estimate("! r2SCAN-3c Opt\n", remote("0-23")).unwrap();
    assert_eq!(hand.estimate_mib, 100_844);
    assert!(ram_warns(hand.estimate_mib, UNI_MEM_KIB));
    assert_eq!(ram_threshold_mib(UNI_MEM_KIB), 57_827, "0.9 × 64253.2 MiB, rounded down");
}

/// U = the threshold does not warn, one MiB above does (`>`); free disk = 10 GiB does not warn, a
/// byte less does (`<`). CONTROLS: `>=` in `ram_warns`, `<=` in `disk_warns` → red.
#[test]
fn the_boundaries_are_strict() {
    let t = ram_threshold_mib(UNI_MEM_KIB);
    assert!(!ram_warns(t, UNI_MEM_KIB));
    assert!(ram_warns(t + 1, UNI_MEM_KIB));
    assert!(!disk_warns(10 * 1024 * 1024 * 1024));
    assert!(disk_warns(10 * 1024 * 1024 * 1024 - 1));
    // memory.md: uni's root has 78 360 924 160 bytes free, the laptop's data dir 101 075 353 600.
    assert!(!disk_warns(78_360_924_160) && !disk_warns(101_075_353_600));
}

/// P's worked values (o26.4) and its rounding down to the step; P < 300 → no proposal; the local
/// default's fallbacks (o26.13.4). CONTROL: round to nearest instead of down → red.
#[test]
fn the_proposal_rounds_down_to_the_step_and_stops_below_300() {
    assert_eq!(proposal_mib(UNI_MEM_KIB, 24), Some(2300));
    assert_eq!(proposal_mib(LAPTOP_MEM_KIB, 8), Some(1600));
    // (0.9 × 15613.7 − 140) / 12 − 100 = 1059.3 → 1000, never 1100.
    assert_eq!(raw_proposal_mib(LAPTOP_MEM_KIB, 12), 1000);
    // 1 GiB host at 8 ranks: (921 − 140) / 8 − 100 = −2.4 → −100 → nothing proposed.
    assert_eq!(proposal_mib(1024 * 1024, 8), None);
    assert_eq!(raw_proposal_mib(1024 * 1024, 8), -100);
    // P exactly 300 is proposed; just below it is not. Budget for n = 1: threshold − 140 − 100.
    let kib_for = |threshold_mib: u64| (threshold_mib * 10240).div_ceil(9);
    assert_eq!(proposal_mib(kib_for(540), 1), Some(300));
    assert_eq!(proposal_mib(kib_for(539), 1), None);
    // The proposal fits: n × (P + 100) + 140 never warns.
    for (kib, n) in [(UNI_MEM_KIB, 24), (LAPTOP_MEM_KIB, 8), (LAPTOP_MEM_KIB, 12), (UNI_MEM_KIB, 48)] {
        let p = proposal_mib(kib, n).unwrap();
        assert!(!ram_warns(u64::from(n) * (p + 100) + 140, kib), "{kib} {n}");
    }
    assert_eq!(fitted_default_mib(None, 8), 2000, "unreadable host: today's value");
    assert_eq!(fitted_default_mib(Some(1024 * 1024), 8), 300, "P < 300 yields 300");
    assert_eq!(fitted_default_mib(Some(LAPTOP_MEM_KIB), 8), 1600);
}

/// The view's check: unknown facts are unknown (no comparison, no P), never 0; an input that cannot
/// be read has no estimate and no P; P < 300 says so. CONTROL: an unknown RAM read as 0 (the
/// `Fact::Unknown` arm → `Known { value: 0 }`) → red.
#[test]
fn a_check_with_unknown_facts_compares_nothing() {
    let est = estimate("! HF\n", remote("0-23"));
    let unknown = check(est.clone(), Fact::Unknown { reason: "not measured".into() }, Fact::Unknown { reason: "not measured".into() }, None);
    assert_eq!(unknown.ram, RamCheck::Unknown { reason: "not measured".into() });
    assert_eq!(unknown.disk, DiskCheck::Unknown { reason: "not measured".into() });
    assert_eq!((unknown.proposal_mib, unknown.too_small_for_ranks, unknown.warns), (None, false, false));

    let known = check(est, Fact::Known { value: UNI_MEM_KIB }, Fact::Known { value: 5 << 30 }, Some("2026-11-18 10:00:00".into()));
    assert_eq!(known.ram, RamCheck::Known { mem_total_kib: UNI_MEM_KIB, threshold_mib: 57_827, warns: Some(true) });
    assert!(matches!(known.disk, DiskCheck::Known { warns: true, .. }));
    assert_eq!(known.proposal_mib, Some(2300));
    assert!(known.warns);

    let unreadable = check(estimate("%maxcore x\n", remote("0-23")), Fact::Known { value: UNI_MEM_KIB }, Fact::Known { value: 1 << 40 }, None);
    assert!(matches!(unreadable.estimate, EstimateState::NotEstimated { .. }));
    assert_eq!(unreadable.ram, RamCheck::Known { mem_total_kib: UNI_MEM_KIB, threshold_mib: 57_827, warns: None });
    assert_eq!(unreadable.proposal_mib, None);

    let tiny = check(estimate("! HF\n", local(8)), Fact::Known { value: 1024 * 1024 }, Fact::Known { value: 1 << 40 }, None);
    assert_eq!((tiny.proposal_mib, tiny.too_small_for_ranks), (None, true));
}

#[test]
fn checks_serialize_with_their_units_in_the_names() {
    let c = check(estimate("! HF\n%maxcore 2000\n", local(8)), Fact::Known { value: LAPTOP_MEM_KIB }, Fact::Unknown { reason: "statvfs: x".into() }, None);
    let json = serde_json::to_value(&c).unwrap();
    assert_eq!(json["estimate"], serde_json::json!({"state": "estimated", "ranks": 8, "maxcore_mib": 2000, "maxcore_from_input": true, "estimate_mib": 16940}));
    assert_eq!(json["ram"], serde_json::json!({"state": "known", "mem_total_kib": LAPTOP_MEM_KIB, "threshold_mib": 14052, "warns": true}));
    assert_eq!(json["disk"], serde_json::json!({"state": "unknown", "reason": "statvfs: x"}));
    assert_eq!(json["proposal_mib"], 1600);
    assert_eq!(json["facts_measured_at"], serde_json::Value::Null);
}

// ---- Host facts --------------------------------------------------------------------------------

/// The measured formats (memory.md): `head -5 /proc/meminfo` and `df --output=avail,target -B1`.
/// Only the values memory.md records are used; MemFree/Buffers/Cached are this laptop's own
/// `head -5` lines of 2026-11-18, kept to give the five-line shape.
pub(crate) const MEMINFO_LAPTOP: &str = "MemTotal:       15988408 kB\nMemFree:         2683044 kB\nMemAvailable:   10368556 kB\nBuffers:          492096 kB\nCached:          6968408 kB\n";
pub(crate) const DF_UNI: &str = "      Avail Mounted on\n78360924160 /home\n";
const DF_LAPTOP: &str = "       Avail Mounted on\n101075353600 /\n";

#[test]
fn the_host_fact_parsers_read_the_measured_formats_strictly() {
    assert_eq!(meminfo_kib(MEMINFO_LAPTOP, "MemTotal"), Ok(15_988_408));
    assert_eq!(meminfo_kib(MEMINFO_LAPTOP, "MemAvailable"), Ok(10_368_556));
    assert_eq!(meminfo_kib("MemTotal:       65795232 kB\n", "MemTotal"), Ok(UNI_MEM_KIB));
    for bad in [
        "",
        "MemTotal:       15988408\n",
        "MemTotal:       15988408 MB\n",
        "MemTotal:       15988408 kB\nMemTotal:       1 kB\n",
        "MemTotal:       -1 kB\n",
        "MemTotal:       1.5 kB\n",
        "memtotal:       15988408 kB\n",
    ] {
        assert!(meminfo_kib(bad, "MemTotal").is_err(), "{bad:?}");
    }
    assert!(meminfo_kib(MEMINFO_LAPTOP, "MemTotalX").is_err());

    assert_eq!(parse_df_avail(DF_UNI), Ok(DfRow { avail_bytes: 78_360_924_160, target: "/home".into() }));
    assert_eq!(parse_df_avail(DF_LAPTOP), Ok(DfRow { avail_bytes: 101_075_353_600, target: "/".into() }));
    for bad in [
        "",
        "78360924160 /home\n",
        "      Avail Mounted on\n",
        "      Avail Mounted on\n78360924160 /home\n1 /\n",
        // `df -P -B1`'s six-column POSIX shape is not this format.
        "Filesystem 1-blocks Used Available Capacity Mounted on\n/dev/sdb4 1 2 78360924160 1% /home\n",
        "      Avail Mounted on\n78.3G /home\n",
        "      Avail Mounted on\n78360924160\n",
    ] {
        assert!(parse_df_avail(bad).is_err(), "{bad:?}");
    }
}

/// `host_resources` behind its path seam (o26.13.8): a fixture `/proc/meminfo`, and `statvfs` of a
/// tmpdir, cross-checked against `df --output=avail -B1` of the same dir (df's Avail is
/// `f_bavail × f_frsize`; the 5 % root reserve makes `f_bfree` gigabytes larger on ext4). CONTROL:
/// `f_bfree` instead of `f_bavail` → red (on a filesystem with a reserve, as the dev laptop's `/`).
#[test]
fn host_resources_reads_the_seam_and_statvfs_matches_df() {
    let dir = std::env::temp_dir().join(format!("orcastudio-hostres-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let meminfo = dir.join("meminfo");
    std::fs::write(&meminfo, MEMINFO_LAPTOP).unwrap();

    let r = host_resources_at(&meminfo, &dir);
    assert_eq!(r.mem_total_kib, Fact::Known { value: 15_988_408 });
    let avail = r.disk_avail_bytes.value().expect("statvfs of a tmpdir");
    let df = std::process::Command::new("df").args(["--output=avail,target", "-B1"]).arg(&dir).output().unwrap();
    let df = parse_df_avail(&String::from_utf8(df.stdout).unwrap()).unwrap().avail_bytes;
    assert!(avail.abs_diff(df) < 256 << 20, "statvfs {avail} vs df {df}: more than 256 MiB apart");

    // Unreadable facts are unknown with a reason, never 0.
    let missing = host_resources_at(&dir.join("no-such"), &dir.join("no-such-dir"));
    assert!(matches!(missing.mem_total_kib, Fact::Unknown { ref reason } if reason.contains("no-such")));
    assert!(matches!(missing.disk_avail_bytes, Fact::Unknown { ref reason } if reason.starts_with("statvfs")));
    std::fs::write(&meminfo, "MemTotal: 1 MB\n").unwrap();
    assert!(matches!(host_resources_at(&meminfo, &dir).mem_total_kib, Fact::Unknown { .. }));
    std::fs::remove_dir_all(&dir).ok();
}

// ---- The submit-time notice --------------------------------------------------------------------

fn facts(total: Option<u64>, avail: Option<u64>, disk: Option<u64>) -> SubmitHostFacts {
    SubmitHostFacts { mem_total_kib: total, mem_available_kib: avail, root_avail_bytes: disk }
}

/// o26.13.2: with no other live job of ours, U is compared with MemAvailable; with one, with
/// MemTotal (MemAvailable as context). CONTROL: always MemAvailable → red (the queue-two-jobs case
/// warns).
#[test]
fn the_submit_notice_compares_with_available_alone_and_with_total_behind_our_own_job() {
    let builder = estimate("! r2SCAN-3c Opt\n%maxcore 2000\n", remote("0-23"));
    // 49.4 GiB estimate; 40 GiB available now (our first job runs), 62.7 GiB in total.
    let busy = facts(Some(UNI_MEM_KIB), Some(40 << 20), Some(78_360_924_160));
    let alone = submit_memory_notice(&builder, &busy, 0).expect("alone: compared with the 40 GiB available");
    assert!(alone.contains("above 0.9 × the server's 40.0 GiB available"), "{alone}");
    assert_eq!(submit_memory_notice(&builder, &busy, 1), None, "behind our own job: compared with MemTotal, fits");

    let hand = estimate("! r2SCAN-3c Opt\n", remote("0-23"));
    let behind = submit_memory_notice(&hand, &busy, 1).expect("98.5 GiB > 0.9 × MemTotal");
    assert!(behind.contains("in total") && behind.contains("40.0 GiB available now, with 1 of our jobs"), "{behind}");
    assert!(behind.contains("%maxcore 2300 fits 24 ranks"), "{behind}");
    assert!(behind.contains("ORCA's default %maxcore") && behind.contains("not an upper bound"), "{behind}");

    let free = facts(Some(UNI_MEM_KIB), Some(UNI_MEM_KIB - (1 << 20)), Some(78_360_924_160));
    assert_eq!(submit_memory_notice(&builder, &free, 0), None, "everything fits: no notice");
}

/// Unknown facts and an unreadable input are said, never silently passed; a small disk warns.
#[test]
fn the_submit_notice_says_what_it_could_not_check() {
    let builder = estimate("! HF\n%maxcore 2000\n", remote("0-23"));
    let none = submit_memory_notice(&builder, &facts(None, None, None), 0).unwrap();
    assert!(none.contains("memory not checked: the server's available memory could not be read"), "{none}");
    assert!(none.contains("free disk not checked"), "{none}");
    let behind = submit_memory_notice(&builder, &facts(None, Some(1 << 30), Some(1 << 40)), 2).unwrap();
    assert!(behind.contains("the server's total memory could not be read"), "{behind}");
    let unreadable = submit_memory_notice(&estimate("%maxcore 1\n%maxcore 2\n", remote("0-23")), &facts(Some(UNI_MEM_KIB), Some(UNI_MEM_KIB), Some(1 << 40)), 0).unwrap();
    assert!(unreadable.contains("memory not checked: cannot read the input's memory setting"), "{unreadable}");
    let disk = submit_memory_notice(&builder, &facts(Some(UNI_MEM_KIB), Some(UNI_MEM_KIB), Some(5 << 30)), 0).unwrap();
    assert!(disk.contains("disk: 5.0 GiB free on the server's job root, below 10.0 GiB"), "{disk}");
}
