use craft_anything_formats::arsc::{Limits, parse};
use craft_anything_testkit::android::resources_arsc;

fn word(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap()) as usize
}

fn put_word(bytes: &mut [u8], at: usize, value: usize) {
    bytes[at..at + 4].copy_from_slice(&(value as u32).to_le_bytes());
}

fn package(bytes: &[u8]) -> usize {
    12 + word(bytes, 16)
}

fn shifted(offset: usize) -> Vec<u8> {
    let mut bytes = resources_arsc();
    let at = package(&bytes);
    put_word(&mut bytes, at + 284, offset);
    bytes
}

#[test]
fn shifted_resource_ids_keep_local_type_names_and_values() {
    let table = parse(&shifted(3), Limits::default()).unwrap();
    assert!(table.warnings.is_empty(), "{:?}", table.warnings);
    assert!(!table.truncated);
    assert_eq!(table.packages[0].types.iter().map(|ty| (ty.id, ty.name.as_str())).collect::<Vec<_>>(), [(4, "string"), (5, "style")]);
    assert_eq!(table.entries.iter().map(|entry| entry.id.as_str()).collect::<Vec<_>>(), ["0x7f040000", "0x7f040001", "0x7f040000", "0x7f040001", "0x7f050000"]);
    let strings = table.strings_by_locale();
    assert_eq!(strings[""][0], ("app_name".into(), "Notes".into()));
    assert_eq!(strings["fr"][1], ("greeting".into(), "Bonjour".into()));
    let style = &table.entries[4];
    assert_eq!(style.type_name, "style");
    assert_eq!(style.key, "AppTheme");
    assert_eq!(style.parent.as_deref(), Some("@0x01030005"));
    assert_eq!(style.bag[0].value, "#ff336699");
}

#[test]
fn separate_package_type_offsets_do_not_alias_resource_identities() {
    let mut bytes = resources_arsc();
    let at = package(&bytes);
    let split = shifted(2);
    bytes.extend_from_slice(&split[at..]);
    let len = bytes.len();
    put_word(&mut bytes, 4, len);
    put_word(&mut bytes, 8, 2);
    let table = parse(&bytes, Limits::default()).unwrap();
    assert!(table.warnings.is_empty(), "{:?}", table.warnings);
    assert_eq!(table.packages.len(), 2);
    assert_eq!(table.entries.len(), 10);
    assert_eq!(table.entries[0].id, "0x7f010000");
    assert_eq!(table.entries[5].id, "0x7f030000");
    assert_eq!(table.entries[9].id, "0x7f040000");
}

#[test]
fn shifted_type_ids_stop_at_their_byte_boundary() {
    let table = parse(&shifted(254), Limits::default()).unwrap();
    assert_eq!(table.entries.len(), 4);
    assert!(table.entries.iter().all(|entry| entry.id.starts_with("0x7fff")));
    assert_eq!(table.packages[0].types.len(), 1);
    assert_eq!(table.packages[0].types[0].id, 255);
    assert!(table.truncated);
    assert!(table.warnings.iter().any(|warning| warning.contains("type ID")));
}

#[test]
fn an_oversized_package_offset_is_not_truncated_or_wrapped() {
    for offset in [256, 65536, u32::MAX as usize] {
        let table = parse(&shifted(offset), Limits::default()).unwrap();
        assert!(table.entries.is_empty(), "offset {offset}");
        assert!(table.packages.is_empty(), "offset {offset}");
        assert!(table.truncated);
        assert!(table.warnings.iter().any(|warning| warning.contains("type ID offset")));
    }
}

#[test]
fn legacy_package_headers_have_no_offset_field() {
    let expected = parse(&resources_arsc(), Limits::default()).unwrap();
    let mut bytes = shifted(255);
    let at = package(&bytes);
    let size = word(&bytes, at + 4);
    let types = word(&bytes, at + 268);
    let keys = word(&bytes, at + 276);
    bytes.drain(at + 284..at + 288);
    bytes[at + 2..at + 4].copy_from_slice(&284u16.to_le_bytes());
    put_word(&mut bytes, at + 4, size - 4);
    put_word(&mut bytes, at + 268, types - 4);
    put_word(&mut bytes, at + 276, keys - 4);
    let len = bytes.len();
    put_word(&mut bytes, 4, len);
    assert_eq!(parse(&bytes, Limits::default()).unwrap(), expected);
}
