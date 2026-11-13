use craft_anything_engine::{CallOptions, Caller, Engine};
use craft_anything_testkit as tk;
use serde_json::{Value, json};

struct Sample {
    engine: Engine,
    directory: std::path::PathBuf,
    entry: u64,
    cold: u64,
    hello: u64,
}

impl Drop for Sample {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Sample {
    fn call(&self, id: &str) -> Value {
        self.engine.call(id, json!({"function": format!("{:#x}", self.entry)}), CallOptions::new(Caller::Cli)).unwrap().to_json()
    }
}

fn sample(entry_first: bool) -> Sample {
    let (mut bytes, layout) = tk::elf64(tk::ElfArch::X86_64, false);
    let (entry_offset, cold_offset) = if entry_first { (0x10usize, 0x30usize) } else { (0x30, 0x10) };
    let entry = layout.code_va + entry_offset as u64;
    let cold = layout.code_va + cold_offset as u64;
    let text = tk::ELF_TEXT_OFF as usize;
    bytes[text..text + tk::x64::CODE_SIZE].fill(0xcc);
    bytes[24..32].copy_from_slice(&entry.to_le_bytes());
    // mov esi, 0; test edi, edi; je cold; ret. The entry reads rdi but
    // defines rsi before the cold block uses it, so only rdi is an input.
    let mut hot = vec![0xbe, 0, 0, 0, 0, 0x85, 0xff, 0x0f, 0x84];
    hot.extend_from_slice(&((cold as i64 - (entry + 13) as i64) as i32).to_le_bytes());
    hot.push(0xc3);
    bytes[text + entry_offset..text + entry_offset + hot.len()].copy_from_slice(&hot);
    // mov eax, esi; lea rdi, [rip+hello]; call [rip+puts]; ret.
    let mut lower = vec![0x89, 0xf0, 0x48, 0x8d, 0x3d];
    lower.extend_from_slice(&((layout.rodata_va as i64 - (cold + 9) as i64) as i32).to_le_bytes());
    lower.extend_from_slice(&[0xff, 0x15]);
    lower.extend_from_slice(&((layout.got_va as i64 - (cold + 15) as i64) as i32).to_le_bytes());
    lower.push(0xc3);
    bytes[text + cold_offset..text + cold_offset + lower.len()].copy_from_slice(&lower);
    // A separate prologue in the gap references a different string. It must not
    // enter the target's evidence just because it lies between its body blocks.
    let gap = layout.code_va + 0x20;
    let mut unrelated = vec![0x55, 0x48, 0x89, 0xe5, 0x48, 0x8d, 0x05];
    unrelated.extend_from_slice(&((layout.rodata_va as i64 + tk::rodata::SECRET as i64 - (gap + 11) as i64) as i32).to_le_bytes());
    unrelated.extend_from_slice(&[0x5d, 0xc3]);
    bytes[text + 0x20..text + 0x20 + unrelated.len()].copy_from_slice(&unrelated);
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("blocks.elf");
    std::fs::write(&path, bytes).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    engine.call("core_target_open", json!({"path": path}), CallOptions::new(Caller::Cli)).unwrap();
    Sample { engine, directory, entry, cold, hello: layout.rodata_va }
}

fn check_listing(sample: &Sample) {
    let target = format!("{:#x}", sample.hello);
    let site = format!("{:#x}", sample.cold + 2);
    let listing = sample.call("native_function_references_list");
    assert!(listing["result"]["references"].as_array().unwrap().iter().any(|r| r["from"] == site && r["to"] == target), "{listing}");
}

fn check_study(sample: &Sample) {
    let target = format!("{:#x}", sample.hello);
    let site = format!("{:#x}", sample.cold + 2);
    let study = sample.call("native_function_study");
    assert!(study["result"]["outgoing_references"].as_array().unwrap().iter().any(|r| r["from"] == site && r["to"] == target), "{study}");
    let strings = study["result"]["referenced_strings"].as_array().unwrap();
    assert_eq!(strings.len(), 1, "{strings:?}");
    assert_eq!(strings[0]["text"], tk::HELLO);
    assert_eq!(strings[0]["from"], site);
}

#[test]
fn references_and_strings_cover_blocks_below_the_entry() {
    check_study(&sample(false));
}

#[test]
fn outgoing_reference_listing_includes_blocks_below_the_entry() {
    check_listing(&sample(false));
}

#[test]
fn parameter_inference_uses_the_entry_block_not_the_lowest_address() {
    let sample = sample(false);
    let result = sample.call("native_function_pseudocode");
    let signature = result["result"]["signature"].as_str().unwrap();
    assert!(signature.contains("int64_t rdi"), "{signature}");
    assert!(!signature.contains("int64_t rsi"), "{signature}");
}

#[test]
fn entry_first_functions_keep_their_evidence_and_parameters() {
    let sample = sample(true);
    check_listing(&sample);
    check_study(&sample);
    let result = sample.call("native_function_pseudocode");
    assert!(result["result"]["signature"].as_str().unwrap().contains("int64_t rdi"));
}
