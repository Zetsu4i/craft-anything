use craft_anything_formats::fsread::{Limits, cramfs};

const TABLE: usize = 92;
const DIRECT: u32 = 0x4000_0000;
const RAW: u32 = 0x8000_0000;

fn put32(bytes: &mut [u8], offset: usize, value: u32, big: bool) {
    let value = if big { value.to_be_bytes() } else { value.to_le_bytes() };
    bytes[offset..offset + 4].copy_from_slice(&value);
}

fn base(size: usize, big: bool) -> Vec<u8> {
    let mut bytes = vec![0; (TABLE + size.div_ceil(4096) * 4 + 16).div_ceil(4) * 4];
    put32(&mut bytes, 0, 0x28cd3d45, big);
    put32(&mut bytes, 8, 0x800, big); // extended block pointers
    for (offset, mode, length, name_words, data_offset) in [(64, 0x41edu16, 16, 0, 76), (76, 0x81a4, size, 1, TABLE)] {
        bytes[offset..offset + 2].copy_from_slice(&if big { mode.to_be_bytes() } else { mode.to_le_bytes() });
        put32(&mut bytes, offset + 4, if big { (length as u32) << 8 } else { length as u32 }, big);
        put32(&mut bytes, offset + 8, if big { (name_words << 26) | (data_offset as u32 / 4) } else { ((data_offset as u32 / 4) << 6) | name_words }, big);
    }
    bytes[88..92].copy_from_slice(b"file");
    bytes
}

fn direct(bytes: &mut Vec<u8>, index: usize, payload: &[u8], raw: bool, big: bool) -> u32 {
    bytes.resize(bytes.len().div_ceil(4) * 4, 0);
    let pointer = DIRECT | if raw { RAW } else { 0 } | (bytes.len() as u32 / 4);
    if raw {
        bytes.extend_from_slice(payload);
    } else {
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(payload, 6);
        let length = compressed.len() as u16;
        bytes.extend_from_slice(&if big { length.to_be_bytes() } else { length.to_le_bytes() });
        bytes.extend_from_slice(&compressed);
    }
    put32(bytes, TABLE + index * 4, pointer, big);
    pointer
}

fn recovered(bytes: &[u8], expected: &[u8]) {
    let result = cramfs::read(bytes, &Limits::default());
    assert!(result.complete, "{:?}", result.warnings);
    assert_eq!(result.entries[0].data.as_deref(), Some(expected));
    assert_eq!(result.bytes, expected.len() as u64);
}

#[test]
fn direct_raw_and_compressed_blocks_decode_in_both_byte_orders() {
    for big in [false, true] {
        for raw in [false, true] {
            for size in [7, 4096] {
                let payload = vec![b'a'; size];
                let mut bytes = base(size, big);
                direct(&mut bytes, 0, &payload, raw, big);
                recovered(&bytes, &payload);
            }
        }
    }
}

#[test]
fn ordinary_blocks_follow_the_physical_end_of_a_direct_block() {
    for big in [false, true] {
        for first_raw in [false, true] {
            for last_raw in [false, true] {
                let first = vec![b'a'; 4096];
                let mut bytes = base(4099, big);
                direct(&mut bytes, 0, &first, first_raw, big);
                let last = if last_raw { b"end".to_vec() } else { miniz_oxide::deflate::compress_to_vec_zlib(b"end", 6) };
                bytes.extend_from_slice(&last);
                let end = bytes.len() as u32 | if last_raw { RAW } else { 0 };
                put32(&mut bytes, TABLE + 4, end, big);
                let mut expected = first;
                expected.extend_from_slice(b"end");
                recovered(&bytes, &expected);
            }
        }
    }
}

#[test]
fn multiple_logical_blocks_can_share_one_direct_payload() {
    for raw in [false, true] {
        let payload = vec![b's'; 4096];
        let mut bytes = base(8192, false);
        let pointer = direct(&mut bytes, 0, &payload, raw, false);
        put32(&mut bytes, TABLE + 4, pointer, false);
        recovered(&bytes, &payload.repeat(2));
    }
}

#[test]
fn an_ordinary_hole_can_follow_a_direct_block() {
    let mut bytes = base(4100, false);
    let payload = vec![b'x'; 4096];
    direct(&mut bytes, 0, &payload, false, false);
    let end = bytes.len() as u32;
    put32(&mut bytes, TABLE + 4, end, false);
    let mut expected = payload;
    expected.extend_from_slice(&[0; 4]);
    recovered(&bytes, &expected);
}

#[test]
fn unavailable_direct_ranges_and_truncated_lengths_are_not_recovered() {
    let mut bad_pointer = base(5, false);
    put32(&mut bad_pointer, TABLE, DIRECT | RAW | 0x3fff_ffff, false);
    let mut bad_length = base(5, false);
    let start = bad_length.len() as u32 / 4;
    put32(&mut bad_length, TABLE, DIRECT | start, false);
    bad_length.extend_from_slice(&100u16.to_le_bytes());
    bad_length.push(0);
    let mut missing_length = bad_length.clone();
    missing_length.truncate(missing_length.len() - 2);
    for bytes in [bad_pointer, bad_length, missing_length] {
        let result = cramfs::read(&bytes, &Limits::default());
        assert!(!result.complete);
        assert!(result.entries[0].data.is_none());
        assert_eq!(result.bytes, 0);
        assert!(!result.warnings.is_empty());
    }
}

#[test]
fn direct_recovery_obeys_the_existing_output_budget() {
    let mut bytes = base(5, false);
    direct(&mut bytes, 0, b"hello", true, false);
    let result = cramfs::read(&bytes, &Limits { max_file_bytes: 4, ..Limits::default() });
    assert!(!result.complete);
    assert!(result.entries[0].data.is_none());
    assert_eq!(result.bytes, 0);
}
