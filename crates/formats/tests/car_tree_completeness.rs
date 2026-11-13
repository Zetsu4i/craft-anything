use craft_anything_formats::car;
use craft_anything_testkit::apple::{BomWriter, assets_car};

fn u32_be(data: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(data[at..at + 4].try_into().unwrap())
}

fn tree_ids(data: &[u8], name: &str) -> (u32, u32) {
    let bom = car::parse_bom(data).unwrap();
    let id = bom.vars.iter().find(|(key, _)| key == name).unwrap().1;
    (id, u32_be(bom.block(data, id).unwrap(), 8))
}

fn block_offset(data: &[u8], id: u32) -> usize {
    car::parse_bom(data).unwrap().blocks[id as usize].0 as usize
}

fn set_block_length(data: &mut [u8], id: u32, length: u32) {
    let field = u32_be(data, 16) as usize + 8 + id as usize * 8;
    data[field..field + 4].copy_from_slice(&length.to_be_bytes());
}

fn assert_partial(data: &[u8], renditions: usize) {
    let catalog = car::parse(data).unwrap();
    assert_eq!(catalog.renditions.len(), renditions);
    assert!(!catalog.warnings.is_empty(), "a short tree must not be silently empty");
    assert_eq!(serde_json::to_value(catalog).unwrap()["tree_incomplete"], true);
}

#[test]
fn a_short_tree_header_is_not_an_empty_catalog() {
    for length in 4..12 {
        let mut data = assets_car();
        let (tree, _) = tree_ids(&data, "RENDITIONS");
        set_block_length(&mut data, tree, length);
        assert_partial(&data, 0);
    }
}

#[test]
fn a_short_node_header_is_not_an_empty_leaf() {
    for length in 0..12 {
        let mut data = assets_car();
        let (_, leaf) = tree_ids(&data, "RENDITIONS");
        set_block_length(&mut data, leaf, length);
        assert_partial(&data, 0);
    }
}

#[test]
fn an_interior_node_requires_its_declared_first_child() {
    for null_child in [false, true] {
        let mut data = assets_car();
        let (_, node) = tree_ids(&data, "RENDITIONS");
        let at = block_offset(&data, node);
        data[at..at + 2].copy_from_slice(&0u16.to_be_bytes());
        data[at + 2..at + 4].copy_from_slice(&1u16.to_be_bytes());
        if null_child {
            data[at + 12..at + 16].fill(0);
        } else {
            set_block_length(&mut data, node, 12);
        }
        assert_partial(&data, 0);
    }
}

#[test]
fn a_declared_tree_with_a_missing_block_is_disclosed() {
    let mut data = assets_car();
    let (tree, _) = tree_ids(&data, "RENDITIONS");
    let field = u32_be(&data, 16) as usize + 4 + tree as usize * 8;
    data[field..field + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    assert_partial(&data, 0);
}

#[test]
fn partial_leaf_entries_remain_available() {
    let mut data = assets_car();
    let (_, leaf) = tree_ids(&data, "RENDITIONS");
    set_block_length(&mut data, leaf, 20); // Header and one of two entries.
    assert_partial(&data, 1);
}

#[test]
fn a_forward_link_cycle_retains_entries_and_marks_the_tree_incomplete() {
    let mut data = assets_car();
    let (_, leaf) = tree_ids(&data, "RENDITIONS");
    let at = block_offset(&data, leaf);
    data[at + 4..at + 8].copy_from_slice(&leaf.to_be_bytes());
    assert_partial(&data, 2);
}

#[test]
fn a_forward_link_to_an_interior_node_is_not_read_as_another_leaf() {
    let mut data = assets_car();
    let (_, leaf) = tree_ids(&data, "RENDITIONS");
    let (_, interior) = tree_ids(&data, "FACETKEYS");
    let at = block_offset(&data, leaf);
    data[at + 4..at + 8].copy_from_slice(&interior.to_be_bytes());
    let at = block_offset(&data, interior);
    data[at..at + 2].fill(0);
    assert_partial(&data, 2);
}

#[test]
fn incomplete_facet_trees_are_disclosed_even_with_complete_renditions() {
    let mut data = assets_car();
    let (_, leaf) = tree_ids(&data, "FACETKEYS");
    set_block_length(&mut data, leaf, 1);
    assert_partial(&data, 2);
}

#[test]
fn complete_empty_trees_and_null_roots_remain_complete() {
    for null_root in [false, true] {
        let mut data = assets_car();
        let (tree, leaf) = tree_ids(&data, "RENDITIONS");
        let bom = car::parse_bom(&data).unwrap();
        let header = bom.vars.iter().find(|(name, _)| name == "CARHEADER").unwrap().1;
        let at = block_offset(&data, header);
        data[at + 16..at + 20].fill(0);
        if null_root {
            let at = block_offset(&data, tree);
            data[at + 8..at + 12].fill(0);
        } else {
            let at = block_offset(&data, leaf);
            data[at + 2..at + 4].fill(0);
            set_block_length(&mut data, leaf, 12);
        }
        let catalog = car::parse(&data).unwrap();
        assert!(catalog.renditions.is_empty());
        assert!(catalog.warnings.is_empty());
        assert_ne!(serde_json::to_value(catalog).unwrap()["tree_incomplete"], true);
    }
}

#[test]
fn ordinary_metadata_warnings_are_not_tree_truncation() {
    let mut writer = BomWriter::new();
    writer.var("APPEARANCEKEYS", b"metadata".to_vec());
    let catalog = car::parse(&writer.finish()).unwrap();
    assert!(catalog.warnings.iter().any(|warning| warning.contains("is not a tree")));
    assert!(catalog.warnings.iter().any(|warning| warning.contains("CARHEADER")));
    assert_ne!(serde_json::to_value(catalog).unwrap()["tree_incomplete"], true);
}

#[test]
fn a_complete_catalog_keeps_its_renditions_and_facets() {
    let catalog = car::parse(&assets_car()).unwrap();
    assert_eq!(catalog.renditions.len(), 2);
    assert_eq!(catalog.facets.len(), 2);
    assert_eq!(catalog.renditions[0].name.as_deref(), Some("AppIcon.png"));
    assert_eq!(catalog.renditions[1].name.as_deref(), Some("Logo@2x.png"));
    assert!(catalog.warnings.is_empty());
    assert_ne!(serde_json::to_value(catalog).unwrap()["tree_incomplete"], true);
}

#[test]
fn the_entry_limit_only_reports_truncation_when_entries_remain() {
    const LIMIT: usize = 1_000_000;
    for total in [LIMIT, LIMIT + 1] {
        let mut writer = BomWriter::new();
        let key = writer.block(b"key".to_vec());
        let value = writer.block(b"value".to_vec());
        let first_leaf = value + 1;
        let mut leaf_id = first_leaf;
        let mut remaining = total;
        while remaining != 0 {
            let count = remaining.min(u16::MAX as usize);
            remaining -= count;
            let mut leaf = Vec::with_capacity(12 + 8 * count);
            leaf.extend_from_slice(&1u16.to_be_bytes());
            leaf.extend_from_slice(&(count as u16).to_be_bytes());
            leaf.extend_from_slice(&(if remaining == 0 { 0 } else { leaf_id + 1 }).to_be_bytes());
            leaf.extend_from_slice(&0u32.to_be_bytes());
            for _ in 0..count {
                leaf.extend_from_slice(&value.to_be_bytes());
                leaf.extend_from_slice(&key.to_be_bytes());
            }
            assert_eq!(writer.block(leaf), leaf_id);
            leaf_id += 1;
        }
        let mut header = b"tree".to_vec();
        for number in [1u32, first_leaf, 4096, total as u32] {
            header.extend_from_slice(&number.to_be_bytes());
        }
        header.push(0);
        writer.var("RENDITIONS", header);
        let data = writer.finish();
        let bom = car::parse_bom(&data).unwrap();
        let mut warnings = Vec::new();
        assert_eq!(bom.tree(&data, "RENDITIONS", &mut warnings).len(), LIMIT);
        assert_eq!(warnings.is_empty(), total == LIMIT);
    }
}
