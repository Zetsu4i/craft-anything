use craft_anything_formats::fsread::{Limits, cramfs};
use craft_anything_testkit::firmware::cramfs_image;

fn version_two(size: usize) -> Vec<u8> {
    let mut data = cramfs_image();
    data[4..8].copy_from_slice(&(size as u32).to_le_bytes());
    data[8..12].copy_from_slice(&1u32.to_le_bytes());
    data
}

#[test]
fn appended_bytes_cannot_complete_version_two_file_data() {
    let data = version_two(140);
    let image = cramfs::read(&data, &Limits::default());
    assert!(!image.complete);
    assert!(image.entries.iter().filter(|e| e.kind == craft_anything_formats::fsread::FsKind::File).all(|e| e.data.is_none()));
}

#[test]
fn appended_bytes_cannot_supply_directory_records() {
    let image = cramfs::read(&version_two(100), &Limits::default());
    assert!(!image.complete);
    assert!(image.entries.iter().all(|e| e.path != "dir" && e.path != "dir/inner"));
}

#[test]
fn invalid_and_physically_short_v2_extents_are_incomplete() {
    for size in [0, 75] {
        let image = cramfs::read(&version_two(size), &Limits::default());
        assert!(!image.complete);
        assert!(image.entries.is_empty());
    }
    let data = version_two(cramfs_image().len() + 64);
    let image = cramfs::read(&data, &Limits::default());
    assert!(!image.complete);
    assert_eq!(image.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.as_deref(), Some(b"Hello from cramfs!\n".as_slice()));
}

#[test]
fn complete_v2_and_legacy_images_keep_their_contents() {
    let legacy = cramfs_image();
    let expected = cramfs::read(&legacy, &Limits::default());
    assert!(expected.complete);
    assert_eq!(cramfs::read(&version_two(legacy.len()), &Limits::default()), expected);
    let mut padded = version_two(legacy.len());
    padded.extend_from_slice(b"unrelated appended bytes");
    assert_eq!(cramfs::read(&padded, &Limits::default()), expected);
    let mut old = legacy;
    old[4..8].fill(0);
    assert_eq!(cramfs::read(&old, &Limits::default()), expected);
}

#[test]
fn big_endian_v2_size_is_checked_in_its_own_byte_order() {
    let mut data = vec![0u8; 100];
    data[..4].copy_from_slice(&0x28cd3d45u32.to_be_bytes());
    data[4..8].copy_from_slice(&76u32.to_be_bytes());
    data[8..12].copy_from_slice(&1u32.to_be_bytes());
    data[64..66].copy_from_slice(&0x41edu16.to_be_bytes());
    data[68..72].copy_from_slice(&(24u32 << 8).to_be_bytes());
    data[72..76].copy_from_slice(&19u32.to_be_bytes());
    data[76..78].copy_from_slice(&0x81a4u16.to_be_bytes());
    data[84..88].copy_from_slice(&(3u32 << 26).to_be_bytes());
    data[88..92].copy_from_slice(b"tail");
    let image = cramfs::read(&data, &Limits::default());
    assert!(!image.complete);
    assert!(image.entries.is_empty());
}

#[test]
fn unavailable_block_pointer_tables_make_declared_extents_incomplete() {
    for size in 136..140 {
        let image = cramfs::read(&version_two(size), &Limits::default());
        assert!(!image.complete, "declared extent {size}: {:?}", image.warnings);
        assert!(image.warnings.iter().any(|warning| warning.contains("block pointer")));
        let files: Vec<_> = image.entries.iter().filter(|entry| entry.kind == craft_anything_formats::fsread::FsKind::File).collect();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|entry| entry.data.is_none() && entry.size > 0));
        assert_eq!(image.bytes, 0);
    }
}

#[test]
fn physically_truncated_legacy_block_pointer_tables_are_incomplete() {
    for size in 136..140 {
        let mut data = cramfs_image();
        data.truncate(size);
        let image = cramfs::read(&data, &Limits::default());
        assert!(!image.complete, "physical extent {size}: {:?}", image.warnings);
        assert!(image.warnings.iter().any(|warning| warning.contains("block pointer")));
        assert_eq!(image.entries.iter().filter(|entry| entry.kind == craft_anything_formats::fsread::FsKind::File).count(), 2);
        assert_eq!(image.bytes, 0);
    }
}
