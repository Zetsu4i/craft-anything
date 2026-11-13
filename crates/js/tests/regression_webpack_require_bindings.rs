use craft_anything_js::{Limits, ParseMode, analyze};

fn dependencies(source: &str, id: &str) -> Vec<String> {
    let analysis = analyze("bundle.js", source, &Limits::default());
    assert_eq!(analysis.facts.mode, ParseMode::Parsed);
    assert_eq!(analysis.facts.parse_errors, 0);
    analysis.bundle.modules.into_iter().find(|module| module.id == id).unwrap().deps
}

#[test]
fn nested_parameters_do_not_load_modules() {
    let source = r#"
var __webpack_modules__ = {
    1: function(module, exports, r) {
        r(2);
        function nested(r) { r(3); }
        const arrow = (r) => r(4);
        function capturesLoader() { r(5); }
    },
    2: function() {},
    3: function() {},
    4: function() {},
    5: function() {}
};
"#;
    assert_eq!(dependencies(source, "1"), ["2", "5"]);
}

#[test]
fn block_and_catch_bindings_do_not_load_modules() {
    let source = r#"
var __webpack_modules__ = {
    1: function(module, exports, r) {
        { const r = () => {}; r(3); }
        try {} catch (r) { r(4); }
        r(2);
    },
    2: function() {},
    3: function() {},
    4: function() {}
};
"#;
    assert_eq!(dependencies(source, "1"), ["2"]);
}

#[test]
fn chunk_arrow_wrappers_keep_only_their_loader_binding() {
    let source = r#"
self.webpackChunkapp.push([[7], {
    1: (module, exports, r) => {
        const nested = (r) => r(3);
        const capturesLoader = () => r(2);
    },
    2: (module, exports, r) => { r(4); },
    3: () => {},
    4: () => {}
}]);
"#;
    assert_eq!(dependencies(source, "1"), ["2"]);
    assert_eq!(dependencies(source, "2"), ["4"]);
}

#[test]
fn missing_or_destructured_loader_parameters_do_not_alias_placeholder_names() {
    let source = r#"
var __webpack_modules__ = {
    1: function(module, exports, { loader }) { function _(id) {} _(3); },
    2: function(module, exports) { function r(id) {} r(3); },
    3: function() {}
};
"#;
    assert!(dependencies(source, "1").is_empty());
    assert!(dependencies(source, "2").is_empty());
}

#[test]
fn captured_loaders_keep_first_seen_distinct_dependencies() {
    let source = r#"
var __webpack_modules__ = [
    function(module, exports, r) {
        function outer() { function inner() { r(2); } r(1); }
        r(2);
    },
    function(module, exports, r) { r(2); },
    function() {}
];
"#;
    assert_eq!(dependencies(source, "0"), ["2", "1"]);
    assert_eq!(dependencies(source, "1"), ["2"]);
}
