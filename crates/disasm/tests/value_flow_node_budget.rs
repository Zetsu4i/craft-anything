use craft_anything_disasm::dataflow::def_use;
use craft_anything_disasm::{Image, Limits, NoControl, Program};
use craft_anything_formats::Arch;
use std::sync::Arc;

fn program(bytes: Vec<u8>, arch: Arch) -> (Image, Program) {
    let image = Image::raw(Arc::from(bytes), arch, 0x1000);
    let program = Program::analyze(&image, &Limits::default(), &NoControl).unwrap();
    assert!(!program.functions.get(&0x1000).unwrap().truncated);
    (image, program)
}

#[test]
fn incoming_parameters_share_the_total_node_budget() {
    let (image, program) = program(vec![0xb8, 7, 0, 0, 0, 0xc3], Arch::X86_64);
    let function = program.functions.get(&0x1000).unwrap();
    for limit in 0..6 {
        let flow = def_use(&image, &program, function, limit, 100);
        assert!(flow.nodes.len() <= limit, "limit {limit}: {} nodes", flow.nodes.len());
        assert!(flow.truncated, "omitted parameters or instructions must be visible");
        assert!(flow.edges.is_empty());
    }
}

#[test]
fn omitted_instructions_are_partial_after_parameters_use_the_budget() {
    let (image, program) = program(vec![0xb8, 7, 0, 0, 0, 0xc3], Arch::X86_64);
    let function = program.functions.get(&0x1000).unwrap();
    let flow = def_use(&image, &program, function, 7, 100);
    assert_eq!(flow.nodes.len(), 7);
    assert!(flow.nodes.iter().any(|node| node.addr == Some(0x1000)));
    assert!(flow.nodes.iter().all(|node| node.addr != Some(0x1005)));
    assert!(flow.truncated);
}

#[test]
fn an_exact_budget_keeps_the_complete_graph() {
    let (image, program) = program(vec![0xb8, 7, 0, 0, 0, 0xc3], Arch::X86_64);
    let function = program.functions.get(&0x1000).unwrap();
    let flow = def_use(&image, &program, function, 8, 100);
    assert_eq!(flow.nodes.len(), 8);
    assert!(!flow.truncated);
    let value = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
    let ret = flow.nodes.iter().find(|node| node.addr == Some(0x1005)).unwrap();
    assert!(flow.edges.iter().any(|edge| edge.from == value.id && edge.to == ret.id && edge.register == "rax"));
}

#[test]
fn a_partial_block_does_not_send_stale_definitions_through_its_omitted_tail() {
    // The loop reads edi, writes 7, then overwrites it with 9 before branching back.
    let bytes = vec![0x31, 0xc0, 0x01, 0xf8, 0xbf, 7, 0, 0, 0, 0xbf, 9, 0, 0, 0, 0x75, 0xf2, 0xc3];
    let (image, program) = program(bytes, Arch::X86_64);
    let function = program.functions.get(&0x1000).unwrap();
    let flow = def_use(&image, &program, function, 9, 100);
    let read = flow.nodes.iter().find(|node| node.addr == Some(0x1002)).unwrap();
    let earlier_write = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
    assert!(!flow.edges.iter().any(|edge| edge.from == earlier_write.id && edge.to == read.id && edge.register == "rdi"));
    assert!(flow.truncated);
    assert!(flow.edges.iter().any(|edge| edge.to == read.id && edge.register == "rdi"));

    let complete = def_use(&image, &program, function, 100, 100);
    assert!(!complete.truncated);
    let read = complete.nodes.iter().find(|node| node.addr == Some(0x1002)).unwrap();
    let last_write = complete.nodes.iter().find(|node| node.addr == Some(0x1009)).unwrap();
    assert!(complete.edges.iter().any(|edge| edge.from == last_write.id && edge.to == read.id && edge.register == "rdi"));
}

#[test]
fn the_existing_per_block_decode_limit_reports_partial_coverage() {
    let mut bytes = vec![0x90; 20_001];
    bytes.push(0xc3);
    let (image, program) = program(bytes, Arch::X86_64);
    let function = program.functions.get(&0x1000).unwrap();
    let flow = def_use(&image, &program, function, 30_000, 100);
    assert!(flow.nodes.iter().all(|node| node.addr != Some(0x1000 + 20_001)));
    assert!(flow.truncated);
}

#[test]
fn omitting_the_entry_block_does_not_invent_parameter_definitions_in_an_earlier_block() {
    let bytes = vec![0x01, 0xf8, 0xc3, 0x90, 0x90, 0x90, 0xbf, 7, 0, 0, 0, 0x75, 0xf3, 0xc3];
    let mut image = Image::raw(Arc::from(bytes), Arch::X86_64, 0x1000);
    image.bin.entry = Some(0x1006);
    let program = Program::analyze(&image, &Limits::default(), &NoControl).unwrap();
    let function = program.functions.get(&0x1006).unwrap();
    assert_eq!(function.blocks.first().unwrap().start, 0x1000);
    let flow = def_use(&image, &program, function, 7, 100);
    let read = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
    assert!(!flow.edges.iter().any(|edge| edge.to == read.id && edge.register == "rdi"));
    assert!(flow.truncated);

    let complete = def_use(&image, &program, function, 100, 100);
    assert!(!complete.truncated);
    let read = complete.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
    let write = complete.nodes.iter().find(|node| node.addr == Some(0x1006)).unwrap();
    assert!(complete.edges.iter().any(|edge| edge.from == write.id && edge.to == read.id && edge.register == "rdi"));
}

#[test]
fn an_architecture_without_parameter_nodes_still_uses_the_entire_budget() {
    let (image, program) = program(vec![0xb8, 7, 0, 0, 0, 0xc3], Arch::X86);
    let function = program.functions.get(&0x1000).unwrap();
    for limit in [0, 1, 2] {
        let flow = def_use(&image, &program, function, limit, 100);
        assert_eq!(flow.nodes.len(), limit);
        assert_eq!(flow.truncated, limit < 2);
    }
}
