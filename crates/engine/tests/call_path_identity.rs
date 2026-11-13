use craft_anything_engine::evidence::{Environment, ErrorCode, EvidenceBuilder, Subject};
use craft_anything_engine::{CallOptions, Caller, Engine};
use craft_anything_testkit as tk;
use serde_json::{Value, json};

fn sample(main_calls_helper: bool, helper_calls_dispatch: bool) -> Vec<u8> {
    let (mut bytes, _) = tk::elf64(tk::ElfArch::X86_64, true);
    let text = tk::ELF_TEXT_OFF as usize;
    bytes[text..text + tk::x64::CODE_SIZE].fill(0xcc);
    for offset in [tk::x64::MAIN, tk::x64::HELPER, tk::x64::HIDDEN, tk::x64::DISPATCH] {
        bytes[text + offset as usize] = 0xc3;
    }
    for (enabled, from, to) in [(main_calls_helper, tk::x64::MAIN, tk::x64::HELPER), (helper_calls_dispatch, tk::x64::HELPER, tk::x64::DISPATCH)] {
        if enabled {
            let off = text + from as usize;
            bytes[off] = 0xe8;
            bytes[off + 1..off + 5].copy_from_slice(&((to as i64 - from as i64 - 5) as i32).to_le_bytes());
            bytes[off + 5] = 0xc3;
        }
    }
    bytes
}

fn call(engine: &Engine, tool: &str, params: Value) -> Value {
    engine.call(tool, params, CallOptions::new(Caller::Cli)).unwrap().to_json()
}

fn studies(engine: &Engine, name: &str, bytes: &[u8]) -> Vec<Value> {
    let dir = craft_anything_engine::testing::temp_dir();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    call(engine, "core_target_open", json!({"path": path}));
    let records = ["main", "helper", "dispatch"].map(|name| call(engine, "native_function_study", json!({"function": name})));
    std::fs::remove_dir_all(dir).unwrap();
    records.into()
}

fn request(functions: Value, goal: u64) -> Value {
    json!({"functions": functions, "start": format!("{:#x}", tk::ELF_BASE + tk::ELF_TEXT_OFF), "goal": format!("{:#x}", tk::ELF_BASE + tk::ELF_TEXT_OFF + goal)})
}

#[test]
fn mixed_inline_studies_cannot_invent_a_cross_binary_call_path() {
    let engine = Engine::new(craft_anything_engine::testing::session());
    let left = studies(&engine, "left.elf", &sample(true, false));
    let right = studies(&engine, "right.elf", &sample(false, true));
    assert_ne!(left[0]["subject"]["sha256"], right[0]["subject"]["sha256"]);
    for records in [&left, &right] {
        let path = call(&engine, "app_call_path_find", request(json!(records), tk::x64::DISPATCH));
        assert_eq!(path["result"]["found"], false, "neither real binary contains the combined path");
    }
    let result = engine.call("app_call_path_find", request(json!([left[0], right[1], right[2]]), tk::x64::DISPATCH), CallOptions::new(Caller::Cli));
    assert!(result.is_err(), "mixed binaries invented a path: {:?}", result.map(|output| output.to_json()));
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[test]
fn mixed_saved_studies_cannot_invent_a_cross_binary_call_path() {
    let engine = Engine::new(craft_anything_engine::testing::session());
    let left = studies(&engine, "left.elf", &sample(true, false));
    let right = studies(&engine, "right.elf", &sample(false, true));
    let result =
        engine.call("app_call_path_find", request(json!([left[0]["id"], right[1]["id"], right[2]["id"]]), tk::x64::DISPATCH), CallOptions::new(Caller::Cli));
    assert!(result.is_err(), "saved records combined different input binaries");
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[test]
fn call_path_records_require_an_input_content_identity() {
    let engine = Engine::new(craft_anything_engine::testing::session());
    let records = studies(&engine, "chain.elf", &sample(true, true));
    let unbound = EvidenceBuilder::new(
        "native_function_study",
        Subject { kind: "file".into(), sha256: None, name: Some("chain.elf".into()) },
        Environment::builtin(false),
    )
    .result(records[0]["result"].clone())
    .summary("Function study without a source content identity")
    .build()
    .unwrap();
    let result = engine.call("app_call_path_find", request(json!([unbound, records[1], records[2]]), tk::x64::DISPATCH), CallOptions::new(Caller::Cli));
    assert!(result.is_err(), "an unbound function study was accepted as belonging to the binary");
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[test]
fn same_input_content_still_connects_across_file_names() {
    let engine = Engine::new(craft_anything_engine::testing::session());
    let bytes = sample(true, true);
    let left = studies(&engine, "first-name.elf", &bytes);
    let right = studies(&engine, "second-name.elf", &bytes);
    assert_eq!(left[0]["subject"]["sha256"], right[0]["subject"]["sha256"]);
    let path = call(&engine, "app_call_path_find", request(json!([left[0], right[1], right[2]]), tk::x64::DISPATCH));
    assert_eq!(path["result"]["found"], true);
    assert_eq!(path["result"]["path"].as_array().unwrap().len(), 3);
}

#[test]
fn one_binary_keeps_its_positive_and_negative_paths() {
    let engine = Engine::new(craft_anything_engine::testing::session());
    let records = studies(&engine, "simple.elf", &sample(true, false));
    let found = call(&engine, "app_call_path_find", request(json!(records), tk::x64::HELPER));
    assert_eq!(found["result"]["found"], true);
    assert_eq!(found["result"]["path"].as_array().unwrap().len(), 2);
    let missing = call(&engine, "app_call_path_find", request(json!(records), tk::x64::DISPATCH));
    assert_eq!(missing["result"]["found"], false);
}
