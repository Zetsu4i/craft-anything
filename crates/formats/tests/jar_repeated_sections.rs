use craft_anything_formats::verify::{self, Digest};
use craft_anything_testkit::crypto;

fn digest(data: &[u8]) -> String {
    verify::base64(&Digest::Sha256.hash(data))
}

#[test]
fn repeated_entry_sections_use_the_last_value_for_each_attribute() {
    let bytes = b"unchanged file contents";
    let good = digest(bytes);
    let old = digest(b"an earlier value");
    let manifest = format!("Manifest-Version: 1.0\r\n\r\nName: file.txt\r\nSHA-256-Digest: {old}\r\n\r\nName: file.txt\r\nsha-256-digest: {good}\r\n\r\n");
    let sf = format!("Signature-Version: 1.0\r\nSHA-256-Digest-Manifest: {}\r\n\r\n", digest(manifest.as_bytes()));
    let cms = crypto::cms_detached_rsa(sf.as_bytes(), "Craft Fixture", "Craft Test");
    assert!(verify::verify_pkcs7_detached(sf.as_bytes(), &cms).unwrap().signature_valid.is_valid());
    let mut reads = 0;
    let checked = verify::verify_jar_integrity(manifest.as_bytes(), sf.as_bytes(), |name| {
        assert_eq!(name, "file.txt");
        reads += 1;
        Some(bytes.to_vec())
    });
    assert_eq!(checked.manifest_digest_matches, Some(true));
    assert_eq!((checked.files_listed, checked.files_matched, checked.files_mismatched, checked.files_unchecked, reads), (1, 1, 0, 0, 1));
}

#[test]
fn missing_attributes_in_later_sections_preserve_earlier_digests() {
    let bytes = b"entry";
    let manifest =
        format!("Manifest-Version: 1.0\n\nName: file.txt\nSHA-256-Digest: {}\n\nName: file.txt\nCustom-Note: retained separately\n\n", digest(bytes));
    let checked = verify::verify_jar_integrity(manifest.as_bytes(), b"", |_| Some(bytes.to_vec()));
    assert_eq!((checked.files_listed, checked.files_matched, checked.files_unchecked), (1, 1, 0));
}

#[test]
fn later_digest_overrides_cannot_be_hidden_by_a_valid_earlier_section() {
    let bytes = b"entry";
    let manifest =
        format!("Manifest-Version: 1.0\n\nName: file.txt\nSHA-256-Digest: {}\n\nName: file.txt\nSHA-256-Digest: {}\n\n", digest(bytes), digest(b"other"));
    let checked = verify::verify_jar_integrity(manifest.as_bytes(), b"", |_| Some(bytes.to_vec()));
    assert_eq!((checked.files_listed, checked.files_matched, checked.files_mismatched), (1, 0, 1));
    assert_eq!(checked.mismatched_names, ["file.txt"]);
}

#[test]
fn merging_attributes_preserves_digest_preference_and_case_sensitive_paths() {
    let bytes = b"entry";
    let manifest = format!(
        "Manifest-Version: 1.0\n\nName: file.txt\nSHA-256-Digest: {}\n\nName: file.txt\nSHA1-Digest: {}\n\nName: FILE.txt\nSHA-256-Digest: {}\n\n",
        digest(bytes),
        verify::base64(&Digest::Sha1.hash(b"other")),
        digest(bytes)
    );
    let checked = verify::verify_jar_integrity(manifest.as_bytes(), b"", |name| ["file.txt", "FILE.txt"].contains(&name).then(|| bytes.to_vec()));
    assert_eq!((checked.files_listed, checked.files_matched, checked.files_mismatched), (2, 2, 0));
}
