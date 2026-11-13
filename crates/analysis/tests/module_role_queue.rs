use craft_anything_analysis::js_app::{AppFile, AppInput, AppOptions, analyze_application};

fn graph_of(files: Vec<(&str, String)>) -> craft_anything_analysis::js_app::AppGraph {
    let files: Vec<_> = files.into_iter().map(|(path, source)| AppFile { path: path.into(), bytes: source.into_bytes() }).collect();
    let listing = files.iter().map(|file| (file.path.clone(), file.bytes.len() as u64)).collect();
    let input = AppInput { label: "role-fixture".into(), format: "directory".into(), files, listing, notes: vec![] };
    analyze_application(&input, &AppOptions::default(), &|| false).unwrap()
}

#[test]
fn repeated_imports_do_not_exhaust_role_propagation_before_reachable_modules() {
    let paths: Vec<_> = (0..52).map(|index| format!("module{index}.js")).collect();
    let mut files = vec![("package.json", r#"{"name":"role-fixture","main":"module0.js"}"#.to_string())];
    for (index, path) in paths.iter().enumerate() {
        let source = if let Some(next) = paths.get(index + 1) { format!("require('./{next}');\n").repeat(2_000) } else { "module.exports = 1;".to_string() };
        files.push((path, source));
    }
    let graph = graph_of(files);
    assert_eq!(graph.modules.len(), paths.len());
    assert_eq!(graph.module("module50.js").unwrap().imports.len(), 2_000);
    for path in paths {
        assert_eq!(graph.module(&path).unwrap().role, "package", "reachable module {path}");
    }
}

#[test]
fn cycles_and_shared_children_keep_reachable_roles_without_claiming_orphans() {
    let graph = graph_of(vec![
        ("package.json", r#"{"name":"roles","main":"main.js"}"#.into()),
        ("main.js", "require('./left.js'); require('./right.js');".into()),
        ("left.js", "require('./shared.js'); require('./main.js');".into()),
        ("right.js", "require('./shared.js');".into()),
        ("shared.js", "module.exports = 1;".into()),
        ("orphan.js", "module.exports = 2;".into()),
    ]);
    for path in ["main.js", "left.js", "right.js", "shared.js"] {
        assert_eq!(graph.module(path).unwrap().role, "package");
    }
    assert_eq!(graph.module("orphan.js").unwrap().role, "unknown");
}

#[test]
fn main_preload_and_renderer_roles_keep_their_existing_precedence() {
    let graph = graph_of(vec![
        ("package.json", r#"{"name":"roles","main":"main.js","dependencies":{"electron":"1"}}"#.into()),
        ("main.js", "const { BrowserWindow } = require('electron'); require('./shared.js'); const win = new BrowserWindow({webPreferences:{preload:'preload.js'}}); win.loadFile('page.html');".into()),
        ("preload.js", "require('./shared.js'); require('./preload-only.js');".into()),
        ("preload-only.js", "module.exports = 1;".into()),
        ("shared.js", "module.exports = 2;".into()),
        ("page.html", "<script>require('./renderer-only.js');</script>".into()),
        ("renderer-only.js", "module.exports = 3;".into()),
    ]);
    for (path, role) in [
        ("main.js", "main"),
        ("shared.js", "main"),
        ("preload.js", "preload"),
        ("preload-only.js", "preload"),
        ("page.html", "renderer"),
        ("page.html#inline-0", "renderer"),
        ("renderer-only.js", "renderer"),
    ] {
        assert_eq!(graph.module(path).unwrap().role, role, "module {path}");
    }
}
