use craft_anything_disasm::{Image, Insn, Kind, Limits, Mode, NoControl, Program, dataflow, decode, pseudo};
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

fn stores() -> Vec<u32> {
    let mut words = Vec::new();
    for size in 0..4 {
        for release in [0, 0x8000] {
            words.push(0x0802_7c83 | (size << 30) | release); // st[x/lx]r[b/h] w2, [w/x]3, [x4]
        }
    }
    for size in [0, 0x4000_0000] {
        for release in [0, 0x8000] {
            words.push(0x8820_0861 | size | release); // st[l]xp w0, [w/x]1, [w/x]2, [x3]
        }
    }
    words
}

#[test]
fn exclusive_stores_write_the_status_and_read_only_values_and_the_address() {
    for word in stores() {
        let i = instruction(word);
        assert_eq!(i.kind, Kind::Store);
        if i.ops.len() == 4 {
            registers(&i, &["x1", "x2", "x3"], &["x0"]);
        } else {
            registers(&i, &["x3", "x4"], &["x2"]);
        }
    }
}

#[test]
fn a_discarded_status_does_not_turn_the_first_value_into_a_definition() {
    registers(&instruction(0xc81f_7c83), &["x3", "x4"], &[]); // stxr wzr, x3, [x4]
    registers(&instruction(0xc81f_ffe3), &["sp", "x3"], &[]); // stlxr wzr, x3, [sp]
    registers(&instruction(0xc83f_0861), &["x1", "x2", "x3"], &[]); // stxp wzr, x1, x2, [x3]
}

#[test]
fn single_exclusive_loads_are_typed_memory_reads() {
    for size in 0..4 {
        for acquire in [0, 0x8000] {
            let i = instruction(0x085f_7c83 | (size << 30) | acquire);
            assert_eq!(i.kind, Kind::Load, "{}", i.text());
            registers(&i, &["x4"], &["x3"]);
        }
    }
}

#[test]
fn exclusive_pair_loads_define_both_results() {
    for word in [0x887f_0440, 0x887f_8440, 0xc87f_0440, 0xc87f_8440] {
        let i = instruction(word);
        assert_eq!(i.kind, Kind::LoadPair);
        registers(&i, &["x2"], &["x0", "x1"]);
    }
}

#[test]
fn exclusive_pair_loads_can_discard_either_result() {
    for word in [0xc87f_045f, 0xc87f_845f] {
        // l[d/a]xp xzr, x1, [x2]
        registers(&instruction(word), &["x2"], &["x1"]);
    }
    for word in [0xc87f_7c40, 0xc87f_fc40] {
        // l[d/a]xp x0, xzr, [x2]
        registers(&instruction(word), &["x2"], &["x0"]);
    }
}

fn with_program<T>(words: &[u32], run: impl FnOnce(&Image, &Program) -> T) -> T {
    let bytes = words.iter().flat_map(|word| word.to_le_bytes()).collect::<Vec<_>>();
    let image = Image::raw(bytes.into(), Arch::Aarch64, 0x1000);
    let program = Program::analyze(&image, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).unwrap();
    run(&image, &program)
}

#[test]
fn an_exclusive_store_status_reaches_the_retry_condition() {
    // ldxr x3, [x4]; stxr w2, x3, [x4]; cbnz w2, loop; ret
    with_program(&[0xc85f_7c83, 0xc802_7c83, 0x35ff_ffc2, 0xd65f_03c0], |image, program| {
        let flow = dataflow::def_use(image, program, program.functions.get(&0x1000).unwrap(), 100, 100);
        let store = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
        let condition = flow.nodes.iter().find(|node| node.addr == Some(0x1008)).unwrap();
        assert_eq!(store.defines, ["x2"]);
        assert!(flow.edges.iter().any(|edge| edge.from == store.id && edge.to == condition.id && edge.register == "x2"));
        assert!(!flow.parameter_uses.iter().any(|(register, _)| register == "x2"));
        let effect = flow.effects.iter().find(|effect| effect.addr == 0x1004).unwrap();
        assert_eq!(effect.kind, "store");
        assert!(!effect.inputs.iter().any(|register| register == "x2"));
    });
}

#[test]
fn the_second_exclusive_load_result_reaches_its_use() {
    // ldxp x0, x1, [x2]; mov x3, x1; ret
    with_program(&[0xc87f_0440, 0xaa01_03e3, 0xd65f_03c0], |image, program| {
        let flow = dataflow::def_use(image, program, program.functions.get(&0x1000).unwrap(), 100, 100);
        let load = flow.nodes.iter().find(|node| node.addr == Some(0x1000)).unwrap();
        let use_result = flow.nodes.iter().find(|node| node.addr == Some(0x1004)).unwrap();
        assert!(load.defines.iter().any(|register| register == "x1"));
        assert!(flow.edges.iter().any(|edge| edge.from == load.id && edge.to == use_result.id && edge.register == "x1"));
        assert!(!flow.parameter_uses.iter().any(|(register, _)| register == "x1"));
    });
}

fn lifted(word: u32) -> pseudo::Pseudo {
    with_program(&[word, 0xd65f_03c0], |image, program| pseudo::lift(image, program, program.functions.get(&0x1000).unwrap(), &|_| None))
}

#[test]
fn exclusive_stores_keep_their_conditional_memory_operation_in_pseudo_c() {
    for word in stores() {
        let p = lifted(word);
        assert_eq!(p.asm_fallbacks, 1, "{}: {}", instruction(word).text(), p.text);
        assert!(p.text.contains(&format!("__asm(\"{}", instruction(word).mnemonic)), "{}", p.text);
    }
}

#[test]
fn exclusive_loads_keep_their_monitor_operation_in_pseudo_c() {
    let mut words = vec![0x887f_0440, 0x887f_8440, 0xc87f_0440, 0xc87f_8440];
    for size in 0..4 {
        for acquire in [0, 0x8000] {
            words.push(0x085f_7c83 | (size << 30) | acquire);
        }
    }
    for word in words {
        let p = lifted(word);
        assert_eq!(p.asm_fallbacks, 1, "{}: {}", instruction(word).text(), p.text);
        assert!(p.text.contains(&format!("__asm(\"{}", instruction(word).mnemonic)), "{}", p.text);
    }
}

#[test]
fn an_exclusive_load_from_a_known_import_slot_is_not_elided() {
    let (mut bytes, layout) = tk::elf64(tk::ElfArch::Aarch64, false);
    let pages = (layout.got_va >> 12).wrapping_sub(layout.code_va >> 12) & 0x1f_ffff;
    let adrp = 0x9000_0001 | (((pages & 3) as u32) << 29) | (((pages >> 2) as u32) << 5);
    let add = 0x9100_0021 | (((layout.got_va & 0xfff) as u32) << 10);
    let code = [adrp, add, 0xc85f_7c20, 0xd65f_03c0].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>();
    let start = tk::ELF_TEXT_OFF as usize;
    bytes[start..start + tk::a64::CODE_SIZE].fill(0);
    bytes[start..start + code.len()].copy_from_slice(&code);
    let image = Image::parse(bytes.into()).unwrap();
    let program = Program::analyze(&image, &Limits { scan_prologues: false, ..Limits::default() }, &NoControl).unwrap();
    assert!(program.import_slots.contains_key(&layout.got_va));
    let p = pseudo::lift(&image, &program, program.functions.get(&layout.code_va).unwrap(), &|_| None);
    assert_eq!(p.asm_fallbacks, 1, "{}", p.text);
    assert!(p.text.contains("__asm(\"ldxr x0"), "{}", p.text);
}

#[test]
fn ordinary_load_store_and_pair_operations_keep_their_existing_pseudo_c() {
    for (word, reads, writes) in [
        (0xf940_0083, vec!["x4"], vec!["x3"]),         // ldr x3, [x4]
        (0xf900_0083, vec!["x3", "x4"], vec![]),       // str x3, [x4]
        (0xa940_0440, vec!["x2"], vec!["x0", "x1"]),   // ldp x0, x1, [x2]
        (0xa900_0440, vec!["x0", "x1", "x2"], vec![]), // stp x0, x1, [x2]
    ] {
        registers(&instruction(word), &reads, &writes);
        let p = lifted(word);
        assert_eq!(p.asm_fallbacks, 0, "{}", p.text);
        assert!(!p.text.contains("__asm"), "{}", p.text);
    }
}
