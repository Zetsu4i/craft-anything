use craft_anything_formats::pe::{self, ImportKind};

fn u32_at(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn directory(bytes: &mut [u8], index: usize, rva: u32, size: u32) {
    let at = 0x98 + 112 + index * 8;
    u32_at(bytes, at, rva);
    u32_at(bytes, at + 4, size);
}

fn image(payload: &[u8], index: usize, size: u32) -> Vec<u8> {
    let mut bytes = craft_anything_testkit::packed::pe_section(".tables", payload);
    directory(&mut bytes, index, 0x1000, size);
    bytes
}

fn imports(count: usize) -> Vec<u8> {
    let mut payload = vec![0u8; 1024 + (count + 1) * 8];
    payload[80..335].fill(b'L');
    payload[338..849].fill(b'N');
    for descriptor in 0..3 {
        let at = descriptor * 20;
        u32_at(&mut payload, at, 0x1400);
        u32_at(&mut payload, at + 12, 0x1050);
        u32_at(&mut payload, at + 16, 0x1400);
    }
    for i in 0..count {
        payload[1024 + i * 8..1032 + i * 8].copy_from_slice(&0x1150u64.to_le_bytes());
    }
    image(&payload, 1, 80)
}

fn exports(count: usize) -> Vec<u8> {
    let names = 256;
    let ordinals = names + count * 4;
    let name = ordinals + count * 2;
    let forwarder = name + 1024;
    let mut payload = vec![0u8; forwarder + 512];
    payload[64..68].copy_from_slice(b"dll\0");
    payload[name..name + 1023].fill(b'N');
    payload[forwarder..forwarder + 511].fill(b'F');
    for (at, value) in [
        (12, 0x1040),
        (16, 1),
        (20, 1),
        (24, count as u32),
        (28, 0x1050),
        (32, 0x1000 + names as u32),
        (36, 0x1000 + ordinals as u32),
        (80, 0x1000 + forwarder as u32),
    ] {
        u32_at(&mut payload, at, value);
    }
    for i in 0..count {
        u32_at(&mut payload, names + i * 4, 0x1000 + name as u32);
    }
    image(&payload, 0, payload.len() as u32)
}

fn import_text(table: &pe::ImportTable) -> usize {
    table.rows.iter().map(|row| row.library.len() + row.name.as_ref().map_or(0, String::len)).sum::<usize>()
        + table.bound_modules.iter().map(|module| module.name.len() + module.forwarders.iter().map(String::len).sum::<usize>()).sum::<usize>()
}

fn export_text(table: &pe::ExportTable) -> usize {
    table.dll_name.as_ref().map_or(0, String::len)
        + table.entries.iter().map(|row| row.name.as_ref().map_or(0, String::len) + row.forwarder.as_ref().map_or(0, String::len)).sum::<usize>()
}

#[test]
fn import_names_and_repeated_libraries_share_the_budget() {
    let data = imports(8);
    let headers = pe::headers(&data).unwrap();
    let complete = pe::import_table(&data, &headers);
    assert_eq!(complete.rows.len(), 24);
    assert_eq!(import_text(&complete), 24 * 766);
    assert!(!complete.truncated);
    assert_eq!(pe::import_table_with_text_limit(&data, &headers, 24 * 766), complete);
    let partial = pe::import_table_with_text_limit(&data, &headers, 2 * 766);
    assert_eq!(partial.rows, complete.rows[..2]);
    assert_eq!(import_text(&partial), 2 * 766);
    assert!(partial.truncated);
    assert!(partial.warnings.iter().any(|warning| warning.contains("name-byte budget")));
    assert_eq!(pe::imports(&data, &headers).len(), complete.rows.len());
}

#[test]
fn export_aliases_and_forwarders_are_charged_per_retained_row() {
    let data = exports(8);
    let headers = pe::headers(&data).unwrap();
    let complete = pe::export_table(&data, &headers).unwrap();
    assert_eq!(complete.entries.len(), 8);
    assert_eq!(export_text(&complete), 3 + 8 * 1534);
    assert!(!complete.truncated);
    assert_eq!(pe::export_table_with_text_limit(&data, &headers, 3 + 8 * 1534).unwrap(), complete);
    let partial = pe::export_table_with_text_limit(&data, &headers, 3 + 2 * 1534).unwrap();
    assert_eq!(partial.entries, complete.entries[..2]);
    assert_eq!(export_text(&partial), 3 + 2 * 1534);
    assert_eq!(partial.declared_names, 8);
    assert_eq!(partial.declared_functions, 1);
    assert!(partial.truncated);
    assert!(partial.warnings.iter().any(|warning| warning.contains("name-byte budget")));
}

#[test]
fn normal_delay_and_bound_imports_spend_one_budget() {
    let mut payload = vec![0u8; 512];
    for (at, value) in [(0, 0x1100), (12, 0x1180), (16, 0x1100), (64, 1), (68, 0x1180), (76, 0x1120), (80, 0x1120), (128, 1)] {
        u32_at(&mut payload, at, value);
    }
    payload[132..134].copy_from_slice(&32u16.to_le_bytes());
    payload[134..136].copy_from_slice(&2u16.to_le_bytes());
    payload[140..142].copy_from_slice(&40u16.to_le_bytes());
    payload[148..150].copy_from_slice(&40u16.to_le_bytes());
    payload[160..165].copy_from_slice(b"base\0");
    payload[168..172].copy_from_slice(b"fwd\0");
    payload[256..264].copy_from_slice(&0x1190u64.to_le_bytes());
    payload[288..296].copy_from_slice(&0x1190u64.to_le_bytes());
    payload[384..388].copy_from_slice(b"lib\0");
    payload[402..407].copy_from_slice(b"Name\0");
    let mut data = image(&payload, 1, 40);
    directory(&mut data, 13, 0x1040, 64);
    directory(&mut data, 11, 0x1080, 48);
    let headers = pe::headers(&data).unwrap();
    let complete = pe::import_table_with_text_limit(&data, &headers, 24);
    assert_eq!(complete.rows.iter().map(|row| row.kind).collect::<Vec<_>>(), [ImportKind::Normal, ImportKind::Delay]);
    assert_eq!(complete.bound_modules[0].forwarders, ["fwd", "fwd"]);
    assert_eq!(import_text(&complete), 24);
    assert!(!complete.truncated);
    let partial = pe::import_table_with_text_limit(&data, &headers, 21);
    assert_eq!(partial.rows, complete.rows);
    assert_eq!(partial.bound_modules[0].name, "base");
    assert_eq!(partial.bound_modules[0].forwarders, ["fwd"]);
    assert_eq!(import_text(&partial), 21);
    assert!(partial.truncated);
    let normal_only = pe::import_table_with_text_limit(&data, &headers, 7);
    assert_eq!(normal_only.rows, complete.rows[..1]);
    assert!(normal_only.bound_modules.is_empty() && normal_only.truncated);
}

#[test]
fn decoded_utf8_bytes_include_lossy_replacement_characters() {
    let mut data = imports(1);
    let headers = pe::headers(&data).unwrap();
    let library = headers.rva_to_offset(0x1050).unwrap();
    data[library..library + 2].copy_from_slice(&[0xff, 0]);
    let name = headers.rva_to_offset(0x1150).unwrap() + 2;
    data[name..name + 4].copy_from_slice("中\0".as_bytes());
    let complete = pe::import_table_with_text_limit(&data, &headers, 18);
    assert_eq!(complete.rows.len(), 3);
    assert_eq!(import_text(&complete), 18);
    assert!(!complete.truncated);
    let partial = pe::import_table_with_text_limit(&data, &headers, 6);
    assert_eq!(partial.rows.len(), 1);
    assert_eq!(partial.rows[0].library, "�");
    assert_eq!(partial.rows[0].name.as_deref(), Some("中"));
    assert!(partial.truncated);
    assert!(pe::import_table_with_text_limit(&data, &headers, 5).rows.is_empty());
}

#[test]
fn zero_budgets_distinguish_empty_tables_from_omitted_names() {
    let data = imports(1);
    let headers = pe::headers(&data).unwrap();
    let imports = pe::import_table_with_text_limit(&data, &headers, 0);
    assert!(imports.rows.is_empty() && imports.truncated);
    let data = exports(1);
    let headers = pe::headers(&data).unwrap();
    let exports = pe::export_table_with_text_limit(&data, &headers, 0).unwrap();
    assert!(exports.entries.is_empty() && exports.dll_name.is_none() && exports.truncated);
    let data = craft_anything_testkit::packed::pe_section(".empty", &[0; 64]);
    let headers = pe::headers(&data).unwrap();
    assert!(!pe::import_table_with_text_limit(&data, &headers, 0).truncated);
    assert!(pe::export_table_with_text_limit(&data, &headers, 0).is_none());
}

#[test]
#[ignore = "memory regression: 128 MiB address space; Linux/glibc test harness also needs MALLOC_ARENA_MAX=1"]
fn shared_import_names_stay_within_the_default_byte_budget() {
    let data = imports(65_535);
    let headers = pe::headers(&data).unwrap();
    let table = pe::import_table(&data, &headers);
    assert_eq!(table.rows.len(), pe::DEFAULT_MAX_TABLE_TEXT_BYTES / 766);
    assert!(import_text(&table) <= pe::DEFAULT_MAX_TABLE_TEXT_BYTES);
    assert!(table.truncated && !table.warnings.is_empty());
}

#[test]
#[ignore = "memory regression: 128 MiB address space; Linux/glibc test harness also needs MALLOC_ARENA_MAX=1"]
fn shared_export_aliases_stay_within_the_default_byte_budget() {
    let data = exports(200_000);
    let headers = pe::headers(&data).unwrap();
    let table = pe::export_table(&data, &headers).unwrap();
    assert_eq!(table.entries.len(), (pe::DEFAULT_MAX_TABLE_TEXT_BYTES - 3) / 1534);
    assert!(export_text(&table) <= pe::DEFAULT_MAX_TABLE_TEXT_BYTES);
    assert!(table.truncated && !table.warnings.is_empty());
}
