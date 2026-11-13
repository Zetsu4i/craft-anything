use craft_anything_engine::{CallOptions, Caller, Engine, ToolOutput};
use serde_json::{Value, json};

fn archive(files: Value) -> Vec<u8> {
    let json = serde_json::to_vec(&json!({ "files": files })).unwrap();
    let padding = (4 - json.len() % 4) % 4;
    let payload = json.len() + 4 + padding;
    let mut bytes = 4u32.to_le_bytes().to_vec();
    bytes.extend_from_slice(&u32::try_from(payload + 4).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(payload).unwrap().to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(json.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&json);
    bytes.extend(std::iter::repeat_n(0, padding));
    bytes.extend_from_slice(b"kept");
    bytes
}

fn evidence(output: ToolOutput) -> Value {
    let ToolOutput::Evidence(record) = output else { panic!("expected evidence") };
    assert!(record.verify());
    serde_json::to_value(&*record).unwrap()
}

fn extract(engine: &Engine, params: Value, relative: &str) -> (Value, Vec<u8>) {
    let output = evidence(engine.call("core_archive_extract", params, CallOptions::new(Caller::Mcp)).unwrap());
    let directory = std::path::PathBuf::from(output["result"]["output_directory"].as_str().unwrap());
    let bytes = std::fs::read(directory.join(relative));
    // Remove the baseline's incorrectly complete result before checking its evidence too.
    std::fs::remove_dir_all(directory).unwrap();
    (output, bytes.unwrap())
}

fn assert_partial_extraction(output: &Value) {
    assert_eq!(output["result"]["files_written"], 1);
    assert_eq!(output["result"]["bytes_written"], 4);
    assert!(output["result"]["skipped"].as_array().unwrap().is_empty());
    assert_eq!(output["coverage"]["status"], "partial");
    assert!(output["coverage"]["total"].is_null());
    let limitation = output["limitations"].as_array().unwrap().iter().find(|limit| limit["code"] == "truncated").unwrap();
    assert!(limitation["message"].as_str().unwrap().contains("not read"));
}

fn path_limited_archive(file: &str) -> Vec<u8> {
    let mut children: serde_json::Map<String, Value> = (0..4_000).map(|index| (format!("folder-{index:05}"), json!({ "files": {} }))).collect();
    for index in 0..15 {
        children = serde_json::Map::from_iter([(format!("{index:02}{}", "d".repeat(238)), json!({ "files": children }))]);
    }
    archive(json!({ file: { "size": 4, "offset": "0" }, "z": { "files": children } }))
}

#[test]
fn asar_entry_limits_keep_recovered_files_and_disclose_the_unread_remainder() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("limited.asar");
    std::fs::write(
        &path,
        archive(json!({
            "a.txt": { "size": 4, "offset": "0" },
            "b": { "files": { "hidden.txt": { "size": 4, "offset": "0" } } }
        })),
    )
    .unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let (output, bytes) = extract(&engine, json!({"path": path, "max_files": 1}), "a.txt");
    assert_eq!(bytes, b"kept");
    assert_partial_extraction(&output);
}

#[test]
fn asar_path_limits_keep_inventory_and_extraction_partial_without_a_claimed_total() {
    let bytes = path_limited_archive("a.txt");
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("paths.asar");
    std::fs::write(&path, &bytes).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let inventory = evidence(engine.call("core_archive_show", json!({"path": path, "limit": 1}), CallOptions::new(Caller::Mcp)).unwrap());
    assert_eq!(inventory["coverage"]["status"], "partial");
    assert_eq!(inventory["result"]["truncated"], true);
    assert_eq!(inventory["result"]["entries"][0]["path"], "a.txt");
    let (output, extracted) = extract(&engine, json!({"path": path}), "a.txt");
    assert_eq!(extracted, b"kept");
    assert_partial_extraction(&output);
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn a_complete_asar_still_extracts_at_the_entry_limit() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("complete.asar");
    std::fs::write(&path, archive(json!({"folder": {"files": {"file.txt": {"size": 4, "offset": "0"}}}}))).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let (output, bytes) = extract(&engine, json!({"path": path, "max_files": 1}), "folder/file.txt");
    assert_eq!(bytes, b"kept");
    assert_eq!(output["result"]["files_written"], 1);
    assert_eq!(output["coverage"]["status"], "complete");
    assert_eq!(output["coverage"]["total"], 1);
    assert!(!output["limitations"].as_array().unwrap().iter().any(|limit| limit["code"] == "truncated"));
}

#[test]
fn zip_listing_limits_keep_known_files_and_disclose_missing_members() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("limited.zip");
    let members: &[(&str, &[u8])] = &[("a.txt", b"kept"), ("folder/", b""), ("folder/hidden.txt", b"kept")];
    let engine = Engine::new(craft_anything_engine::testing::session());
    for count in [2, 3] {
        std::fs::write(&path, craft_anything_testkit::electron::zip_stored(&members[..count])).unwrap();
        let (output, bytes) = extract(&engine, json!({"path": path, "max_files": 1}), "a.txt");
        assert_eq!(bytes, b"kept");
        if count == 3 {
            assert_partial_extraction(&output);
        } else {
            assert_eq!(output["coverage"]["status"], "complete");
            assert_eq!(output["coverage"]["total"], 1);
        }
    }
}

#[test]
fn js_app_map_preserves_asar_path_truncation_in_direct_and_packaged_folder_inputs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.asar");
    std::fs::write(&path, path_limited_archive("a.js")).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    for input in [path.as_path(), directory.path()] {
        let output = evidence(engine.call("js_app_map", json!({"input_path": input}), CallOptions::new(Caller::Mcp)).unwrap());
        assert_eq!(output["result"]["modules_total"], 1);
        assert_eq!(output["coverage"]["status"], "partial");
        assert!(output["coverage"]["total"].is_null());
        let limitations = output["limitations"].as_array().unwrap();
        assert!(limitations.iter().any(|limit| limit["code"] == "truncated"));
        assert!(limitations.iter().any(|limit| limit["message"].as_str().is_some_and(|message| message.contains("path-byte") && message.contains("depth"))));
    }
}

#[test]
fn js_app_map_keeps_complete_coverage_for_a_fully_read_asar() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("complete.asar");
    std::fs::write(&path, archive(json!({"a.js": { "size": 4, "offset": "0" }}))).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let output = evidence(engine.call("js_app_map", json!({"input_path": path}), CallOptions::new(Caller::Mcp)).unwrap());
    assert_eq!(output["result"]["modules_total"], 1);
    assert_eq!(output["coverage"]["status"], "complete");
    assert_eq!(output["coverage"]["total"], 1);
    assert!(!output["limitations"].as_array().unwrap().iter().any(|limit| limit["code"] == "truncated"));
}
