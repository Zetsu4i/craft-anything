use craft_anything_engine::{CallOptions, Caller, Engine};
use serde_json::{Value, json};
use std::path::Path;

fn call(engine: &Engine, tool: &str, params: Value) -> Value {
    engine.call(tool, params, CallOptions::new(Caller::Cli)).unwrap().to_json()
}

fn export_pages(input: &Path, bundle: &Path, offsets: &[usize]) {
    let engine = Engine::new(craft_anything_engine::testing::session());
    for offset in offsets {
        call(&engine, "core_text_read", json!({"path": input, "offset": offset, "limit": 1}));
    }
    let exported = call(&engine, "core_evidence_export", json!({"path": bundle}));
    assert_eq!(exported["result"]["records"], offsets.len());
}

fn compare(left: &Path, right: &Path) -> Value {
    let engine = Engine::new(craft_anything_engine::testing::session());
    call(&engine, "core_evidence_exports_compare", json!({"older_bundle": left, "newer_bundle": right, "keep_pairs": true}))
}

#[test]
fn identical_exported_pages_are_unchanged_in_either_record_order() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    std::fs::write(&input, b"first\nsecond\nthird\n").unwrap();
    export_pages(&input, &left, &[0, 1]);
    export_pages(&input, &right, &[1, 0]);
    let result = compare(&left, &right);
    let counts = &result["result"]["counts"];
    assert_eq!(counts["changed"], 0, "record order changed a comparison of identical evidence: {result}");
    assert_eq!(counts["unchanged"], 2, "both independently requested pages must be retained");
    let pairs = result["result"]["keep_pairs"].as_array().unwrap();
    assert_eq!(pairs.len(), 2);
    assert!(pairs.iter().all(|pair| pair["left"] == pair["right"]));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn removing_one_exported_page_is_reported() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    std::fs::write(&input, b"first\nsecond\n").unwrap();
    export_pages(&input, &left, &[0, 1]);
    export_pages(&input, &right, &[0]);
    let result = compare(&left, &right);
    assert_eq!(result["result"]["counts"]["removed"], 1, "a second observation vanished from the bundle without a removal: {result}");
    assert_eq!(result["result"]["counts"]["unchanged"], 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn adding_one_exported_page_is_reported() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    std::fs::write(&input, b"first\nsecond\n").unwrap();
    export_pages(&input, &left, &[0]);
    export_pages(&input, &right, &[0, 1]);
    let result = compare(&left, &right);
    assert_eq!(result["result"]["counts"]["added"], 1, "a second observation appeared without an addition: {result}");
    assert_eq!(result["result"]["counts"]["unchanged"], 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_tampered_page_is_not_hidden_by_another_page_of_the_same_file() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    std::fs::write(&input, b"first\nsecond\n").unwrap();
    export_pages(&input, &left, &[0, 1]);
    let mut bundle: Value = serde_json::from_slice(&std::fs::read(&left).unwrap()).unwrap();
    let page = bundle["records"].as_array_mut().unwrap().iter_mut().find(|record| record["params"]["offset"] == 1).unwrap();
    page["result"]["text"] = json!("edited after export\n");
    std::fs::write(&right, serde_json::to_vec(&bundle).unwrap()).unwrap();
    let result = compare(&left, &right);
    assert_eq!(result["result"]["counts"]["tampered"], 1);
    assert_eq!(result["result"]["counts"]["changed"], 1, "the altered page was dropped before comparison: {result}");
    assert_eq!(result["result"]["counts"]["unchanged"], 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn single_record_export_remains_unchanged() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let bundle = dir.join("one.json");
    std::fs::write(&input, b"one page\n").unwrap();
    export_pages(&input, &bundle, &[0]);
    let result = compare(&bundle, &bundle);
    assert_eq!(result["result"]["counts"]["unchanged"], 1);
    assert_eq!(result["result"]["counts"]["changed"], 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn multiple_versions_with_identical_parameters_are_retained() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("sample.elf");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    let (bytes, _) = craft_anything_testkit::elf64(craft_anything_testkit::ElfArch::X86_64, true);
    std::fs::write(&input, bytes).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    call(&engine, "core_target_open", json!({"path": input}));
    let address = format!("{:#x}", craft_anything_testkit::ELF_BASE + craft_anything_testkit::ELF_TEXT_OFF);
    let before = call(&engine, "native_address_show", json!({"address": address}));
    call(&engine, "native_comment_write", json!({"address": address, "comment": "observed later"}));
    let after = call(&engine, "native_address_show", json!({"address": address}));
    assert_eq!(before["params"], after["params"]);
    assert_eq!(before["subject"], after["subject"]);
    assert_ne!(before["id"], after["id"]);
    call(&engine, "core_evidence_export", json!({"path": left}));
    let mut bundle: Value = serde_json::from_slice(&std::fs::read(&left).unwrap()).unwrap();
    assert_eq!(bundle["records"].as_array().unwrap().len(), 2);
    bundle["records"].as_array_mut().unwrap().reverse();
    std::fs::write(&right, serde_json::to_vec(&bundle).unwrap()).unwrap();
    let result = compare(&left, &right);
    assert_eq!(result["result"]["counts"]["unchanged"], 2, "different versions of one observation must survive: {result}");
    assert_eq!(result["result"]["counts"]["changed"], 0);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn repeated_identical_records_keep_their_multiplicity() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let once = dir.join("once.json");
    let twice = dir.join("twice.json");
    std::fs::write(&input, b"one page\n").unwrap();
    export_pages(&input, &once, &[0]);
    let mut bundle: Value = serde_json::from_slice(&std::fs::read(&once).unwrap()).unwrap();
    let copy = bundle["records"][0].clone();
    bundle["records"].as_array_mut().unwrap().push(copy);
    std::fs::write(&twice, serde_json::to_vec(&bundle).unwrap()).unwrap();
    for (left, right, extra) in [(&twice, &once, "removed"), (&once, &twice, "added")] {
        let result = compare(left, right);
        assert_eq!(result["result"]["counts"][extra], 1, "duplicate evidence counts must be preserved: {result}");
        assert_eq!(result["result"]["counts"]["unchanged"], 1);
        assert_eq!(result["result"]["counts"]["changed"], 0);
        assert_eq!(result["result"]["keep_pairs"].as_array().unwrap().len(), 1);
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn verified_equal_ids_match_before_a_tampered_copy() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    std::fs::write(&input, b"one page\n").unwrap();
    export_pages(&input, &left, &[0]);
    let mut bundle: Value = serde_json::from_slice(&std::fs::read(&left).unwrap()).unwrap();
    let mut altered = bundle["records"][0].clone();
    altered["result"]["text"] = json!("altered copy\n");
    bundle["records"].as_array_mut().unwrap().push(altered);
    for _ in 0..2 {
        std::fs::write(&right, serde_json::to_vec(&bundle).unwrap()).unwrap();
        let result = compare(&left, &right);
        assert_eq!(result["result"]["counts"]["unchanged"], 1, "verified evidence should be matched first: {result}");
        assert_eq!(result["result"]["counts"]["changed"], 0);
        assert_eq!(result["result"]["counts"]["added"], 1);
        assert_eq!(result["result"]["counts"]["tampered"], 1);
        bundle["records"].as_array_mut().unwrap().reverse();
    }
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn changed_pairs_are_stable_when_both_record_orders_reverse() {
    let dir = craft_anything_engine::testing::temp_dir();
    let input = dir.join("notes.txt");
    let left = dir.join("left.json");
    let right = dir.join("right.json");
    std::fs::write(&input, b"first\nsecond\nthird\nfourth\n").unwrap();
    export_pages(&input, &left, &[0, 1]);
    export_pages(&input, &right, &[2, 3]);
    let before = compare(&left, &right);
    assert_eq!(before["result"]["counts"]["changed"], 2);
    for path in [&left, &right] {
        let mut bundle: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        bundle["records"].as_array_mut().unwrap().reverse();
        std::fs::write(path, serde_json::to_vec(&bundle).unwrap()).unwrap();
    }
    let after = compare(&left, &right);
    assert_eq!(before["result"], after["result"], "record order must not select different changed pairs");
    std::fs::remove_dir_all(dir).unwrap();
}
