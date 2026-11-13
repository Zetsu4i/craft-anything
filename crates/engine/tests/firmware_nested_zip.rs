use craft_anything_engine::{CallOptions, Caller, Consent, Engine};
use craft_anything_testkit::electron::zip_stored;
use serde_json::{Value, json};

fn call(engine: &Engine, tool: &str, params: Value) -> Value {
    engine.call(tool, params, CallOptions::new(Caller::Cli).with_consent(Consent::Granted)).unwrap().to_json()
}

#[test]
fn firmware_listing_keeps_nested_zip_members_in_one_outer_region() {
    let inner = zip_stored(&[("leaf", b"ok")]);
    let archive = zip_stored(&[("nested.zip", &inner), ("sibling", b"preserved")]);
    let mut firmware = vec![0; 37];
    firmware.extend_from_slice(&archive);
    let directory = craft_anything_engine::testing::temp_dir();
    let input = directory.join("firmware.bin");
    std::fs::write(&input, &firmware).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let evidence = call(&engine, "firmware_regions_list", json!({"path": input}));
    assert_eq!(evidence["result"]["regions_total"], 1, "{evidence}");
    let region = &evidence["result"]["regions"][0];
    assert_eq!(region["offset"], 37);
    assert_eq!(region["length"], archive.len());
    assert_eq!(region["details"]["entries"], 2);
    assert_eq!(region["confidence"], "high");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn firmware_extraction_preserves_outer_members_and_nested_parent_paths() {
    let inner = zip_stored(&[("leaf", b"ok")]);
    let archive = zip_stored(&[("nested.zip", &inner), ("sibling", b"preserved")]);
    let directory = craft_anything_engine::testing::temp_dir();
    let input = directory.join("firmware.zip");
    let output = directory.join("output");
    std::fs::write(&input, &archive).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let evidence = call(&engine, "firmware_image_extract", json!({"path": input, "output_directory": output, "max_depth": 2}));
    let rows = evidence["result"]["outputs"].as_array().unwrap();
    assert_eq!(rows.len(), 4, "{evidence}");
    assert_eq!(std::fs::read(output.join("0-zip.bin")).unwrap(), archive);
    assert_eq!(std::fs::read(output.join("0-zip.extracted/nested.zip")).unwrap(), inner);
    assert_eq!(std::fs::read(output.join("0-zip.extracted/sibling")).unwrap(), b"preserved");
    assert_eq!(std::fs::read(output.join("0-zip.extracted/nested.zip.extracted/leaf")).unwrap(), b"ok");
    assert_eq!(rows[1]["parent"], "0-zip.bin");
    assert_eq!(rows[2]["parent"], "0-zip.bin");
    assert_eq!(rows[3]["parent"], "0-zip.extracted/nested.zip");
    assert_eq!(rows[3]["offset"], Value::Null);
    assert_eq!(evidence["result"]["skipped"].as_array().unwrap().len(), 0);
    assert_eq!(evidence["result"]["truncated"], false);
    assert_eq!(evidence["coverage"]["status"], "complete");
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn firmware_extraction_keeps_adjacent_zip_archives_separate() {
    let first = zip_stored(&[("first", b"one")]);
    let second = zip_stored(&[("second", b"two")]);
    let mut firmware = first.clone();
    firmware.extend_from_slice(&second);
    let directory = craft_anything_engine::testing::temp_dir();
    let input = directory.join("firmware.bin");
    let output = directory.join("output");
    std::fs::write(&input, &firmware).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let evidence = call(&engine, "firmware_image_extract", json!({"path": input, "output_directory": output, "max_depth": 1}));
    assert_eq!(evidence["result"]["files_written"], 4, "{evidence}");
    assert_eq!(std::fs::read(output.join("0-zip.bin")).unwrap(), first);
    assert_eq!(std::fs::read(output.join(format!("{:x}-zip.bin", first.len()))).unwrap(), second);
    assert_eq!(std::fs::read(output.join("0-zip.extracted/first")).unwrap(), b"one");
    assert_eq!(std::fs::read(output.join(format!("{:x}-zip.extracted/second", first.len()))).unwrap(), b"two");
    assert_eq!(evidence["result"]["truncated"], false);
    assert_eq!(evidence["coverage"]["status"], "complete");
    std::fs::remove_dir_all(directory).unwrap();
}
