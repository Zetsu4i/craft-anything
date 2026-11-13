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
/// Where the sandbox's ZCode agent lives (the agent bundle unpacks here).
const REMOTE_AGENT: &str = "/craft-anything/agent";
/// The app-side Linux CLI a release can carry for the sandbox, when one isn't bundled beside
/// this executable and the sandbox has to fetch its own.
pub const DEFAULT_CLI_URL: &str = "https://github.com/Zetsu4i/craft-anything/releases/latest/download/craft-anything-cli-linux";

/// What a ZCode builder needs from outside the sandbox, decided by its caller: the backend's
/// saved Z.ai key, when there is one. It becomes the sandbox's direct route; without it the
/// sandbox's bridge needs Z.ai credentials to have travelled.
#[derive(Debug, Clone, Default)]
pub struct ZcodeSetup {
    pub api_key: Option<String>,
}

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
    /// ZCode agent's free built-in provider works there. They go to the sandbox's
    /// `/etc/.z-ai-config` and nowhere else; nothing else carries them.
    pub provide_zai_credentials: bool,
    /// Where the sandbox fetches the ZCode agent bundle from, when its template doesn't carry
    /// the agent. Empty uses the agent repository's own release; a private repository wants the
    /// token saved with it (`sandbox:zcode-token`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zcode_agent_url: Option<String>,
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
            zcode_agent_url: None,
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
    /// The ZCode model the sandbox's provider config serves, once setup put the agent there.
    /// `None` when no ZCode run needs it (yet).
    pub zcode_model: Mutex<Option<String>>,
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
            zcode_model: Mutex::new(None),
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
    /// there (our CLI tool server; the ZCode agent and its route, when a ZCode builder runs).
    pub fn sync_up(&self, project_root: &Path, status: &mut dyn FnMut(String), zcode: Option<&ZcodeSetup>) -> Result<SyncSummary, ToolError> {
        let envd = self.envd()?;
        let summary = e2b::sync_up(&envd, project_root, &self.remote_root, &self.config.ignore, &mut *status)?;
        self.setup(&envd, status, zcode)?;
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

    /// One-time setup inside the sandbox: our tool server, then everything the ZCode agent needs
    /// when a ZCode builder runs here. Cheap when it's already done: one `test` command. Work
    /// that only the sandbox can do — fetching the CLI or the agent — happens inside the sandbox,
    /// so nothing lands on this computer that wasn't already on it.
    fn setup(&self, envd: &Envd, status: &mut dyn FnMut(String), zcode: Option<&ZcodeSetup>) -> Result<(), ToolError> {
        let fresh = e2b::run_command(envd, &format!("test -f {SETUP_MARKER}"), Duration::from_secs(60)).is_ok();
        if !fresh {
            status("Setting the sandbox up for Craft Anything…".into());
            self.ensure_tool_server(envd, status)?;
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
        }
        if let Some(zc) = zcode {
            self.setup_zcode(envd, status, zc)?;
        }
        if !fresh {
            e2b::run_command(envd, &format!("touch {SETUP_MARKER}"), Duration::from_secs(60))?;
        }
        Ok(())
    }

    /// The sandbox's tool server: the app's own Linux CLI. Uploaded from beside this executable
    /// when the package carries it, otherwise fetched by the sandbox from the app's release. A
    /// template that ships it is left as it is.
    fn ensure_tool_server(&self, envd: &Envd, status: &mut dyn FnMut(String)) -> Result<(), ToolError> {
        if e2b::run_command(envd, &format!("test -x {REMOTE_CLI}"), Duration::from_secs(60)).is_ok() {
            return Ok(());
        }
        if let Some(cli) = sandbox_cli_path() {
            status("Uploading the tool server…".into());
            let bytes = std::fs::read(&cli).map_err(|e| ToolError::io(cli.display(), e))?;
            e2b::run_command(envd, &format!("mkdir -p {}", parent_of(REMOTE_CLI)), Duration::from_secs(60))?;
            envd.write_file(&format!("{REMOTE_CLI}.upload"), &bytes)?;
            e2b::run_command(envd, &format!("mv {REMOTE_CLI}.upload {REMOTE_CLI} && chmod +x {REMOTE_CLI}"), Duration::from_secs(60))?;
            return Ok(());
        }
        status("Fetching the tool server into the sandbox…".into());
        let url = std::env::var("CRAFT_ANYTHING_SANDBOX_CLI_URL").ok().filter(|u| !u.trim().is_empty()).unwrap_or_else(|| DEFAULT_CLI_URL.to_string());
        let cmd = format!(
            "mkdir -p {} && curl -fsSL '{url}' -o {REMOTE_CLI}.upload && mv {REMOTE_CLI}.upload {REMOTE_CLI} && chmod +x {REMOTE_CLI}",
            parent_of(REMOTE_CLI),
        );
        if e2b::run_command(envd, &cmd, Duration::from_secs(600)).is_err() {
            return Err(ToolError::new(
                ErrorCode::EngineMissing,
                "The sandbox has no tool server: none was bundled with this app and the sandbox couldn't fetch one.",
            )
            .with_remediation(format!(
                "Check the sandbox's internet access, or ship `resources/sandbox/craft-anything-cli-linux` beside the app (looked at {url})."
            )));
        }
        Ok(())
    }

    /// The ZCode agent inside the sandbox: its files (a template's copy, the app's own, or a
    /// bundle fetched by the sandbox itself), the provider route, and the bridge behind the free
    /// built-in provider when Z.ai credentials serve it.
    fn setup_zcode(&self, envd: &Envd, status: &mut dyn FnMut(String), zc: &ZcodeSetup) -> Result<(), ToolError> {
        let have_agent =
            e2b::run_command(envd, &format!("test -f {REMOTE_AGENT}/zcode.cjs -a -f {REMOTE_AGENT}/config/zcode-builtin.json"), Duration::from_secs(60))
                .is_ok();
        if !have_agent {
            let mut uploaded = false;
            if let Some(dir) = crate::cli_agents::zcode::assets() {
                status("Uploading the ZCode agent…".into());
                for (src, dest) in [
                    (dir.join("zcode.cjs"), format!("{REMOTE_AGENT}/zcode.cjs")),
                    (dir.join("config").join("zcode-builtin.json"), format!("{REMOTE_AGENT}/config/zcode-builtin.json")),
                    (dir.join("zai-bridge.cjs"), format!("{REMOTE_AGENT}/zai-bridge.cjs")),
                ] {
                    let Ok(bytes) = std::fs::read(&src) else { continue };
                    e2b::run_command(envd, &format!("mkdir -p {}", parent_of(&dest)), Duration::from_secs(60))?;
                    envd.write_file(&format!("{dest}.upload"), &bytes)?;
                    e2b::run_command(envd, &format!("mv {dest}.upload {dest}"), Duration::from_secs(60))?;
                    uploaded = true;
                }
                // The agent's search tools from its own folder, when they are there.
                for name in ["rg", "ugrep"] {
                    let dest = format!("{}/bin/{name}", self.remote_root);
                    if let Ok(bytes) = std::fs::read(dir.join("resources").join(name)) {
                        e2b::run_command(envd, &format!("mkdir -p {}/bin", self.remote_root), Duration::from_secs(60))?;
                        let _ = envd.write_file(&dest, &bytes);
                        let _ = e2b::run_command(envd, &format!("chmod +x {dest}"), Duration::from_secs(60));
                    }
                }
            }
            if !uploaded {
                self.fetch_agent(envd, status)?;
                // The agent finds its search tools by name; a template without them gets the
                // common ones. Best effort: the agent runs without them, the build doesn't stop.
                let _ = e2b::run_command(
                    envd,
                    "command -v rg >/dev/null 2>&1 || (apt-get update -qq && apt-get install -y -qq ripgrep) || true",
                    Duration::from_secs(600),
                );
            }
        }
        // The route the agent's provider takes. Credentials that travelled serve the free bridge
        // route; otherwise the saved key goes straight to Z.ai; without either there is nothing
        // to talk to.
        if self.config.provide_zai_credentials
            && let Some(config) = z_ai_config_bytes()
        {
            e2b::run_command(envd, "mkdir -p /etc", Duration::from_secs(60))?;
            envd.write_file("/etc/.z-ai-config", &config)?;
        }
        let route = if e2b::run_command(envd, "test -f /etc/.z-ai-config", Duration::from_secs(60)).is_ok() {
            crate::cli_agents::zcode::Route::Bridge
        } else if let Some(key) = zc.api_key.clone().map(|k| k.trim().to_string()).filter(|k| !k.is_empty()) {
            crate::cli_agents::zcode::Route::Direct(key)
        } else {
            return Err(ToolError::new(ErrorCode::BadSettings, "The ZCode agent in the sandbox has no way to reach its GLM model.")
                .with_remediation("Save a Z.ai API key for ZCode in Settings → AI, or allow the Z.ai credentials to travel in Settings → AI → Sandbox (\"Z.ai credentials in the sandbox\")."));
        };
        e2b::run_command(envd, &format!("mkdir -p {REMOTE_AGENT}/config {}/zcode/home", self.remote_root), Duration::from_secs(60))?;
        envd.write_file(&format!("{REMOTE_AGENT}/provider_config.json"), crate::cli_agents::zcode::provider_config_for(&route).as_bytes())?;
        *self.zcode_model.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(route.model().to_string());
        // The bridge behind the free route: started once, kept by the sandbox, answered before
        // the first turn asks for a model.
        if route == crate::cli_agents::zcode::Route::Bridge {
            let healthy = e2b::run_command(envd, "curl -fsS http://127.0.0.1:3031/health >/dev/null 2>&1", Duration::from_secs(60)).is_ok();
            if !healthy {
                status("Starting the sandbox's GLM bridge…".into());
                let cmd = format!(
                    "cd {REMOTE_AGENT} && (nohup node zai-bridge.cjs >> {REMOTE_AGENT}/bridge.log 2>&1 &) && \
                     for i in $(seq 1 40); do curl -fsS http://127.0.0.1:3031/health >/dev/null 2>&1 && exit 0; sleep 1; done; \
                     echo 'the GLM bridge did not come up'; tail -5 {REMOTE_AGENT}/bridge.log 2>/dev/null; exit 1",
                );
                if let Err(e) = e2b::run_command(envd, &cmd, Duration::from_secs(120)) {
                    return Err(ToolError::new(
                        ErrorCode::EngineMissing,
                        "The sandbox's GLM bridge didn't start, so the free built-in provider can't serve the model.",
                    )
                    .with_remediation(format!("Its log is {REMOTE_AGENT}/bridge.log in the sandbox. {}", first_line(&e.message))));
                }
            }
        }
        Ok(())
    }

    /// Fetch the agent bundle into the sandbox: the URL the settings name, else the agent
    /// repository's release listing, with the saved token when there is one. The bundle never
    /// touches this computer.
    fn fetch_agent(&self, envd: &Envd, status: &mut dyn FnMut(String)) -> Result<(), ToolError> {
        status("Fetching the ZCode agent into the sandbox…".into());
        let url = self
            .config
            .zcode_agent_url
            .clone()
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| crate::cli_agents::zcode::DEFAULT_AGENT_URL.to_string());
        let token = crate::keys::agent_token(&crate::keys::KeyStore::system());
        let auth = token.map(|t| format!(" -H \"Authorization: Bearer {t}\"")).unwrap_or_default();
        // A release listing on the API (the default) is resolved here to its bundle asset; the
        // sandbox then downloads the asset itself. Anything already pointing at a file is used
        // as it is. octet-stream is what makes the API's asset endpoint hand over bytes.
        let (fetch_url, octet) = if url.contains("api.github.com") && !url.contains("/releases/assets/") {
            let listing = e2b::run_command(envd, &format!("curl -fsSL{auth} '{url}'"), Duration::from_secs(120)).map_err(|e| {
                ToolError::new(ErrorCode::EngineMissing, "The agent repository's release couldn't be reached.").with_remediation(format!(
                    "If the repository is private, save its access token in Settings → AI → Sandbox (ZCode repo token). The listing URL was {url} ({})",
                    first_line(&e.message)
                ))
            })?;
            let id = listing
                .rsplit_once(&format!("\"name\": \"{}\"", crate::cli_agents::zcode::AGENT_BUNDLE_ASSET))
                .and_then(|(before, _)| before.rsplit_once("\"id\": "))
                .and_then(|(_, after)| Some(after.trim_start().chars().take_while(|c| c.is_ascii_digit()).collect::<String>()))
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    ToolError::new(ErrorCode::EngineMissing, format!("The release at {url} holds no {} asset.", crate::cli_agents::zcode::AGENT_BUNDLE_ASSET))
                        .with_remediation(
                            "Upload the bundle to the release (or point Settings → AI → Sandbox → ZCode agent URL at a direct file URL), then try again.",
                        )
                })?;
            // The asset lives under the listing's own repository, whatever the settings named.
            let repo = url.split("/repos/").nth(1).and_then(|r| r.split("/releases").next()).unwrap_or(crate::cli_agents::zcode::DEFAULT_AGENT_REPO);
            (format!("https://api.github.com/repos/{repo}/releases/assets/{id}"), true)
        } else {
            (url.clone(), url.contains("api.github.com"))
        };
        let accept = if octet { " -H \"Accept: application/octet-stream\"" } else { "" };
        let cmd = format!(
            "mkdir -p {REMOTE_AGENT}/config && curl -fsSL{auth}{accept} '{fetch_url}' -o /tmp/zcode-agent.tgz && \
             tar -xzf /tmp/zcode-agent.tgz -C {REMOTE_AGENT} --strip-components=1 && rm /tmp/zcode-agent.tgz && \
             test -f {REMOTE_AGENT}/zcode.cjs",
        );
        if e2b::run_command(envd, &cmd, Duration::from_secs(600)).is_err() {
            return Err(ToolError::new(ErrorCode::EngineMissing, "The sandbox couldn't fetch the ZCode agent.").with_remediation(format!(
                "If the agent's repository is private, save its access token in Settings → AI → Sandbox (ZCode repo token), and check the URL: {url}"
            )));
        }
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

/// The sandbox's readiness for the ZCode agent, in plain words.
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
