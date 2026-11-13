use craft_anything_formats::dart::string_pool;

#[test]
fn duplicates_at_the_limit_do_not_claim_missing_results() {
    let data = b"package:app/main.dart\0HomeWidget\0package:app/main.dart\0HomeWidget\0";
    let pool = string_pool(data, 1, 1);
    assert_eq!(pool.uris, ["package:app/main.dart"]);
    assert_eq!(pool.tokens, ["HomeWidget"]);
    assert!(!pool.uris_truncated);
    assert!(!pool.tokens_truncated);
    assert_eq!(pool.scanned_bytes, data.len() as u64);
}

#[test]
fn unseen_values_past_each_limit_mark_only_that_collection_partial() {
    let pool = string_pool(b"dart:core\0dart:async\0HomeWidget\0", 1, 2);
    assert_eq!(pool.uris, ["dart:core"]);
    assert_eq!(pool.tokens, ["HomeWidget"]);
    assert!(pool.uris_truncated);
    assert!(!pool.tokens_truncated);

    let pool = string_pool(b"dart:core\0HomeWidget\0OtherWidget\0", 2, 1);
    assert_eq!(pool.uris, ["dart:core"]);
    assert_eq!(pool.tokens, ["HomeWidget"]);
    assert!(!pool.uris_truncated);
    assert!(pool.tokens_truncated);
}

#[test]
fn zero_limits_still_scan_and_report_omitted_values() {
    let data = b"package:app/main.dart\0HomeWidget\0";
    let pool = string_pool(data, 0, 0);
    assert!(pool.uris.is_empty());
    assert!(pool.tokens.is_empty());
    assert!(pool.uris_truncated && pool.tokens_truncated);
    assert_eq!(pool.scanned_bytes, data.len() as u64);
    let empty = string_pool(b"\0\xff\x01", 0, 0);
    assert!(!empty.uris_truncated && !empty.tokens_truncated);
}

#[test]
fn retained_values_keep_first_seen_order_and_uri_runs_stay_out_of_tokens() {
    let data = b"WidgetZulu\0WidgetAlpha\0WidgetZulu\0WidgetBeta\0label package:z/main.dart package:a/main.dart\0dart:core\0";
    let pool = string_pool(data, 2, 2);
    assert_eq!(pool.tokens, ["WidgetZulu", "WidgetAlpha"]);
    assert_eq!(pool.uris, ["package:z/main.dart", "package:a/main.dart"]);
    assert!(pool.tokens_truncated && pool.uris_truncated);
}

/// Run each case in a separate process with a 128 MiB address-space limit. Both inputs fit
/// within the scanner's 64 MiB snapshot region, while the output asks for one entry only.
/// Keeping discarded unique strings in the deduplication set exhausts that limit.
#[test]
#[ignore = "memory regression: run separately with a 128 MiB address-space limit"]
fn millions_of_discarded_tokens_keep_one_result() {
    use std::fmt::Write;
    let mut data = String::with_capacity(36_000_000);
    for i in 0..2_000_000 {
        write!(data, "Candidate_{i:07}\0").unwrap();
    }
    let pool = string_pool(data.as_bytes(), 1, 1);
    assert_eq!(pool.tokens, ["Candidate_0000000"]);
    assert!(pool.tokens_truncated);
    assert!(pool.uris.is_empty());
    assert!(!pool.uris_truncated);
    assert_eq!(pool.scanned_bytes, data.len() as u64);
}

#[test]
#[ignore = "memory regression: run separately with a 128 MiB address-space limit"]
fn millions_of_discarded_uris_keep_one_result() {
    use std::fmt::Write;
    let mut data = String::with_capacity(46_500_000);
    for i in 0..1_500_000 {
        write!(data, "package:app_{i:07}/widget.dart\0").unwrap();
    }
    let pool = string_pool(data.as_bytes(), 1, 1);
    assert_eq!(pool.uris, ["package:app_0000000/widget.dart"]);
    assert!(pool.uris_truncated);
    assert!(pool.tokens.is_empty());
    assert!(!pool.tokens_truncated);
    assert_eq!(pool.scanned_bytes, data.len() as u64);
}
