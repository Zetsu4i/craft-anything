use craft_anything_formats::dotnet::cil::{Operand, decode, normalized_hash};
use std::collections::HashSet;

fn hash(opcode: u8, operand: &[u8]) -> String {
    let mut body = vec![opcode];
    body.extend_from_slice(operand);
    body.push(0x2a); // ret
    let (instructions, issue) = decode(&body, 100);
    assert!(issue.is_none(), "{issue:?}");
    assert_eq!(instructions.len(), 2);
    normalized_hash(&instructions)
}

#[test]
fn single_precision_infinities_and_nan_do_not_share_a_method_hash() {
    let hashes: HashSet<_> = [f32::INFINITY, f32::NEG_INFINITY, f32::NAN].into_iter().map(|v| hash(0x22, &v.to_le_bytes())).collect();
    assert_eq!(hashes.len(), 3);
}

#[test]
fn double_precision_infinities_and_nan_do_not_share_a_method_hash() {
    let hashes: HashSet<_> = [f64::INFINITY, f64::NEG_INFINITY, f64::NAN].into_iter().map(|v| hash(0x23, &v.to_le_bytes())).collect();
    assert_eq!(hashes.len(), 3);
}

#[test]
fn decoded_nan_sign_and_payload_bits_remain_distinct() {
    let patterns = [0x7ff8_0000_0000_0001u64, 0x7ff8_0000_0000_0002, 0xfff8_0000_0000_0001];
    let hashes: HashSet<_> = patterns.into_iter().map(|bits| hash(0x23, &bits.to_le_bytes())).collect();
    assert_eq!(hashes.len(), patterns.len());
}

#[test]
fn finite_operands_keep_the_existing_hash_encoding() {
    assert_eq!(hash(0x23, &1.5f64.to_le_bytes()), "195f2ec0d52206e04f4e58b7cedf0b75469f4054d818265725463f6007b15439");
    assert_ne!(hash(0x23, &0.0f64.to_le_bytes()), hash(0x23, &(-0.0f64).to_le_bytes()));
}

#[test]
fn single_precision_signaling_and_quiet_nan_payloads_remain_distinct() {
    let patterns = [0x7f80_0001u32, 0x7fc0_0001, 0xff80_0001, 0xffc0_0001];
    let hashes: HashSet<_> = patterns.into_iter().map(|bits| hash(0x22, &bits.to_le_bytes())).collect();
    assert_eq!(hashes.len(), patterns.len());
}

#[test]
fn single_precision_nan_fingerprints_use_the_original_bytes() {
    let cases = [
        (0x7f80_0001u32, "1b99a9969a50260dd677196a9b8ae0aca5fd7dfe6032992e3dd22d41e4d5e41b"),
        (0x7fc0_0001, "3f164858f86409ab10d47646098ab9af0d7faab97dd7eec25a634049c3a831d8"),
        (0xff80_0001, "4bf7a757536a28f8a2055eae06cdb62f10098abd174d0f61c5b3c668b175508a"),
        (0xffc0_0001, "ea3bbcbb6f507f8a01267024a603b64e14d5e95f3e242f0145c9c64b4e85ffb7"),
    ];
    for (bits, expected) in cases {
        assert_eq!(hash(0x22, &bits.to_le_bytes()), expected, "{bits:08x}");
    }
}

#[test]
fn single_precision_finite_and_infinite_fingerprints_stay_compatible() {
    let cases = [
        (0.0f32, "1279ca16ff7126f17bde5cdf2a96f81eee1f63272453d6d392d5c75553dbe466"),
        (-0.0, "8f7e317dd16be322493cd1d30fa7cea99b0f12af163144b5d18a5e96883fb2a6"),
        (1.5, "414d8ae0acb8320ff7d74705a3bfae31ba4171eabb7145756280674957e7e74d"),
        (f32::INFINITY, "22e6d32bb534c3c072ce51ae0121123c9cd95981fb554eeea5780bb14959e590"),
        (f32::NEG_INFINITY, "4bf81c6fe4eaa15e18348f95bef93249eaf32a3f9799c810dd273843bc70b966"),
    ];
    for (value, expected) in cases {
        assert_eq!(hash(0x22, &value.to_le_bytes()), expected, "{value:?}");
    }
}

#[test]
fn short_float_operands_keep_their_numeric_value_and_serialized_shape() {
    for value in [0.0f32, -0.0, 1.5, f32::INFINITY, f32::NEG_INFINITY, f32::NAN] {
        let mut bytes = vec![0x22];
        bytes.extend_from_slice(&value.to_le_bytes());
        let (instructions, issue) = decode(&bytes, 1);
        assert!(issue.is_none());
        assert_eq!(instructions.len(), 1);
        let Operand::Float(decoded) = instructions[0].operand else { panic!("float operand expected") };
        if value.is_nan() {
            assert!(decoded.is_nan());
        } else {
            assert_eq!(decoded.to_bits(), f64::from(value).to_bits());
        }
        let serialized = serde_json::to_value(&instructions[0]).unwrap();
        assert_eq!(serialized.as_object().unwrap().len(), 4);
        assert_eq!(serialized["opcode"], "ldc.r4");
        assert_eq!(serialized["kind"], "short_r");
        assert_eq!(serialized["offset"], 0);
        assert_eq!(serialized["operand"], serde_json::to_value(f64::from(value)).unwrap());
    }
}

#[test]
fn finite_instruction_equality_keeps_its_numeric_semantics() {
    let mut positive = vec![0x22];
    positive.extend_from_slice(&0.0f32.to_le_bytes());
    let mut negative = vec![0x22];
    negative.extend_from_slice(&(-0.0f32).to_le_bytes());
    let (positive, positive_issue) = decode(&positive, 1);
    let (negative, negative_issue) = decode(&negative, 1);
    assert!(positive_issue.is_none() && negative_issue.is_none());
    assert_eq!(positive, negative);
    assert_ne!(normalized_hash(&positive), normalized_hash(&negative));
}
