use craft_anything_engine::{CallOptions, Caller, Engine};
use craft_anything_testkit::apple as fx;
use serde_json::{Value, json};

fn call(engine: &Engine, path: &std::path::Path, offset: usize, limit: usize) -> Value {
    engine.call("apple_archive_decode", json!({"path": path, "offset": offset, "limit": limit}), CallOptions::new(Caller::Cli)).unwrap().to_json()
}

#[test]
fn archive_tool_pages_keep_global_counts_and_root_projection() {
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("sample.archive");
    std::fs::write(&path, fx::keyed_archive()).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let full = call(&engine, &path, 0, 20_000);
    let rows = full["result"]["objects"].as_array().unwrap();
    for (offset, limit) in [(0, 1), (1, 3), (rows.len() - 1, 20_000), (rows.len(), 1), (rows.len() + 1, 1), (usize::MAX, 1)] {
        let output = call(&engine, &path, offset, limit);
        let expected: Vec<_> = rows.iter().skip(offset).take(limit).cloned().collect();
        assert_eq!(output["result"]["objects"], json!(expected));
        for field in ["objects_total", "class_counts", "root", "root_object", "tree", "top"] {
            assert_eq!(output["result"][field], full["result"][field], "{field} at offset {offset}");
        }
        assert_eq!(output["coverage"]["examined"], expected.len());
        if offset < rows.len() && offset + expected.len() < rows.len() {
            assert_eq!(output["coverage"]["next_offset"], offset + expected.len());
        }
    }
    std::fs::remove_dir_all(directory).unwrap();
}

#[test]
fn a_small_tool_page_keeps_shared_classes_counted_globally() {
    let class = "C".repeat(64 << 10);
    let mut objects = vec![fx::s("$null"), fx::dict(vec![("$classname", fx::s(&class))])];
    objects.extend((0..1024).map(|_| fx::dict(vec![("$class", fx::P::Uid(1))])));
    let bytes = fx::to_bplist(&fx::dict(vec![
        ("$archiver", fx::s("NSKeyedArchiver")),
        ("$objects", fx::P::Array(objects)),
        ("$top", fx::dict(vec![("root", fx::P::Uid(2))])),
    ]));
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("shared.archive");
    std::fs::write(&path, bytes).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let output = call(&engine, &path, 513, 1);
    assert_eq!(output["result"]["objects_total"], 1026);
    assert_eq!(output["result"]["objects"].as_array().unwrap().len(), 1);
    assert_eq!(output["result"]["objects"][0]["index"], 513);
    assert_eq!(output["result"]["objects"][0]["class"], class);
    assert_eq!(output["result"]["class_counts"][&class], 1024);
    assert_eq!(output["coverage"]["next_offset"], 514);
    std::fs::remove_dir_all(directory).unwrap();
}
