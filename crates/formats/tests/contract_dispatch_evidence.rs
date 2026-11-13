use craft_anything_formats::evm::analyze;
fn dispatched(prefix: &[u8], body: &[u8], valid: bool) -> Vec<u8> {
    let mut c = prefix.to_vec();
    c.extend([0x60, 0, 0x35, 0x60, 0xe0, 0x1c, 0x80, 0x63, 1, 2, 3, 4, 0x14, 0x60, 0, 0x57, 0x00]);
    let pk = c.len() - 3;
    c[pk] = c.len() as u8;
    if valid {
        c.push(0x5b);
    }
    c.extend(body);
    c
}
#[test]
fn selector_divisor_constant_alone_does_not_extract_a_selector() {
    let mut c = vec![0x60, 0, 0x35, 0x7c, 1];
    c.extend([0; 28]);
    c.extend([0x50, 0x00]);
    assert!(analyze(&c).unwrap().selector_extraction.is_none());
    c.truncate(c.len() - 2);
    c.push(0x04);
    assert!(analyze(&c).unwrap().selector_extraction.is_none()); // divisor/word is the wrong order
    c.pop();
    c.extend([0x90, 0x04]);
    assert_eq!(analyze(&c).unwrap().selector_extraction, Some("div-2^224"));
}
#[test]
fn comparison_to_another_threshold_is_not_a_short_selector_check() {
    let r = analyze(&dispatched(&[0x60, 32, 0x36, 0x10, 0x60, 7, 0x57, 0x5b], &[0x00], true)).unwrap();
    assert!(!r.short_calldata_check);
}
#[test]
fn unrelated_arithmetic_is_not_a_memory_loaded_vyper_selector() {
    let r = analyze(&[0x63, 1, 2, 3, 4, 0x01, 0x14, 0x15, 0x60, 12, 0x57, 0x00, 0x5b, 0x00]).unwrap();
    assert!(r.functions.is_empty());
}
#[test]
fn invalid_jump_target_cannot_supply_payability() {
    let r = analyze(&dispatched(&[], &[0x00], false)).unwrap();
    assert!(!r.functions[0].jumpdest_valid);
    assert_eq!(r.functions[0].mutability, "unknown");
}
