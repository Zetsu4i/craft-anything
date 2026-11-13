use craft_anything_formats::tarcpio::{MemberKind, member_data, tar};
use craft_anything_testkit::firmware as fx;

fn record(key: &str, value: &str) -> String {
    let tail = format!(" {key}={value}\n");
    let mut len = tail.len() + 1;
    loop {
        let record = format!("{len}{tail}");
        if record.len() == len {
            return record;
        }
        len = record.len();
    }
}

fn clear_size(data: &mut [u8], at: usize) {
    let header = &mut data[at..at + 512];
    header[124..136].copy_from_slice(b"00000000000\0");
    header[148..156].fill(b' ');
    let sum: u32 = header.iter().map(|&b| u32::from(b)).sum();
    header[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
}

#[test]
fn global_path_and_size_persist_across_members_and_metadata() {
    let global = record("path", "global-name") + &record("size", "3");
    let local = record("comment", "unrelated");
    let mut data = fx::tar_entries(&[
        ("global", b'g', global.as_bytes(), ""),
        ("first", b'0', b"abc", ""),
        ("metadata", b'x', local.as_bytes(), ""),
        ("second", b'0', b"def", ""),
    ]);
    clear_size(&mut data, 1024);
    clear_size(&mut data, 3072);
    let listing = tar(&data, 10).unwrap();
    assert!(listing.complete, "{:?}", listing.warnings);
    assert_eq!(listing.members.len(), 2);
    assert!(listing.members.iter().all(|member| member.name == "global-name" && member.size == 3));
    assert_eq!(member_data(&data, &listing.members[0]), Some(b"abc".as_slice()));
    assert_eq!(member_data(&data, &listing.members[1]), Some(b"def".as_slice()));
}

#[test]
fn local_attributes_override_only_one_member() {
    let global = record("path", "inherited") + &record("size", "3");
    let local = record("path", "one-member") + &record("size", "0");
    let data = fx::tar_entries(&[
        ("global", b'g', global.as_bytes(), ""),
        ("local", b'x', local.as_bytes(), ""),
        ("first", b'0', b"", ""),
        ("second", b'0', b"abc", ""),
    ]);
    let listing = tar(&data, 10).unwrap();
    assert!(listing.complete);
    assert_eq!(listing.members.iter().map(|m| (m.name.as_str(), m.size)).collect::<Vec<_>>(), [("one-member", 0), ("inherited", 3)]);
}

#[test]
fn later_global_records_update_and_delete_only_their_keys() {
    let first = record("path", "first-global") + &record("size", "3");
    let second = record("path", "second-global");
    let remove = record("path", "") + &record("size", "");
    let data = fx::tar_entries(&[
        ("global", b'g', first.as_bytes(), ""),
        ("one", b'0', b"abc", ""),
        ("global", b'g', second.as_bytes(), ""),
        ("two", b'0', b"def", ""),
        ("global", b'g', remove.as_bytes(), ""),
        ("plain", b'0', b"normal", ""),
    ]);
    let listing = tar(&data, 10).unwrap();
    assert!(listing.complete);
    assert_eq!(listing.members.iter().map(|m| (m.name.as_str(), m.size)).collect::<Vec<_>>(), [("first-global", 3), ("second-global", 3), ("plain", 6)]);
}

#[test]
fn global_link_names_persist_until_deleted() {
    let global = record("linkpath", "shared-target");
    let local = record("linkpath", "local-target");
    let remove = record("linkpath", "");
    let data = fx::tar_entries(&[
        ("global", b'g', global.as_bytes(), ""),
        ("one", b'2', b"", "fallback"),
        ("local", b'x', local.as_bytes(), ""),
        ("two", b'1', b"", "fallback"),
        ("three", b'2', b"", "fallback"),
        ("global", b'g', remove.as_bytes(), ""),
        ("four", b'2', b"", "original"),
    ]);
    let listing = tar(&data, 10).unwrap();
    assert!(listing.complete);
    assert_eq!(
        listing.members.iter().map(|m| m.link_target.as_deref()).collect::<Vec<_>>(),
        [Some("shared-target"), Some("local-target"), Some("shared-target"), Some("original")]
    );
}

#[test]
fn malformed_or_excess_global_records_do_not_claim_complete_members() {
    for metadata in [String::from("10 size=3!"), record("comment", "").repeat(257), record("path", &"a".repeat(4097))] {
        let data = fx::tar_entries(&[("keep", b'0', b"ok", ""), ("global", b'g', metadata.as_bytes(), ""), ("after", b'0', b"later", "")]);
        let listing = tar(&data, 10).unwrap();
        assert!(!listing.complete);
        assert_eq!(listing.members.len(), 1);
        assert_eq!(listing.members[0].name, "keep");
        assert!(listing.warnings.iter().any(|warning| warning.contains("pax")));
    }
}

#[test]
fn global_sparse_metadata_keeps_stored_extents_out_of_regular_files() {
    let global = record("GNU.sparse.map", "0,3");
    let remove = record("GNU.sparse.map", "");
    let data = fx::tar_entries(&[
        ("global", b'g', global.as_bytes(), ""),
        ("one", b'0', b"abc", ""),
        ("two", b'0', b"def", ""),
        ("global", b'g', remove.as_bytes(), ""),
        ("plain", b'0', b"ghi", ""),
    ]);
    let listing = tar(&data, 10).unwrap();
    assert!(listing.complete);
    assert_eq!(listing.members.iter().map(|m| m.kind).collect::<Vec<_>>(), [MemberKind::Other, MemberKind::Other, MemberKind::File]);
    assert_eq!(listing.warnings.iter().filter(|warning| warning.contains("sparse")).count(), 2);
}

#[test]
fn unrelated_global_fields_preserve_local_and_gnu_names() {
    let global = record("comment", "not a member name");
    let local = record("path", "local");
    let data = fx::tar_entries(&[
        ("local", b'x', local.as_bytes(), ""),
        ("global", b'g', global.as_bytes(), ""),
        ("first", b'0', b"one", ""),
        ("././@LongLink", b'L', b"gnu-name\0", ""),
        ("second", b'0', b"two", ""),
    ]);
    let listing = tar(&data, 10).unwrap();
    assert!(listing.complete && listing.warnings.is_empty());
    assert_eq!(listing.members.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), ["local", "gnu-name"]);
}

#[test]
fn global_names_obey_the_existing_member_limit() {
    let path = "a".repeat(4096);
    let global = record("path", &path);
    let data = fx::tar_entries(&[("global", b'g', global.as_bytes(), ""), ("one", b'0', b"abc", ""), ("two", b'0', b"def", "")]);
    let listing = tar(&data, 1).unwrap();
    assert!(listing.truncated && !listing.complete);
    assert_eq!(listing.members.len(), 1);
    assert_eq!(listing.members[0].name, path);
}

#[test]
fn global_pax_names_override_and_consume_gnu_name_records() {
    let global = record("path", "global") + &record("linkpath", "global-target");
    let remove = record("path", "") + &record("linkpath", "");
    let data = fx::tar_entries(&[
        ("global", b'g', global.as_bytes(), ""),
        ("LongLink", b'L', b"gnu-name\0", ""),
        ("LongLink", b'K', b"gnu-target\0", ""),
        ("first", b'2', b"", "fallback"),
        ("global", b'g', remove.as_bytes(), ""),
        ("plain", b'2', b"", "original"),
    ]);
    let listing = tar(&data, 10).unwrap();
    assert!(listing.complete);
    assert_eq!(
        listing.members.iter().map(|m| (m.name.as_str(), m.link_target.as_deref())).collect::<Vec<_>>(),
        [("global", Some("global-target")), ("plain", Some("original"))]
    );
}

#[test]
fn nonempty_global_sizes_must_be_unsigned_decimal_numbers() {
    for size in ["+3", "-3", "3x", " 3", "18446744073709551616"] {
        let global = record("size", size);
        let data = fx::tar_entries(&[("keep", b'0', b"abc", ""), ("global", b'g', global.as_bytes(), ""), ("after", b'0', b"def", "")]);
        let listing = tar(&data, 10).unwrap();
        assert!(!listing.complete);
        assert_eq!(listing.members.len(), 1);
        assert!(listing.warnings.iter().any(|warning| warning.contains("global pax size")));
    }
}
