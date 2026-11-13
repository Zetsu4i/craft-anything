use craft_anything_formats::pe;
use craft_anything_testkit::pe64;

fn normal(size: u32) -> (Vec<u8>, pe::PeHeaders) {
    let data = pe64();
    let mut headers = pe::headers(&data).unwrap();
    headers.data_dirs[1].1 = size;
    (data, headers)
}

fn delayed(size: u32) -> (Vec<u8>, pe::PeHeaders) {
    let mut data = pe64();
    for (at, value) in [(0x800, 1u32), (0x804, 0x2190), (0x80c, 0x3040), (0x810, 0x2140)] {
        data[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    let mut headers = pe::headers(&data).unwrap();
    headers.data_dirs[1] = (0, 0);
    headers.data_dirs[13] = (0x3000, size);
    (data, headers)
}

fn bound(size: u32, refs: u16) -> (Vec<u8>, pe::PeHeaders) {
    let mut data = pe64();
    data[0x800..0x804].copy_from_slice(&1u32.to_le_bytes());
    data[0x804..0x806].copy_from_slice(&24u16.to_le_bytes());
    data[0x806..0x808].copy_from_slice(&refs.to_le_bytes());
    data[0x808..0x80c].copy_from_slice(&2u32.to_le_bytes());
    data[0x80c..0x80e].copy_from_slice(&32u16.to_le_bytes());
    data[0x818..0x820].copy_from_slice(b"one.dll\0");
    data[0x820..0x828].copy_from_slice(b"two.dll\0");
    let mut headers = pe::headers(&data).unwrap();
    headers.data_dirs[1] = (0, 0);
    headers.data_dirs[11] = (0x3000, size);
    (data, headers)
}

#[test]
fn short_declared_sizes_warn_without_hiding_normal_imports() {
    for size in [0, 19, 20, 39] {
        let (data, headers) = normal(size);
        let table = pe::import_table(&data, &headers);
        assert_eq!(table.rows.len(), 1, "declared Size = {size}");
        assert!(!table.truncated, "{:?}", table.warnings);
        assert_eq!(table.warnings, ["the import descriptor table extends beyond its declared size"]);
        assert_eq!(table.rows[0].name.as_deref(), Some("puts"));
        assert_eq!(table.rows[0].library, "msvcrt.dll");
    }
}

#[test]
fn delay_imports_cannot_borrow_the_rest_of_a_descriptor() {
    for size in [20, 31, 32, 63] {
        let (data, headers) = delayed(size);
        let table = pe::import_table(&data, &headers);
        assert_eq!(table.rows.len(), usize::from(size >= 32));
        assert!(table.truncated);
        assert!(!table.warnings.is_empty());
    }
}

#[test]
fn bound_import_records_and_names_stay_inside_the_directory() {
    for size in [7, 8, 12] {
        let (data, headers) = bound(size, 1);
        let table = pe::import_table(&data, &headers);
        assert!(table.truncated);
        assert!(table.bound_modules.iter().all(|module| module.forwarders.is_empty()));
        assert!(table.bound_modules.iter().all(|module| module.name != "one.dll"));
    }
    let (data, headers) = bound(32, 1);
    let table = pe::import_table(&data, &headers);
    assert!(table.truncated);
    assert!(table.bound_modules.iter().all(|module| !module.forwarders.iter().any(|name| name == "two.dll")));
}

#[test]
fn import_directories_stop_at_the_selected_mapping_extent() {
    let (data, mut headers) = normal(40);
    let section = headers.sections.iter_mut().find(|section| section.name == ".rdata").unwrap();
    section.raw_size = 0x113;
    let table = pe::import_table(&data, &headers);
    assert!(table.rows.is_empty());
    assert!(table.truncated);
}

#[test]
fn bound_names_require_a_terminator_inside_the_directory() {
    let (data, headers) = bound(39, 1);
    let table = pe::import_table(&data, &headers);
    assert!(table.truncated);
    assert_eq!(table.bound_modules[0].name, "one.dll");
    assert!(table.bound_modules[0].forwarders.is_empty());
    assert_eq!(table.warnings, ["a bound forwarder name is outside its directory"]);

    let (data, headers) = bound(31, 1);
    let table = pe::import_table(&data, &headers);
    assert!(table.truncated);
    assert!(table.bound_modules[0].name.is_empty());
    assert!(table.warnings.iter().any(|warning| warning == "a bound import name is outside its directory"));
}

#[test]
fn complete_import_directories_keep_names_referenced_outside_the_descriptor_span() {
    for (data, headers) in [normal(40), delayed(64)] {
        let table = pe::import_table(&data, &headers);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.rows[0].name.as_deref(), Some("puts"));
        assert!(!table.truncated, "{:?}", table.warnings);
    }
    let (data, headers) = bound(40, 1);
    let table = pe::import_table(&data, &headers);
    assert!(!table.truncated, "{:?}", table.warnings);
    assert_eq!(table.bound_modules[0].name, "one.dll");
    assert_eq!(table.bound_modules[0].forwarders, ["two.dll"]);
}

#[test]
fn repeated_invalid_bound_forwarder_names_produce_one_module_warning() {
    let (mut data, headers) = bound(288, 32);
    data[0x800..0x920].fill(0);
    data[0x800..0x804].copy_from_slice(&1u32.to_le_bytes());
    data[0x804..0x806].copy_from_slice(&272u16.to_le_bytes());
    data[0x806..0x808].copy_from_slice(&32u16.to_le_bytes());
    for i in 0..32 {
        let at = 0x808 + i * 8;
        data[at..at + 4].copy_from_slice(&2u32.to_le_bytes());
        data[at + 4..at + 6].copy_from_slice(&u16::MAX.to_le_bytes());
    }
    data[0x910..0x918].copy_from_slice(b"one.dll\0");
    let table = pe::import_table(&data, &headers);
    assert!(table.truncated);
    assert_eq!(table.bound_modules.len(), 1);
    assert_eq!(table.bound_modules[0].name, "one.dll");
    assert!(table.bound_modules[0].forwarders.is_empty());
    assert_eq!(table.warnings, ["a bound forwarder name is outside its directory"]);
}
