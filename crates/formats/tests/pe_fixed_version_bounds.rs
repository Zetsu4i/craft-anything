use craft_anything_formats::pe;

fn node(key: &str, value: &[u8], declared_value: u16, kind: u16, children: &[Vec<u8>]) -> Vec<u8> {
    let mut bytes = vec![0; 6];
    bytes.extend(key.encode_utf16().chain([0]).flat_map(u16::to_le_bytes));
    for part in std::iter::once(value).chain(children.iter().map(Vec::as_slice)) {
        while !bytes.len().is_multiple_of(4) {
            bytes.push(0);
        }
        bytes.extend_from_slice(part);
    }
    let length = bytes.len() as u16;
    bytes[..2].copy_from_slice(&length.to_le_bytes());
    bytes[2..4].copy_from_slice(&declared_value.to_le_bytes());
    bytes[4..6].copy_from_slice(&kind.to_le_bytes());
    bytes
}

fn fixed() -> Vec<u8> {
    [0xfeef_04bdu32, 0x0001_0000, 0x0001_0002, 0x0003_0004, 0x0005_0006, 0x0007_0008, 0, 0, 0, 0, 0, 0, 0].into_iter().flat_map(u32::to_le_bytes).collect()
}

#[test]
fn absent_or_short_declared_values_do_not_borrow_adjacent_bytes() {
    for length in [0, 4, 24, 51] {
        let bytes = node("VS_VERSION_INFO", &fixed(), length, 0, &[]);
        assert!(pe::parse_version_info(&bytes).is_none(), "wValueLength={length}");
    }
}

#[test]
fn the_entire_fixed_value_must_fit_inside_the_version_node() {
    for available in [0u16, 4, 24, 51] {
        let mut bytes = node("VS_VERSION_INFO", &fixed(), 52, 0, &[]);
        bytes[..2].copy_from_slice(&(40 + available).to_le_bytes());
        assert!(pe::parse_version_info(&bytes).is_none(), "node contains {available} value bytes");
    }
}

#[test]
fn a_physically_cut_or_overdeclared_value_cannot_publish_versions() {
    for available in [24, 51] {
        let bytes = node("VS_VERSION_INFO", &fixed()[..available], 52, 0, &[]);
        assert!(pe::parse_version_info(&bytes).is_none(), "file contains {available} value bytes");
    }
    let bytes = node("VS_VERSION_INFO", &fixed(), 53, 0, &[]);
    assert!(pe::parse_version_info(&bytes).is_none());
}

#[test]
fn fixed_versions_require_a_binary_value() {
    for kind in [1, 2, u16::MAX] {
        let bytes = node("VS_VERSION_INFO", &fixed(), 52, kind, &[]);
        assert!(pe::parse_version_info(&bytes).is_none(), "wType={kind}");
    }
}

#[test]
fn complete_values_keep_file_and_product_versions() {
    for extra in [0, 8] {
        let mut value = fixed();
        value.resize(value.len() + extra, 0);
        let bytes = node("VS_VERSION_INFO", &value, value.len() as u16, 0, &[]);
        let info = pe::parse_version_info(&bytes).unwrap();
        assert_eq!(info.file_version.as_deref(), Some("1.2.3.4"));
        assert_eq!(info.product_version.as_deref(), Some("5.6.7.8"));
    }
}

#[test]
fn string_tables_survive_an_absent_or_short_fixed_value() {
    let text: Vec<u8> = "Fixture Product".encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect();
    let string = node("ProductName", &text, (text.len() / 2) as u16, 1, &[]);
    let table = node("040904b0", &[], 0, 1, &[string]);
    let strings = node("StringFileInfo", &[], 0, 1, &[table]);
    for value in [&[][..], &fixed()[..4]] {
        let bytes = node("VS_VERSION_INFO", value, value.len() as u16, 0, std::slice::from_ref(&strings));
        let info = pe::parse_version_info(&bytes).unwrap();
        assert!(info.file_version.is_none());
        assert!(info.product_version.is_none());
        assert_eq!(info.strings.get("ProductName").map(String::as_str), Some("Fixture Product"));
    }
}
