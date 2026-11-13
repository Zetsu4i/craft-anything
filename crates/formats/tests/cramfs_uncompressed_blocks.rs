use craft_anything_formats::fsread::{self, FsImage, FsKind, Limits};

const RAW: u32 = 0x8000_0000;

fn fixture(contents: &[u8], raw_blocks: &[usize]) -> (Vec<u8>, usize) {
    let mut data = craft_anything_testkit::firmware::cramfs_image();
    data.resize(data.len().div_ceil(4) * 4, 0);
    let pointers = data.len();
    data.resize(pointers + contents.len().div_ceil(4096) * 4, 0);
    for (index, block) in contents.chunks(4096).enumerate() {
        let raw = raw_blocks.contains(&index);
        if raw {
            data.extend_from_slice(block);
        } else {
            data.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(block, 6));
        }
        let end = data.len() as u32 | if raw { RAW } else { 0 };
        data[pointers + index * 4..pointers + index * 4 + 4].copy_from_slice(&end.to_le_bytes());
    }
    let end = data.len() as u32;
    data[4..8].copy_from_slice(&end.to_le_bytes());
    data[8..12].copy_from_slice(&if raw_blocks.is_empty() { 0u32 } else { 0x800 }.to_le_bytes());
    data[80..84].copy_from_slice(&(contents.len() as u32).to_le_bytes());
    data[84..88].copy_from_slice(&(((pointers as u32 / 4) << 6) | 3).to_le_bytes());
    (data, pointers)
}

fn assert_recovered(contents: &[u8], raw_blocks: &[usize]) {
    let (data, _) = fixture(contents, raw_blocks);
    let image = fsread::cramfs::read(&data, &Limits::default());
    assert!(image.complete, "{:?}", image.warnings);
    assert!(image.warnings.is_empty());
    assert_eq!(image.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.as_deref(), Some(contents));
    assert_eq!(image.entries.iter().find(|entry| entry.path == "dir/inner").unwrap().data.as_deref(), Some(b"nested".as_slice()));
    assert_eq!(image.bytes, contents.len() as u64 + 6);
}

fn assert_skipped_with_sibling(image: &FsImage) {
    assert!(!image.complete);
    assert!(!image.warnings.is_empty());
    assert!(image.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.is_none());
    assert_eq!(image.entries.iter().find(|entry| entry.path == "dir/inner").unwrap().data.as_deref(), Some(b"nested".as_slice()));
    assert_eq!(image.bytes, 6);
}

#[test]
fn uncompressed_full_and_partial_blocks_recover_exact_bytes() {
    for size in [1, 17, 4096, 4113] {
        let contents: Vec<u8> = (0..size).map(|index| (index % 251) as u8).collect();
        assert_recovered(&contents, &[0, 1]);
    }
}

#[test]
fn mixed_uncompressed_and_compressed_blocks_keep_their_positions() {
    let mut contents: Vec<u8> = (0..4096).map(|index| (index % 251) as u8).collect();
    contents.extend_from_slice(&[b'Q'; 4096]);
    contents.extend_from_slice(b"uncompressed tail");
    assert_recovered(&contents, &[0, 2]);
}

#[test]
fn uncompressed_symlink_targets_recover() {
    let (mut data, _) = fixture(b"dir/inner", &[0]);
    data[76..78].copy_from_slice(&0xa1ffu16.to_le_bytes());
    let image = fsread::cramfs::read(&data, &Limits::default());
    assert!(image.complete, "{:?}", image.warnings);
    let link = image.entries.iter().find(|entry| entry.path == "hello.txt").unwrap();
    assert_eq!(link.kind, FsKind::Symlink);
    assert_eq!(link.symlink_target.as_deref(), Some("dir/inner"));
    assert_eq!(image.bytes, 15);
}

#[test]
fn short_and_overlong_uncompressed_blocks_do_not_recover() {
    for declared_size in [16u32, 18] {
        let (mut data, _) = fixture(&[b'a'; 17], &[0]);
        data[80..84].copy_from_slice(&declared_size.to_le_bytes());
        assert_skipped_with_sibling(&fsread::cramfs::read(&data, &Limits::default()));
    }
}

#[test]
fn uncompressed_contents_still_obey_file_and_total_budgets() {
    let (data, _) = fixture(&[b'x'; 4096], &[0]);
    for limits in [Limits { max_file_bytes: 4095, ..Limits::default() }, Limits { max_total_bytes: 4095, ..Limits::default() }] {
        let image = fsread::cramfs::read(&data, &limits);
        assert!(image.warnings.iter().any(|warning| warning.contains("size budget")));
        assert_skipped_with_sibling(&image);
    }
}

#[test]
fn ordinary_compressed_blocks_still_recover() {
    assert_recovered(&[b'z'; 4113], &[]);
}

#[test]
fn corrupt_ordinary_and_direct_pointers_are_not_recovered() {
    let (data, pointers) = fixture(&[b'x'; 17], &[0]);
    for encoded in [(data.len() as u32).div_ceil(4) | 0xc000_0000, (data.len() as u32 + 1) | RAW, pointers as u32 | RAW] {
        let mut corrupt = data.clone();
        corrupt[pointers..pointers + 4].copy_from_slice(&encoded.to_le_bytes());
        assert_skipped_with_sibling(&fsread::cramfs::read(&corrupt, &Limits::default()));
    }
}
