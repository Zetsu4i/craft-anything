use craft_anything_analysis::js_app::{AppFile, AppGraph, AppInput, AppOptions, analyze_application};

fn graph_of(files: Vec<(&str, String)>) -> AppGraph {
    let files: Vec<_> = files.into_iter().map(|(path, source)| AppFile { path: path.into(), bytes: source.into_bytes() }).collect();
    let listing = files.iter().map(|file| (file.path.clone(), file.bytes.len() as u64)).collect();
    let input = AppInput { label: "worker-role-fixture".into(), format: "directory".into(), files, listing, notes: vec![] };
    analyze_application(&input, &AppOptions::default(), &|| false).unwrap()
}

fn assert_worker(graph: &AppGraph, path: &str, helper: &str, edge_kind: &str) {
    assert!(graph.entry_points.workers.iter().any(|worker| worker == path));
    assert_eq!(graph.module(path).unwrap().role, "worker", "entry {path}");
    assert_eq!(graph.module(helper).unwrap().role, "worker", "dependency {helper}");
    assert!(graph.endpoints.iter().any(|row| row["module"] == path && row["role"] == "worker"));
    let node = graph.nodes.iter().find(|node| node.kind == "module" && node.module.as_deref() == Some(path)).unwrap();
    assert_eq!(node.role.as_deref(), Some("worker"));
    assert!(graph.edges.iter().any(|edge| edge.to == node.id && edge.kind == edge_kind));
}

#[test]
fn workers_and_shared_workers_keep_their_own_role_after_package_launches() {
    for launch in ["new Worker('./worker.js');", "new SharedWorker('./worker.js');", "new Worker(new URL('./worker.js', import.meta.url));"] {
        let graph = graph_of(vec![
            ("package.json", r#"{"name":"workers","main":"main.js"}"#.into()),
            ("main.js", launch.into()),
            ("worker.js", "require('./helper.js'); fetch('https://example.test/worker');".into()),
            ("helper.js", "module.exports = 1;".into()),
        ]);
        assert_eq!(graph.module("main.js").unwrap().role, "package");
        assert_worker(&graph, "worker.js", "helper.js", "worker");
    }
}

#[test]
fn service_worker_roles_do_not_leak_from_the_registering_page() {
    let graph = graph_of(vec![
        ("page.html", "<script src=\"app.js\"></script>".into()),
        ("app.js", "navigator.serviceWorker.register('./sw.js');".into()),
        ("sw.js", "importScripts('./helper.js'); fetch('https://example.test/sw');".into()),
        ("helper.js", "self.ready = true;".into()),
    ]);
    assert_eq!(graph.module("page.html").unwrap().role, "page");
    assert_eq!(graph.module("app.js").unwrap().role, "page");
    assert_worker(&graph, "sw.js", "helper.js", "service-worker");
}

#[test]
fn forked_process_roles_do_not_leak_from_the_parent_process() {
    for launch in [
        "const { utilityProcess } = require('electron'); utilityProcess.fork('./child.js');",
        "const child_process = require('child_process'); child_process.fork('./child.js');",
    ] {
        let graph = graph_of(vec![
            ("package.json", r#"{"name":"workers","main":"main.js","dependencies":{"electron":"1"}}"#.into()),
            ("main.js", launch.into()),
            ("child.js", "require('./helper.js'); fetch('https://example.test/child');".into()),
            ("helper.js", "module.exports = 1;".into()),
        ]);
        assert_eq!(graph.module("main.js").unwrap().role, "main");
        assert_worker(&graph, "child.js", "helper.js", "utility-process");
    }
}

#[test]
fn ordinary_imports_shared_with_a_worker_keep_the_existing_parent_precedence() {
    let graph = graph_of(vec![
        ("package.json", r#"{"name":"workers","main":"main.js"}"#.into()),
        ("main.js", "require('./shared.js'); new Worker('./worker.js');".into()),
        ("worker.js", "require('./shared.js');".into()),
        ("shared.js", "require('./shared-child.js');".into()),
        ("shared-child.js", "module.exports = 1;".into()),
        ("orphan.js", "module.exports = 2;".into()),
    ]);
    for path in ["main.js", "shared.js", "shared-child.js"] {
        assert_eq!(graph.module(path).unwrap().role, "package");
    }
    assert_eq!(graph.module("orphan.js").unwrap().role, "unknown");
}

#[test]
fn worker_discovery_still_reaches_nested_workers_without_a_package_entry() {
    let graph = graph_of(vec![
        ("launcher.js", "new Worker('./worker.js');".into()),
        ("worker.js", "new Worker('./nested.js'); require('./helper.js');".into()),
        ("nested.js", "require('./helper.js');".into()),
        ("helper.js", "module.exports = 1;".into()),
    ]);
    assert_eq!(graph.module("launcher.js").unwrap().role, "unknown");
    assert_eq!(graph.entry_points.workers, ["worker.js", "nested.js"]);
    for path in ["worker.js", "nested.js", "helper.js"] {
        assert_eq!(graph.module(path).unwrap().role, "worker");
    }
}
