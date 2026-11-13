use craft_anything_disasm::{Flow, Image, Insn, Kind, Limits, Mode, NoControl, Program, dataflow::def_use, decode};
use craft_anything_formats::Arch;

fn instruction(word: u32) -> Insn {
    decode(Mode::A64, &word.to_le_bytes(), 0x1000, true).unwrap()
}

fn reads_and_writes(i: &Insn, reads: &[&str], writes: &[&str]) {
    let mut actual_reads = i.reads.iter().map(String::as_str).collect::<Vec<_>>();
    let mut actual_writes = i.writes.iter().map(String::as_str).collect::<Vec<_>>();
    let mut expected_reads = reads.to_vec();
    let mut expected_writes = writes.to_vec();
    actual_reads.sort_unstable();
    actual_writes.sort_unstable();
    expected_reads.sort_unstable();
    expected_writes.sort_unstable();
    assert_eq!(actual_reads, expected_reads, "{} reads", i.text());
    assert_eq!(actual_writes, expected_writes, "{} writes", i.text());
}

#[test]
fn ordinary_returns_read_the_selected_target_without_writing_it() {
    for (word, register) in [(0xd65f_03c0, "x30"), (0xd65f_0260, "x19"), (0xd65f_0000, "x0")] {
        let i = instruction(word);
        assert_eq!(i.flow, Flow::Return);
        assert_eq!(i.kind, Kind::Ret);
        reads_and_writes(&i, &[register], &[]);
    }
}

#[test]
fn authenticated_jumps_read_targets_and_modifiers() {
    for (word, reads) in [
        (0xd71f_0a74, vec!["x19", "x20"]), // braa x19, x20
        (0xd71f_0e74, vec!["x19", "x20"]), // brab x19, x20
        (0xd71f_0a7f, vec!["x19", "sp"]),  // braa x19, sp
        (0xd61f_0a7f, vec!["x19"]),        // braaz x19
        (0xd61f_0e7f, vec!["x19"]),        // brabz x19
    ] {
        let i = instruction(word);
        assert_eq!(i.flow, Flow::IndirectJump { slot: None });
        assert_eq!(i.kind, Kind::Branch);
        reads_and_writes(&i, &reads, &[]);
    }
}

#[test]
fn authenticated_calls_write_only_the_link_register() {
    for (word, reads) in [
        (0xd73f_0a74, vec!["x19", "x20"]), // blraa x19, x20
        (0xd73f_0e74, vec!["x19", "x20"]), // blrab x19, x20
        (0xd73f_0a7f, vec!["x19", "sp"]),  // blraa x19, sp
        (0xd63f_0a7f, vec!["x19"]),        // blraaz x19
        (0xd63f_0e7f, vec!["x19"]),        // blrabz x19
        (0xd63f_0bdf, vec!["x30"]),        // blraaz x30
    ] {
        let i = instruction(word);
        assert_eq!(i.flow, Flow::IndirectCall { slot: None });
        assert_eq!(i.kind, Kind::Call);
        reads_and_writes(&i, &reads, &["x30"]);
    }
}

#[test]
fn authenticated_returns_read_the_link_stack_and_possible_second_modifier() {
    for word in [0xd65f_0bff, 0xd65f_0fff] {
        let i = instruction(word);
        assert_eq!(i.flow, Flow::Return);
        assert_eq!(i.kind, Kind::Ret);
        reads_and_writes(&i, &["x30", "sp", "x16"], &[]);
    }
}

#[test]
fn ordinary_indirect_branches_and_calls_keep_their_register_effects() {
    reads_and_writes(&instruction(0xd61f_0260), &["x19"], &[]);
    reads_and_writes(&instruction(0xd63f_0260), &["x19"], &["x30"]);
}

fn graph(words: &[u32]) -> craft_anything_disasm::dataflow::DefUse {
    let bytes = words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
    let image = Image::raw(bytes.into(), Arch::Aarch64, 0x1000);
    let program = Program::analyze(&image, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).unwrap();
    def_use(&image, &program, program.functions.get(&0x1000).unwrap(), 100, 100)
}

#[test]
fn return_targets_retain_their_reaching_definition() {
    let flow = graph(&[0xaa00_03f3, 0xd65f_0260]); // mov x19, x0; ret x19
    let value = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
    let ret = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
    assert!(!ret.defines.iter().any(|r| r == "x19"));
    assert!(flow.edges.iter().any(|edge| edge.from == value.id && edge.to == ret.id && edge.register == "x19"));
}

#[test]
fn authenticated_returns_retain_the_possible_second_modifier_definition() {
    for word in [0xd65f_0bff, 0xd65f_0fff] {
        let flow = graph(&[0xaa00_03f0, word]); // mov x16, x0; retaa/retab
        let value = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
        let ret = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
        assert!(!ret.defines.iter().any(|r| r == "x16"));
        assert!(flow.edges.iter().any(|edge| edge.from == value.id && edge.to == ret.id && edge.register == "x16"));
    }
}

#[test]
fn ordinary_returns_only_read_x16_when_it_is_the_selected_target() {
    for (word, reads_x16) in [(0xd65f_03c0, false), (0xd65f_0260, false), (0xd65f_0000, false), (0xd65f_0200, true)] {
        let flow = graph(&[0xaa00_03f0, word]); // mov x16, x0; ret x30/x19/x0/x16
        let value = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
        let ret = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
        assert!(!ret.defines.iter().any(|r| r == "x16"));
        assert_eq!(flow.edges.iter().any(|edge| edge.from == value.id && edge.to == ret.id && edge.register == "x16"), reads_x16);
    }
}

#[test]
fn authenticated_jump_targets_retain_their_reaching_definition() {
    let flow = graph(&[0xaa00_03f3, 0xd61f_0a7f]); // mov x19, x0; braaz x19
    let value = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
    let jump = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
    assert!(!jump.defines.iter().any(|r| r == "x19"));
    assert!(flow.edges.iter().any(|edge| edge.from == value.id && edge.to == jump.id && edge.register == "x19"));
}

#[test]
fn authenticated_calls_do_not_replace_callee_saved_target_values() {
    let flow = graph(&[0xaa00_03f3, 0xd63f_0a7f, 0xaa13_03e0, 0xd65f_03c0]); // mov x19, x0; blraaz x19; mov x0, x19; ret
    let value = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
    let call = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
    let use_after_call = flow.nodes.iter().find(|node| node.addr == Some(0x1008)).unwrap();
    assert!(!call.defines.iter().any(|r| r == "x19"));
    assert!(flow.edges.iter().any(|edge| edge.from == value.id && edge.to == call.id && edge.register == "x19"));
    assert!(flow.edges.iter().any(|edge| edge.from == value.id && edge.to == use_after_call.id && edge.register == "x19"));
}
