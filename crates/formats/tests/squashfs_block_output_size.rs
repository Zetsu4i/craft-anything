use craft_anything_formats::fsread::{Limits, squashfs};

fn put16(data: &mut [u8], at: usize, value: u16) {
    data[at..at + 2].copy_from_slice(&value.to_le_bytes());
}
fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}
fn put64(data: &mut [u8], at: usize, value: u64) {
    data[at..at + 8].copy_from_slice(&value.to_le_bytes());
}

fn image(blocks: &[(Vec<u8>, bool)], size: usize) -> Vec<u8> {
    let mut data = vec![0; 96];
    for (bytes, _) in blocks {
        data.extend_from_slice(bytes);
    }
    let root = 32 + blocks.len() * 4;
    let mut inodes = vec![0; root + 32];
    put16(&mut inodes, 0, 2);
    put32(&mut inodes, 12, 2);
    put32(&mut inodes, 16, 96);
    put32(&mut inodes, 20, u32::MAX);
    put32(&mut inodes, 28, size as u32);
    for (i, (bytes, raw)) in blocks.iter().enumerate() {
        put32(&mut inodes, 32 + 4 * i, bytes.len() as u32 | if *raw && !bytes.is_empty() { 0x0100_0000 } else { 0 });
    }
    put16(&mut inodes, root, 1);
    put32(&mut inodes, root + 12, 1);
    put32(&mut inodes, root + 20, 2);
    put16(&mut inodes, root + 24, 27);
    put32(&mut inodes, root + 28, 1);
    let inode_table = data.len() as u64;
    data.extend_from_slice(&(inodes.len() as u16 | 0x8000).to_le_bytes());
    data.extend_from_slice(&inodes);
    let dir_table = data.len() as u64;
    let mut dirs = vec![0; 24];
    put32(&mut dirs, 8, 1);
    put16(&mut dirs, 14, 1);
    put16(&mut dirs, 16, 2);
    put16(&mut dirs, 18, 3);
    dirs[20..24].copy_from_slice(b"file");
    data.extend_from_slice(&(24u16 | 0x8000).to_le_bytes());
    data.extend_from_slice(&dirs);
    let id_table = data.len() as u64;
    data.extend_from_slice(&(id_table + 8).to_le_bytes());
    data.extend_from_slice(&[4, 0x80, 0, 0, 0, 0]);
    data[..4].copy_from_slice(b"hsqs");
    put32(&mut data, 4, 2);
    put32(&mut data, 12, 4096);
    put16(&mut data, 20, 1);
    put16(&mut data, 22, 12);
    put16(&mut data, 26, 1);
    put16(&mut data, 28, 4);
    put64(&mut data, 32, root as u64);
    let used = data.len() as u64;
    put64(&mut data, 40, used);
    put64(&mut data, 48, id_table);
    put64(&mut data, 56, u64::MAX);
    put64(&mut data, 64, inode_table);
    put64(&mut data, 72, dir_table);
    put64(&mut data, 80, u64::MAX);
    put64(&mut data, 88, u64::MAX);
    data
}

fn unreadable(data: &[u8]) {
    let result = squashfs::read(data, &Limits::default());
    assert!(!result.complete, "{:?}", result.entries);
    assert!(result.entries.iter().find(|e| e.path == "file").unwrap().data.is_none());
    assert!(result.warnings.iter().any(|w| w.contains("logical file span")));
}

#[test]
fn short_first_blocks_cannot_shift_later_file_contents() {
    unreadable(&image(&[(vec![b'a'; 4095], true), (b"tail".to_vec(), true)], 4099));
    unreadable(&image(&[(craft_anything_testkit::firmware::zlib(&vec![b'a'; 4095]), false), (b"tail".to_vec(), true)], 4099));
}

#[test]
fn short_or_long_final_blocks_are_not_reported_as_complete() {
    for length in [2, 4] {
        unreadable(&image(&[(vec![b'x'; length], true)], 3));
        unreadable(&image(&[(craft_anything_testkit::firmware::zlib(&vec![b'x'; length]), false)], 3));
    }
}

#[test]
fn exact_data_blocks_and_partial_sparse_tails_keep_their_offsets() {
    for (first, raw) in [(vec![b'a'; 4096], true), (craft_anything_testkit::firmware::zlib(&vec![b'a'; 4096]), false)] {
        let result = squashfs::read(&image(&[(first, raw), (b"end".to_vec(), true)], 4099), &Limits::default());
        assert!(result.complete, "{:?}", result.warnings);
        let content = result.entries.iter().find(|e| e.path == "file").unwrap().data.as_ref().unwrap();
        assert_eq!(&content[..4096], vec![b'a'; 4096]);
        assert_eq!(&content[4096..], b"end");
    }
    let result = squashfs::read(&image(&[(vec![], false), (vec![], false)], 4099), &Limits::default());
    assert!(result.complete, "{:?}", result.warnings);
    assert_eq!(result.entries.iter().find(|e| e.path == "file").unwrap().data.as_deref(), Some(vec![0; 4099].as_slice()));
    assert_eq!(result.bytes, 4099);
}
