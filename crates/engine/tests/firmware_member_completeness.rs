use craft_anything_engine::{CallOptions, Caller, Consent, Engine};
use craft_anything_testkit::{electron::zip_stored, firmware};
use serde_json::{Value, json};
use std::path::Path;

fn extract(archive: &[u8], files: usize, check: impl FnOnce(&Value, &Path)) {
    let directory = craft_anything_engine::testing::temp_dir();
    let input = directory.join("firmware.bin");
    let output = directory.join("output");
    std::fs::write(&input, archive).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let evidence = engine
        .call(
            "firmware_image_extract",
            json!({"path": input, "output_directory": output, "max_output_files": files}),
            CallOptions::new(Caller::Cli).with_consent(Consent::Granted),
        )
        .unwrap()
        .to_json();
    check(&evidence, &output);
    std::fs::remove_dir_all(directory).unwrap();
}

fn assert_capped(evidence: &Value, expected_files: usize) {
    assert_eq!(evidence["result"]["files_written"], expected_files);
    assert_eq!(evidence["result"]["skipped"], json!([]));
    assert_eq!(evidence["result"]["truncated"], true, "{evidence}");
    assert_eq!(evidence["coverage"]["status"], "partial");
    assert!(evidence["limitations"].as_array().unwrap().iter().any(|limit| {
        limit["code"] == "truncated"
            && limit["message"].as_str().unwrap().contains("member scan")
            && limit["message"].as_str().unwrap().contains("max_output_files")
    }));
}

fn assert_complete(evidence: &Value) {
    assert_eq!(evidence["result"]["skipped"], json!([]));
    assert_eq!(evidence["result"]["truncated"], false, "{evidence}");
    assert_eq!(evidence["coverage"]["status"], "complete");
    assert!(!evidence["limitations"].as_array().unwrap().iter().any(|limit| limit["code"] == "truncated"));
}

#[test]
fn zip_directory_entries_cannot_hide_a_capped_member_walk() {
    let archive = zip_stored(&[("a/", b""), ("b/", b""), ("later", b"kept")]);
    extract(&archive, 1, |evidence, output| {
        assert_capped(evidence, 1);
        assert_eq!(std::fs::read(output.join("0-zip.bin")).unwrap(), archive);
        assert!(!output.join("0-zip.extracted/later").exists());
    });
}

#[test]
fn tar_directory_entries_cannot_hide_a_capped_member_walk() {
    let archive = firmware::tar(&[("a/", b""), ("later", b"kept")]);
    extract(&archive, 1, |evidence, output| {
        assert_capped(evidence, 1);
        assert_eq!(std::fs::read(output.join("0-tar.bin")).unwrap(), archive);
        assert!(!output.join("0-tar.extracted/later").exists());
    });
}

#[test]
fn cpio_directory_entries_cannot_hide_a_capped_member_walk() {
    let archive = firmware::cpio_newc(&[("a", b"", 0o040755), ("later", b"kept", 0o100644)]);
    extract(&archive, 1, |evidence, output| {
        assert_capped(evidence, 1);
        assert_eq!(std::fs::read(output.join("0-cpio-newc.bin")).unwrap(), archive);
        assert!(!output.join("0-cpio-newc.extracted/later").exists());
    });
}

#[test]
fn a_nested_capped_member_walk_keeps_extraction_partial() {
    let archive = firmware::tar(&[("a/", b""), ("b/", b""), ("later", b"kept")]);
    let compressed = firmware::gzip(&archive, Some("members.tar"));
    extract(&compressed, 2, |evidence, output| {
        assert_capped(evidence, 2);
        assert_eq!(std::fs::read(output.join("0-gzip.extracted/members.tar")).unwrap(), archive);
    });
}

#[test]
fn a_damaged_zip_directory_reports_the_same_incomplete_walk() {
    let mut archive = zip_stored(&[("a/", b""), ("later", b"kept")]);
    let directory = u32::from_le_bytes(archive[archive.len() - 6..archive.len() - 2].try_into().unwrap()) as usize;
    archive[directory + 46 + "a/".len()] = b'X';
    extract(&archive, 10, |evidence, _| {
        assert_capped(evidence, 1);
        assert!(
            evidence["limitations"]
                .as_array()
                .unwrap()
                .iter()
                .any(|limit| { limit["code"] == "truncated" && limit["message"].as_str().unwrap().contains("stopped early") })
        );
    });
}

#[test]
fn complete_zip_directories_and_recovered_members_stay_complete() {
    extract(&zip_stored(&[("a/", b""), ("b/", b"")]), 1, |evidence, _| assert_complete(evidence));
    extract(&zip_stored(&[("a/", b""), ("b/", b""), ("later", b"kept")]), 2, |evidence, output| {
        assert_complete(evidence);
        assert_eq!(std::fs::read(output.join("0-zip.extracted/later")).unwrap(), b"kept");
    });
}

#[test]
fn complete_tar_directories_and_recovered_members_stay_complete() {
    extract(&firmware::tar(&[("a/", b"")]), 1, |evidence, _| assert_complete(evidence));
    extract(&firmware::tar(&[("a/", b""), ("later", b"kept")]), 2, |evidence, output| {
        assert_complete(evidence);
        assert_eq!(std::fs::read(output.join("0-tar.extracted/later")).unwrap(), b"kept");
    });
}

#[test]
fn complete_cpio_directories_and_recovered_members_stay_complete() {
    extract(&firmware::cpio_newc(&[("a", b"", 0o040755)]), 1, |evidence, _| assert_complete(evidence));
    extract(&firmware::cpio_newc(&[("a", b"", 0o040755), ("later", b"kept", 0o100644)]), 2, |evidence, output| {
        assert_complete(evidence);
        assert_eq!(std::fs::read(output.join("0-cpio-newc.extracted/later")).unwrap(), b"kept");
    });
}
