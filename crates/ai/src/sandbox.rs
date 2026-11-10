//! The project's sandbox: one E2B box per project that a coding agent builds in, with all its own
//! tools, instead of this computer. The box is created the first time a project builds, paused
//! (snapshotted) when it isn't needed — on idle timeout E2B does it on its own — and brought back
//! for the next turn. What the agent builds is copied back after every turn, so checks, previews
//! and the person's own edits all stay on this computer as usual.
//!
//! Where a project's sandbox is (its id) is kept in `sandbox.e2b.json` beside the project's
//! manifest. The API key is the person's own: Settings → AI → Sandbox, or `E2B_API_KEY`.

use crate::keys;
use craft_anything_engines::e2b::{self, E2B, Envd, SandboxInfo, SandboxState, SyncSummary};
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::sync::Mutex;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

/// Where the sandbox's copy of our CLI tool server lives (uploaded at setup).
pub const REMOTE_CLI: &str = "/craft-anything/bin/craft-anything-cli";
/// Marker of a completed setup, with the setup's version in it.
const SETUP_MARKER: &str = "/craft-anything/.craft-setup-v1";
/// The sandbox's Node install, when the template's Node is too old for the agent.
const REMOTE_NODE_HOME: &str = "/craft-anything/node";

/// Everything the sandbox needs from `config.toml` (`[ai.sandbox]`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SandboxConfig {
    /// Build in a cloud sandbox instead of this computer's folders.
    pub enabled: bool,
    /// Which sandbox service: `e2b` (the one there is today).
    pub provider: String,
    /// The E2B template sandboxes start from (Debian + Node + Python + git).
    pub template: String,
    /// E2B's sandbox domain (`e2b.app`; only different for self-hosted gateways).
    pub domain: String,
    /// How long an unused sandbox stays up before E2B pauses it.
    pub timeout_secs: u64,
    /// Pause (snapshot) the sandbox instead of killing it at the timeout, and bring it back on
    /// the next request. Off, the sandbox is killed and its disk is lost.
    pub auto_pause: bool,
    /// Pause the sandbox right after every builder turn (cheapest between turns).
    pub pause_after_turn: bool,
    /// Copy this machine's Z.ai credentials (`.z-ai-config`) into the sandbox at setup, so the
    /// bundled ZCode agent's built-in provider works there. They go to the sandbox's
    /// `/etc/.z-ai-config` and nowhere else; nothing else carries them.
    pub provide_zai_credentials: bool,
    /// Extra names that never travel (besides the built-in list: `.git`, `node_modules`, …).
    pub ignore: Vec<String>,
}

impl Default for SandboxConfig {
    fn default() -> Self {
        SandboxConfig {
            enabled: false,
            provider: "e2b".into(),
            template: e2b::DEFAULT_TEMPLATE.into(),
            domain: "e2b.app".into(),
            timeout_secs: e2b::DEFAULT_TIMEOUT_SECS,
            auto_pause: true,
            pause_after_turn: true,
            provide_zai_credentials: false,
            ignore: Vec::new(),
        }
    }
}

impl SandboxConfig {
    /// Whether this is a sandbox we know how to drive.
    pub fn supported(&self) -> bool {
        self.provider == "e2b"
    }

    /// Whether nothing was changed from the defaults (so `[ai.sandbox]` isn't written out).
    pub fn is_default(&self) -> bool {
        *self == SandboxConfig::default()
    }
}

/// Where a project's sandbox is recorded, beside its manifest.
fn sidecar(project_root: &Path) -> PathBuf {
    project_root.join("sandbox.e2b.json")
}

#[derive(Debug, Serialize, Deserialize)]
struct Sidecar {
    version: u32,
    sandbox_id: String,
    template: String,
    saved_at: u64,
}

fn save_sidecar(project_root: &Path, sandbox_id: &str, template: &str) -> Result<(), ToolError> {
    let saved_at = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let record = Sidecar { version: 1, sandbox_id: sandbox_id.to_string(), template: template.to_string(), saved_at };
    let text = serde_json::to_string_pretty(&record).unwrap_or_else(|_| "{}".into());
    craft_anything_platform::write_atomic_private(&sidecar(project_root), text.as_bytes())
}

/// The project's sandbox: the client, its id, and what the settings say.
pub struct Sandbox {
    pub client: E2B,
    pub envd: Mutex<Option<Envd>>,
    pub info: SandboxInfo,
    pub config: SandboxConfig,
    /// The project's root inside the sandbox.
    pub remote_root: String,
}

impl std::fmt::Debug for Sandbox {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sandbox").field("sandbox_id", &self.info.sandbox_id).field("template", &self.config.template).finish()
    }
}

impl Sandbox {
    /// The sandbox for a project: bring back the one it had, or start a new one and write down
    /// its id. `status` gets a line for the app's status while it works.
    pub fn open(config: SandboxConfig, key: &str, project_root: &Path, mut status: impl FnMut(String)) -> Result<Arc<Sandbox>, ToolError> {
        if !config.supported() {
            return Err(ToolError::new(ErrorCode::BadSettings, format!("Unknown sandbox provider {}.", config.provider))
                .with_remediation("Set the sandbox provider to \"e2b\" in config.toml."));
        }
        let client = E2B::new(key, &config.domain);
        let saved = std::fs::read_to_string(sidecar(project_root)).ok().and_then(|t| serde_json::from_str::<Sidecar>(&t).ok());
        let info = match saved {
            Some(record) if record.template == config.template => {
                status("Bringing the project's sandbox back…".into());
                match client.connect(&record.sandbox_id, config.timeout_secs) {
                    Ok(info) => info,
                    // A sandbox that is gone for good (killed, expired past its snapshot) starts over.
                    Err(e) => {
                        status(format!("The old sandbox couldn't be brought back ({}), starting a new one…", first_line(&e.message)));
                        client.create(&config.template, config.timeout_secs, config.auto_pause, true)?
                    }
                }
            }
            _ => {
                status("Starting a sandbox for this project…".into());
                client.create(&config.template, config.timeout_secs, config.auto_pause, true)?
            }
        };
        save_sidecar(project_root, &info.sandbox_id, &config.template)?;
        Ok(Arc::new(Sandbox {
            client,
            envd: Mutex::new(Some(Envd::new(info.clone(), &config.domain))),
            info,
            config,
            remote_root: e2b::REMOTE_ROOT.to_string(),
        }))
    }

    /// Whether the sandbox answers (used after a resume, so a build starts against a live box).
    pub fn healthy(&self) -> bool {
        self.envd.lock().unwrap_or_else(std::sync::PoisonError::into_inner).as_ref().is_some_and(|e| e.health().unwrap_or(false))
    }

    /// The id, for status lines.
    pub fn id(&self) -> &str {
        &self.info.sandbox_id
    }

    /// Copy the project into the sandbox, then make sure everything the agent needs is installed
    /// there (our CLI tool server; Node 22 and the ZCode agent when that's the builder).
    pub fn sync_up(&self, project_root: &Path, status: &mut dyn FnMut(String)) -> Result<SyncSummary, ToolError> {
        let envd = self.envd()?;
        let summary = e2b::sync_up(&envd, project_root, &self.remote_root, &self.config.ignore, &mut *status)?;
        self.setup(&envd, status)?;
        Ok(summary)
    }

    /// Copy what the agent built back to this computer.
    pub fn sync_down(&self, project_root: &Path, status: &mut dyn FnMut(String)) -> Result<SyncSummary, ToolError> {
        let envd = self.envd()?;
        e2b::sync_down(&envd, project_root, &self.remote_root, &self.config.ignore, &mut *status)
    }

    /// Run a short command in the sandbox and collect its output.
    pub fn run_command(&self, command: &str, timeout: Duration) -> Result<String, ToolError> {
        e2b::run_command(&self.envd()?, command, timeout)
    }

    /// Pause (snapshot) the sandbox: nothing runs, the disk is kept, the next turn brings it back.
    /// A sandbox that's already paused (or gone) is left as it is.
    pub fn pause(&self) {
        let _ = self.client.pause(&self.info.sandbox_id);
    }

    /// Stop the sandbox for good and forget it (its disk is deleted). Used when a project's
    /// sandbox is retired on purpose.
    pub fn kill(&self, project_root: &Path) -> Result<(), ToolError> {
        let killed = self.client.kill(&self.info.sandbox_id)?;
        if killed {
            let _ = std::fs::remove_file(sidecar(project_root));
        }
        Ok(())
    }

    /// Where the sandbox is, for the settings panel and status lines.
    pub fn state(&self) -> SandboxState {
        self.client.state(&self.info.sandbox_id).unwrap_or(SandboxState::Gone)
    }

    /// The sandbox's own API client, while the connection is up.
    pub fn envd(&self) -> Result<Envd, ToolError> {
        self.envd.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().ok_or_else(|| {
            ToolError::new(ErrorCode::EngineMissing, "The sandbox is no longer connected.")
                .with_remediation("Start the build again: the sandbox is brought back on its own.")
        })
    }

    /// One-time setup inside the sandbox: our tool server (and the agent's own runtime, when the
    /// builder is one we carry). Cheap when it's already done: one `test` command.
    fn setup(&self, envd: &Envd, status: &mut dyn FnMut(String)) -> Result<(), ToolError> {
        if e2b::run_command(envd, &format!("test -f {SETUP_MARKER}"), Duration::from_secs(60)).is_ok() {
            return Ok(());
        }
        status("Setting the sandbox up for Craft Anything…".into());
        // Our own CLI (the agent's tool server) for Linux.
        if let Some(cli) = sandbox_cli_path() {
            let bytes = std::fs::read(&cli).map_err(|e| ToolError::io(cli.display(), e))?;
            e2b::run_command(envd, "mkdir -p /craft-anything/bin", Duration::from_secs(60))?;
            envd.write_file(&format!("{REMOTE_CLI}.upload"), &bytes)?;
            e2b::run_command(envd, &format!("mv {REMOTE_CLI}.upload {REMOTE_CLI} && chmod +x {REMOTE_CLI}"), Duration::from_secs(60))?;
        }
        // Node 22 for the ZCode agent, when the template's Node is older than it needs.
        e2b::run_command(
            envd,
            &format!(
                "if ! command -v node >/dev/null 2>&1 || [ \"$(node -p 'parseInt(process.versions.node)' 2>/dev/null || echo 0)\" -lt 22 ]; then \
                  mkdir -p {REMOTE_NODE_HOME} && cd {REMOTE_NODE_HOME} && \
                  curl -fsSL https://nodejs.org/dist/v22.20.0/node-v22.20.0-linux-x64.tar.xz -o node.tar.xz && \
                  tar -xJf node.tar.xz --strip-components=1 && rm node.tar.xz && \
                  ln -sf {REMOTE_NODE_HOME}/bin/node /usr/local/bin/node && ln -sf {REMOTE_NODE_HOME}/bin/npm /usr/local/bin/npm && ln -sf {REMOTE_NODE_HOME}/bin/npx /usr/local/bin/npx; \
                fi",
            ),
            Duration::from_secs(600),
        )?;
        // The ZCode agent's own files, when they're bundled with this app.
        for (src, dest) in zcode_uploads() {
            if let Ok(bytes) = std::fs::read(&src) {
                let dir = parent_of(dest);
                e2b::run_command(envd, &format!("mkdir -p {dir}"), Duration::from_secs(60))?;
                envd.write_file(dest, &bytes)?;
            }
        }
        if self.config.provide_zai_credentials
            && let Some(config) = z_ai_config_bytes()
        {
            e2b::run_command(envd, "mkdir -p /etc", Duration::from_secs(60))?;
            envd.write_file("/etc/.z-ai-config", &config)?;
        }
        if crate::cli_agents::zcode::sandbox_env_ready() {
            // The agent's search binaries and its runtime folder.
            e2b::run_command(envd, "mkdir -p /craft-anything/zcode/home /craft-anything/agent/config", Duration::from_secs(60))?;
            let config = crate::cli_agents::zcode::sandbox_provider_config()?;
            envd.write_file("/craft-anything/agent/provider_config.json", config.as_bytes())?;
            if let Some(builtin) = crate::cli_agents::zcode::builtin_config_bytes() {
                envd.write_file("/craft-anything/agent/config/zcode-builtin.json", &builtin)?;
            }
        }
        e2b::run_command(envd, &format!("touch {SETUP_MARKER}"), Duration::from_secs(60))?;
        Ok(())
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

fn parent_of(path: &str) -> String {
    match path.rsplit_once('/') {
        Some((dir, _)) if !dir.is_empty() => dir.to_string(),
        _ => "/".into(),
    }
}

/// This machine's Z.ai credentials, for the sandbox's built-in provider, when the person said
/// they may travel.
fn z_ai_config_bytes() -> Option<Vec<u8>> {
    let home = dirs::home_dir().map(|h| h.join(".z-ai-config"));
    [Some(PathBuf::from("/etc/.z-ai-config")), home].into_iter().flatten().find_map(|p| std::fs::read(p).ok())
}

/// The Linux `craft-anything-cli` that ships with the app for sandbox runs, when it's there.
fn sandbox_cli_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("resources").join("sandbox").join("craft-anything-cli-linux"), dir.join("sandbox").join("craft-anything-cli-linux")]
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// The ZCode agent's bundled files that travel to the sandbox: `(local path, sandbox path)`.
fn zcode_uploads() -> Vec<(PathBuf, &'static str)> {
    crate::cli_agents::zcode::assets()
        .map(|dir| {
            vec![
                (dir.join("zcode.cjs"), "/craft-anything/agent/zcode.cjs"),
                (dir.join("resources").join("rg"), "/craft-anything/bin/rg"),
                (dir.join("resources").join("ugrep"), "/craft-anything/bin/ugrep"),
                (dir.join("resources").join("bfs"), "/craft-anything/bin/bfs"),
            ]
        })
        .unwrap_or_default()
}

/// The sandbox's readiness for the ZCode agent, in plain words: its files are bundled and Node 22
/// lands in the sandbox at setup.
pub fn describe(config: &SandboxConfig) -> String {
    if !config.enabled {
        return "Off".into();
    }
    format!("E2B, template {}, pause after {}s", config.template, config.timeout_secs)
}

/// Resolve the sandbox API key for a config: the stored one, else `E2B_API_KEY`.
pub fn resolve_key(store: &keys::KeyStore) -> Option<String> {
    store
        .get(keys::SANDBOX_KEY)
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
        .or_else(|| std::env::var("E2B_API_KEY").ok().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings the app writes round-trip, and the defaults are what a new user wants.
    #[test]
    fn config_defaults_and_round_trip() {
        let c = SandboxConfig::default();
        assert!(!c.enabled);
        assert_eq!(c.provider, "e2b");
        assert_eq!(c.template, "base");
        assert!(c.auto_pause && c.pause_after_turn);
        assert!(c.supported());
        let text = toml::to_string(&c).unwrap();
        let back: SandboxConfig = toml::from_str(&text).unwrap();
        assert_eq!(back, c);
    }

    #[test]
    fn an_unknown_provider_is_refused() {
        let c = SandboxConfig { provider: "acme".into(), ..Default::default() };
        assert!(!c.supported());
    }

    #[test]
    fn the_sidecar_records_the_sandbox_id() {
        let dir = tempfile::tempdir().unwrap();
        save_sidecar(dir.path(), "sbx-1", "base").unwrap();
        let record: Sidecar = serde_json::from_str(&std::fs::read_to_string(sidecar(dir.path())).unwrap()).unwrap();
        assert_eq!(record.sandbox_id, "sbx-1");
        assert_eq!(record.version, 1);
    }

    #[test]
    fn describe_says_what_is_on() {
        assert_eq!(describe(&SandboxConfig::default()), "Off");
        assert!(describe(&SandboxConfig { enabled: true, ..Default::default() }).contains("E2B"));
    }
}
