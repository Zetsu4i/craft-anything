//! `rebuild` picks the AI it was told to use: a run on an API backend must not add a coding agent.
//! Each test gives the binary its own config, home and `PATH`, so the person's settings and the
//! agents installed on the machine don't decide the result.
//!
//! A run with no `--backend`, or an enabled one, stops when it opens `--continue-in`'s empty
//! folder, right after the builder check: the test sees exactly the choice `rebuild` made before
//! doing any work. An invalid command, an unknown name or a switched-off backend is refused before
//! that, so it cannot add an agent or leave a new project folder behind.
#![cfg(unix)]

use craft_anything_platform::process::{Output, Process};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Run the built CLI with only what the test gives it: no provider keys or installed agents leak in.
fn cli(args: &[&str], config: &Path, home: &Path, bin: &Path) -> Output {
    let path = format!("{}:/usr/bin:/bin", bin.display());
    Process::new(env!("CARGO_BIN_EXE_craft-anything-cli"))
        .args(args)
        .env_clear()
        .env("CRAFT_ANYTHING_CONFIG_DIR", config)
        .env("HOME", home)
        .env("PATH", path)
        .timeout(Duration::from_secs(90))
        .run()
        .unwrap()
}

/// A stand-in `claude` that Detect can find; it is never asked to build in these tests.
fn fake_claude(bin: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let program = bin.join("claude");
    std::fs::write(&program, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    program
}

/// A stand-in builder that answers one turn and exits, so a run that really starts finishes.
fn fake_builder(bin: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let program = bin.join("claude");
    std::fs::write(
        &program,
        "#!/bin/sh\nif [ \"$1\" = \"--version\" ]; then echo 'fake-claude 1.0'; exit 0; fi\nif [ \"$1\" = \"--help\" ]; then echo '--output-format stream-json'; exit 0; fi\ncat >/dev/null\nprintf '%s\\n' '{\"type\":\"system\",\"subtype\":\"init\",\"session_id\":\"fake\"}'\nprintf '%s\\n' '{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"Built it.\"}]}}'\nprintf '%s\\n' '{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"num_turns\":1}'\n",
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    program
}

fn api_config(config: &Path) {
    std::fs::write(
        config.join("config.toml"),
        "[[ai.backends]]\nname = \"API\"\nkind = \"openai_compatible\"\nmodel = \"test-model\"\nbase_url = \"http://127.0.0.1:0/v1\"\nenabled = true\n",
    )
    .unwrap();
}

/// A configured backend that is switched off, plus an enabled one for the default path.
fn disabled_config(config: &Path) {
    std::fs::write(
        config.join("config.toml"),
        "[[ai.backends]]\nname = \"Off\"\nkind = \"anthropic\"\nmodel = \"claude-test\"\nenabled = false\n\n[[ai.backends]]\nname = \"API\"\nkind = \"openai_compatible\"\nmodel = \"test-model\"\nbase_url = \"http://127.0.0.1:0/v1\"\nenabled = true\n",
    )
    .unwrap();
}

fn stopped_run(project: &Path, backend: Option<&str>) -> Vec<String> {
    let mut args = vec![
        "rebuild".to_string(),
        "-".to_string(),
        "make a small page".to_string(),
        "--continue-in".to_string(),
        project.display().to_string(),
        "--rounds".to_string(),
        "0".to_string(),
        "--unattended".to_string(),
    ];
    if let Some(name) = backend {
        args.extend(["--backend".to_string(), name.to_string()]);
    }
    args
}

/// R5.5: the run names the API backend, so it needs no agent app and must not add one.
#[test]
fn an_api_backend_build_does_not_add_a_coding_agent() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    api_config(config.path());
    fake_claude(bin.path());

    let args = stopped_run(project.path(), Some("API"));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = cli(&refs, config.path(), home.path(), bin.path());
    let log = out.stderr_text();
    assert!(!out.success, "the empty project folder must stop the run:\n{log}");
    assert!(log.contains("craft.json"), "the run must stop at the missing project, before any build:\n{log}");
    assert!(!log.contains("Using "), "a run on the API backend must not pick an agent:\n{log}");
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
    assert!(!saved.contains("agent_cli"), "no agent app may be saved for a run on an API backend:\n{saved}");
}

/// An enabled API backend is enough even when the request doesn't name it: the build can fall back
/// to it, so no agent app should be added either.
#[test]
fn an_enabled_api_backend_is_enough_without_being_named() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    api_config(config.path());
    fake_claude(bin.path());

    let args = stopped_run(project.path(), None);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = cli(&refs, config.path(), home.path(), bin.path());
    let log = out.stderr_text();
    assert!(!out.success, "the empty project folder must stop the run:\n{log}");
    assert!(log.contains("craft.json"), "the run must stop at the missing project, before any build:\n{log}");
    assert!(!log.contains("Using "), "an enabled API backend must be enough without naming it:\n{log}");
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
    assert!(!saved.contains("agent_cli"), "no agent app may be saved when an API backend is enabled:\n{saved}");
}

/// A named agent that is installed but not added yet is still registered, so the run can use it.
#[test]
fn a_named_installed_agent_is_added_when_missing() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    api_config(config.path());
    fake_claude(bin.path());

    let args = stopped_run(project.path(), Some("Claude Code"));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = cli(&refs, config.path(), home.path(), bin.path());
    let log = out.stderr_text();
    assert!(!out.success, "the empty project folder must stop the run:\n{log}");
    assert!(log.contains("craft.json"), "the run must stop at the missing project, before any build:\n{log}");
    assert!(log.contains("Using Claude Code"), "the named agent must still be registered:\n{log}");
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
    assert!(saved.contains("agent_cli") && saved.contains("name = \"Claude Code\""), "the named agent must be saved:\n{saved}");
}

/// An unknown name is named in the error, and must not add a different agent that happens to be
/// installed or blame a missing setup.
#[test]
fn an_unknown_backend_is_named_and_does_not_add_an_agent() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    fake_claude(bin.path());

    let args = stopped_run(project.path(), Some("No Such AI"));
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = cli(&refs, config.path(), home.path(), bin.path());
    let log = out.stderr_text();
    assert!(!out.success, "an unknown backend must stop the run:\n{log}");
    assert!(log.contains("No AI backend named") && log.contains("No Such AI"), "the error must name the name:\n{log}");
    assert!(!log.contains("Building needs a coding agent"), "the error must not blame the setup:\n{log}");
    assert!(!log.contains("Using "), "an unknown name must not pick an installed agent:\n{log}");
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap_or_default();
    assert!(!saved.contains("agent_cli"), "an unknown name must not save an agent:\n{saved}");
}

/// A backend the person switched off must not be used just because it was named: refuse it before
/// a project is made or opened, and say how to turn it on.
#[test]
fn a_disabled_backend_is_refused_before_a_project_is_opened_or_created() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    disabled_config(config.path());
    fake_claude(bin.path());

    let project = parent.path().join("new-project");
    let out = cli(
        &["rebuild", "-", "make a small page", "--project-folder", project.to_str().unwrap(), "--backend", "Off", "--rounds", "0", "--unattended"],
        config.path(),
        home.path(),
        bin.path(),
    );
    let log = out.stderr_text();
    assert!(!out.success, "the switched-off backend must stop the run:\n{log}");
    assert!(log.contains("Off") && log.contains("turned off"), "the error must name the backend and its state:\n{log}");
    assert!(log.contains("Turn it on"), "the error must say how to fix it:\n{log}");
    assert!(!log.contains("Project:"), "it must stop before a project is made:\n{log}");
    assert!(!project.exists(), "a refused command must not leave a project folder:\n{log}");

    // An existing project is refused before it is opened or a build starts.
    let existing = tempfile::tempdir().unwrap();
    let out = cli(
        &["rebuild", "-", "make a small page", "--continue-in", existing.path().to_str().unwrap(), "--backend", "Off", "--rounds", "0", "--unattended"],
        config.path(),
        home.path(),
        bin.path(),
    );
    let log = out.stderr_text();
    assert!(!out.success, "the switched-off backend must stop an existing project too:\n{log}");
    assert!(log.contains("Off") && log.contains("turned off"), "the error must name the backend and its state:\n{log}");
    assert!(!log.contains("craft.json") && !log.contains("Couldn't reach"), "it must stop before opening or building:\n{log}");
}

/// A command refused before any build starts must not change the person's AI settings either.
#[test]
fn an_invalid_command_does_not_add_an_agent() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    fake_claude(bin.path());

    let out = cli(
        &["rebuild", "-", "make a small page", "--continue-in", project.path().to_str().unwrap(), "--build-with", "rust", "--rounds", "0", "--unattended"],
        config.path(),
        home.path(),
        bin.path(),
    );
    let log = out.stderr_text();
    assert!(!out.success, "the refused command must fail:\n{log}");
    assert!(log.contains("--build-with is for new projects"), "the command must stop at validation:\n{log}");
    assert!(!log.contains("Using "), "a refused command must not pick an agent:\n{log}");
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap_or_default();
    assert!(!saved.contains("agent_cli"), "a refused command must not save an agent:\n{saved}");
}

/// A folder that already holds a project is attached; new-project-only options are refused and
/// its saved builder, budget, asset setting and stack stay.
#[test]
fn a_new_project_option_is_refused_at_an_attached_project() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    fake_claude(bin.path());
    let parent = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(parent.path(), "Notes", "A notes app", TargetRef::None).unwrap();
    ws.set_person(|m| {
        m.builder = Some("Claude".into());
        m.asset_budget_usd = 42.0;
        m.generate_assets = true;
    })
    .unwrap();
    ws.manifest.stack = "Godot".into();
    ws.save().unwrap();
    let root = ws.root.to_string_lossy().into_owned();
    for (args, said) in [
        (
            vec!["rebuild", "-", "add a jump", "--project-folder", root.as_str(), "--build-with", "rust", "--rounds", "0", "--unattended"],
            "--build-with is for new projects",
        ),
        (
            vec!["rebuild", "-", "add a jump", "--project-folder", root.as_str(), "--generate-assets", "--rounds", "0", "--unattended"],
            "--generate-assets is for new projects",
        ),
        (
            vec!["rebuild", "-", "add a jump", "--project-folder", root.as_str(), "--existing", "edit", "--rounds", "0", "--unattended"],
            "already holds a project",
        ),
    ] {
        let out = cli(&args, config.path(), home.path(), bin.path());
        let log = out.stderr_text();
        assert!(!out.success, "{said}: {log}");
        assert!(log.contains(said), "{said}: {log}");
    }
    let saved = Workspace::open(&ws.root).unwrap().manifest;
    assert_eq!(saved.builder.as_deref(), Some("Claude"), "the saved builder is kept");
    assert_eq!(saved.asset_budget_usd, 42.0, "the saved budget is kept");
    assert!(saved.generate_assets, "the saved asset setting is kept");
    assert_eq!(saved.stack, "Godot", "the saved stack is kept");
}

/// The success path: an attached folder keeps its settings and its typed request becomes the
/// run's follow-up (the API has no listener, so the run fails after it starts; the feed is kept).
#[test]
fn an_attached_project_sends_the_request_as_a_follow_up() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    api_config(config.path());
    let parent = tempfile::tempdir().unwrap();
    let mut ws = Workspace::create(parent.path(), "Notes", "A notes app", TargetRef::None).unwrap();
    ws.set_person(|m| {
        m.builder = Some("Claude".into());
        m.asset_budget_usd = 42.0;
        m.generate_assets = true;
    })
    .unwrap();
    ws.manifest.stack = "Godot".into();
    ws.save().unwrap();
    let out = cli(
        &["rebuild", "-", "add a jump", "--project-folder", ws.root.to_str().unwrap(), "--backend", "API", "--rounds", "0", "--unattended"],
        config.path(),
        home.path(),
        bin.path(),
    );
    let feed = craft_anything_engine::feed::load(&ws);
    assert!(
        feed.iter().any(|i| matches!(i, craft_anything_engine::feed::FeedItem::User(u) if u.text() == "add a jump")),
        "the request is the run's follow-up:\n{}",
        out.stderr_text()
    );
    let saved = Workspace::open(&ws.root).unwrap().manifest;
    assert_eq!(saved.builder.as_deref(), Some("Claude"), "the saved builder is kept");
    assert_eq!(saved.asset_budget_usd, 42.0, "the saved budget is kept");
    assert!(saved.generate_assets, "the saved asset setting is kept");
    assert_eq!(saved.stack, "Godot", "the saved stack is kept");
}

/// The guard must not stop a computer with no AI at all from finding and saving an agent.
#[test]
fn a_computer_with_no_ai_still_finds_an_agent() {
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    fake_claude(bin.path());

    let args = stopped_run(project.path(), None);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = cli(&refs, config.path(), home.path(), bin.path());
    let log = out.stderr_text();
    assert!(!out.success, "the empty project folder must stop the run:\n{log}");
    assert!(log.contains("craft.json"), "the run must stop at the missing project, before any build:\n{log}");
    assert!(log.contains("Using Claude Code"), "an agent must still be found when no AI is set up:\n{log}");
    let saved = std::fs::read_to_string(config.path().join("config.toml")).unwrap();
    assert!(saved.contains("agent_cli"), "the found agent must still be saved:\n{saved}");
}

/// A project whose saved builder was switched off: once the run starts, it names the saved AI
/// and the one building instead.
#[test]
fn a_replaced_saved_builder_is_named_once_the_run_starts() {
    use craft_anything_engine::workspace::{TargetRef, Workspace};
    let config = tempfile::tempdir().unwrap();
    let home = tempfile::tempdir().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let script = fake_builder(bin.path());
    std::fs::write(
        config.path().join("config.toml"),
        format!(
            "[[ai.backends]]\nname = \"Off\"\nkind = \"anthropic\"\nmodel = \"claude-test\"\nenabled = false\n\n[[ai.backends]]\nname = \"Claude\"\nkind = \"agent_cli\"\nmodel = \"claude-code\"\ncommand = \"{}\"\nenabled = true\n",
            script.display()
        ),
    )
    .unwrap();
    let mut ws = Workspace::create(parent.path(), "Notes", "A notes app", TargetRef::None).unwrap();
    ws.set_person(|m| m.builder = Some("Off".into())).unwrap();

    let args = stopped_run(&ws.root, None);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = cli(&refs, config.path(), home.path(), bin.path());
    let log = out.stderr_text();
    assert!(!out.success, "the run must stop at the empty app:\n{log}");
    assert!(log.contains("Off") && log.contains("Claude") && log.contains("instead"), "the saved builder and its replacement must be named:\n{log}");
    assert!(!log.contains("Using "), "a replacement is not an added agent:\n{log}");
}
