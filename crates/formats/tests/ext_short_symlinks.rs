use craft_anything_formats::fsread::{FsKind, Limits, extfs};

const INODE: usize = 6400;

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn link(extents: bool, target: &[u8], inline: bool) -> Vec<u8> {
    let mut bytes = craft_anything_testkit::firmware::ext_image(extents);
    bytes[INODE..INODE + 2].copy_from_slice(&0xa1ffu16.to_le_bytes());
    put32(&mut bytes, INODE + 4, target.len() as u32);
    if inline {
        put32(&mut bytes, INODE + 32, 0);
        bytes[INODE + 40..INODE + 100].fill(0);
        bytes[INODE + 40..INODE + 40 + target.len()].copy_from_slice(target);
    } else {
        put32(&mut bytes, INODE + 28, 2);
        bytes[8192..8192 + target.len()].copy_from_slice(target);
    }
    bytes
}

fn target(bytes: &[u8]) -> Option<String> {
    let result = extfs::read(bytes, &Limits::default());
    assert!(result.complete, "{:?}", result.warnings);
    let entry = result.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
    assert_eq!(entry.kind, FsKind::Symlink);
    entry.symlink_target.clone()
}

#[test]
fn short_targets_in_data_blocks_are_not_read_from_block_pointers() {
    for extents in [false, true] {
        for text in ["x", "destination", "a".repeat(59).as_str(), "b".repeat(60).as_str()] {
            assert_eq!(target(&link(extents, text.as_bytes(), false)).as_deref(), Some(text));
        }
    }
}

#[test]
fn inline_targets_allow_an_external_attribute_block() {
    for attribute_block in [0, 6] {
        let mut bytes = link(false, b"destination", true);
        put32(&mut bytes, INODE + 104, attribute_block);
        put32(&mut bytes, INODE + 28, if attribute_block == 0 { 0 } else { 2 });
        assert_eq!(target(&bytes).as_deref(), Some("destination"));
    }
}

#[test]
fn ext4_attribute_features_use_their_own_allocation_units() {
    let mut clustered = link(true, b"clustered", true);
    put32(&mut clustered, 1024 + 100, 0x200); // BIGALLOC
    put32(&mut clustered, 1024 + 28, 2); // 4096-byte allocation cluster
    put32(&mut clustered, INODE + 104, 6);
    put32(&mut clustered, INODE + 28, 8);
    assert_eq!(target(&clustered).as_deref(), Some("clustered"));

    let mut ea_inode = link(true, b"external-values", true);
    put32(&mut ea_inode, 1024 + 96, 0x40 | 0x400); // EXTENTS and EA_INODE
    put32(&mut ea_inode, INODE + 28, 12);
    assert_eq!(target(&ea_inode).as_deref(), Some("external-values"));

    let mut huge = link(true, b"huge-unit", true);
    put32(&mut huge, 1024 + 100, 8);
    put32(&mut huge, INODE + 32, 0x4_0000); // HUGE_FILE counts filesystem blocks
    put32(&mut huge, INODE + 104, 6);
    put32(&mut huge, INODE + 28, 1);
    assert_eq!(target(&huge).as_deref(), Some("huge-unit"));
}

#[test]
fn ext4_high_allocation_bits_cannot_make_a_block_symlink_look_inline() {
    let mut bytes = link(true, b"allocated", false);
    put32(&mut bytes, 1024 + 100, 8); // HUGE_FILE supports high i_blocks bits
    put32(&mut bytes, INODE + 28, 0);
    bytes[INODE + 116..INODE + 118].copy_from_slice(&1u16.to_le_bytes());
    assert_eq!(target(&bytes).as_deref(), Some("allocated"));
}

#[test]
fn unavailable_target_data_remains_unknown_with_its_declared_size() {
    let mut bytes = link(false, b"missing", false);
    put32(&mut bytes, INODE + 40, u32::MAX);
    let result = extfs::read(&bytes, &Limits::default());
    let entry = result.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
    assert!(!result.complete);
    assert_eq!(entry.kind, FsKind::Symlink);
    assert_eq!(entry.size, 7);
    assert!(entry.symlink_target.is_none());
    assert!(result.warnings.iter().any(|warning| warning.contains("outside the image")));
}

#[test]
fn inline_data_inodes_are_not_mistaken_for_fast_symlinks() {
    let mut bytes = link(true, b"unread", true);
    put32(&mut bytes, INODE + 32, 0x1000_0000);
    let result = extfs::read(&bytes, &Limits::default());
    let entry = result.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
    assert!(!result.complete);
    assert!(entry.symlink_target.is_none());
    assert!(result.warnings.iter().any(|warning| warning.contains("inline-data")));
}
