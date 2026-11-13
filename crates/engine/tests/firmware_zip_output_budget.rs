use craft_anything_engine::{CallOptions, Caller, Consent, Engine};
use craft_anything_testkit::electron::zip_stored;
use serde_json::{Value, json};
use std::path::Path;

fn extract(archive: &[u8], member_budget: usize, files: usize, depth: u32, check: impl FnOnce(&Value, &Path)) {
    let directory = craft_anything_engine::testing::temp_dir();
    let input = directory.join("firmware.zip");
    let output = directory.join("output");
    std::fs::write(&input, archive).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let result = engine
        .call(
            "firmware_image_extract",
            json!({"path": input, "output_directory": output, "max_output_bytes": archive.len() + member_budget,
                   "max_output_files": files, "max_depth": depth}),
            CallOptions::new(Caller::Cli).with_consent(Consent::Granted),
        )
        .unwrap()
        .to_json();
    check(&result, &output);
    std::fs::remove_dir_all(directory).unwrap();
}

fn assert_truncated(evidence: &Value) {
    assert_eq!(evidence["result"]["truncated"], true, "{evidence}");
    assert_eq!(evidence["coverage"]["status"], "partial");
    assert!(evidence["limitations"].as_array().unwrap().iter().any(|limit| limit["code"] == "truncated"));
}

#[test]
fn zip_members_exceeding_the_remaining_byte_budget_report_truncation() {
    let archive = zip_stored(&[("large", b"four")]);
    extract(&archive, 3, 10, 1, |evidence, output| {
        assert_eq!(evidence["result"]["files_written"], 1);
        assert_eq!(evidence["result"]["bytes_written"], archive.len());
        assert!(!output.join("0-zip.extracted/large").exists());
        assert_truncated(evidence);
    });
}

#[test]
fn an_oversized_zip_member_leaves_room_for_later_smaller_members() {
    let archive = zip_stored(&[("large", b"large"), ("small", b"ok")]);
    extract(&archive, 2, 10, 1, |evidence, output| {
        assert_eq!(evidence["result"]["files_written"], 2);
        assert_eq!(std::fs::read(output.join("0-zip.extracted/small")).unwrap(), b"ok");
        assert!(!output.join("0-zip.extracted/large").exists());
        assert_truncated(evidence);
    });
}

#[test]
fn a_zip_decode_cap_is_reported_when_the_declared_size_is_too_small() {
    let mut archive = zip_stored(&[("large", b"four")]);
    let directory = u32::from_le_bytes(archive[archive.len() - 6..archive.len() - 2].try_into().unwrap()) as usize;
    archive[22..26].copy_from_slice(&0u32.to_le_bytes());
    archive[directory + 24..directory + 28].copy_from_slice(&0u32.to_le_bytes());
    extract(&archive, 3, 10, 1, |evidence, output| {
        assert_eq!(evidence["result"]["files_written"], 1);
        assert!(!output.join("0-zip.extracted/large").exists());
        assert_truncated(evidence);
    });
}

#[test]
fn an_exact_zip_byte_budget_keeps_later_empty_files() {
    let archive = zip_stored(&[("data", b"a"), ("empty", b"")]);
    extract(&archive, 1, 3, 1, |evidence, output| {
        assert_eq!(evidence["result"]["files_written"], 3);
        assert_eq!(evidence["result"]["bytes_written"], archive.len() + 1);
        assert_eq!(std::fs::read(output.join("0-zip.extracted/data")).unwrap(), b"a");
        assert_eq!(std::fs::metadata(output.join("0-zip.extracted/empty")).unwrap().len(), 0);
        assert_eq!(evidence["result"]["truncated"], false);
        assert_eq!(evidence["coverage"]["status"], "complete");
    });
}

#[test]
fn zip_name_collisions_and_refused_paths_do_not_spend_the_byte_budget() {
    let archive = zip_stored(&[("same", b"good"), ("same", b"lost"), ("../refused", b"deny"), ("last", b"kept")]);
    extract(&archive, 8, 10, 1, |evidence, output| {
        assert_eq!(evidence["result"]["files_written"], 3);
        assert_eq!(evidence["result"]["bytes_written"], archive.len() + 8);
        assert_eq!(std::fs::read(output.join("0-zip.extracted/same")).unwrap(), b"good");
        assert_eq!(std::fs::read(output.join("0-zip.extracted/last")).unwrap(), b"kept");
        assert!(!output.join("refused").exists());
        assert_eq!(evidence["result"]["skipped"].as_array().unwrap().len(), 2);
        assert_eq!(evidence["result"]["truncated"], false);
    });
}

#[test]
fn failed_zip_crc_checks_leave_the_budget_for_readable_members() {
    let mut archive = zip_stored(&[("bad", b"lost"), ("good", b"kept")]);
    let directory = u32::from_le_bytes(archive[archive.len() - 6..archive.len() - 2].try_into().unwrap()) as usize;
    archive[directory + 16] ^= 1;
    extract(&archive, 4, 10, 1, |evidence, output| {
        assert_eq!(evidence["result"]["files_written"], 2);
        assert_eq!(std::fs::read(output.join("0-zip.extracted/good")).unwrap(), b"kept");
        assert!(!output.join("0-zip.extracted/bad").exists());
        assert_eq!(evidence["result"]["truncated"], false);
    });
}

#[test]
fn the_zip_file_limit_is_checked_before_decoding_a_member() {
    let mut archive = zip_stored(&[("bad", b"data")]);
    let directory = u32::from_le_bytes(archive[archive.len() - 6..archive.len() - 2].try_into().unwrap()) as usize;
    archive[directory + 16] ^= 1;
    extract(&archive, 4, 1, 1, |evidence, output| {
        assert_eq!(evidence["result"]["files_written"], 1);
        assert!(!output.join("0-zip.extracted/bad").exists());
        assert_truncated(evidence);
    });
}

#[test]
fn zip_nested_outputs_keep_their_order_and_parent_metadata() {
    let inner = craft_anything_testkit::firmware::gzip(b"ok", None);
    let archive = zip_stored(&[("nested.gz", &inner)]);
    extract(&archive, inner.len() + 2, 10, 2, |evidence, output| {
        let rows = evidence["result"]["outputs"].as_array().unwrap();
        assert_eq!(rows.len(), 3, "{evidence}");
        assert_eq!(rows[0]["path"], "0-zip.bin");
        assert_eq!(rows[1]["path"], "0-zip.extracted/nested.gz");
        assert_eq!(rows[1]["parent"], "0-zip.bin");
        assert_eq!(rows[2]["parent"], rows[1]["path"]);
        assert_eq!(rows[1]["offset"], Value::Null);
        assert_eq!(rows[2]["offset"], Value::Null);
        assert_eq!(std::fs::read(output.join(rows[2]["path"].as_str().unwrap())).unwrap(), b"ok");
        assert_eq!(evidence["result"]["truncated"], false);
        assert_eq!(evidence["coverage"]["status"], "complete");
    });
}
