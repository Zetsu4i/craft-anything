use craft_anything_formats::fsread::{Limits, jffs2};

fn image(payload: &[u8], compression: u8, declared: u32, file_size: u32) -> Vec<u8> {
    let mut bytes = craft_anything_testkit::firmware::jffs2_image();
    let node = 96;
    bytes.truncate(node + 68);
    for (field, value) in [(4, 68 + payload.len() as u32), (28, file_size), (48, payload.len() as u32), (52, declared)] {
        bytes[node + field..node + field + 4].copy_from_slice(&value.to_le_bytes());
    }
    bytes[node + 56] = compression;
    bytes.extend_from_slice(payload);
    bytes.resize(bytes.len().div_ceil(4) * 4, 0);
    bytes
}

fn assert_unrecovered(bytes: &[u8]) {
    let result = jffs2::read(bytes, &Limits::default());
    let entry = result.entries.iter().find(|entry| entry.path == "config/settings").unwrap();
    assert!(entry.data.is_none(), "invalid extent became {:?}", entry.data);
    assert!(!result.complete);
    assert_eq!(result.bytes, 0);
    assert!(result.warnings.iter().any(|warning| warning.contains("data node")));
}

fn append_node(bytes: &mut Vec<u8>, payload: &[u8], compression: u8, declared: u32, file_size: u32, offset: u32, version: u32) {
    let mut node = image(payload, compression, declared, file_size).split_off(96);
    node[16..20].copy_from_slice(&version.to_le_bytes());
    node[44..48].copy_from_slice(&offset.to_le_bytes());
    bytes.extend_from_slice(&node);
}

fn damaged_payloads() -> [(Vec<u8>, u8); 5] {
    [
        (b"abc".to_vec(), 0),
        (b"abcde".to_vec(), 0),
        (miniz_oxide::deflate::compress_to_vec_zlib(b"abc", 6), 6),
        (miniz_oxide::deflate::compress_to_vec_zlib(b"abcde", 6), 6),
        (b"not a zlib stream".to_vec(), 6),
    ]
}

fn assert_recovered_with_warning(bytes: &[u8], expected: &[u8]) {
    let result = jffs2::read(bytes, &Limits::default());
    let entry = result.entries.iter().find(|entry| entry.path == "config/settings").unwrap();
    assert_eq!(entry.data.as_deref(), Some(expected), "{:?}", result.warnings);
    assert_eq!(entry.size, expected.len() as u64);
    assert_eq!(result.bytes, expected.len() as u64);
    assert!(result.complete, "{:?}", result.warnings);
    assert!(result.warnings.iter().any(|warning| warning.contains("data node")));
}

#[test]
fn uncompressed_nodes_must_match_their_declared_length() {
    for declared in [3, 5] {
        assert_unrecovered(&image(b"abcd", 0, declared, 8));
    }
}

#[test]
fn compressed_nodes_must_match_their_declared_length() {
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(b"abcd", 6);
    for declared in [3, 5] {
        assert_unrecovered(&image(&compressed, 6, declared, 8));
    }
}

#[test]
fn a_failed_inflate_does_not_become_a_zero_filled_file() {
    assert_unrecovered(&image(b"not a zlib stream", 6, 4, 4));
}

#[test]
fn exact_nodes_can_be_clipped_by_the_final_file_size() {
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(b"abcd", 6);
    for (payload, compression) in [(b"abcd".as_slice(), 0), (compressed.as_slice(), 6)] {
        for (size, expected) in [(2, b"ab".as_slice()), (4, b"abcd".as_slice())] {
            let result = jffs2::read(&image(payload, compression, 4, size), &Limits::default());
            assert!(result.complete, "{:?}", result.warnings);
            assert_eq!(result.entries.iter().find(|entry| entry.path == "config/settings").unwrap().data.as_deref(), Some(expected));
            assert_eq!(result.bytes, u64::from(size));
        }
    }
}

#[test]
fn zero_nodes_keep_their_explicit_zero_extent() {
    let result = jffs2::read(&image(&[], 1, 4, 4), &Limits::default());
    assert!(result.complete);
    assert_eq!(result.entries.iter().find(|entry| entry.path == "config/settings").unwrap().data.as_deref(), Some([0; 4].as_slice()));
}

#[test]
fn a_newer_valid_node_recovers_a_fully_overwritten_bad_node() {
    let compressed = miniz_oxide::deflate::compress_to_vec_zlib(b"good", 6);
    for (damaged, compression) in damaged_payloads() {
        for (replacement, replacement_compression) in [(b"good".as_slice(), 0), (compressed.as_slice(), 6)] {
            for versions in [[1, 2], [2, 1]] {
                let mut bytes = image(&[], 0, 0, 4);
                bytes.truncate(96);
                for version in versions {
                    let (payload, compr) = if version == 1 { (damaged.as_slice(), compression) } else { (replacement, replacement_compression) };
                    append_node(&mut bytes, payload, compr, 4, 4, 0, version);
                }
                assert_recovered_with_warning(&bytes, b"good");
            }
        }
    }
}

#[test]
fn newer_valid_nodes_can_repair_a_bad_extent_in_parts() {
    for (damaged, compression) in damaged_payloads() {
        let mut bytes = image(&damaged, compression, 4, 4);
        append_node(&mut bytes, &[], 1, 2, 4, 1, 2);
        assert_unrecovered(&bytes);
        append_node(&mut bytes, b"a", 0, 1, 4, 0, 3);
        assert_unrecovered(&bytes);
        append_node(&mut bytes, b"d", 0, 1, 4, 3, 4);
        assert_recovered_with_warning(&bytes, b"a\0\0d");
    }
}

#[test]
fn a_newer_bad_node_cannot_reuse_older_valid_bytes() {
    for (damaged, compression) in damaged_payloads() {
        let mut bytes = image(b"old!", 0, 4, 4);
        append_node(&mut bytes, &damaged, compression, 4, 4, 0, 2);
        assert_unrecovered(&bytes);
        append_node(&mut bytes, b"od", 0, 2, 4, 2, 3);
        assert_unrecovered(&bytes);
        append_node(&mut bytes, b"go", 0, 2, 4, 0, 4);
        assert_recovered_with_warning(&bytes, b"good");
    }
}

#[test]
fn only_bad_bytes_within_the_final_file_size_need_replacement() {
    for (damaged, compression) in damaged_payloads() {
        for offset in [0, 2, u32::MAX] {
            let mut bytes = image(&[], 0, 0, 4);
            bytes.truncate(96);
            append_node(&mut bytes, &damaged, compression, 4, 4, offset, 1);
            append_node(&mut bytes, b"ok", 0, 2, 2, 0, 2);
            assert_recovered_with_warning(&bytes, b"ok");
        }
    }
}

#[test]
fn equal_versions_keep_their_physical_order() {
    let mut repaired = image(b"bad", 0, 4, 4);
    append_node(&mut repaired, b"good", 0, 4, 4, 0, 1);
    assert_recovered_with_warning(&repaired, b"good");

    let mut damaged = image(b"good", 0, 4, 4);
    append_node(&mut damaged, b"bad", 0, 4, 4, 0, 1);
    assert_unrecovered(&damaged);
}

#[test]
fn a_later_bad_node_invalidates_a_previously_repaired_extent() {
    let mut bytes = image(b"bad", 0, 4, 4);
    append_node(&mut bytes, b"good", 0, 4, 4, 0, 2);
    append_node(&mut bytes, b"x", 0, 2, 4, 1, 3);
    assert_unrecovered(&bytes);
    append_node(&mut bytes, b"oo", 0, 2, 4, 1, 4);
    assert_recovered_with_warning(&bytes, b"good");
}

#[test]
fn unsupported_nodes_need_the_same_later_valid_coverage() {
    for compression in [2, 3, 4, 5, 7, 255] {
        let mut bytes = image(b"old!", 0, 4, 4);
        append_node(&mut bytes, b"data", compression, 4, 4, 0, 2);
        assert_unrecovered(&bytes);
        append_node(&mut bytes, b"new", 0, 3, 4, 0, 3);
        assert_unrecovered(&bytes);
        append_node(&mut bytes, &[], 1, 1, 4, 3, 4);
        assert_recovered_with_warning(&bytes, b"new\0");
    }
}
