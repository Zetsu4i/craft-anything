use craft_anything_formats::{binary::parse_binary, macho};
use craft_anything_testkit::{MACHO_BASE, OBJC_CLASS, OBJC_PROTOCOL, OBJC_SELECTOR, macho64};

fn pointer(data: &mut [u8], offset: usize, value: u64) {
    data[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn table(data: &mut [u8], old: &str, name: &str, offset: usize, values: &[u64]) {
    let mut original = [0; 16];
    original[..old.len()].copy_from_slice(old.as_bytes());
    let header = data.windows(16).position(|bytes| bytes == original).unwrap();
    data[header..header + 16].fill(0);
    data[header..header + name.len()].copy_from_slice(name.as_bytes());
    pointer(data, header + 32, MACHO_BASE + offset as u64);
    pointer(data, header + 40, values.len() as u64 * 8);
    data[header + 48..header + 52].copy_from_slice(&(offset as u32).to_le_bytes());
    data[header + 64..header + 68].copy_from_slice(&0x1000_0000u32.to_le_bytes());
    for (index, value) in values.iter().enumerate() {
        pointer(data, offset + index * 8, *value);
    }
}

fn classes(data: &[u8]) -> Vec<macho::ObjcClass> {
    let binary = parse_binary(data).unwrap();
    let metadata = macho::objc_metadata(data, &binary);
    assert!(metadata.warnings.is_empty(), "{:?}", metadata.warnings);
    metadata.classes
}

#[test]
fn ordinary_and_nonlazy_class_lists_name_one_class() {
    for registered in [MACHO_BASE + 0x1018, 0x8000_0000_0000_1018] {
        let mut data = macho64();
        table(&mut data, "__got", "__objc_nlclslist", 0x1000, &[registered]);
        let rows = classes(&data);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, OBJC_CLASS);
        assert_eq!(rows[0].addr, MACHO_BASE + 0x1018);
        assert_eq!(rows[0].methods[0].selector, OBJC_SELECTOR);
        assert_eq!(rows[0].methods[0].imp, Some(MACHO_BASE + 0x660));
    }
}

#[test]
fn class_addresses_preserve_first_occurrence_and_distinct_same_named_definitions() {
    let mut data = macho64();
    data.copy_within(0x1018..0x1040, 0x1200);
    table(&mut data, "__got", "__objc_nlclslist", 0x1000, &[MACHO_BASE + 0x1200]);
    table(&mut data, "__objc_classlist", "__objc_classlist", 0x1300, &[MACHO_BASE + 0x1018, MACHO_BASE + 0x1200]);
    let rows = classes(&data);
    assert_eq!(rows.iter().map(|row| row.addr).collect::<Vec<_>>(), [MACHO_BASE + 0x1200, MACHO_BASE + 0x1018]);
    assert!(rows.iter().all(|row| row.name == OBJC_CLASS && row.methods[0].selector == OBJC_SELECTOR));
}

#[test]
fn overlapping_category_lists_preserve_distinct_category_addresses() {
    let mut data = macho64();
    let category_name = u64::from_le_bytes(data[0x10b0..0x10b8].try_into().unwrap());
    for offset in [0x1200, 0x1240] {
        pointer(&mut data, offset, category_name);
        pointer(&mut data, offset + 8, MACHO_BASE + 0x1018);
        pointer(&mut data, offset + 16, MACHO_BASE + 0x1088);
    }
    table(&mut data, "__got", "__objc_catlist", 0x1300, &[MACHO_BASE + 0x1200, MACHO_BASE + 0x1240]);
    table(&mut data, "__objc_protolist", "__objc_nlcatlist", 0x1320, &[MACHO_BASE + 0x1240, MACHO_BASE + 0x1200]);
    let rows = classes(&data);
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].category, None);
    assert_eq!(rows[1].addr, MACHO_BASE + 0x1200);
    assert_eq!(rows[2].addr, MACHO_BASE + 0x1240);
    for category in &rows[1..] {
        assert_eq!(category.name, OBJC_CLASS);
        assert_eq!(category.category.as_deref(), Some(OBJC_PROTOCOL));
        assert_eq!(category.methods[0].selector, OBJC_SELECTOR);
    }
}

#[test]
fn one_ordinary_registration_keeps_class_and_protocol_metadata() {
    let data = macho64();
    let binary = parse_binary(&data).unwrap();
    let metadata = macho::objc_metadata(&data, &binary);
    assert_eq!(metadata.classes.len(), 1);
    assert_eq!(metadata.classes[0].name, OBJC_CLASS);
    assert_eq!(metadata.protocols.len(), 1);
    assert_eq!(metadata.protocols[0].name, OBJC_PROTOCOL);
}
