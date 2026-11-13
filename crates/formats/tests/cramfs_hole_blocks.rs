use craft_anything_formats::fsread::{self, Limits};

fn fixture(contents: &[u8], holes: bool) -> (Vec<u8>, usize) {
    let mut data = craft_anything_testkit::firmware::cramfs_image();
    data.resize(data.len().div_ceil(4) * 4, 0);
    let pointers = data.len();
    data.resize(pointers + contents.len().div_ceil(4096) * 4, 0);
    for (index, block) in contents.chunks(4096).enumerate() {
        if !holes || block.iter().any(|&byte| byte != 0) {
            data.extend_from_slice(&miniz_oxide::deflate::compress_to_vec_zlib(block, 6));
        }
        let end = data.len() as u32;
        data[pointers + index * 4..pointers + index * 4 + 4].copy_from_slice(&end.to_le_bytes());
    }
    let end = data.len() as u32;
    data[4..8].copy_from_slice(&end.to_le_bytes());
    data[8..12].copy_from_slice(&if holes { 0x100u32 } else { 0 }.to_le_bytes());
    data[80..84].copy_from_slice(&(contents.len() as u32).to_le_bytes());
    data[84..88].copy_from_slice(&(((pointers as u32 / 4) << 6) | 3).to_le_bytes());
    (data, pointers)
}

fn assert_recovered(contents: &[u8], holes: bool) {
    let (data, _) = fixture(contents, holes);
    let image = fsread::cramfs::read(&data, &Limits::default());
    assert!(image.complete, "{:?}", image.warnings);
    assert!(image.warnings.is_empty());
    assert_eq!(image.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.as_deref(), Some(contents));
    assert_eq!(image.entries.iter().find(|entry| entry.path == "dir/inner").unwrap().data.as_deref(), Some(b"nested".as_slice()));
    assert_eq!(image.bytes, contents.len() as u64 + 6);
}

#[test]
fn full_and_partial_zero_files_recover_their_declared_size() {
    for size in [1, 17, 4096, 4113, 8192] {
        assert_recovered(&vec![0; size], true);
    }
}

#[test]
fn a_middle_hole_preserves_the_following_compressed_bytes() {
    let mut contents = vec![b'A'; 4096];
    contents.resize(8192, 0);
    contents.extend_from_slice(b"tail after the zero page");
    assert_recovered(&contents, true);
}

#[test]
fn a_partial_final_hole_stops_at_the_file_length() {
    let mut contents = vec![b'Q'; 4096];
    contents.resize(4113, 0);
    assert_recovered(&contents, true);
}

#[test]
fn consecutive_leading_holes_keep_the_compressed_tail_position() {
    let mut contents = vec![0; 8192];
    contents.extend_from_slice(b"last");
    assert_recovered(&contents, true);
}

#[test]
fn compressed_zero_pages_still_recover() {
    assert_recovered(&vec![0; 4113], false);
}

#[test]
fn hole_contents_still_obey_file_and_total_budgets() {
    let (data, _) = fixture(&vec![0; 4096], true);
    for limits in [Limits { max_file_bytes: 4095, ..Limits::default() }, Limits { max_total_bytes: 4095, ..Limits::default() }] {
        let image = fsread::cramfs::read(&data, &limits);
        assert!(!image.complete);
        assert!(image.warnings.iter().any(|warning| warning.contains("size budget")));
        assert!(image.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.is_none());
        assert_eq!(image.entries.iter().find(|entry| entry.path == "dir/inner").unwrap().data.as_deref(), Some(b"nested".as_slice()));
        assert_eq!(image.bytes, 6);
    }
}

#[test]
fn a_backward_pointer_is_not_treated_as_a_hole() {
    let (mut data, pointers) = fixture(&vec![0; 4096], true);
    data[pointers..pointers + 4].copy_from_slice(&(pointers as u32).to_le_bytes());
    let image = fsread::cramfs::read(&data, &Limits::default());
    assert!(!image.complete);
    assert!(image.warnings.iter().any(|warning| warning.contains("block pointer")));
    assert!(image.entries.iter().find(|entry| entry.path == "hello.txt").unwrap().data.is_none());
}
