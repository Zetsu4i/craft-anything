use craft_anything_js::{Analysis, Limits, ParseMode, analyze};

fn bundle(source: &str) -> Analysis {
    let analysis = analyze("bundle.js", source, &Limits::default());
    assert_eq!(analysis.facts.mode, ParseMode::Parsed);
    assert_eq!(analysis.facts.parse_errors, 0);
    analysis
}

#[test]
fn numeric_module_keys_and_loads_use_the_same_identity() {
    let analysis = bundle(
        r#"
var __webpack_modules__ = {
    1: function(module, exports, r) { r(16); r(0b10); r(0o3); r(1e3); r(1_000); },
    0x10: function() {},
    2: function() {},
    3: function() {},
    1000: function() {}
};
"#,
    );
    let ids: Vec<_> = analysis.bundle.modules.iter().map(|module| module.id.as_str()).collect();
    assert_eq!(ids, ["1", "16", "2", "3", "1000"]);
    assert_eq!(analysis.bundle.modules.first().unwrap().deps, ["16", "2", "3", "1000"]);
}

#[test]
fn string_ids_keep_their_literal_spelling() {
    let analysis = bundle(
        r#"
var __webpack_modules__ = {
    1: function(module, exports, r) { r("0x10"); r(0x10); r("0002"); },
    "0x10": function() {},
    16: function() {},
    "0002": function() {}
};
"#,
    );
    let ids: Vec<_> = analysis.bundle.modules.iter().map(|module| module.id.as_str()).collect();
    assert_eq!(ids, ["1", "0x10", "16", "0002"]);
    assert_eq!(analysis.bundle.modules.first().unwrap().deps, ["0x10", "16", "0002"]);
}

#[test]
fn array_tables_match_non_decimal_loader_arguments() {
    let analysis = bundle(
        r#"
var __webpack_modules__ = [
    function(module, exports, r) { r(0x1); r(0b10); },
    function() {},
    function() {}
];
"#,
    );
    assert_eq!(analysis.bundle.modules.first().unwrap().deps, ["1", "2"]);
}

#[test]
fn browserify_maps_match_javascript_exponent_keys() {
    let analysis = bundle(
        r#"
var modules = {
    0x1: [function(require, module, exports) { require("./large"); }, { "./large": 1e21 }],
    1e21: [function(require, module, exports) {}, {}]
};
"#,
    );
    let ids: Vec<_> = analysis.bundle.modules.iter().map(|module| module.id.as_str()).collect();
    assert_eq!(ids, ["1", "1e+21"]);
    assert_eq!(analysis.bundle.modules.first().unwrap().deps, ["1e+21"]);
}

#[test]
fn chunk_ids_use_javascript_number_spelling_without_changing_strings() {
    let analysis = bundle(
        r#"
self.webpackChunkapp.push([[1e21, 0x10, "0x10"], {
    1: function(module, exports, r) {},
    2: function() {}
}]);
"#,
    );
    assert_eq!(analysis.bundle.chunk_ids, ["1e+21", "16", "0x10"]);
    assert_eq!(analysis.bundle.modules.first().unwrap().chunk_ids, ["1e+21", "16", "0x10"]);
}

#[test]
fn ordinary_decimal_ids_and_duplicate_loads_are_unchanged() {
    let analysis = bundle(
        r#"
var __webpack_modules__ = {
    1: function(module, exports, r) { r(2); r(2); },
    2: function() {}
};
"#,
    );
    assert_eq!(analysis.bundle.modules.first().unwrap().deps, ["2"]);
    assert_eq!(analysis.bundle.modules.last().unwrap().id, "2");
}
