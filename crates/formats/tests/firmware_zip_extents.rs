use craft_anything_formats::{firmware, zip};
use craft_anything_testkit::electron::zip_stored;

fn zip_regions(data: &[u8]) -> Vec<firmware::Region> {
    firmware::scan(data, &firmware::ScanOptions::default(), &|| false).regions.into_iter().filter(|region| region.kind == "zip").collect()
}

fn assert_outer(archive: &[u8], names: &[&str]) {
    let regions = zip_regions(archive);
    assert_eq!(regions.len(), 1, "{regions:?}");
    assert_eq!(regions[0].offset, 0);
    assert_eq!(regions[0].length, Some(archive.len() as u64));
    assert_eq!(regions[0].confidence, "high");
    let parsed = zip::parse(archive, 100).unwrap();
    assert_eq!(parsed.entries.iter().map(|entry| entry.name.as_str()).collect::<Vec<_>>(), names);
}

fn zip64(extended: bool) -> Vec<u8> {
    let mut archive = zip_stored(&[("file", b"data")]);
    let at = archive.len() - 22;
    let mut end = archive.split_off(at);
    let size = u32::from_le_bytes(end[12..16].try_into().unwrap());
    let offset = u32::from_le_bytes(end[16..20].try_into().unwrap());
    archive.extend_from_slice(b"PK\x06\x06");
    archive.extend_from_slice(&(if extended { 54u64 } else { 44 }).to_le_bytes());
    archive.extend_from_slice(&45u16.to_le_bytes());
    archive.extend_from_slice(&45u16.to_le_bytes());
    archive.extend_from_slice(&[0; 8]);
    archive.extend_from_slice(&1u64.to_le_bytes());
    archive.extend_from_slice(&1u64.to_le_bytes());
    archive.extend_from_slice(&u64::from(size).to_le_bytes());
    archive.extend_from_slice(&u64::from(offset).to_le_bytes());
    if extended {
        archive.extend_from_slice(&[0xfe, 0xca, 4, 0, 0, 0, 1, 2, 3, 4]);
    }
    archive.extend_from_slice(b"PK\x06\x07");
    archive.extend_from_slice(&0u32.to_le_bytes());
    archive.extend_from_slice(&(at as u64).to_le_bytes());
    archive.extend_from_slice(&1u32.to_le_bytes());
    end[8..12].fill(0xff);
    archive.extend_from_slice(&end);
    assert!(zip::parse(&archive, 100).unwrap().zip64);
    archive
}

#[test]
fn stored_nested_zip_end_records_do_not_shorten_the_outer_archive() {
    let inner = zip_stored(&[("leaf", b"ok")]);
    let first = zip_stored(&[("nested.zip", &inner), ("sibling", b"kept")]);
    assert_outer(&first, &["nested.zip", "sibling"]);
    let later = zip_stored(&[("before", b"first"), ("nested.zip", &inner), ("after", b"last")]);
    assert_outer(&later, &["before", "nested.zip", "after"]);
}

#[test]
fn end_record_bytes_in_member_data_cannot_close_a_nonempty_outer_zip() {
    for adjusted_offset in [false, true] {
        let mut payload = vec![0; 22];
        payload[..4].copy_from_slice(b"PK\x05\x06");
        if adjusted_offset {
            // The marker sits at archive offset 34, but has no central directory.
            payload[16..20].copy_from_slice(&34u32.to_le_bytes());
        }
        let archive = zip_stored(&[("fake", &payload), ("sibling", b"kept")]);
        assert_outer(&archive, &["fake", "sibling"]);
    }
}

#[test]
fn firmware_offsets_do_not_change_nested_zip_extent_matching() {
    let inner = zip_stored(&[("leaf", b"ok")]);
    let archive = zip_stored(&[("nested.zip", &inner)]);
    let mut firmware = vec![0; 37];
    firmware.extend_from_slice(&archive);
    let regions = zip_regions(&firmware);
    assert_eq!(regions.len(), 1, "{regions:?}");
    assert_eq!(regions[0].offset, 37);
    assert_eq!(regions[0].length, Some(archive.len() as u64));
    assert_eq!(regions[0].confidence, "high");
}

#[test]
fn nested_zip64_end_records_stay_inside_their_member() {
    let inner = zip64(true);
    let archive = zip_stored(&[("nested.zip", &inner), ("sibling", b"kept")]);
    assert_outer(&archive, &["nested.zip", "sibling"]);
}

#[test]
fn adjacent_zip_archives_remain_separate_regions() {
    let first = zip_stored(&[("first", b"one")]);
    let second = zip_stored(&[("second", b"two")]);
    let mut firmware = first.clone();
    firmware.extend_from_slice(&second);
    let regions = zip_regions(&firmware);
    assert_eq!(regions.len(), 2);
    assert_eq!((regions[0].offset, regions[0].length), (0, Some(first.len() as u64)));
    assert_eq!((regions[1].offset, regions[1].length), (first.len() as u64, Some(second.len() as u64)));
}

#[test]
fn zip_comments_and_directory_signatures_keep_their_full_extent() {
    let mut archive = zip_stored(&[("file", b"data")]);
    let at = archive.len() - 22;
    let mut end = archive.split_off(at);
    let size = u32::from_le_bytes(end[12..16].try_into().unwrap());
    let signature = b"PK\x05\x05\x04\0test";
    archive.extend_from_slice(signature);
    end[12..16].copy_from_slice(&(size + signature.len() as u32).to_le_bytes());
    let comment = b"comment with PK\x05\x06 marker";
    end[20..22].copy_from_slice(&(comment.len() as u16).to_le_bytes());
    archive.extend_from_slice(&end);
    archive.extend_from_slice(comment);
    assert_outer(&archive, &["file"]);
}

#[test]
fn zip64_end_records_and_extensible_data_keep_their_full_extent() {
    for extended in [false, true] {
        assert_outer(&zip64(extended), &["file"]);
    }
}

#[test]
fn a_missing_zip_end_record_keeps_the_first_member_fallback() {
    let mut archive = zip_stored(&[("file", b"data")]);
    archive.truncate(38);
    let regions = zip_regions(&archive);
    assert_eq!(regions.len(), 1);
    assert_eq!(regions[0].length, Some(38));
    assert_eq!(regions[0].confidence, "medium");
    assert!(regions[0].details["note"].as_str().unwrap().contains("first entry"));
}
