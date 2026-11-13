use craft_anything_formats::fsread::{Limits, jffs2};
use craft_anything_testkit::firmware::jffs2_image;

fn big_endian(mut data: Vec<u8>) -> Vec<u8> {
    let mut at = 0;
    while at + 12 <= data.len() {
        let kind = u16::from_le_bytes(data[at + 2..at + 4].try_into().unwrap());
        let size = u32::from_le_bytes(data[at + 4..at + 8].try_into().unwrap()) as usize;
        for off in [0, 2] {
            data[at + off..at + off + 2].reverse();
        }
        let fields: &[usize] = if kind == 0xe001 { &[4, 8, 12, 16, 20, 24, 32, 36] } else { &[4, 8, 12, 16, 20, 28, 32, 36, 40, 44, 48, 52, 60, 64] };
        for &off in fields {
            data[at + off..at + off + 4].reverse();
        }
        if kind == 0xe002 {
            for off in [24, 26, 58] {
                data[at + off..at + off + 2].reverse();
            }
        }
        at += (size + 3) & !3;
    }
    data
}

#[test]
fn both_byte_orders_recover_the_same_tree_and_payload() {
    let data = jffs2_image();
    let expected = jffs2::read(&data, &Limits::default());
    assert!(expected.complete);
    assert_eq!(jffs2::read(&big_endian(data), &Limits::default()), expected);
}

#[test]
fn erased_leading_space_does_not_hide_the_byte_order() {
    let data = big_endian(jffs2_image());
    for padding in [0u8, 0xff] {
        let mut padded = vec![padding; 16];
        padded.extend_from_slice(&data);
        assert_eq!(jffs2::read(&padded, &Limits::default()), jffs2::read(&data, &Limits::default()));
    }
}

#[test]
fn directory_versions_and_deletions_use_big_endian_words() {
    let mut data = jffs2_image();
    let mut deletion = data[48..96].to_vec();
    deletion[16..20].copy_from_slice(&0x10203u32.to_le_bytes());
    deletion[20..24].fill(0);
    data.extend(deletion);
    let image = jffs2::read(&big_endian(data), &Limits::default());
    assert!(image.complete);
    assert!(image.entries.iter().any(|entry| entry.path == "config"));
    assert!(image.entries.iter().all(|entry| entry.path != "config/settings"));
}

#[test]
fn mixed_byte_order_and_cut_nodes_are_incomplete() {
    let original = jffs2_image();
    let mut mixed = big_endian(original.clone());
    mixed[48..96].copy_from_slice(&original[48..96]);
    assert!(!jffs2::read(&mixed, &Limits::default()).complete);
    let mut short = big_endian(original);
    short.truncate(short.len() - 4);
    let image = jffs2::read(&short, &Limits::default());
    assert!(!image.complete);
    assert!(image.entries.iter().find(|entry| entry.path == "config/settings").unwrap().data.is_none());
}
