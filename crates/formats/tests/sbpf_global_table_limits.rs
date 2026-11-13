use craft_anything_formats::solana;
use craft_anything_testkit::Buf;

const LIMIT: usize = 400_000;

// Every symbol table starts with its null entry. Repeated SYMTAB sections deliberately
// exercise input a linker may reject: inspection must keep its limits on hostile files too.
fn tables(symbol_counts: &[usize], relocation_counts: &[usize], rela: bool) -> Vec<u8> {
    let section_count = 2 + symbol_counts.len() + relocation_counts.len();
    let headers = 64;
    let strings = headers + section_count * 64;
    let mut next = (strings + 8 + 7) & !7;
    let mut sections = Vec::new();
    for &count in symbol_counts {
        let size = (count + 1) * 24;
        sections.push((next, size));
        next += size;
    }
    let relocation_stride = if rela { 24 } else { 16 };
    for &count in relocation_counts {
        let size = count * relocation_stride;
        sections.push((next, size));
        next += size;
    }
    let mut elf = Buf::with_len(next);
    elf.put(0, b"\x7fELF\x02\x01\x01\0\0\0\0\0\0\0\0\0");
    elf.u16(16, 3);
    elf.u16(18, solana::EM_BPF);
    elf.u32(20, 1);
    elf.u64(40, headers as u64);
    elf.u16(52, 64);
    elf.u16(58, 64);
    elf.u16(60, section_count as u16);
    elf.put(strings, b"\0record\0");
    elf.u32(headers + 64 + 4, 3); // SHT_STRTAB
    elf.u64(headers + 64 + 24, strings as u64);
    elf.u64(headers + 64 + 32, 8);
    let mut value = 1;
    for (index, &count) in symbol_counts.iter().enumerate() {
        let (offset, size) = sections[index];
        let header = headers + (index + 2) * 64;
        elf.u32(header + 4, if index == 0 { 11 } else { 2 }); // DYNSYM, then SYMTAB
        elf.u64(header + 24, offset as u64);
        elf.u64(header + 32, size as u64);
        elf.u32(header + 40, 1);
        elf.u64(header + 56, 24);
        for row in 0..count {
            let record = offset + (row + 1) * 24;
            elf.u32(record, 1);
            elf.u8(record + 4, 0x11); // GLOBAL OBJECT, no syscall candidates
            elf.u16(record + 6, 1);
            elf.u64(record + 8, value);
            value += 1;
        }
    }
    let mut address = 0;
    for (index, &count) in relocation_counts.iter().enumerate() {
        let section = symbol_counts.len() + index;
        let (offset, size) = sections[section];
        let header = headers + (section + 2) * 64;
        elf.u32(header + 4, if rela { 4 } else { 9 });
        elf.u64(header + 24, offset as u64);
        elf.u64(header + 32, size as u64);
        elf.u32(header + 40, 2);
        elf.u64(header + 56, relocation_stride as u64);
        for row in 0..count {
            let record = offset + row * relocation_stride;
            elf.u64(record, address);
            elf.u64(record + 8, 1); // null symbol, R_BPF_64_64
            address += 8;
        }
    }
    elf.into_vec()
}

#[test]
fn repeated_symbol_tables_cannot_extend_the_global_limit() {
    let elf = tables(&[1, LIMIT - 1, 1], &[], true);
    let report = solana::inspect(&elf).unwrap();
    assert_eq!(report.symbols.len(), LIMIT);
    assert_eq!(report.symbols.last().unwrap().value, LIMIT as u64);
}

#[test]
fn repeated_rela_tables_cannot_extend_the_global_limit() {
    let elf = tables(&[0], &[LIMIT, 1], true);
    assert_eq!(solana::inspect(&elf).unwrap().relocations.len(), LIMIT);
}

#[test]
fn repeated_rel_tables_cannot_extend_the_global_limit() {
    let elf = tables(&[0], &[LIMIT, 1], false);
    assert_eq!(solana::inspect(&elf).unwrap().relocations.len(), LIMIT);
}

#[test]
fn smaller_tables_keep_their_order_and_all_records() {
    let report = solana::inspect(&tables(&[2, 1, 3], &[2, 1], true)).unwrap();
    assert_eq!(report.symbols.iter().map(|s| s.value).collect::<Vec<_>>(), [1, 2, 3, 4, 5, 6]);
    assert_eq!(report.symbols.iter().map(|s| s.dynamic).collect::<Vec<_>>(), [true, true, false, false, false, false]);
    assert!(report.symbols.iter().all(|s| s.name == "record"));
    assert_eq!(report.relocations.iter().map(|r| r.offset).collect::<Vec<_>>(), [0, 8, 16]);
}

#[test]
fn exact_symbol_limit_keeps_the_final_nonnull_record() {
    let report = solana::inspect(&tables(&[1, LIMIT - 1], &[], true)).unwrap();
    assert_eq!(report.symbols.len(), LIMIT);
    assert_eq!(report.symbols.last().unwrap().value, LIMIT as u64);
}

#[test]
fn exact_relocation_limit_keeps_the_final_record() {
    let report = solana::inspect(&tables(&[0], &[LIMIT - 1, 1, 0], true)).unwrap();
    assert_eq!(report.relocations.len(), LIMIT);
    assert_eq!(report.relocations.last().unwrap().offset, ((LIMIT - 1) * 8) as u64);
}

#[test]
fn null_and_empty_tables_do_not_consume_the_output_budget() {
    let report = solana::inspect(&tables(&[0, 0, 2], &[0, 2, 0], false)).unwrap();
    assert_eq!(report.symbols.len(), 2);
    assert_eq!(report.relocations.len(), 2);
}
