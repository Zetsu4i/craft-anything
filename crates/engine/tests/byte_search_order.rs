use craft_anything_engine::{CallOptions, Caller, Engine};
use serde_json::{Value, json};

const CAP: usize = 100_000;

fn search(data: &[u8], mut params: Value) -> Value {
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("matches.bin");
    std::fs::write(&path, data).unwrap();
    params["path"] = json!(path);
    let engine = Engine::new(craft_anything_engine::testing::session());
    let result = engine.call("native_bytes_find", params, CallOptions::new(Caller::Cli)).unwrap().to_json();
    std::fs::remove_dir_all(directory).unwrap();
    result
}

#[test]
fn early_text_survives_later_pattern_match_cap() {
    let mut bytes = b"needle".to_vec();
    bytes.resize(bytes.len() + CAP, 0xaa);
    let v = search(&bytes, json!({"pattern": "aa", "text": "needle", "encoding": "utf8", "limit": 2}));
    assert_eq!(v["result"]["matches"][0]["file_offset"], 0, "{v}");
    assert_eq!(v["result"]["matches"][0]["matched"], "utf8");
    assert_eq!(v["result"]["matches"][1]["file_offset"], 6);
    assert_eq!(v["result"]["total_matches"], CAP);
    assert_eq!(v["result"]["match_cap_reached"], true);
    assert_eq!(v["coverage"]["status"], "partial");
    assert!(v["coverage"]["total"].is_null());
}

#[test]
fn early_wide_text_survives_later_narrow_match_cap() {
    let mut bytes: Vec<u8> = "word".encode_utf16().flat_map(u16::to_le_bytes).collect();
    for _ in 0..CAP {
        bytes.extend_from_slice(b"word");
    }
    let v = search(&bytes, json!({"text": "word", "limit": 2}));
    assert_eq!(v["result"]["matches"][0]["file_offset"], 0, "{v}");
    assert_eq!(v["result"]["matches"][0]["matched"], "utf16le");
    assert_eq!(v["result"]["matches"][1]["file_offset"], 8);
    assert_eq!(v["result"]["match_cap_reached"], true);
}

#[test]
fn final_page_uses_global_order_when_all_three_needles_match() {
    let mut bytes: Vec<u8> = "word".encode_utf16().flat_map(u16::to_le_bytes).collect();
    bytes.extend_from_slice(b"word");
    bytes.resize(bytes.len() + CAP, 0xaa);
    let v = search(&bytes, json!({"pattern": "aa", "text": "word", "offset": CAP - 2, "limit": 5}));
    let rows = v["result"]["matches"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["file_offset"], 12 + CAP - 4, "{v}");
    assert_eq!(rows[1]["file_offset"], 12 + CAP - 3);
    assert_eq!(v["result"]["total_matches"], CAP);
    assert_eq!(v["result"]["match_cap_reached"], true);
}

#[test]
fn exact_cap_across_needles_has_complete_count() {
    let mut bytes = b"needle".to_vec();
    bytes.resize(bytes.len() + CAP - 1, 0xaa);
    let v = search(&bytes, json!({"pattern": "aa", "text": "needle", "encoding": "utf8", "limit": 2}));
    assert_eq!(v["result"]["matches"][0]["file_offset"], 0);
    assert_eq!(v["result"]["total_matches"], CAP);
    assert_eq!(v["result"]["match_cap_reached"], false);
    assert_eq!(v["coverage"]["total"], CAP);
}

#[test]
fn overlapping_matches_and_same_offset_ties_are_preserved() {
    let v = search(b"aaa", json!({"pattern": "61 61", "text": "aa", "encoding": "utf8"}));
    let rows = v["result"]["matches"].as_array().unwrap();
    let actual: Vec<_> = rows.iter().map(|r| (r["file_offset"].as_u64().unwrap(), r["length"].as_u64().unwrap(), r["matched"].as_str().unwrap())).collect();
    assert_eq!(actual, [(0, 2, "pattern"), (0, 2, "utf8"), (1, 2, "pattern"), (1, 2, "utf8")]);
    assert_eq!(v["result"]["total_matches"], 4);
    assert_eq!(v["result"]["match_cap_reached"], false);
}
