use craft_anything_engine::{CallOptions, Caller, Engine};
use serde_json::{Value, json};

fn call(engine: &Engine, tool: &str, params: Value) -> Value {
    engine.call(tool, params, CallOptions::new(Caller::Cli)).unwrap().to_json()
}

#[cfg(target_os = "linux")]
fn reject_live_pipe(tool: &'static str) {
    use craft_anything_engine::evidence::ErrorCode;
    use std::os::fd::AsRawFd;
    use std::sync::mpsc;
    use std::time::Duration;

    let directory = craft_anything_engine::testing::temp_dir();
    let bundle = directory.join("empty-bundle.json");
    let engine = Engine::new(craft_anything_engine::testing::session());
    call(&engine, "core_evidence_export", json!({"path": bundle}));
    let (reader, writer) = std::io::pipe().unwrap();
    let pipe = format!("/proc/self/fd/{}", reader.as_raw_fd());
    let params = if tool == "core_evidence_import" { json!({"path": pipe}) } else { json!({"older_bundle": pipe, "newer_bundle": bundle}) };
    let (sender, receiver) = mpsc::channel();
    let worker = std::thread::spawn(move || {
        let result = engine.call(tool, params, CallOptions::new(Caller::Cli));
        drop(reader);
        sender.send(result).ok();
    });
    let immediate = receiver.recv_timeout(Duration::from_secs(1));
    // Baseline reads wait for EOF; close our writer so the failing test can release its worker.
    drop(writer);
    if immediate.is_err() {
        let _ = receiver.recv_timeout(Duration::from_secs(1));
    }
    if worker.is_finished() {
        worker.join().unwrap();
    }
    std::fs::remove_dir_all(directory).unwrap();
    let result = immediate.expect("reading an evidence bundle waited for a pipe writer");
    assert_eq!(result.unwrap_err().code, ErrorCode::BadInput);
}

#[cfg(target_os = "linux")]
#[test]
fn bundle_import_rejects_a_pipe_before_reading() {
    reject_live_pipe("core_evidence_import");
}

#[cfg(target_os = "linux")]
#[test]
fn bundle_comparison_rejects_a_pipe_before_reading() {
    reject_live_pipe("core_evidence_exports_compare");
}

#[test]
fn ordinary_exported_bundles_still_import_and_compare() {
    let directory = craft_anything_engine::testing::temp_dir();
    let text = directory.join("notes.txt");
    let bundle = directory.join("bundle.json");
    std::fs::write(&text, b"evidence remains readable\n").unwrap();
    let source = Engine::new(craft_anything_engine::testing::session());
    let record = call(&source, "core_text_read", json!({"path": text}));
    let exported = call(&source, "core_evidence_export", json!({"path": bundle}));
    assert_eq!(exported["result"]["records"], 1);
    let receiver = Engine::new(craft_anything_engine::testing::session());
    let imported = call(&receiver, "core_evidence_import", json!({"path": bundle}));
    assert_eq!(imported["result"]["imported"], 1);
    assert!(imported["result"]["rejected"].as_array().unwrap().is_empty());
    let shown = call(&receiver, "core_evidence_show", json!({"evidence_id": record["id"]}));
    assert_eq!(shown["result"]["records"][0]["result"]["text"], "evidence remains readable\n");
    let compared = call(&receiver, "core_evidence_exports_compare", json!({"older_bundle": bundle, "newer_bundle": bundle}));
    assert_eq!(compared["result"]["counts"]["unchanged"], 1);
    assert_eq!(compared["result"]["counts"]["tampered"], 0);
    std::fs::remove_dir_all(directory).unwrap();
}
