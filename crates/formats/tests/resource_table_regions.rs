use craft_anything_formats::{
    arsc::{Limits, parse},
    axml::chunk,
    bytes::Reader,
};
use craft_anything_testkit::android::resources_arsc;

fn children(data: &[u8], start: usize) -> Vec<(usize, u16)> {
    let r = Reader::new(data);
    let (_, head, size) = chunk(&r, start).unwrap();
    let mut pos = start + head;
    let mut result = Vec::new();
    while pos < start + size {
        let (kind, _, bytes) = chunk(&r, pos).unwrap();
        result.push((pos, kind));
        pos += bytes;
    }
    result
}
fn package(data: &[u8]) -> usize {
    children(data, 0).into_iter().find(|(_, k)| *k == 0x200).unwrap().0
}
fn types(data: &[u8]) -> Vec<usize> {
    children(data, package(data)).into_iter().filter(|(_, k)| *k == 0x201).map(|r| r.0).collect()
}
#[test]
fn offset_table_is_never_parsed_as_resource_data() {
    let mut d = resources_arsc();
    let pos = types(&d)[0];
    let h = u16::from_le_bytes(d[pos + 2..pos + 4].try_into().unwrap());
    d[pos + 16..pos + 20].copy_from_slice(&u32::from(h).to_le_bytes());
    let out = parse(&d, Limits::default()).unwrap();
    assert!(out.entries.iter().all(|e| e.type_name != "string" || e.config != "default"));
    assert!(!out.warnings.is_empty());
}
#[test]
fn a_truncated_bag_retains_its_identity_without_incomplete_values() {
    let mut d = resources_arsc();
    let pos = *types(&d).last().unwrap();
    let e = pos + u32::from_le_bytes(d[pos + 16..pos + 20].try_into().unwrap()) as usize;
    d[e + 12..e + 16].copy_from_slice(&2u32.to_le_bytes());
    let out = parse(&d, Limits::default()).unwrap();
    assert_incomplete_style(&out, "incomplete bag data");
}

fn assert_incomplete_style(out: &craft_anything_formats::arsc::Table, warning: &str) {
    let style = out.entries.iter().find(|entry| entry.type_name == "style").unwrap();
    assert_eq!(style.key, "AppTheme");
    assert_eq!(style.id, "0x7f020000");
    assert_eq!(style.config, "night-xhdpi-v21");
    assert!(style.parent.is_none());
    assert!(style.bag.is_empty());
    assert!(style.value.is_none());
    assert!(style.raw.is_none());
    assert!(out.truncated);
    assert!(out.warnings.iter().any(|note| note.contains(warning)));
    assert!(out.entries.iter().any(|entry| entry.key == "app_name" && entry.value.as_deref() == Some("Notes")));
}

#[test]
fn short_bag_headers_retain_the_readable_key_and_type() {
    for header_size in [8u16, 12, u16::MAX] {
        let mut d = resources_arsc();
        let pos = *types(&d).last().unwrap();
        let e = pos + u32::from_le_bytes(d[pos + 16..pos + 20].try_into().unwrap()) as usize;
        d[e..e + 2].copy_from_slice(&header_size.to_le_bytes());
        let out = parse(&d, Limits::default()).unwrap();
        assert_incomplete_style(&out, "incomplete bag header");
    }
}

#[test]
fn complete_bags_keep_their_parent_and_values() {
    let out = parse(&resources_arsc(), Limits::default()).unwrap();
    let style = out.entries.iter().find(|entry| entry.type_name == "style").unwrap();
    assert_eq!(style.key, "AppTheme");
    assert_eq!(style.parent.as_deref(), Some("@0x01030005"));
    assert_eq!(style.bag.len(), 1);
    assert_eq!(style.bag[0].value, "#ff336699");
    assert!(!out.truncated);
    assert!(out.warnings.is_empty());
}
