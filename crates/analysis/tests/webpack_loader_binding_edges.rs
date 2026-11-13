use craft_anything_analysis::js_app::{AppFile, AppInput, AppOptions, analyze_application};

#[test]
fn application_graph_does_not_link_calls_to_shadowed_loader_parameters() {
    let source = r#"
var __webpack_modules__ = {
    1: function(module, exports, r) {
        function nested(r) { r(3); }
        function captured() { r(2); }
    },
    2: function() {},
    3: function() {}
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
    assert_eq!(edges, [("bundle-module:bundle.js#1", "bundle-module:bundle.js#2")]);
    let module = graph.module("bundle.js").unwrap().bundle_modules.iter().find(|module| module.id == "1").unwrap();
    assert_eq!(module.deps, ["2"]);
}
