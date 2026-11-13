use craft_anything_formats::fsread::{Limits, extfs};

const TABLE: usize = 12 * 1024;
const INODE: usize = TABLE + 10 * 256;

fn put32(data: &mut [u8], at: usize, value: u32) {
    data[at..at + 4].copy_from_slice(&value.to_le_bytes());
}

fn image(payload: &[u8]) -> Vec<u8> {
    let mut data = craft_anything_testkit::firmware::ext_image(false);
    let inodes = data[5120..7168].to_vec();
    data.resize(TABLE + 16 * 256, 0);
    put32(&mut data, 1028, 16);
    put32(&mut data, 2056, 12);
    data[1112..1114].copy_from_slice(&256u16.to_le_bytes());
    put32(&mut data, 1120, 0x8000); // INLINE_DATA filesystem feature.
    for number in 0..16 {
        data[TABLE + number * 256..TABLE + number * 256 + 128].copy_from_slice(&inodes[number * 128..number * 128 + 128]);
    }
    put32(&mut data, INODE + 4, payload.len() as u32);
    put32(&mut data, INODE + 32, 0x1000_0000);
    data[INODE + 40..INODE + 100].fill(0);
    data[INODE + 40..INODE + 40 + payload.len().min(60)].copy_from_slice(&payload[..payload.len().min(60)]);
    // A valid ibody attribute area with the required zero-length system.data value.
    data[INODE + 128..INODE + 130].copy_from_slice(&32u16.to_le_bytes());
    put32(&mut data, INODE + 160, 0xea02_0000);
    data[INODE + 164] = 4;
    data[INODE + 165] = 7;
    data[INODE + 180..INODE + 184].copy_from_slice(b"data");
    data
}

#[test]
fn small_inline_files_are_bytes_in_the_inode_not_block_numbers() {
    for length in [0, 1, 59, 60] {
        let payload: Vec<_> = (0..length).map(|i| (i * 17 + 1) as u8).collect();
        let result = extfs::read(&image(&payload), &Limits::default());
        assert!(result.complete, "length {length}: {:?}", result.warnings);
        assert_eq!(result.format, "ext4");
        let entry = result.entries.iter().find(|e| e.path == "hello.txt").unwrap();
        assert_eq!(entry.size, length as u64);
        assert_eq!(entry.data.as_deref(), Some(payload.as_slice()));
        assert_eq!(result.entries.iter().find(|e| e.path == "sub/note").unwrap().data.as_deref(), Some(b"nested note".as_slice()));
    }
}

#[test]
fn larger_or_unadvertised_inline_data_remains_unrecovered() {
    for feature in [0, 0x8000] {
        let mut data = image(&[0; 61]);
        put32(&mut data, 1120, feature);
        let result = extfs::read(&data, &Limits::default());
        assert!(!result.complete);
        assert!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.is_none());
    }
    let mut data = image(b"content");
    put32(&mut data, 1120, 0);
    let result = extfs::read(&data, &Limits::default());
    assert!(!result.complete);
    assert!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.is_none());
}

#[test]
fn inline_files_share_existing_file_and_total_byte_budgets() {
    let data = image(b"inline content");
    let result = extfs::read(&data, &Limits { max_file_bytes: 5, ..Limits::default() });
    assert!(!result.complete);
    assert!(result.entries.iter().find(|e| e.path == "hello.txt").unwrap().data.is_none());
    let result = extfs::read(&data, &Limits::default());
    assert_eq!(result.bytes, 1024 + 1024 + 14 + 11);
}

#[test]
fn inline_directories_do_not_use_the_block_directory_layout() {
    let mut data = image(b"content");
    let root = TABLE + 256;
    put32(&mut data, root + 32, 0x1000_0000);
    let result = extfs::read(&data, &Limits::default());
    assert!(!result.complete);
    assert!(result.entries.is_empty());
    assert!(result.warnings.iter().any(|w| w.contains("inline directory")));
}
