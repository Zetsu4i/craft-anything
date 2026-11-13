use craft_anything_formats::{FormatError, ai_model};

fn integer(bytes: &mut Vec<u8>, value: u64, width: usize, big: bool) {
    let value = if big { value.to_be_bytes() } else { value.to_le_bytes() };
    bytes.extend_from_slice(if big { &value[8 - width..] } else { &value[..width] });
}

fn string(bytes: &mut Vec<u8>, value: &str, big: bool) {
    integer(bytes, value.len() as u64, 8, big);
    bytes.extend_from_slice(value.as_bytes());
}

fn header(metadata: &[(&str, &str)], tensors: &[&str], big: bool) -> Vec<u8> {
    let mut bytes = b"GGUF".to_vec();
    integer(&mut bytes, 3, 4, big);
    integer(&mut bytes, tensors.len() as u64, 8, big);
    integer(&mut bytes, metadata.len() as u64, 8, big);
    for &(key, value) in metadata {
        string(&mut bytes, key, big);
        integer(&mut bytes, 8, 4, big); // string metadata
        string(&mut bytes, value, big);
    }
    for (index, name) in tensors.iter().enumerate() {
        string(&mut bytes, name, big);
        integer(&mut bytes, 1, 4, big);
        integer(&mut bytes, 4, 8, big);
        integer(&mut bytes, 0, 4, big); // float32
        integer(&mut bytes, index as u64 * 32, 8, big);
    }
    bytes
}

fn duplicate(bytes: &[u8], identity: &str) {
    let error = match ai_model::parse(bytes, bytes.len() as u64 + 4096) {
        Err(error) => error,
        Ok(_) => panic!("duplicate {identity} was accepted"),
    };
    assert!(matches!(error, FormatError::Malformed(_)));
    assert!(error.to_string().contains("duplicate") && error.to_string().contains(identity), "{error}");
}

#[test]
fn metadata_duplicates_cannot_silently_change_model_identity() {
    for big in [false, true] {
        duplicate(&header(&[("general.architecture", "alpha"), ("general.architecture", "beta")], &[], big), "metadata key");
        duplicate(&header(&[("general.name", "same"), ("general.name", "same")], &[], big), "metadata key");
    }
}

#[test]
fn tensor_duplicates_cannot_double_count_parameters() {
    for big in [false, true] {
        duplicate(&header(&[], &["weights", "weights"], big), "tensor name");
    }
}

#[test]
fn duplicate_checks_use_full_names_before_display_truncation() {
    let first = "x".repeat(1100) + "a";
    let second = "x".repeat(1100) + "b";
    duplicate(&header(&[], &[&first, &first], false), "tensor name");
    let bytes = header(&[], &[&first, &second], false);
    let report = ai_model::parse(&bytes, bytes.len() as u64 + 4096).unwrap();
    assert!(report.complete);
    assert_eq!(report.tensors_read, 2);
    assert_eq!(report.parameters, 8);
}

#[test]
fn metadata_keys_remain_unique_beyond_the_retained_entry_limit() {
    let names = (0..65_536).map(|index| format!("key.{index}")).collect::<Vec<_>>();
    let mut metadata = names.iter().map(|name| (name.as_str(), "value")).collect::<Vec<_>>();
    metadata.push(("key.0", "duplicate after the retained prefix"));
    duplicate(&header(&metadata, &[], false), "metadata key");
}

#[test]
fn distinct_names_and_partial_headers_keep_existing_evidence() {
    let bytes = header(&[("general.name", "example"), ("custom.name", "other")], &["a", "b"], false);
    let report = ai_model::parse(&bytes, bytes.len() as u64 + 4096).unwrap();
    assert!(report.complete);
    assert_eq!(report.name.as_deref(), Some("example"));
    assert_eq!(report.parameters, 8);
    let partial = ai_model::parse(&bytes[..bytes.len() - 1], bytes.len() as u64 + 4096).unwrap();
    assert!(!partial.complete);
    assert_eq!(partial.tensors_read, 1);
    assert_eq!(partial.parameters, 4);
}
