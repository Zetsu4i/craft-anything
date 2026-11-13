use craft_anything_engine::{CallOptions, Caller, Engine, ToolOutput};
use craft_anything_testkit::managed as fx;
use serde_json::{Value, json};

fn call(engine: &Engine, tool: &str, params: Value) -> Value {
    let ToolOutput::Evidence(record) = engine.call(tool, params, CallOptions::new(Caller::Mcp)).unwrap() else {
        panic!("expected evidence from {tool}");
    };
    assert!(record.verify());
    serde_json::to_value(&*record).unwrap()
}

fn has_signature_note(result: &Value) -> bool {
    signature_note_count(result) != 0
}

fn signature_note_count(result: &Value) -> usize {
    result["limitations"].as_array().unwrap().iter().filter(|l| l["message"].as_str().is_some_and(|m| m.contains("trailing bytes"))).count()
}

#[test]
fn padded_signatures_keep_types_and_surface_a_limitation() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Sample.dll");
    for padded in [false, true] {
        let bytes = if padded { fx::managed_assembly_with_padded_signatures(1) } else { fx::managed_assembly(1) };
        std::fs::write(&path, &bytes).unwrap();
        let engine = Engine::new(craft_anything_engine::testing::session());
        for tool in ["managed_assembly_show", "managed_members_list", "managed_native_calls_list"] {
            let result = call(&engine, tool, json!({"path": path}));
            assert_eq!(has_signature_note(&result), padded, "{tool}: {result}");
        }
        let graph = call(&engine, "managed_app_graph_build", json!({"input": path}));
        assert_eq!(signature_note_count(&graph), usize::from(padded));
        let members = call(&engine, "managed_members_list", json!({"path": path, "type_names": ["Sample.Calculator"], "include_il": true}));
        let calc = &members["result"]["types"][0];
        assert_eq!(calc["fields"][0]["type"], "int");
        assert_eq!(calc["methods"][0]["signature"], "public int Add(int a, int b)");
        assert_eq!(calc["methods"][1]["body"]["locals"][0], "int");
        assert_eq!(members["result"]["warnings"].as_array().unwrap().len(), usize::from(padded));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn graph_retains_attribute_only_signature_warnings() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Sample.dll");
    let mut clean_graph = None;
    for padded in [false, true] {
        let bytes = if padded { fx::managed_assembly_with_padded_target_framework_signature(1) } else { fx::managed_assembly(1) };
        let assembly = craft_anything_formats::dotnet::open(&bytes).unwrap();
        let inventory = craft_anything_formats::dotnet::inventory(&assembly);
        let members = craft_anything_formats::dotnet::members(&assembly, &Default::default());
        let native = craft_anything_formats::dotnet::native_boundaries(&assembly);
        assert_eq!(inventory.target_frameworks, [fx::TFM]);
        assert_eq!(inventory.warnings.len(), usize::from(padded));
        assert!(members.warnings.is_empty());
        assert!(native.warnings.is_empty());

        std::fs::write(&path, &bytes).unwrap();
        let engine = Engine::new(craft_anything_engine::testing::session());
        let graph = call(&engine, "managed_app_graph_build", json!({"input": path}));
        assert_eq!(signature_note_count(&graph), usize::from(padded));
        if padded {
            assert_eq!(Some(graph["result"].clone()), clean_graph);
        } else {
            clean_graph = Some(graph["result"].clone());
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn graph_deduplicates_view_warnings_before_the_display_cap() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Sample.dll");
    let mut baseline_graph = None;
    for invalid_mappings in [false, true] {
        let bytes = if invalid_mappings { fx::managed_assembly_with_native_mapping_warnings(1) } else { fx::managed_assembly_with_padded_signatures(1) };
        let assembly = craft_anything_formats::dotnet::open(&bytes).unwrap();
        let inventory = craft_anything_formats::dotnet::inventory(&assembly);
        let members = craft_anything_formats::dotnet::members(&assembly, &Default::default());
        let native = craft_anything_formats::dotnet::native_boundaries(&assembly);
        for warnings in [&inventory.warnings, &members.warnings, &native.warnings] {
            assert_eq!(warnings.iter().filter(|warning| warning.contains("trailing bytes")).count(), 1);
        }
        assert_eq!(inventory.warnings.len(), 1);
        assert_eq!(members.warnings.len(), 1);
        let mapping_warnings: Vec<_> = if invalid_mappings { (3..=27).map(|row| format!("ImplMap row {row} has no member")).collect() } else { vec![] };
        assert_eq!(native.warnings[1..], mapping_warnings);

        std::fs::write(&path, &bytes).unwrap();
        let engine = Engine::new(craft_anything_engine::testing::session());
        let graph = call(&engine, "managed_app_graph_build", json!({"input": path}));
        assert_eq!(signature_note_count(&graph), 1);
        let messages: Vec<_> = graph["limitations"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|limitation| limitation["code"] == "metadata")
            .map(|limitation| limitation["message"].as_str().unwrap().to_string())
            .collect();
        let mut expected = inventory.warnings.clone();
        expected.extend(mapping_warnings.into_iter().take(19));
        assert_eq!(messages, expected);
        assert_eq!(messages.len(), if invalid_mappings { 20 } else { 1 });
        if invalid_mappings {
            assert_eq!(Some(graph["result"].clone()), baseline_graph);
        } else {
            baseline_graph = Some(graph["result"].clone());
        }
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn derived_managed_views_retain_the_signature_limitation() {
    let dir = tempfile::tempdir().unwrap();
    let padded = dir.path().join("Padded.dll");
    let clean = dir.path().join("Clean.dll");
    let native = dir.path().join("craft.dll");
    std::fs::write(&padded, fx::managed_assembly_with_padded_signatures(1)).unwrap();
    std::fs::write(&clean, fx::managed_assembly(1)).unwrap();
    std::fs::write(&native, craft_anything_testkit::pe64()).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let compare = call(&engine, "managed_assemblies_compare", json!({"left": padded, "right": clean}));
    assert!(has_signature_note(&compare));
    for change in ["changed", "renamed_or_moved", "added", "removed", "field_changes"] {
        assert_eq!(compare["result"]["counts"][change], 0, "{change}: {compare}");
    }
    let verify = call(&engine, "managed_native_calls_verify", json!({"managed": padded, "native": [native]}));
    assert!(has_signature_note(&verify));
    assert_eq!(verify["result"]["pinvokes"][0]["status"], "found");
    let reconstruction = call(&engine, "managed_reconstruction_check", json!({"members": padded, "source_text": fx::RECONSTRUCTION_CS}));
    assert!(has_signature_note(&reconstruction));
    assert_eq!(reconstruction["result"]["counts"]["missing"], 0);
}
