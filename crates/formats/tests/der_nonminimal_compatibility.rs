use craft_anything_formats::{authenticode, der, pe, pkcs7, verify};
use craft_anything_testkit::{android, crypto, program_facts};

const CONTENT: &[u8] = b"retained signed content";

fn redundant_length(encoded: &[u8]) -> Vec<u8> {
    let value = der::read(encoded, 0).unwrap();
    let length = value.len.to_be_bytes();
    let start = length.iter().position(|byte| *byte != 0).unwrap_or(length.len() - 1);
    let length = &length[start..];
    let mut out = vec![value.tag, 0x80 | (length.len() + 1) as u8, 0];
    out.extend_from_slice(length);
    out.extend_from_slice(value.bytes(encoded).unwrap());
    out
}

fn redundant_at(encoded: &[u8], path: &[usize]) -> Vec<u8> {
    let Some((&index, rest)) = path.split_first() else { return redundant_length(encoded) };
    let root = der::read(encoded, 0).unwrap();
    let children = der::children(encoded, &root);
    assert!(index < children.len());
    let mut content = Vec::new();
    for (i, child) in children.iter().enumerate() {
        let bytes = child.whole(encoded).unwrap();
        if i == index {
            content.extend_from_slice(&redundant_at(bytes, rest));
        } else {
            content.extend_from_slice(bytes);
        }
    }
    android::der(root.tag, &content)
}

fn has_length_note(notes: &[String]) -> bool {
    notes.iter().any(|note| note.contains("not strict DER"))
}

#[test]
fn redundant_lengths_preserve_readable_element_bytes() {
    for (header, length) in [(&[0x04, 0x81, 0x00][..], 0), (&[0x04, 0x81, 0x7f], 127), (&[0x04, 0x82, 0x00, 0x80], 128), (&[0x04, 0x83, 0x00, 0x01, 0x00], 256)]
    {
        let mut bytes = header.to_vec();
        bytes.resize(header.len() + length, 0x2a);
        let value = der::read(&bytes, 0).expect("redundant lengths must retain readable content");
        assert_eq!(value.len, length);
        assert_eq!(value.bytes(&bytes), Some(&bytes[header.len()..]));
        assert_eq!(value.whole(&bytes), Some(bytes.as_slice()));
    }
}

#[test]
fn certificates_keep_identity_and_report_nested_redundant_lengths() {
    let certificate = android::certificate_der("Length Signer", "Fixture Org");
    let expected = der::certificate(&certificate).unwrap();
    for path in [&[][..], &[0], &[0, 3], &[0, 5], &[0, 6], &[1]] {
        let changed = redundant_at(&certificate, path);
        let info = der::certificate(&changed).expect("certificate facts survive a redundant length");
        assert_eq!(info.subject, expected.subject, "{path:?}");
        assert_eq!(info.issuer, expected.issuer, "{path:?}");
        assert_eq!(info.serial, expected.serial, "{path:?}");
        assert_eq!(info.public_key_algorithm, expected.public_key_algorithm, "{path:?}");
        assert_eq!(info.signature_algorithm, expected.signature_algorithm, "{path:?}");
        let json = serde_json::to_value(&info).unwrap();
        assert!(json["notes"].as_array().is_some_and(|notes| notes.iter().any(|note| note.as_str().is_some_and(|note| note.contains("not strict DER")))));
    }
}

#[test]
fn signed_cms_views_keep_metadata_and_warn_without_changing_signed_bytes() {
    let canonical = crypto::cms_detached_rsa(CONTENT, "Length Signer", "Fixture Org");
    let expected = pkcs7::parse_signed_data(&canonical).unwrap();
    assert!(verify::verify_macho_cms(&canonical, CONTENT).unwrap().authenticated);
    assert!(verify::verify_pkcs7_detached(CONTENT, &canonical).unwrap().signature_valid.is_valid());
    for path in [&[][..], &[1, 0, 4, 0], &[1, 0, 4, 0, 2], &[1, 0, 3, 0]] {
        let changed = redundant_at(&canonical, path);
        let parsed = pkcs7::parse_signed_data(&changed).expect("CMS metadata survives a redundant length");
        assert_eq!(parsed.digest, expected.digest);
        assert_eq!(parsed.signature_algorithm, expected.signature_algorithm);
        assert_eq!(parsed.signature, expected.signature);
        assert_eq!(parsed.signed_attrs_reencoded, expected.signed_attrs_reencoded);
        assert_eq!(parsed.certificates.len(), expected.certificates.len());
        let macho = verify::verify_macho_cms(&changed, CONTENT).unwrap();
        assert!(macho.authenticated, "{path:?}: {macho:?}");
        assert_eq!(macho.signer_subject.as_deref(), Some("CN=Length Signer, O=Fixture Org"));
        assert!(has_length_note(&macho.notes));
        let jar = verify::verify_pkcs7_detached(CONTENT, &changed).unwrap();
        assert!(jar.signature_valid.is_valid(), "{path:?}: {jar:?}");
        assert!(has_length_note(&jar.notes));
    }
    assert!(!has_length_note(&verify::verify_macho_cms(&canonical, CONTENT).unwrap().notes));
    assert!(!has_length_note(&verify::verify_pkcs7_detached(CONTENT, &canonical).unwrap().notes));
}

#[test]
fn pe_reports_keep_length_warnings_with_or_without_encapsulated_content() {
    for canonical in
        [program_facts::authenticode_block(&[7; 32], "Length Signer", "Fixture Org", false), crypto::cms_detached_rsa(CONTENT, "Length Signer", "Fixture Org")]
    {
        for path in [&[][..], &[1, 0, 4, 0]] {
            let changed = redundant_at(&canonical, path);
            let file = program_facts::pe_with_certificate(&program_facts::pe_facts(), 2, &changed);
            let table = authenticode::certificate_table(&file, &pe::headers(&file).unwrap()).unwrap();
            let signature = table.entries[0].signature.as_ref().unwrap();
            assert!(signature.parsed);
            assert_eq!(signature.signer.as_ref().unwrap().subject, "CN=Length Signer, O=Fixture Org");
            assert_eq!(signature.certificates.len(), 1);
            assert_eq!(signature.notes.iter().filter(|note| note.contains("not strict DER")).count(), 1);
        }
    }
}

#[test]
fn a_redundant_outer_certificate_header_retains_chain_evidence() {
    let canonical = crypto::self_signed_rsa("Length Signer", "Fixture Org");
    let changed = redundant_length(&canonical);
    let chain = verify::verify_chain(&[&changed]);
    assert!(chain.fully_linked, "{chain:?}");
    assert!(chain.links[0].signature_verified.is_valid());
    assert_eq!(chain.links[0].subject, "CN=Length Signer, O=Fixture Org");
    assert!(has_length_note(&chain.notes));
}
