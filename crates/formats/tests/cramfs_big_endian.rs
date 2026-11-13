use craft_anything_formats::fsread::{self, Limits};

fn word(data: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap())
}

fn big_endian(little: &[u8]) -> Vec<u8> {
    let mut data = little.to_vec();
    for offset in [0, 4, 8, 12, 32, 36, 40, 44] {
        data[offset..offset + 4].copy_from_slice(&word(little, offset).to_be_bytes());
    }
    for offset in [64, 76, 100, 116] {
        for field in [offset, offset + 2] {
            let value = u16::from_le_bytes(little[field..field + 2].try_into().unwrap());
            data[field..field + 2].copy_from_slice(&value.to_be_bytes());
        }
        let size_gid = word(little, offset + 4);
        let size = size_gid & 0x00ff_ffff;
        let packed_size = (size << 8) | (size_gid >> 24);
        data[offset + 4..offset + 8].copy_from_slice(&packed_size.to_be_bytes());
        let name_offset = word(little, offset + 8);
        let packed_offset = ((name_offset & 0x3f) << 26) | (name_offset >> 6);
        data[offset + 8..offset + 12].copy_from_slice(&packed_offset.to_be_bytes());
    }
    for inode in [76, 116] {
        let blocks = (word(little, inode + 4) & 0x00ff_ffff).div_ceil(4096);
        let pointers = (word(little, inode + 8) >> 6) as usize * 4;
        for block in 0..blocks as usize {
            let offset = pointers + block * 4;
            data[offset..offset + 4].copy_from_slice(&word(little, offset).to_be_bytes());
        }
    }
    data
}

#[test]
fn big_endian_files_and_nested_names_match_little_endian() {
    let little = craft_anything_testkit::firmware::cramfs_image();
    let expected = fsread::cramfs::read(&little, &Limits::default());
    assert!(expected.complete);
    assert_eq!(expected.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.as_deref(), Some(b"Hello from cramfs!\n".as_slice()));
    assert_eq!(fsread::cramfs::read(&big_endian(&little), &Limits::default()), expected);
}

#[test]
fn uncompressed_flags_are_decoded_after_the_pointer_byte_order() {
    let contents = [b'R'; 4113];
    let mut little = craft_anything_testkit::firmware::cramfs_image();
    little.resize(little.len().div_ceil(4) * 4, 0);
    let pointers = little.len();
    little.resize(pointers + 8, 0);
    for (index, block) in contents.chunks(4096).enumerate() {
        little.extend_from_slice(block);
        let end = little.len() as u32 | 0x8000_0000;
        little[pointers + index * 4..pointers + index * 4 + 4].copy_from_slice(&end.to_le_bytes());
    }
    let end = little.len() as u32;
    little[4..8].copy_from_slice(&end.to_le_bytes());
    little[8..12].copy_from_slice(&0x800u32.to_le_bytes());
    little[80..84].copy_from_slice(&(contents.len() as u32).to_le_bytes());
    little[84..88].copy_from_slice(&(((pointers as u32 / 4) << 6) | 3).to_le_bytes());
    let expected = fsread::cramfs::read(&little, &Limits::default());
    assert!(expected.complete, "{:?}", expected.warnings);
    assert_eq!(expected.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.as_deref(), Some(contents.as_slice()));
    assert_eq!(fsread::cramfs::read(&big_endian(&little), &Limits::default()), expected);
}

#[test]
fn owner_bits_do_not_change_big_endian_modes_or_sizes() {
    let mut little = craft_anything_testkit::firmware::cramfs_image();
    for inode in [64, 76, 100, 116] {
        little[inode + 2..inode + 4].copy_from_slice(&0xabcd_u16.to_le_bytes());
        little[inode + 7] = 0x7e;
    }
    let expected = fsread::cramfs::read(&little, &Limits::default());
    assert!(expected.complete);
    assert_eq!(expected.bytes, 25);
    assert_eq!(fsread::cramfs::read(&big_endian(&little), &Limits::default()), expected);
}

#[test]
fn big_endian_recovery_obeys_the_same_limits() {
    let little = craft_anything_testkit::firmware::cramfs_image();
    let big = big_endian(&little);
    for limits in [
        Limits { max_file_bytes: 18, ..Limits::default() },
        Limits { max_total_bytes: 6, ..Limits::default() },
        Limits { max_entries: 1, ..Limits::default() },
        Limits { max_depth: 0, ..Limits::default() },
    ] {
        assert_eq!(fsread::cramfs::read(&big, &limits), fsread::cramfs::read(&little, &limits));
    }
}

#[test]
fn a_big_endian_bad_block_pointer_keeps_valid_siblings() {
    let little = craft_anything_testkit::firmware::cramfs_image();
    let mut data = big_endian(&little);
    data[136..140].copy_from_slice(&136u32.to_be_bytes());
    let image = fsread::cramfs::read(&data, &Limits::default());
    assert!(!image.complete);
    assert!(image.warnings.iter().any(|warning| warning.contains("block pointer")));
    assert!(image.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.is_none());
    assert_eq!(image.entries.iter().find(|entry| entry.path == "dir/inner").unwrap().data.as_deref(), Some(b"nested".as_slice()));
}

#[test]
fn truncated_headers_stay_incomplete_in_both_byte_orders() {
    let little = craft_anything_testkit::firmware::cramfs_image();
    for data in [&little, &big_endian(&little)] {
        for length in 0..76 {
            let image = fsread::cramfs::read(&data[..length], &Limits::default());
            assert!(!image.complete);
            assert!(image.entries.is_empty());
        }
    }
}
