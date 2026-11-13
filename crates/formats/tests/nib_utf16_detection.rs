use craft_anything_formats::{nib, plist};
use craft_anything_testkit::apple::{self as fx, P, dict, s};

fn archive() -> P {
    dict(vec![
        ("$archiver", s("NSKeyedArchiver")),
        ("$version", P::Int(100_000)),
        ("$top", dict(vec![("root", P::Uid(1))])),
        (
            "$objects",
            P::Array(vec![
                s("$null"),
                dict(vec![("$class", P::Uid(3)), ("NSSubviews", P::Array(vec![P::Uid(2)])), ("NSHidden", P::Bool(false))]),
                dict(vec![("$class", P::Uid(4)), ("NSText", s("保存 🗂"))]),
                dict(vec![("$classname", s("NSView"))]),
                dict(vec![("$classname", s("NSTextField"))]),
            ]),
        ),
    ])
}

fn utf16(text: &str, little_endian: bool) -> Vec<u8> {
    let mut out = if little_endian { vec![0xff, 0xfe] } else { vec![0xfe, 0xff] };
    for unit in text.encode_utf16() {
        out.extend(if little_endian { unit.to_le_bytes() } else { unit.to_be_bytes() });
    }
    out
}

fn check_archive(little_endian: bool) {
    let fixture = archive();
    let xml = fx::to_xml(&fixture).replace("encoding=\"UTF-8\"", "encoding=\"UTF-16\"");
    let bytes = utf16(&xml, little_endian);
    // The XML decoder already accepts these bytes; the NIB entry point must reach it.
    assert!(plist::parse(&bytes).is_ok());
    let actual = nib::parse(&bytes).expect("valid UTF-16 keyed-archive NIB");
    let expected = nib::parse(&fx::to_bplist(&fixture)).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(actual.objects[0].class, "NSView");
    assert!(actual.objects[0].values.contains(&("NSSubviews".into(), nib::IbValue::Object(2))));
    assert_eq!(actual.objects[1].class, "NSTextField");
    assert!(actual.objects[1].values.contains(&("NSText".into(), nib::IbValue::Json("保存 🗂".into()))));
}

#[test]
fn reads_little_endian_xml_nib_objects_and_references() {
    check_archive(true);
}

#[test]
fn reads_big_endian_xml_nib_objects_and_references() {
    check_archive(false);
}

#[test]
fn utf16_detection_respects_code_unit_alignment_and_scan_limit() {
    for little in [true, false] {
        assert!(plist::looks_like_plist(&utf16(&(" ".repeat(505) + "<plist"), little)));
        assert!(!plist::looks_like_plist(&utf16(&(" ".repeat(506) + "<plist"), little)));
        let mut misaligned = utf16("<plist", little);
        misaligned.insert(2, 0);
        assert!(!plist::looks_like_plist(&misaligned));
        assert!(!plist::looks_like_plist(&utf16("plain text", little)));
    }
}

#[test]
fn existing_utf8_binary_and_non_plist_detection_stays_compatible() {
    let fixture = archive();
    let xml = fx::to_xml(&fixture);
    let binary = fx::to_bplist(&fixture);
    assert!(plist::looks_like_plist(xml.as_bytes()));
    assert!(plist::looks_like_plist(&binary));
    assert_eq!(nib::parse(xml.as_bytes()).unwrap(), nib::parse(&binary).unwrap());
    assert!(plist::looks_like_plist(&(" ".repeat(1018) + "<plist").into_bytes()));
    assert!(!plist::looks_like_plist(&(" ".repeat(1019) + "<plist").into_bytes()));
    for bytes in [b"".as_slice(), b"plain text", &[0xff], &[0xff, 0xfe], &[0xfe, 0xff]] {
        assert!(!plist::looks_like_plist(bytes));
        assert!(nib::parse(bytes).is_err());
    }
}
