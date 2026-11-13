use craft_anything_formats::dotnet::cil::{Operand, decode};

fn rejected(code: &[u8]) {
    let (instructions, issue) = decode(code, 100);
    let issue = issue.expect("out-of-body destination must be reported");
    assert_eq!(issue.offset, 1);
    assert!(issue.reason.contains("destination") && issue.reason.contains("body"), "{}", issue.reason);
    assert_eq!(instructions.len(), 1);
    assert_eq!(instructions[0].opcode, "nop");
}

#[test]
fn short_branches_reject_negative_and_past_end_destinations() {
    for opcode in [0x2b, 0x2c, 0x2d, 0x37, 0xde] {
        for displacement in [-4i8, 0, 127] {
            rejected(&[0, opcode, displacement as u8]);
        }
    }
}

#[test]
fn long_branches_never_wrap_signed_displacements_into_unsigned_targets() {
    for opcode in [0x38, 0x39, 0x44, 0xdd] {
        for displacement in [i32::MIN, -7, 0, i32::MAX] {
            let mut code = vec![0, opcode];
            code.extend_from_slice(&displacement.to_le_bytes());
            rejected(&code);
        }
    }
}

#[test]
fn switch_discards_the_instruction_if_any_destination_is_outside_the_body() {
    for displacement in [i32::MIN, -15, 1, i32::MAX] {
        let mut code = vec![0, 0x45];
        code.extend_from_slice(&2u32.to_le_bytes());
        code.extend_from_slice(&(-14i32).to_le_bytes()); // valid first target: the leading nop
        code.extend_from_slice(&displacement.to_le_bytes());
        code.push(0x2a);
        rejected(&code);
    }
}

#[test]
fn backward_forward_and_empty_switch_controls_remain_decodable() {
    for code in [vec![0, 0x2b, 0xfd], vec![0x2b, 0, 0x2a], vec![0, 0x38, 0xfa, 0xff, 0xff, 0xff]] {
        let (instructions, issue) = decode(&code, 100);
        assert!(issue.is_none(), "{issue:?}");
        let branch = instructions.iter().find(|instruction| matches!(instruction.operand, Operand::Targets(_))).unwrap();
        assert_eq!(branch.operand, Operand::Targets(vec![if code[0] == 0 { 0 } else { 2 }]));
    }
    let mut code = vec![0, 0x45];
    code.extend_from_slice(&2u32.to_le_bytes());
    code.extend_from_slice(&(-14i32).to_le_bytes());
    code.extend_from_slice(&0i32.to_le_bytes());
    code.push(0x2a);
    let (instructions, issue) = decode(&code, 100);
    assert!(issue.is_none());
    assert_eq!(instructions[1].operand, Operand::Targets(vec![0, 14]));
    let (instructions, issue) = decode(&[0x45, 0, 0, 0, 0, 0x2a], 100);
    assert!(issue.is_none());
    assert_eq!(instructions[0].operand, Operand::Targets(vec![]));
}

#[test]
fn truncated_operands_and_instruction_limits_keep_their_existing_diagnostics() {
    for code in [&[0x2b][..], &[0x38, 0][..], &[0x45, 1, 0, 0, 0, 1][..]] {
        let (_, issue) = decode(code, 100);
        assert!(issue.unwrap().reason.contains("operand"));
    }
    let (instructions, issue) = decode(&[0, 0x2b, 0xff], 1);
    assert_eq!(instructions.len(), 1);
    assert!(issue.unwrap().reason.contains("stopped after 1"));
}
