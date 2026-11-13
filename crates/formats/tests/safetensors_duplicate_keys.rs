#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing)]

use craft_anything_formats::ai_model;

fn model(json: &str) -> Vec<u8> {
    let mut data = (json.len() as u64).to_le_bytes().to_vec();
    data.extend_from_slice(json.as_bytes());
    data
}

#[test]
fn duplicate_tensor_names_do_not_silently_replace_earlier_weights() {
    for second in ["weight", r"\u0077eight"] {
        let header = format!(r#"{{"weight":{{"dtype":"U8","shape":[1],"data_offsets":[0,1]}},"{second}":{{"dtype":"U8","shape":[2],"data_offsets":[0,2]}}}}"#);
        let data = model(&header);
        let error = ai_model::parse(&data, data.len() as u64 + 2).expect_err("ambiguous tensor identities must not produce an inventory");
        assert!(error.to_string().contains("duplicate key"), "{error}");
    }
}

#[test]
fn duplicate_tensor_fields_and_metadata_are_rejected_before_summary() {
    for header in [
        r#"{"weight":{"dtype":"U8","dtype":"I8","shape":[1],"data_offsets":[0,1]}}"#,
        r#"{"weight":{"dtype":"U8","shape":[2],"shape":[1],"data_offsets":[0,1]}}"#,
        r#"{"weight":{"dtype":"U8","shape":[1],"data_offsets":[4,5],"data_offsets":[0,1]}}"#,
        r#"{"__metadata__":{"architecture":"first","architecture":"second"}}"#,
        r#"{"__metadata__":{"name":"first"},"__metadata__":{"name":"second"}}"#,
    ] {
        let data = model(header);
        let error = ai_model::parse(&data, data.len() as u64 + 1).expect_err("duplicate fields are not unambiguous evidence");
        assert!(error.to_string().contains("duplicate key"), "{error}");
    }
}

#[test]
fn the_same_field_names_in_different_objects_remain_valid() {
    let data = model(
        r#"{"a":{"dtype":"U8","shape":[1],"data_offsets":[0,1]},"b":{"dtype":"U8","shape":[1],"data_offsets":[1,2]},"__metadata__":{"name":"two weights","architecture":"fixture"}}"#,
    );
    let report = ai_model::parse(&data, data.len() as u64 + 2).unwrap();
    assert_eq!(report.tensors_read, 2);
    assert_eq!(report.parameters, 2);
    assert_eq!(report.name.as_deref(), Some("two weights"));
    assert!(report.warnings.is_empty());
}

#[test]
fn other_invalid_values_still_reach_the_existing_format_diagnostics() {
    let data = model(r#"{"__metadata__":{"float":1.5,"negative":-1,"boolean":true,"null":null,"array":["a",2],"object":{"value":"nested"}}}"#);
    let report = ai_model::parse(&data, data.len() as u64).unwrap();
    assert_eq!(report.declared_metadata, 6);
    assert_eq!(report.metadata_read, 0);
    assert_eq!(report.warnings.len(), 6);
}
