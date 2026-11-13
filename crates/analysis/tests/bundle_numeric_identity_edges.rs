use craft_anything_analysis::js_app::{AppFile, AppInput, AppOptions, analyze_application};

#[test]
fn application_graph_connects_equivalent_numeric_module_ids() {
    let source = r#"
var __webpack_modules__ = {
    1: function(module, exports, r) { r(16); r(1_000); },
    0x10: function() {},
    1000: function() {}
};
"#;
    let input = AppInput {
        label: "bundle".into(),
        format: "file".into(),
        files: vec![AppFile { path: "bundle.js".into(), bytes: source.as_bytes().to_vec() }],
        listing: vec![("bundle.js".into(), source.len() as u64)],
        notes: vec![],
    };
    let graph = analyze_application(&input, &AppOptions::default(), &|| false).unwrap();
    let edges: Vec<_> = graph.edges.iter().filter(|edge| edge.kind == "requires").map(|edge| (edge.from.as_str(), edge.to.as_str())).collect();
    assert_eq!(edges, [("bundle-module:bundle.js#1", "bundle-module:bundle.js#1000"), ("bundle-module:bundle.js#1", "bundle-module:bundle.js#16"),]);
    for (_, target) in edges {
        assert!(graph.nodes.iter().any(|node| node.id == target), "dependency target must be a module node: {target}");
    }
}
