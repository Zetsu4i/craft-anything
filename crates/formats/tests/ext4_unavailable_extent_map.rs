use craft_anything_formats::fsread::{Limits, extfs};

const INODE: usize = 6400;
const ROOT: usize = INODE + 40;
const CHILD: usize = 11 * 1024;

fn put16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn indexed() -> Vec<u8> {
    let mut data = craft_anything_testkit::firmware::ext_image(true);
    data.resize(CHILD + 1024, 0);
    put32(&mut data, 1028, 12);
    put16(&mut data, ROOT + 6, 1);
    data[ROOT + 12..ROOT + 60].fill(0);
    put32(&mut data, ROOT + 16, 11);
    put16(&mut data, CHILD, 0xf30a);
    put16(&mut data, CHILD + 2, 1);
    put16(&mut data, CHILD + 4, 84);
    put16(&mut data, CHILD + 16, 1);
    put32(&mut data, CHILD + 20, 8);
    data
}

fn unrecovered(data: &[u8]) {
    let result = extfs::read(data, &Limits::default());
    assert!(!result.complete);
    let entry = result.entries.iter().find(|e| e.path == "hello.txt").unwrap();
    assert!(entry.data.is_none(), "invalid mappings must not fabricate file bytes");
    assert!(result.warnings.iter().any(|w| w.contains("extent mapping")));
    assert_eq!(result.entries.iter().find(|e| e.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
}

#[test]
fn invalid_root_headers_do_not_recover_zero_filled_files() {
    for (field, value) in [(0, 0), (2, 5), (4, 5), (6, 9), (16, 0)] {
        let mut data = craft_anything_testkit::firmware::ext_image(true);
        put16(&mut data, ROOT + field, value);
        unrecovered(&data);
    }
}

#[test]
fn missing_and_partial_child_mapping_blocks_leave_data_unknown() {
    let mut missing = indexed();
    put32(&mut missing, ROOT + 16, 1000);
    unrecovered(&missing);
    let mut partial = indexed();
    partial.truncate(CHILD + 24); // the entry fits, but its physical metadata block is unavailable.
    unrecovered(&partial);
    let mut bad_magic = indexed();
    put16(&mut bad_magic, CHILD, 0);
    unrecovered(&bad_magic);
}

#[test]
fn incomplete_later_subtrees_discard_the_partial_mapping() {
    let mut data = indexed();
    put32(&mut data, INODE + 4, 1025);
    put16(&mut data, ROOT + 2, 2);
    put32(&mut data, ROOT + 24, 1);
    put32(&mut data, ROOT + 28, 1000);
    unrecovered(&data);
}

#[test]
fn child_depth_and_capacity_must_match_the_parent_node() {
    for (field, value) in [(6, 1), (2, 85), (4, 85)] {
        let mut data = indexed();
        put16(&mut data, CHILD + field, value);
        unrecovered(&data);
    }
}

#[test]
fn complete_mappings_sparse_files_and_unwritten_extents_still_recover() {
    for data in [craft_anything_testkit::firmware::ext_image(true), indexed()] {
        let result = extfs::read(&data, &Limits::default());
        assert!(result.complete, "{:?}", result.warnings);
        assert_eq!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.as_deref(), Some(b"Hello ext filesystem!\n".as_slice()));
    }
    for sparse in [false, true] {
        let mut data = craft_anything_testkit::firmware::ext_image(true);
        if sparse {
            put16(&mut data, ROOT + 2, 0);
        } else {
            put16(&mut data, ROOT + 16, 32769);
        }
        let result = extfs::read(&data, &Limits::default());
        assert!(result.complete, "{:?}", result.warnings);
        assert_eq!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.as_deref(), Some([0; 22].as_slice()));
    }
}
