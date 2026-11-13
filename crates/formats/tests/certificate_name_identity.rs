use craft_anything_formats::der::{name, read};
fn tlv(tag: u8, b: &[u8]) -> Vec<u8> {
    let mut out = vec![tag, b.len() as u8];
    out.extend(b);
    out
}
fn attribute(id: u8, tag: u8, value: &[u8]) -> Vec<u8> {
    let mut b = tlv(6, &[0x55, 4, id]);
    b.extend(tlv(tag, value));
    tlv(0x30, &b)
}
fn render(sets: &[Vec<u8>]) -> String {
    let payload = sets.iter().flat_map(|s| tlv(0x31, s)).collect::<Vec<_>>();
    let root = tlv(0x30, &payload);
    name(&root, &read(&root, 0).unwrap())
}
#[test]
fn unicode_directory_string_values_are_lossless_or_explicitly_binary() {
    assert_eq!(render(&[attribute(3, 0x1c, &[0, 1, 0xf6, 0x80])]), "CN=🚀");
    assert_eq!(render(&[attribute(3, 0x1e, &[0, 65, 0])]), "CN=#1e03004100");
    assert_eq!(render(&[attribute(3, 4, &[0, 255])]), "CN=#040200ff");
}

#[test]
fn malformed_text_keeps_its_encoded_identity() {
    assert_eq!(render(&[attribute(3, 0x0c, &[0xff])]), "CN=#0c01ff");
    assert_eq!(render(&[attribute(3, 0x0c, &[0xfe])]), "CN=#0c01fe");
    assert_eq!(render(&[attribute(3, 0x1c, &[0, 0x11, 0, 0])]), "CN=#1c0400110000");
    assert_eq!(render(&[attribute(3, 0x1e, &[0xd8, 0])]), "CN=#1e02d800");
    assert_eq!(render(&[attribute(3, 0x0c, "🚀".as_bytes())]), "CN=🚀");
    assert_eq!(render(&[attribute(3, 0x1e, &[0, 65])]), "CN=A");
}

#[test]
fn legacy_name_strings_keep_readable_text_without_lossy_substitution() {
    assert_eq!(render(&[attribute(3, 0x14, b"Ren\xe9")]), "CN=René");
    assert_eq!(render(&[attribute(3, 0x14, b"A,B+\xf8")]), r"CN=A\,B\+ø");
    assert_eq!(render(&[attribute(3, 0x1a, b"Visible Name")]), "CN=Visible Name");
    assert_eq!(render(&[attribute(5, 0x12, b"123 456")]), "serialNumber=123 456");
    assert_eq!(render(&[attribute(3, 0x1a, &[0xff])]), "CN=#1a01ff");
    assert_eq!(render(&[attribute(5, 0x12, &[0xff])]), "serialNumber=#1201ff");
    assert_eq!(render(&[attribute(3, 0x15, b"A")]), "CN=#150141");
}
