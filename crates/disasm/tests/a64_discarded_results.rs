use craft_anything_disasm::{Image, Insn, Limits, Mode, NoControl, Program, dataflow, decode};
use craft_anything_formats::Arch;
use craft_anything_testkit as tk;

fn instruction(word: u32) -> Insn {
    decode(Mode::A64, &word.to_le_bytes(), 0x1000, true).unwrap()
}

fn registers(i: &Insn, reads: &[&str], writes: &[&str]) {
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
fn discarded_arithmetic_results_read_both_inputs_without_overwriting_either() {
    for word in [0x8b01_001f, 0xcb01_001f, 0xaa01_001f, 0x8a01_001f, 0xca01_001f, 0x9b01_7c1f, 0x9ac1_0c1f, 0x9ac1_081f] {
        for encoding in [word, word & !0x8000_0000] {
            registers(&instruction(encoding), &["x0", "x1"], &[]);
        }
    }
}

#[test]
fn discarded_results_preserve_the_reaching_definition_of_a_source() {
    // mov x2, x0; add xzr, x2, x1; mov x0, x2; ret
    let bytes = [0xaa00_03e2u32, 0x8b01_005f, 0xaa02_03e0, 0xd65f_03c0].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    let image = Image::raw(bytes.into(), Arch::Aarch64, 0x1000);
    let program = Program::analyze(&image, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).unwrap();
    let flow = dataflow::def_use(&image, &program, program.functions.get(&0x1000).unwrap(), 100, 100);
    let original = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
    let discarded = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
    let later = flow.nodes.iter().find(|node| node.addr == Some(0x1008)).unwrap();
    assert!(discarded.defines.is_empty());
    for target in [discarded.id, later.id] {
        assert!(flow.edges.iter().any(|edge| edge.from == original.id && edge.to == target && edge.register == "x2"));
    }
    assert!(!flow.edges.iter().any(|edge| edge.from == discarded.id && edge.to == later.id && edge.register == "x2"));
}

#[test]
fn discarded_results_do_not_erase_a_proven_import_slot_base() {
    let (mut bytes, layout) = tk::elf64(tk::ElfArch::Aarch64, false);
    let pages = (layout.got_va >> 12).wrapping_sub(layout.code_va >> 12) & 0x1f_ffff;
    let adrp = 0x9000_0001 | (((pages & 3) as u32) << 29) | (((pages >> 2) as u32) << 5);
    let add = 0x9100_0021 | (((layout.got_va & 0xfff) as u32) << 10);
    // Address in x1; orr xzr, x1, x2; ldr x16, [x1]; blr x16; ret
    let code = [adrp, add, 0xaa02_003f, 0xf940_0030, 0xd63f_0200, 0xd65f_03c0].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    let start = tk::ELF_TEXT_OFF as usize;
    bytes[start..start + tk::a64::CODE_SIZE].fill(0);
    bytes[start..start + code.len()].copy_from_slice(&code);
    let image = Image::parse(bytes.into()).unwrap();
    let program = Program::analyze(&image, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).unwrap();
    let function = program.functions.get(&layout.code_va).unwrap();
    assert_eq!(function.calls.len(), 1);
    assert_eq!(function.calls[0].import.as_deref(), Some("puts"));
}

#[test]
fn flag_setting_zero_destinations_keep_their_inputs_and_flags() {
    for word in [0xab01_001f, 0xeb01_001f, 0xea01_001f] {
        for encoding in [word, word & !0x8000_0000] {
            let i = instruction(encoding);
            registers(&i, &["x0", "x1"], &[]);
            assert!(i.sets_flags, "{}", i.text());
        }
    }
}

#[test]
fn a_discarded_first_exclusive_load_result_keeps_the_second_destination() {
    for word in [0xc87f_045f, 0x887f_045f] {
        let i = instruction(word);
        assert_eq!(i.mnemonic, "ldxp");
        registers(&i, &["x2"], &["x1"]);
    }
}

#[test]
fn an_atomic_zero_source_keeps_its_result_destination() {
    for word in [0xf83f_0041, 0xb83f_0041] {
        let i = instruction(word);
        assert_eq!(i.mnemonic, "ldadd");
        registers(&i, &["x2"], &["x1"]);
    }
}

#[test]
fn real_destinations_stack_updates_and_zero_sources_keep_their_register_effects() {
    registers(&instruction(0x8b02_0020), &["x1", "x2"], &["x0"]); // add x0, x1, x2
    registers(&instruction(0x9100_43ff), &["sp"], &["sp"]); // add sp, sp, #16
    registers(&instruction(0xf840_843f), &["x1"], &["x1"]); // ldr xzr, [x1], #8
    registers(&instruction(0xaa00_03ff), &["x0"], &[]); // mov xzr, x0
    registers(&instruction(0xf900_003f), &["x1"], &[]); // str xzr, [x1]
    for (word, mnemonic) in [(0xba01_001f, "adcs"), (0xfa01_001f, "sbcs"), (0xea21_001f, "bics")] {
        let i = instruction(word);
        assert_eq!(i.mnemonic, mnemonic);
        registers(&i, &["x0", "x1"], &[]);
    }
}
