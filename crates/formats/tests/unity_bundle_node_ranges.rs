use craft_anything_formats::{FormatError, unity_bundle};

/// Read a complete directory from the header/table prefix of a virtual stored bundle.
/// Advertised file data is not allocated, including the multi-gigabyte boundary control.
fn parse(blocks: &[u32], nodes: &[(u64, u64)]) -> Result<(Vec<unity_bundle::Block>, Vec<unity_bundle::Node>), FormatError> {
    let mut table = vec![0; 16];
    table.extend_from_slice(&(blocks.len() as i32).to_be_bytes());
    for size in blocks {
        table.extend_from_slice(&size.to_be_bytes());
        table.extend_from_slice(&size.to_be_bytes());
        table.extend_from_slice(&0u16.to_be_bytes());
    }
    table.extend_from_slice(&(nodes.len() as i32).to_be_bytes());
    for (i, (offset, size)) in nodes.iter().enumerate() {
        table.extend_from_slice(&offset.to_be_bytes());
        table.extend_from_slice(&size.to_be_bytes());
        table.extend_from_slice(&0u32.to_be_bytes());
        table.extend_from_slice(format!("member-{i}\0").as_bytes());
    }
    let mut bytes = b"UnityFS\0".to_vec();
    bytes.extend_from_slice(&8u32.to_be_bytes());
    bytes.extend_from_slice(b"fixture\0fixture\0");
    let size_at = bytes.len();
    bytes.extend_from_slice(&0u64.to_be_bytes());
    bytes.extend_from_slice(&(table.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&(table.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.resize(bytes.len().next_multiple_of(16), 0);
    bytes.extend_from_slice(&table);
    let file_len = bytes.len() as u64 + blocks.iter().map(|&n| u64::from(n)).sum::<u64>();
    bytes[size_at..size_at + 8].copy_from_slice(&file_len.to_be_bytes());
    let h = unity_bundle::header(&bytes, file_len).unwrap();
    unity_bundle::table(&table, &h)
}

#[test]
fn files_cannot_start_after_the_uncompressed_stream() {
    for range in [(8, 0), (8, 1), (u64::MAX, 0)] {
        assert!(!parse(&[4, 3], &[range]).unwrap().1[0].range_valid, "{range:?}");
    }
}

#[test]
fn file_end_must_fit_without_unsigned_wraparound() {
    for range in [(0, 8), (6, 2), (1, u64::MAX), (u64::MAX, 2)] {
        assert!(!parse(&[4, 3], &[range]).unwrap().1[0].range_valid, "{range:?}");
    }
}

#[test]
fn files_cannot_claim_data_when_there_are_no_data_blocks() {
    for range in [(0, 1), (1, 0)] {
        assert!(!parse(&[], &[range]).unwrap().1[0].range_valid, "{range:?}");
    }
}

#[test]
fn cross_block_ranges_aliases_and_empty_end_ranges_remain_readable() {
    let expected = [(0, 7), (2, 4), (4, 3), (7, 0), (2, 0), (2, 4)];
    let (blocks, nodes) = parse(&[4, 0, 3], &expected).unwrap();
    assert_eq!(blocks.len(), 3);
    assert_eq!(nodes.iter().map(|node| (node.offset, node.size)).collect::<Vec<_>>(), expected);
    assert!(nodes.iter().all(|node| node.range_valid));
    assert_eq!(parse(&[], &[(0, 0)]).unwrap().1[0].size, 0);
    assert!(parse(&[], &[(0, 0)]).unwrap().1[0].range_valid);
}

#[test]
fn cumulative_uncompressed_sizes_remain_64_bit() {
    let total = 2 * u64::from(u32::MAX);
    let ranges = [(total - 5, 5), (total, 0), (0, total)];
    let (blocks, nodes) = parse(&[u32::MAX, u32::MAX], &ranges).unwrap();
    assert_eq!(blocks.len(), 2);
    assert_eq!(nodes.iter().map(|node| (node.offset, node.size)).collect::<Vec<_>>(), ranges);
    assert!(nodes.iter().all(|node| node.range_valid));
    assert!(!parse(&[u32::MAX, u32::MAX], &[(total, 1)]).unwrap().1[0].range_valid);
}

#[test]
fn invalid_members_keep_the_valid_siblings_and_original_directory_order() {
    let ranges = [(0, 3), (10, 1), (3, 4), (u64::MAX, 2), (7, 0)];
    let (_, nodes) = parse(&[4, 3], &ranges).unwrap();
    assert_eq!(nodes.iter().map(|node| (node.offset, node.size)).collect::<Vec<_>>(), ranges);
    assert_eq!(nodes.iter().map(|node| node.range_valid).collect::<Vec<_>>(), [true, false, true, false, true]);
    assert_eq!(nodes.iter().map(|node| node.path.as_str()).collect::<Vec<_>>(), ["member-0", "member-1", "member-2", "member-3", "member-4"]);
}
