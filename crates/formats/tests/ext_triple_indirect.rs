use craft_anything_formats::fsread::{Limits, extfs};

const BS: usize = 1024;
const INODE: usize = 6400;
const TRIPLE: usize = 12 + 256 + 256 * 256;

fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn image(skipped_subtrees: usize) -> Vec<u8> {
    let mut data = craft_anything_testkit::firmware::ext_image(false);
    data.resize(44 * BS, 0);
    put32(&mut data, 1028, 44);
    data[INODE + 40..INODE + 100].fill(0);
    let logical = TRIPLE + skipped_subtrees * 256 * 256;
    put32(&mut data, INODE + 4, (logical * BS + 3) as u32);
    put32(&mut data, INODE + 96, 40);
    put32(&mut data, 40 * BS + skipped_subtrees * 4, 41);
    put32(&mut data, 41 * BS, 42);
    put32(&mut data, 42 * BS, 43);
    data[43 * BS..43 * BS + 3].copy_from_slice(b"end");
    data
}

#[test]
fn triple_indirect_data_follows_all_three_levels() {
    let result = extfs::read(&image(0), &Limits::default());
    assert!(result.complete, "{:?}", result.warnings);
    let entry = result.entries.iter().find(|e| e.path == "hello.txt").unwrap();
    let contents = entry.data.as_ref().unwrap();
    assert_eq!(contents.len(), TRIPLE * BS + 3);
    assert!(contents[..TRIPLE * BS].iter().all(|&b| b == 0));
    assert_eq!(&contents[TRIPLE * BS..], b"end");
}

#[test]
fn a_missing_double_indirect_subtree_keeps_its_full_sparse_span() {
    let result = extfs::read(&image(1), &Limits::default());
    assert!(result.complete, "{:?}", result.warnings);
    let contents = result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.as_ref().unwrap();
    let offset = (TRIPLE + 256 * 256) * BS;
    assert_eq!(contents.len(), offset + 3);
    assert!(contents[..offset].iter().all(|&b| b == 0));
    assert_eq!(&contents[offset..], b"end");
}

#[test]
fn unavailable_triple_metadata_is_not_replaced_by_zeros() {
    for slot in [INODE + 96, 40 * BS, 41 * BS] {
        let mut data = image(0);
        put32(&mut data, slot, 1000);
        let result = extfs::read(&data, &Limits::default());
        assert!(!result.complete);
        assert!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.is_none());
        assert!(result.warnings.iter().any(|w| w.contains("indirect block 1000 is unavailable")));
    }
}

#[test]
fn unneeded_pointers_and_existing_recovery_budgets_stay_bounded() {
    let mut small = image(0);
    put32(&mut small, INODE + 4, 3);
    put32(&mut small, INODE + 40, 43);
    put32(&mut small, INODE + 96, u32::MAX);
    let result = extfs::read(&small, &Limits::default());
    assert!(result.complete, "{:?}", result.warnings);
    assert_eq!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.as_deref(), Some(b"end".as_slice()));
    let result = extfs::read(&image(0), &Limits { max_file_bytes: 1 << 20, ..Limits::default() });
    assert!(!result.complete);
    assert!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.is_none());
    assert!(result.warnings.iter().any(|w| w.contains("size budget")));
}
