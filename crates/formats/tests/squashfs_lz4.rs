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

fn literals(bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![(bytes.len().min(15) as u8) << 4];
    if bytes.len() >= 15 {
        let mut remainder = bytes.len() - 15;
        while remainder >= 255 {
            out.push(255);
            remainder -= 255;
        }
        out.push(remainder as u8);
    }
    out.extend_from_slice(bytes);
    out
}

fn meta(out: &mut Vec<u8>, bytes: &[u8], compressed: bool) -> u64 {
    let start = out.len() as u64;
    let block = if compressed { literals(bytes) } else { bytes.to_vec() };
    out.extend_from_slice(&((block.len() as u16) | if compressed { 0 } else { 0x8000 }).to_le_bytes());
    out.extend_from_slice(&block);
    start
}

fn image(block: &[u8], size: usize, compressed: bool, compressed_meta: bool, fragment: Option<(&[u8], u32)>) -> Vec<u8> {
    let mut data = vec![0; 96];
    data.extend_from_slice(block);
    let frag_start = data.len() as u64;
    let frag_block = fragment.map(|(bytes, _)| literals(bytes));
    if let Some(bytes) = &frag_block {
        data.extend_from_slice(bytes);
    }
    let nblocks = usize::from(fragment.is_none());
    let root = 32 + nblocks * 4;
    let mut inodes = vec![0; root + 32];
    put16(&mut inodes, 0, 2);
    put32(&mut inodes, 12, 2);
    put32(&mut inodes, 16, 96);
    put32(&mut inodes, 20, if fragment.is_some() { 0 } else { u32::MAX });
    put32(&mut inodes, 24, fragment.map_or(0, |(_, offset)| offset));
    put32(&mut inodes, 28, size as u32);
    if nblocks != 0 {
        put32(&mut inodes, 32, block.len() as u32 | if compressed { 0 } else { 0x0100_0000 });
    }
    put16(&mut inodes, root, 1);
    put32(&mut inodes, root + 12, 1);
    put32(&mut inodes, root + 20, 2);
    put16(&mut inodes, root + 24, 27);
    put32(&mut inodes, root + 28, 1);
    let inode_table = meta(&mut data, &inodes, compressed_meta);
    let mut dirs = vec![0; 24];
    put32(&mut dirs, 8, 1);
    put16(&mut dirs, 14, 1);
    put16(&mut dirs, 16, 2);
    put16(&mut dirs, 18, 3);
    dirs[20..24].copy_from_slice(b"file");
    let dir_table = meta(&mut data, &dirs, compressed_meta);
    let frag_table = if let Some(bytes) = frag_block {
        let mut entry = vec![0; 16];
        put64(&mut entry, 0, frag_start);
        put32(&mut entry, 8, bytes.len() as u32);
        let table = meta(&mut data, &entry, compressed_meta);
        let index = data.len() as u64;
        data.extend_from_slice(&table.to_le_bytes());
        index
    } else {
        u64::MAX
    };
    let id_table = data.len() as u64;
    data.extend_from_slice(&(id_table + 8).to_le_bytes());
    meta(&mut data, &[0; 4], false);
    data[..4].copy_from_slice(b"hsqs");
    put32(&mut data, 4, 2);
    put32(&mut data, 12, 4096);
    put32(&mut data, 16, u32::from(fragment.is_some()));
    put16(&mut data, 20, 5);
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
    put64(&mut data, 80, frag_table);
    put64(&mut data, 88, u64::MAX);
    data
}

fn recovered(data: &[u8]) -> Vec<u8> {
    let result = squashfs::read(data, &Limits::default());
    assert!(result.complete, "{:?}", result.warnings);
    result.entries.iter().find(|e| e.path == "file").unwrap().data.clone().unwrap()
}

#[test]
fn compressed_metadata_and_regular_file_blocks_are_recovered() {
    let payload = b"compressed metadata can be shorter than 8192 bytes";
    assert_eq!(recovered(&image(&literals(payload), payload.len(), true, true, None)), payload);
    assert_eq!(recovered(&image(payload, payload.len(), false, false, None)), payload);
}

#[test]
fn repeated_matches_and_compressed_fragments_are_recovered() {
    let block = [0x3b, b'a', b'b', b'c', 3, 0, 0x50, b't', b'a', b'i', b'l', b'!'];
    assert_eq!(recovered(&image(&block, 23, true, true, None)), b"abcabcabcabcabcabctail!");
    assert_eq!(recovered(&image(&[], 4, true, true, Some((b"xxxtailzzz", 3)))), b"tail");
}

#[test]
fn corrupt_and_oversized_lz4_data_is_not_returned() {
    for block in [vec![0xf0], vec![0x00, 0, 0], literals(&vec![b'x'; 4097])] {
        let result = squashfs::read(&image(&block, 4096, true, false, None), &Limits::default());
        assert!(!result.complete);
        assert!(result.entries.iter().find(|e| e.path == "file").unwrap().data.is_none());
    }
    let bytes = image(&literals(b"hello"), 5, true, true, None);
    let result = squashfs::read(&bytes, &Limits { max_file_bytes: 4, ..Limits::default() });
    assert!(!result.complete);
    assert!(result.entries.iter().find(|e| e.path == "file").unwrap().data.is_none());
}

#[test]
fn metadata_output_keeps_the_eight_kibibyte_limit() {
    let mut data = image(&literals(b"hello"), 5, true, true, None);
    let inode = u64::from_le_bytes(data[64..72].try_into().unwrap()) as usize;
    let directory = u64::from_le_bytes(data[72..80].try_into().unwrap()) as usize;
    let id = u64::from_le_bytes(data[48..56].try_into().unwrap()) as usize;
    let oversized = literals(&vec![0; 8193]);
    let mut replacement = (oversized.len() as u16).to_le_bytes().to_vec();
    replacement.extend_from_slice(&oversized);
    let shift = replacement.len() - (directory - inode);
    data.splice(inode..directory, replacement);
    put64(&mut data, 72, (directory + shift) as u64);
    put64(&mut data, 48, (id + shift) as u64);
    let used = data.len() as u64;
    put64(&mut data, 40, used);
    let result = squashfs::read(&data, &Limits::default());
    assert!(!result.complete);
    assert!(result.entries.is_empty());
    assert!(result.warnings.iter().any(|w| w.contains("metadata block did not decompress")));
}
