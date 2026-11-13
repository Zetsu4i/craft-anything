use craft_anything_formats::FormatError;
use craft_anything_formats::packed::PackedFormat;
use craft_anything_formats::pyc;
use craft_anything_formats::pyinstaller::{PyInstaller, PyzArchive};
use craft_anything_testkit::packed::{code_311, pyinstaller, pyz};

// Marshal's None value is unchanged across these releases. These fixtures test byte preservation
// and header framing, not execution of a legacy Python code object.
const BODY: &[u8] = b"N";

#[test]
fn legacy_wrappers_put_the_original_marshal_body_after_the_versioned_header() {
    for (magic, header_bytes) in [(62211, 8), (3379, 12)] {
        let bytes = pyc::wrap_marshal(magic, BODY);
        let header = pyc::parse_header(&bytes).unwrap();
        assert_eq!(header.size, header_bytes);
        assert_eq!(bytes.len(), header_bytes + BODY.len());
        assert_eq!(&bytes[header.size..], BODY);
        assert!(pyc::read_value(&bytes[header.size..], None).is_ok());
        assert_eq!(header.source_mtime, Some(0));
        assert_eq!(header.flags, None);
    }
}

#[test]
fn standalone_code_and_packages_respect_the_exact_versioned_output_limit() {
    for (magic, header_bytes) in [(62211, 8), (3379, 12), (3495, 16), (0x1234, 16)] {
        let bytes = pyz(magic, &[("module", 0, BODY), ("package", 1, BODY)], magic == 62211, true);
        let listing = PyzArchive.list(&bytes).unwrap();
        assert!(!listing.truncated);
        assert_eq!(listing.entries.len(), 2);
        for entry in &listing.entries {
            let exact_limit = (header_bytes + BODY.len()) as u64;
            let extracted = PyzArchive.read(&bytes, entry, exact_limit).unwrap();
            assert_eq!(extracted.len() as u64, exact_limit);
            assert_eq!(&extracted[header_bytes..], BODY);
            assert!(matches!(PyzArchive.read(&bytes, entry, exact_limit - 1), Err(FormatError::TooLarge(_))));
            assert!(matches!(PyzArchive.read(&bytes, entry, header_bytes as u64 - 1), Err(FormatError::TooLarge(_))));
        }
    }
}

#[test]
fn nested_legacy_pyz_extraction_preserves_the_payload_and_raw_archive() {
    for (magic, python, header_bytes) in [(62211, 27, 8), (3379, 36, 12)] {
        let inner = pyz(magic, &[("demo", 0, BODY)], true, true);
        let bytes = pyinstaller(&[("PYZ.pyz", b'z', &inner, false)], true, python);
        let listing = PyInstaller.list(&bytes).unwrap();
        assert!(!listing.truncated);
        let module = listing.entries.iter().find(|entry| entry.path == "PYZ.pyz.modules/demo.pyc").unwrap();
        let extracted = PyInstaller.read(&bytes, module, 1024).unwrap();
        assert_eq!(&extracted[header_bytes..], BODY);
        assert_eq!(PyInstaller.read(&bytes, module, (header_bytes + BODY.len()) as u64).unwrap(), extracted);
        let raw = listing.entries.iter().find(|entry| entry.path == "PYZ.pyz").unwrap();
        assert_eq!(PyInstaller.read(&bytes, raw, inner.len() as u64).unwrap(), inner);
    }
}

#[test]
fn current_code_objects_and_unknown_magic_keep_the_modern_header() {
    let body = code_311("<module>", "<module>", "demo.py", 1, &[], &["kept constant"], &[]);
    let bytes = pyz(3495, &[("demo", 0, &body)], false, true);
    let listing = PyzArchive.list(&bytes).unwrap();
    let extracted = PyzArchive.read(&bytes, &listing.entries[0], (16 + body.len()) as u64).unwrap();
    assert_eq!(&extracted[16..], body);
    let parsed = pyc::read_pyc(&extracted).unwrap();
    assert_eq!(parsed.header.version, Some("3.11"));
    assert_eq!(parsed.header.size, 16);
    assert!(parsed.code.is_some());
    let unknown = pyc::wrap_marshal(0x1234, BODY);
    assert_eq!(pyc::parse_header(&unknown).unwrap().size, 16);
    assert_eq!(&unknown[16..], BODY);
}

#[test]
fn legacy_pyz_resources_never_spend_a_code_header_budget() {
    let body = b"resource bytes";
    let bytes = pyz(62211, &[("assets/data.txt", 2, body), ("namespace", 3, b"")], true, true);
    let listing = PyzArchive.list(&bytes).unwrap();
    assert_eq!(listing.entries.len(), 1);
    assert_eq!(listing.facts["namespace_packages"][0], "namespace");
    assert_eq!(PyzArchive.read(&bytes, &listing.entries[0], body.len() as u64).unwrap(), body);
    assert!(matches!(PyzArchive.read(&bytes, &listing.entries[0], body.len() as u64 - 1), Err(FormatError::TooLarge(_))));
}
