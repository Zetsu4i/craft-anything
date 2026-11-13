use craft_anything_engine::{CallOptions, Caller, Engine};
use craft_anything_formats::car;
use craft_anything_testkit::apple::{BomWriter, assets_car};
use serde_json::{Value, json};

fn output(data: &[u8]) -> Value {
    let directory = craft_anything_engine::testing::temp_dir();
    let path = directory.join("Assets.car");
    std::fs::write(&path, data).unwrap();
    let engine = Engine::new(craft_anything_engine::testing::session());
    let output = engine.call("apple_assets_list", json!({"path": path, "limit": 100}), CallOptions::new(Caller::Cli)).unwrap().to_json();
    std::fs::remove_dir_all(directory).unwrap();
    output
}

#[test]
fn short_tree_reads_report_partial_coverage_without_hiding_retained_rows() {
    for (tree_header, length, expected) in [(true, 8u32, 0), (false, 1, 0), (false, 20, 1)] {
        let mut data = assets_car();
        let bom = car::parse_bom(&data).unwrap();
        let tree = bom.vars.iter().find(|(name, _)| name == "RENDITIONS").unwrap().1;
        let bytes = bom.block(&data, tree).unwrap();
        let leaf = u32::from_be_bytes(bytes[8..12].try_into().unwrap());
        let index = u32::from_be_bytes(data[16..20].try_into().unwrap()) as usize;
        let id = if tree_header { tree } else { leaf };
        let field = index + 8 + id as usize * 8;
        data[field..field + 4].copy_from_slice(&length.to_be_bytes());
        let result = output(&data);
        assert_eq!(result["result"]["renditions_total"], expected);
        assert_eq!(result["coverage"]["status"], "partial");
        assert_eq!(result["result"]["tree_incomplete"], true);
        assert!(result["summary"].as_str().unwrap().contains("could not be read"));
    }
}

#[test]
fn complete_and_non_tree_metadata_keep_their_existing_coverage() {
    let valid = output(&assets_car());
    assert_eq!(valid["result"]["renditions_total"], 2);
    assert_eq!(valid["coverage"]["status"], "complete");
    assert_ne!(valid["result"]["tree_incomplete"], true);
    let mut writer = BomWriter::new();
    writer.var("APPEARANCEKEYS", b"metadata".to_vec());
    let ordinary = output(&writer.finish());
    assert_eq!(ordinary["result"]["renditions_total"], 0);
    assert_eq!(ordinary["coverage"]["status"], "complete");
    assert_ne!(ordinary["result"]["tree_incomplete"], true);
}
