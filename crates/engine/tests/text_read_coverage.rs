use craft_anything_engine::{CallOptions, Caller, Engine};
use serde_json::{Value, json};

const PAGE_BYTES: usize = 256 * 1024;

fn read(data: &[u8], mut params: Value) -> Value {
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("source.txt");
    std::fs::write(&path, data).unwrap();
    params["path"] = json!(path);
    let engine = Engine::new(craft_anything_engine::testing::session());
    let result = engine.call("core_text_read", params, CallOptions::new(Caller::Cli)).unwrap().to_json();
    std::fs::remove_dir_all(directory).unwrap();
    result
}

fn assert_cut_is_partial(v: &Value, line: usize) {
    assert_eq!(v["result"]["cut_lines"], json!([line]));
    assert_eq!(v["result"]["more"], false);
    assert!(v["coverage"]["next_offset"].is_null());
    assert!(v["limitations"].as_array().unwrap().iter().any(|l| l["code"] == "long_lines_cut"));
    assert_eq!(v["coverage"]["status"], "partial", "coverage is {}", v["coverage"]);
}

#[test]
fn a_cut_single_line_does_not_claim_complete_coverage() {
    let bytes = format!("{} OMITTED", "x".repeat(PAGE_BYTES));
    let v = read(bytes.as_bytes(), json!({}));
    assert_eq!(v["result"]["total_lines"], 1);
    assert!(!v["result"]["text"].as_str().unwrap().contains("OMITTED"));
    assert_cut_is_partial(&v, 1);
}

#[test]
fn a_cut_line_on_the_final_page_is_partial() {
    let bytes = format!("first\n{} tail\n", "y".repeat(PAGE_BYTES));
    let v = read(bytes.as_bytes(), json!({"offset": 1, "limit": 1}));
    assert_eq!(v["result"]["total_lines"], 2);
    assert_eq!(v["coverage"]["examined"], 1);
    assert_cut_is_partial(&v, 2);
}

#[test]
fn a_decoded_wide_line_cut_at_a_character_boundary_is_partial() {
    let text = "界".repeat(PAGE_BYTES / 3 + 2);
    let mut bytes = vec![0xff, 0xfe];
    bytes.extend(text.encode_utf16().flat_map(u16::to_le_bytes));
    let v = read(&bytes, json!({}));
    assert_eq!(v["result"]["encoding"], "utf16le");
    assert_eq!(v["result"]["text"].as_str().unwrap().len(), PAGE_BYTES);
    assert_cut_is_partial(&v, 1);
}

#[test]
fn a_complete_short_file_keeps_complete_coverage() {
    let v = read(b"first\nlast\n", json!({}));
    assert_eq!(v["result"]["text"], "first\nlast\n");
    assert_eq!(v["result"]["cut_lines"], json!([]));
    assert_eq!(v["coverage"]["status"], "complete");
    assert_eq!(v["coverage"]["total"], 2);
}

#[test]
fn ordinary_pagination_keeps_its_next_offset() {
    let v = read(b"first\nlast\n", json!({"limit": 1}));
    assert_eq!(v["result"]["text"], "first\n");
    assert_eq!(v["coverage"]["status"], "partial");
    assert_eq!(v["coverage"]["next_offset"], 1);
    assert_eq!(v["result"]["more"], true);
}
