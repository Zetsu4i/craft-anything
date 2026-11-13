use craft_anything_formats::fsread::{self, Limits};

const BS: usize = 1024;
const PER: usize = BS / 4;
const FILE_INODE: usize = 5120 + 10 * 128;
const FIRST_DOUBLE: usize = 12 + PER;
const FIRST_TRIPLE: usize = FIRST_DOUBLE + PER * PER;

fn put(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn sparse_file(nblocks: usize) -> Vec<u8> {
    let mut data = craft_anything_testkit::firmware::ext_image(false);
    data.resize(15 * BS, 0);
    put(&mut data, 1024 + 4, 15);
    data[FILE_INODE + 40..FILE_INODE + 100].fill(0);
    put(&mut data, FILE_INODE + 4, (nblocks * BS) as u32);
    data
}

fn assert_unrecovered(data: &[u8], nblocks: usize, warning: &str) {
    let image = fsread::extfs::read(data, &Limits::default());
    let hello = image.entries.iter().find(|e| e.path == "hello.txt").unwrap();
    assert_eq!(hello.size, (nblocks * BS) as u64);
    assert!(hello.data.is_none(), "unknown indirect mappings must not become recovered zero bytes");
    assert!(!image.complete);
    assert!(image.warnings.iter().any(|w| w.contains(warning)), "{:?}", image.warnings);
    assert_eq!(image.entries.iter().find(|e| e.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
}

fn recovered_file(data: &[u8], nblocks: usize) -> Vec<u8> {
    let image = fsread::extfs::read(data, &Limits::default());
    assert!(image.complete, "{:?}", image.warnings);
    assert!(image.warnings.is_empty(), "{:?}", image.warnings);
    assert_eq!(image.entries.iter().find(|e| e.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
    let hello = image.entries.into_iter().find(|e| e.path == "hello.txt").unwrap();
    assert_eq!(hello.size, (nblocks * BS) as u64);
    let bytes = hello.data.expect("known sparse holes and readable mappings are recovered");
    assert_eq!(bytes.len(), nblocks * BS);
    bytes
}

#[test]
fn nonzero_indirect_roots_outside_the_image_are_unrecovered() {
    for (slot, first) in [(88, 12), (92, FIRST_DOUBLE)] {
        for block in [15, u32::MAX] {
            let mut data = sparse_file(first + 1);
            put(&mut data, FILE_INODE + slot, block);
            assert_unrecovered(&data, first + 1, "indirect block");
        }
    }
}

#[test]
fn a_truncated_indirect_root_is_not_a_sparse_hole() {
    for (slot, first) in [(88, 12), (92, FIRST_DOUBLE)] {
        let mut data = sparse_file(first + 1);
        put(&mut data, FILE_INODE + slot, 14);
        data.truncate(15 * BS - 1);
        assert_unrecovered(&data, first + 1, "indirect block");
    }
}

#[test]
fn a_missing_double_indirect_child_cannot_shift_later_mappings() {
    let nblocks = FIRST_DOUBLE + PER + 1;
    for (missing, truncated) in [(15, false), (u32::MAX, false), (14, true)] {
        let mut data = sparse_file(nblocks);
        put(&mut data, FILE_INODE + 92, 11);
        put(&mut data, 11 * BS, missing);
        put(&mut data, 11 * BS + 4, 12);
        put(&mut data, 12 * BS, 13);
        data[13 * BS..13 * BS + 4].copy_from_slice(b"tail");
        if truncated {
            data.truncate(15 * BS - 1);
        }
        assert_unrecovered(&data, nblocks, "indirect block");
    }
}

#[test]
fn zero_indirect_roots_remain_sparse_through_the_first_triple_block() {
    let nblocks = FIRST_TRIPLE + 1;
    let bytes = recovered_file(&sparse_file(nblocks), nblocks);
    assert!(bytes.iter().all(|b| *b == 0));
}

#[test]
fn a_zero_double_indirect_child_keeps_the_following_data_in_place() {
    let logical = FIRST_DOUBLE + PER;
    let mut data = sparse_file(logical + 1);
    put(&mut data, FILE_INODE + 92, 11);
    put(&mut data, 11 * BS + 4, 12);
    put(&mut data, 12 * BS, 13);
    data[13 * BS..13 * BS + 4].copy_from_slice(b"tail");
    let bytes = recovered_file(&data, logical + 1);
    assert!(bytes[..logical * BS].iter().all(|b| *b == 0));
    assert_eq!(&bytes[logical * BS..logical * BS + 4], b"tail");
    assert!(bytes[logical * BS + 4..].iter().all(|b| *b == 0));
}

#[test]
fn bad_indirect_roots_past_the_file_end_are_not_followed() {
    for (slot, first) in [(88, 12), (92, FIRST_DOUBLE), (96, FIRST_TRIPLE)] {
        let mut data = sparse_file(first);
        put(&mut data, FILE_INODE + slot, u32::MAX);
        let bytes = recovered_file(&data, first);
        assert!(bytes.iter().all(|b| *b == 0));
    }
}

#[test]
fn bad_indirect_children_past_the_file_end_are_not_followed() {
    for (slot, first, double) in [(88, 12, false), (92, FIRST_DOUBLE, true)] {
        let mut data = sparse_file(first + 1);
        put(&mut data, FILE_INODE + slot, 11);
        put(&mut data, 11 * BS + 4, u32::MAX);
        if double {
            put(&mut data, 11 * BS, 12);
            put(&mut data, 12 * BS, 13);
            put(&mut data, 12 * BS + 4, u32::MAX);
        } else {
            put(&mut data, 11 * BS, 13);
        }
        data[13 * BS..13 * BS + 4].copy_from_slice(b"tail");
        let bytes = recovered_file(&data, first + 1);
        assert!(bytes[..first * BS].iter().all(|b| *b == 0));
        assert_eq!(&bytes[first * BS..first * BS + 4], b"tail");
        assert!(bytes[first * BS + 4..].iter().all(|b| *b == 0));
    }
}

#[test]
fn triple_roots_distinguish_readable_sparse_maps_from_missing_metadata() {
    let nblocks = FIRST_TRIPLE + 1;
    for block in [15, u32::MAX] {
        let mut data = sparse_file(nblocks);
        put(&mut data, FILE_INODE + 96, block);
        assert_unrecovered(&data, nblocks, "indirect block");
    }
    let mut sparse = sparse_file(nblocks);
    put(&mut sparse, FILE_INODE + 96, 11);
    assert!(recovered_file(&sparse, nblocks).iter().all(|&byte| byte == 0));
}
