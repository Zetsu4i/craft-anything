//! Building a project with its own tools (cargo, npm, or the game engine's), for [`BuildCheck`].
//! A plain static site has no build, and passes as it is. A person's own project builds and tests
//! itself the way its toolchain says.

use super::*;

/// Run a step of a project's build in `app`: whether it succeeded, and what it printed.
fn run_in(app: &Path, prog: &Path, args: &[String], minutes: u64, stop: &CheckStop) -> Result<(bool, String), String> {
    // A round with a time limit (the builder's own checks) cuts a step that would run past it,
    // instead of waiting out the step's own, much longer, deadline.
    let limit = Duration::from_secs(minutes * 60);
    let timeout = stop.time_left().map_or(limit, |left| limit.min(left));
    // Nothing starts once it's time to stop: a step starting past the limit could exit before its
    // zero deadline is seen and count as passed.
    stop.check().map_err(|e| e.message)?;
    let s = stop.clone();
    craft_anything_platform::Process::new(prog)
        .args(args)
        .cwd(app)
        .timeout(timeout)
        // Engines go quiet for long stretches (shader compiles, cooking, a cold cargo build).
        .idle_timeout(Duration::from_secs(60 * 60))
        .stdout_cap(8 << 20)
        // A pause stops the program now; the round's limit reaches it as the clamped deadline
        // above, which says the step took longer than allowed.
        .cancel_when(move || s.paused())
        .run()
        .map(|o| {
            let (stdout, stderr) = if o.success { (o.stdout_text(), o.stderr_text()) } else { (o.stdout_end_text(), o.stderr_end_text()) };
            (o.success, format!("{stdout}{stderr}"))
        })
        .map_err(|e| e.message)
}

/// Run one of a toolchain's commands in `app`.
fn run_tool(app: &Path, tool: &stacks::Toolchain, command: &[String], minutes: u64, stop: &CheckStop) -> Result<(bool, String), String> {
    let Some((program, args)) = tool.command(app, command) else { return Err(format!("{} has no such command.", tool.name)) };
    run_in(app, &program, &args, minutes, stop)
}

/// What a Unity without a licence prints, and what to tell the person.
const UNITY_NO_LICENCE: &str = "No valid Unity Editor license";
const UNITY_LICENCE_FIX: &str = "Unity needs its free licence first: open Unity Hub, sign in and accept the Personal licence, then try again.";

/// A person's own project: its engine's build, else its toolchain's, else a static site. One we
/// know no way to build isn't a failure, and the note says nothing ran.
pub(super) fn own_build_check(app: &Path, place: &str, stack: &str, editor: Option<&Path>, stop: &CheckStop) -> (bool, String) {
    let engine = engine_for(stack, app);
    if let (Some(game::Builder::Unity), Some(ed)) = (engine, editor) {
        return own_unity_check(app, ed, stop);
    }
    match stacks::toolchain_in(app).filter(|_| engine.is_none()) {
        Some(tool) => own_build(app, tool, stop),
        None if engine.is_some() || static_site(app) => build_check(app, place, stack, editor, stop),
        None => (true, "Nothing was built: Craft Anything doesn't know how to build this kind of project.".into()),
    }
}

/// How a Unity project we didn't make is opened: in batch mode, which compiles its scripts. There's
/// no `CraftBuild.Build` to call, as there is in one we made ([`build_check`]).
fn own_unity_args(app: &Path) -> Vec<String> {
    let app = craft_anything_engine::workspace::absolute(app);
    ["-batchmode", "-quit", "-nographics", "-projectPath", &app.display().to_string(), "-logFile", "-"].map(String::from).to_vec()
}

fn own_unity_check(app: &Path, editor: &Path, stop: &CheckStop) -> (bool, String) {
    match run_in(app, editor, &own_unity_args(app), 4 * 60, stop) {
        Ok((_, out)) if out.contains(UNITY_NO_LICENCE) => (false, UNITY_LICENCE_FIX.into()),
        Ok((true, _)) => (true, "The Unity project opened and its scripts compiled.".into()),
        Ok((false, out)) => (false, format!("Build failed: {}", error_lines(&out))),
        Err(e) => (false, format!("Couldn't run the build: {e}")),
    }
}

fn own_build(app: &Path, tool: &stacks::Toolchain, stop: &CheckStop) -> (bool, String) {
    match run_tool(app, tool, &tool.build, 2 * 60, stop) {
        Ok((true, _)) => (true, format!("`{}` passed.", tool.build.join(" "))),
        Ok((false, out)) => (false, format!("Build failed: {}", error_lines(&out))),
        Err(e) => (false, format!("Couldn't run the build: {e}")),
    }
}

/// A person's own project: run its tests. Output that says none ran isn't a failure, and neither
/// is a project we know no way to test, and the note says so.
pub(super) fn own_tests_check(app: &Path, tool: Option<&stacks::Toolchain>, stop: &CheckStop) -> (bool, String) {
    let Some(tool) = tool else { return (true, "No tests ran: Craft Anything doesn't know how to test this kind of project.".into()) };
    let says_none = |text: &str| tool.no_tests.iter().find(|n| text.contains(n.as_str())).cloned();
    if let Some(name) = &tool.test_script {
        // shortcut: `workspaces=true` in .npmrc can give a root with no script tests; they aren't
        // run, and the note says so. Read .npmrc if that turns up.
        let script = craft_anything_engine::outputs::npm_script(app, name).unwrap_or_default();
        if script.is_empty() || tool.no_tests.iter().any(|n| n == script.trim()) {
            return (true, format!("No tests ran: package.json has no `{name}` script to run."));
        }
    }
    match run_tool(app, tool, &tool.test, 30, stop) {
        Ok((true, _)) => (true, format!("`{}` passed.", tool.test.join(" "))),
        Ok((false, out)) => match says_none(&out).filter(|_| tool.test_script.is_none()) {
            Some(why) => (true, format!("No tests ran: {}.", why.trim_end_matches('.'))),
            None => (false, format!("The project's tests failed:\n{}", last_lines(&out, 20))),
        },
        Err(e) => (false, format!("Couldn't run the tests: {e}")),
    }
}

/// The last `n` lines with something on them: where a test run sums up what failed.
fn last_lines(out: &str, n: usize) -> String {
    let mut lines: Vec<&str> = out.lines().rev().filter(|l| !l.trim().is_empty()).take(n).collect();
    lines.reverse();
    lines.join("\n")
}

/// For non-website builds: run the project's build (its engine's own, for Unity, Unreal and
/// Godot) and report whether it succeeded, with a note (the first error lines when it didn't).
/// `place` says where `app` is, for messages (`Workspace::app_place`).
pub(super) fn build_check(app: &Path, place: &str, stack: &str, editor: Option<&Path>, stop: &CheckStop) -> (bool, String) {
    let run = |prog: &Path, args: &[String], minutes: u64| run_in(app, prog, args, minutes, stop).map_err(|e| format!("Couldn't run the build: {e}"));
    let app_abs = craft_anything_engine::workspace::absolute(app);
    let app_s = app_abs.display().to_string();
    let built = || packaged_program(&app_abs.join("Build")).is_some();
    let result: Result<(bool, String), String> = match (engine_for(stack, app), editor) {
        (Some(game::Builder::Unity), Some(ed)) => {
            if !app.join("Assets").is_dir() {
                return (false, missing(app, place, format!("No Unity project found in {place} yet.")));
            }
            let args = ["-batchmode", "-quit", "-nographics", "-projectPath", &app_s, "-executeMethod", "CraftBuild.Build", "-logFile", "-"].map(String::from);
            run(ed, &args, 4 * 60).map(|(ok, out)| if out.contains(UNITY_NO_LICENCE) { (false, UNITY_LICENCE_FIX.into()) } else { (ok && built(), out) })
        }
        (Some(game::Builder::Unreal), Some(ed)) => {
            let Some(uproject) = uproject(app) else {
                return (false, missing(app, place, format!("No Unreal project (.uproject) found in {place} yet.")));
            };
            // The editor is <engine>/Engine/Binaries/<platform>/UnrealEditor; its packager is beside it.
            let Some(root) = ed.ancestors().nth(4) else { return (false, "Couldn't find Unreal's build tools.".into()) };
            let uat = root.join("Engine/Build/BatchFiles").join(if cfg!(windows) { "RunUAT.bat" } else { "RunUAT.sh" });
            let platform = if cfg!(windows) {
                "Win64"
            } else if cfg!(target_os = "macos") {
                "Mac"
            } else {
                "Linux"
            };
            let args = vec![
                "BuildCookRun".to_string(),
                format!("-project={}", uproject.display()),
                format!("-platform={platform}"),
                "-clientconfig=Development".into(),
                "-build".into(),
                "-cook".into(),
                "-stage".into(),
                "-pak".into(),
                "-archive".into(),
                format!("-archivedirectory={}", app_abs.join("Build").display()),
                "-unattended".into(),
                "-utf8output".into(),
                "-nop4".into(),
            ];
            // A first Unreal build compiles and cooks for a long while. Run RunUAT.bat directly:
            // std quotes .bat launches itself, whereas `cmd /C "<path with spaces>" "<arg>"` strips the quotes.
            run(&uat, &args, 8 * 60).map(|(ok, out)| (ok && built(), out))
        }
        (Some(game::Builder::Godot), Some(ed)) => {
            if !app.join("project.godot").is_file() {
                return (false, missing(app, place, format!("No Godot project (project.godot) found in {place} yet.")));
            }
            let import = ["--headless", "--path", &app_s, "--import"].map(String::from);
            let play = ["--headless", "--path", &app_s, "--quit-after", "120"].map(String::from);
            // Godot exits cleanly even when scripts fail, so its error lines decide.
            run(ed, &import, 60).and_then(|(_, a)| run(ed, &play, 10).map(|(ok, b)| (ok, a + &b))).map(|(ok, out)| {
                let bad = out.lines().any(|l| l.contains("SCRIPT ERROR") || l.contains("Parse Error") || l.trim_start().starts_with("ERROR:"));
                (ok && !bad, out)
            })
        }
        _ if app.join("Cargo.toml").is_file() => run(Path::new("cargo"), &["build".into(), "--quiet".into()], 2 * 60),
        _ if app.join("package.json").is_file() => run(Path::new(if cfg!(windows) { "npm.cmd" } else { "npm" }), &["run".into(), "build".into()], 2 * 60),
        (None, _) => {
            return stacks::toolchain_in(app).map_or_else(
                || {
                    if static_site(app) {
                        (true, "Nothing to build: a static site.".into())
                    } else {
                        (false, missing(app, place, format!("No project to build found in {place} yet.")))
                    }
                },
                |tool| own_build(app, tool, stop),
            );
        }
        _ => return (false, missing(app, place, format!("No project to build found in {place} yet."))),
    };
    match result {
        Ok((true, _)) => (true, "Build succeeded.".into()),
        Ok((false, out)) => (false, format!("Build failed: {}", error_lines(&out))),
        Err(e) => (false, e),
    }
}

/// The engine whose own build runs: the stack's, unless app/ holds another kind of project
/// rather than the engine's. What was built decides, since the person may have asked for
/// something other than the engine picked at the start.
fn engine_for(stack: &str, app: &Path) -> Option<game::Builder> {
    if static_site(app) {
        return None;
    }
    let own_project = stacks::toolchain_in(app).is_some();
    game::Builder::from_stack(stack).filter(|b| !own_project || engine_project(*b, app))
}

/// Whether `app` is a static site with nothing to build: a page with something in it, as the
/// preview finds it, and no game engine's project beside it. A site's `assets/` folder is no sign
/// of Unity (file names ignore case on Windows and macOS), so Unity needs its settings folder too.
fn static_site(app: &Path) -> bool {
    let engine = app.join("project.godot").is_file() || uproject(app).is_some() || (app.join("Assets").is_dir() && app.join("ProjectSettings").is_dir());
    let page = |site: PathBuf| std::fs::read(site.join("index.html")).is_ok_and(|html| html.iter().any(|b| !b.is_ascii_whitespace()));
    !engine && site_dir(app).is_some_and(page)
}

/// A missing-project note, saying where a project is when one was made a folder down (`cargo new
/// app` run inside app/ makes app/app/), so the builder can move it instead of starting again.
/// Only app/, which is the build's own, is told the files belong in it: a person's own project
/// folder, edited in place, may hold projects of its own below it.
fn missing(app: &Path, place: &str, none: String) -> String {
    let Some((dir, manifest)) = nested_project(app) else { return none };
    if place.ends_with('/') {
        format!("{none} There's a {manifest} one folder down, in {place}{dir}/, but the build runs in {place} itself: its files belong there.")
    } else {
        format!("{none} There's a {manifest} one folder down, in {dir}/ in {place}; the build runs in {place} itself.")
    }
}

/// The first folder directly inside `app` (by name, of the first 256 entries) that holds a cargo or
/// npm project, and that project's manifest.
fn nested_project(app: &Path) -> Option<(String, &'static str)> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(app).ok()?.flatten().take(256).map(|e| e.path()).filter(|p| p.is_dir()).collect();
    dirs.sort();
    dirs.iter().find_map(|d| {
        let manifest = ["Cargo.toml", "package.json"].into_iter().find(|m| d.join(m).is_file())?;
        Some((d.file_name()?.to_string_lossy().into_owned(), manifest))
    })
}

/// The Unreal project file in `app`, if there is one.
fn uproject(app: &Path) -> Option<PathBuf> {
    let project = std::fs::read_dir(app).into_iter().flatten().flatten().map(|e| e.path()).find(|p| p.extension().is_some_and(|x| x == "uproject"))?;
    Some(craft_anything_engine::workspace::absolute(&project))
}

/// Whether `app` holds a project of the engine `b` builds with (Rust's is an ordinary cargo one).
fn engine_project(b: game::Builder, app: &Path) -> bool {
    match b {
        game::Builder::Unity => app.join("Assets").is_dir(),
        game::Builder::Unreal => uproject(app).is_some(),
        game::Builder::Godot => app.join("project.godot").is_file(),
        game::Builder::Rust => false,
    }
}

/// The lines that explain a failed build: error lines first, else the last lines.
pub(super) fn error_lines(out: &str) -> String {
    let lines: Vec<&str> = out.lines().filter(|l| !l.trim().is_empty()).collect();
    let errors: Vec<&str> = lines.iter().copied().filter(|l| l.to_ascii_lowercase().contains("error") || l.contains("licen")).take(12).collect();
    if errors.is_empty() { lines[lines.len().saturating_sub(12)..].join("\n") } else { errors.join("\n") }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stand-in toolchain whose build and tests are this shell line.
    #[cfg(unix)]
    fn stand_in(line: &str, no_tests: &[&str]) -> stacks::Toolchain {
        let command = ["sh", "-c", line].map(String::from).to_vec();
        stacks::Toolchain { build: command.clone(), test: command, no_tests: no_tests.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[cfg(unix)]
    #[test]
    fn a_persons_own_tests_decide_and_no_tests_is_no_failure() {
        let (d, stop) = (tempfile::tempdir().unwrap(), CheckStop::default());
        let tests = |tool: &stacks::Toolchain| own_tests_check(d.path(), Some(tool), &stop);
        assert!(tests(&stand_in("exit 0", &[])).0);
        let (passed, note) = tests(&stand_in("echo 'adds: 4 is not 5'; echo; echo '1 failed'; exit 1", &[]));
        assert!(!passed && note.ends_with("adds: 4 is not 5\n1 failed"), "the summary, not only lines saying error: {note}");
        let (passed, note) = tests(&stand_in("echo no tests here; exit 5", &["no tests here"]));
        assert!(passed && note.starts_with("No tests ran"), "{note}");
        let missing = stacks::Toolchain { test: vec!["no-such-program-here".into()], ..Default::default() };
        let (passed, note) = tests(&missing);
        assert!(!passed && note.starts_with("Couldn't run the tests"), "{note}");
        let (passed, note) = own_tests_check(d.path(), None, &stop);
        assert!(passed && note.starts_with("No tests ran"), "{note}");
    }

    #[cfg(unix)]
    #[test]
    fn a_projects_own_copy_of_the_program_comes_first() {
        let (d, stop) = (tempfile::tempdir().unwrap(), CheckStop::default());
        std::os::unix::fs::symlink("/bin/sh", d.path().join("own-sh")).unwrap();
        std::fs::write(d.path().join("own-sh.bat"), "not for this system").unwrap();
        let test = ["no-such-program-here", "-c", "echo 'error: from its own copy'; exit 1"].map(String::from).to_vec();
        let tool = stacks::Toolchain { test, local_programs: vec!["not-here".into(), "own-sh.bat".into(), "own-sh".into()], ..Default::default() };
        let (passed, note) = own_tests_check(d.path(), Some(&tool), &stop);
        assert!(!passed && note.contains("from its own copy"), "{note}");
    }

    /// The table's own words, on what the real make (3.81; 4.x quotes alike) and npm (11) print.
    #[cfg(unix)]
    #[test]
    fn only_the_tests_own_absence_means_no_tests() {
        let (d, stop) = (tempfile::tempdir().unwrap(), CheckStop::default());
        let named_by = |file: &str| {
            let f = tempfile::tempdir().unwrap();
            std::fs::write(f.path().join(file), "").unwrap();
            stacks::toolchain_in(f.path()).expect("the table has it")
        };
        let printing = |tool: &stacks::Toolchain, line: &str| {
            let test = ["sh", "-c", "printf '%s\\n' \"$1\"; exit 2", "sh", line].map(String::from).to_vec();
            own_tests_check(d.path(), Some(&stacks::Toolchain { test, ..tool.clone() }), &stop).0
        };
        let (make, node) = (named_by("Makefile"), named_by("package.json"));
        assert!(printing(make, "make: *** No rule to make target `test'.  Stop."));
        assert!(printing(make, "make: *** No rule to make target 'test'.  Stop."));
        assert!(!printing(make, "make: *** No rule to make target `fixture.txt', needed by `test'.  Stop."), "a prerequisite is a failure");
        assert!(!printing(make, "make: *** No rule to make target 'fixture.txt', needed by 'test'.  Stop."));
        assert!(!printing(make, "make[1]: *** No rule to make target `test'.  Stop."), "a make it runs is a failure");
        std::fs::write(d.path().join("package.json"), r#"{"scripts":{"test":"npm test --workspaces"}}"#).unwrap();
        assert!(!printing(node, "npm error Missing script: \"test\""), "one package of several, maybe beside a real failure");
        assert!(!printing(node, "npm error Missing script: \"verify\""), "a script its tests call is a failure");
    }

    #[test]
    fn a_node_project_says_in_its_package_json_whether_it_has_tests() {
        let (d, stop) = (tempfile::tempdir().unwrap(), CheckStop::default());
        std::fs::write(d.path().join("package.json"), "").unwrap();
        let table = stacks::toolchain_in(d.path()).expect("the table has Node.js");
        // Without the Windows name (npm.cmd), which would start the real npm instead of the stand-in.
        let node = stacks::Toolchain { test: vec!["no-such-program-here".into()], windows_program: None, ..table.clone() };
        for json in [r#"{"name":"x"}"#, r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#] {
            std::fs::write(d.path().join("package.json"), json).unwrap();
            let (passed, note) = own_tests_check(d.path(), Some(&node), &stop);
            assert!(passed && note.starts_with("No tests ran"), "nothing is started: {note}");
        }
        let real = r#"{"scripts":{"test":"jest && echo 'no test specified for integration'"}}"#;
        std::fs::write(d.path().join("package.json"), real).unwrap();
        let (passed, note) = own_tests_check(d.path(), Some(&node), &stop);
        assert!(!passed && note.starts_with("Couldn't run the tests"), "it's started: {note}");
    }

    /// A person's own Unity project is opened in batch mode, not built through our execute
    /// method, and its project path is native and absolute.
    #[test]
    fn a_persons_own_unity_project_is_opened_not_built_with_our_script() {
        let project = std::env::current_dir().unwrap().join("game");
        let expected = project.display().to_string();
        let args = own_unity_args(&project);
        assert!(args.iter().any(|a| a == "-batchmode") && args.iter().any(|a| a == &expected), "{args:?}");
        assert!(!args.iter().any(|a| a == "-executeMethod" || a.contains("CraftBuild")), "{args:?}");
    }

    #[cfg(unix)]
    #[test]
    fn a_persons_own_build_is_judged_by_its_toolchain() {
        let (d, stop) = (tempfile::tempdir().unwrap(), CheckStop::default());
        assert!(own_build(d.path(), &stand_in("exit 0", &[]), &stop).0);
        let (passed, note) = own_build(d.path(), &stand_in("echo 'error: no such file'; exit 2", &[]), &stop);
        assert!(!passed && note.contains("no such file"), "{note}");
    }

    /// A round with a time limit cuts a running build step at the limit, instead of waiting out
    /// the step's own, much longer, deadline. Without a limit the step keeps it.
    #[cfg(unix)]
    #[test]
    fn a_running_build_step_is_cut_at_the_rounds_time_limit() {
        let d = tempfile::tempdir().unwrap();
        let limited = CheckStop::limited(Instant::now() + Duration::from_millis(1_200));
        let args = ["-c", "sleep 30; exit 7"].map(String::from).to_vec();
        let began = Instant::now();
        let out = run_in(d.path(), Path::new("sh"), &args, 60, &limited).unwrap_err();
        assert!(began.elapsed() < Duration::from_secs(10), "the step ran past the round's limit: {} s", began.elapsed().as_secs());
        assert!(out.contains("stopped"), "the step's own program was stopped: {out}");
        assert!(limited.expired() && limited.cancelled() && limited.time_left().is_some_and(|l| l.is_zero()));
        // A stop without a limit only stops when the person pauses.
        let paused = CancelToken::default();
        paused.cancel();
        let stop = CheckStop::from(&paused);
        assert!(stop.cancelled() && stop.time_left().is_none(), "a pause alone cancels, a missing limit isn't one");
    }

    /// A step that would start after the round's limit, or while the build is paused, is refused
    /// before it starts: a zero deadline alone could let a quick exit count as a pass.
    #[cfg(unix)]
    #[test]
    fn a_step_is_refused_before_it_starts_past_the_limit_or_paused() {
        let d = tempfile::tempdir().unwrap();
        let marker = d.path().join("started");
        let args = ["-c", "touch started"].map(String::from).to_vec();
        let started = |stop: &CheckStop| run_in(d.path(), Path::new("sh"), &args, 60, stop);
        let e = started(&CheckStop::limited(Instant::now())).unwrap_err();
        assert!(e.contains("time limit"), "{e}");
        assert!(!marker.exists(), "the step ran after the round's limit");
        let paused = CancelToken::default();
        paused.cancel();
        let e = started(&CheckStop::from(&paused)).unwrap_err();
        assert!(e.contains("cancelled"), "{e}");
        assert!(!marker.exists(), "the step ran while paused");
    }

    #[test]
    fn a_persons_own_project_with_nothing_to_build_says_so() {
        let stop = CheckStop::default();
        let notes = tempfile::tempdir().unwrap();
        std::fs::write(notes.path().join("notes.md"), "ideas").unwrap();
        let (passed, note) = own_build_check(notes.path(), "the project folder", stacks::BUILDERS_CHOICE, None, &stop);
        assert!(passed && note.starts_with("Nothing was built"), "{note}");
        let site = tempfile::tempdir().unwrap();
        std::fs::write(site.path().join("index.html"), "<p>hi</p>").unwrap();
        let (passed, note) = own_build_check(site.path(), "the project folder", "Plain HTML and CSS", None, &stop);
        assert!(passed && note.contains("static site"), "{note}");
    }

    #[test]
    fn the_build_follows_what_was_built_not_the_stack_picked_first() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path();
        // nothing built yet: the engine's build says what's missing
        assert_eq!(engine_for("Unity", app), Some(game::Builder::Unity));
        // the person asked for something else and got it: npm or cargo builds it
        std::fs::write(app.join("package.json"), "{}").unwrap();
        assert_eq!(engine_for("Unity", app), None);
        assert_eq!(engine_for("Godot", app), None);
        assert_eq!(engine_for("Unreal Engine", app), None);
        // an engine's own project is built by the engine, whatever else is beside it
        std::fs::create_dir(app.join("Assets")).unwrap();
        assert_eq!(engine_for("Unity", app), Some(game::Builder::Unity));
        std::fs::write(app.join("project.godot"), "").unwrap();
        assert_eq!(engine_for("Godot", app), Some(game::Builder::Godot));
        std::fs::write(app.join("Game.uproject"), "{}").unwrap();
        assert_eq!(engine_for("Unreal Engine", app), Some(game::Builder::Unreal));
        // stacks without an engine of their own
        assert_eq!(engine_for("Electron", app), None);
        std::fs::write(app.join("Cargo.toml"), "").unwrap();
        assert_eq!(engine_for("Rust + macroquad", app), None);
    }

    #[test]
    fn a_project_one_folder_down_is_named() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path();
        let stop = CheckStop::default();
        let check = |place| build_check(app, place, "Rust + egui (eframe)", None, &stop);
        assert_eq!(check("app/"), (false, "No project to build found in app/ yet.".into()), "nothing anywhere");
        std::fs::create_dir(app.join("notes")).unwrap();
        assert_eq!(check("app/").1, "No project to build found in app/ yet.", "a folder without a project");
        // What `cargo new app` run inside app/ leaves.
        std::fs::create_dir_all(app.join("app/src")).unwrap();
        std::fs::write(app.join("app/Cargo.toml"), "[package]").unwrap();
        let (ok, note) = check("app/");
        assert!(!ok);
        assert_eq!(
            note,
            "No project to build found in app/ yet. There's a Cargo.toml one folder down, in app/app/, but the build runs in app/ itself: its files belong there."
        );
        // A person's own folder may hold projects below it: where, but no move.
        assert_eq!(
            check("the project folder").1,
            "No project to build found in the project folder yet. There's a Cargo.toml one folder down, in app/ in the project folder; the build runs in the project folder itself."
        );
        // The engines' own missing-project notes say it too.
        let unity = Path::new("C:/Engines/Unity/Editor/Unity.exe");
        let note = build_check(app, "app/", "Unity", Some(unity), &stop).1;
        assert!(note.starts_with("No Unity project found in app/ yet. There's a Cargo.toml one folder down, in app/app/"), "{note}");
    }

    /// A relative project must give an engine build absolute project paths: the child already
    /// runs with app/ as its working folder, so a relative `-projectPath` would point inside app/
    /// again.
    #[cfg(unix)]
    #[test]
    fn a_relative_project_gives_engine_builds_absolute_project_paths() {
        use std::os::unix::fs::PermissionsExt;

        let cwd = std::env::current_dir().unwrap();
        let d = tempfile::tempdir_in(&cwd).unwrap();
        let app = d.path().strip_prefix(&cwd).unwrap();
        std::fs::create_dir_all(app.join("Assets")).unwrap();
        std::fs::create_dir_all(app.join("Build")).unwrap();
        let game = app.join("Build/game");
        std::fs::write(&game, "#!/bin/sh\nexit 0\n").unwrap();
        std::fs::set_permissions(&game, std::fs::Permissions::from_mode(0o755)).unwrap();
        let editor = d.path().join("editor");
        std::fs::write(&editor, "#!/bin/sh\nprintf '%s\\n' \"$@\" > args.txt\nexit 0\n").unwrap();
        std::fs::set_permissions(&editor, std::fs::Permissions::from_mode(0o755)).unwrap();

        let stop = CheckStop::default();
        assert!(build_check(app, "app/", "Unity", Some(&editor), &stop).0);
        let args = std::fs::read_to_string(app.join("args.txt")).unwrap();
        let project = args.lines().skip_while(|l| *l != "-projectPath").nth(1).unwrap();
        assert!(Path::new(project).is_absolute(), "{project}");

        let own = own_unity_args(app);
        let project = own.iter().skip_while(|l| *l != "-projectPath").nth(1).unwrap();
        assert!(Path::new(project).is_absolute(), "{project}");

        std::fs::write(app.join("Game.uproject"), "{}").unwrap();
        assert!(uproject(app).is_some_and(|p| p.is_absolute()));
    }

    #[test]
    fn a_static_site_has_nothing_to_build() {
        let d = tempfile::tempdir().unwrap();
        let app = d.path();
        let stop = CheckStop::default();
        let passed = (true, "Nothing to build: a static site.".to_string());
        let check = |stack, editor| build_check(app, "app/", stack, editor, &stop);
        assert!(!check("Plain HTML and CSS", None).0, "nothing there yet");

        // #496: the page itself, with no manifest.
        std::fs::write(app.join("index.html"), "<!doctype html><h1>Hello</h1>").unwrap();
        assert_eq!(check("Plain HTML and CSS", None), passed);
        // What was built decides: a page made under a game engine's stack isn't a missing project.
        let unity = Path::new("C:/Engines/Unity/Editor/Unity.exe");
        assert_eq!(check("Unity", Some(unity)), passed);

        // A site's own assets folder doesn't make it a Unity project.
        std::fs::create_dir(app.join("Assets")).unwrap();
        assert_eq!(check("Plain HTML and CSS", None), passed);
        assert_eq!(check("Unity", Some(unity)), passed);

        // A real engine project doesn't pass for the page beside it, with or without the editor,
        // and whichever engine the stack named.
        std::fs::create_dir(app.join("ProjectSettings")).unwrap();
        assert!(!check("Unity", None).0);
        assert!(!check("Plain HTML and CSS", None).0);
        std::fs::remove_dir(app.join("ProjectSettings")).unwrap();
        for project in ["project.godot", "Game.uproject"] {
            std::fs::write(app.join(project), "").unwrap();
            assert!(!check("Unity", None).0, "{project}");
            std::fs::remove_file(app.join(project)).unwrap();
        }

        // A blank page isn't a site, and doesn't pass for one under an engine's stack either.
        std::fs::write(app.join("index.html"), " \n").unwrap();
        assert!(!check("Plain HTML and CSS", None).0);
        std::fs::remove_dir(app.join("Assets")).unwrap();
        assert_eq!(check("Unity", Some(unity)), (false, "No Unity project found in app/ yet.".into()));

        // Already-built pages count too, as they do for the preview.
        let built = tempfile::tempdir().unwrap();
        std::fs::create_dir(built.path().join("dist")).unwrap();
        std::fs::write(built.path().join("dist").join("index.html"), "<h1>Hello</h1>").unwrap();
        assert_eq!(build_check(built.path(), "app/", "Builder's choice", None, &stop), passed);
    }
}
