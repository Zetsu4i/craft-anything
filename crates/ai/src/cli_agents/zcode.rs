//! The ZCode agent (`zcode.cjs`, the person's own coding agent): run in the background as its
//! app-server, spoken to over its NDJSON protocol — one JSON message per line on stdio — instead
//! of driving a per-message CLI run the way Claude Code and Codex are driven. One server process
//! serves a whole conversation; its own tools (Bash, Read, Write, Edit…) work on the project's
//! folder wherever the server runs, here or in the project's sandbox.
//!
//! The agent is not shipped with the app: on this computer it is whatever copy the person points
//! Settings → AI at (or finds through `CRAFT_ANYTHING_ZCODE_DIR`), and in the project's sandbox it
//! is fetched there at setup — from the sandbox template that carries it, or straight from the
//! agent's release. Its "GLM (built-in)" provider is either the bridge (`zai-bridge.cjs`, next to
//! the agent, fed by Z.ai credentials) or a Z.ai API key saved for this backend in Settings → AI.

use super::acp;
use super::adapters::{AgentAdapter, ArgsCx};
use crate::cli_agents::CliSpec;
use crate::config::BackendOptions;
use crate::failure::FailureKind;
use crate::keys::KeyStore;
use crate::model::Stop;
use crate::{AgentEvent, BackendConfig, Conversation, TurnFailure};
use craft_anything_engines::e2b::{Envd, ProcEvent, RemoteProcess};
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::process::Process;
use craft_anything_platform::{RunningProcess, StdinMode};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

/// The agent id this adapter is registered under.
pub const AGENT_ID: &str = "zcode";
/// How it's shown in the app.
pub const DISPLAY: &str = "ZCode (GLM built-in)";
/// Where the local bridge listens (the same port the agent's docs use).
pub const BRIDGE_PORT: u16 = 3031;
/// The provider and model the bridge serves.
pub const BRIDGE_PROVIDER: &str = "zai-bridge";
pub const BRIDGE_MODEL: &str = "zai-glm";
/// The direct Z.ai endpoint, when a key is saved instead, and the model it serves.
pub const ZAI_BASE_URL: &str = "https://api.z.ai/api/paas/v4";
pub const ZAI_MODEL: &str = "glm-4.6";
/// Where a sandbox fetches the agent bundle from, when its template doesn't carry it and this app
/// doesn't hold a copy: the agent repository's release listing. The app resolves it to the
/// bundle's asset and the sandbox downloads that. A private repository needs a token saved in
/// Settings → AI → Sandbox (`sandbox:zcode-token`): GitHub's download pages answer 404 to a bare
/// token there, while the API's asset endpoint (octet-stream) serves the file.
pub const DEFAULT_AGENT_URL: &str = "https://api.github.com/repos/Zetsu4i/zcode-agent-core/releases/latest";
/// The asset looked for in the release listing.
pub const AGENT_BUNDLE_ASSET: &str = "zcode-agent-bundle.tar.gz";
/// The repository the default listing points at, when a custom URL doesn't name one.
pub const DEFAULT_AGENT_REPO: &str = "Zetsu4i/zcode-agent-core";

/// The runtime preferences the app answers `session/requestRuntimePreferences` with.
fn runtime_preferences() -> Value {
    json!({
        "nativeSearchEnhancementsEnabled": true,
        "memoryEnabled": false,
        "askUserQuestionAutoResolutionEnabled": true,
        "modelContextBudgetStrategy": "preflight-v1",
    })
}

/// Where the agent's provider config and state live for this app (never an installed ZCode's).
pub fn app_data_dir() -> PathBuf {
    craft_anything_engine::settings::config_dir().join("zcode")
}

/// The ZCode agent folder in the settings file (`[ai] zcode_dir = "…"`), when it was set.
pub(crate) fn settings_zcode_dir() -> Option<String> {
    let table = craft_anything_engine::Settings::load().ai;
    table.get("zcode_dir").and_then(|v| v.as_str()).map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// Where a copy of the agent lives on this computer, in the order the person's words win:
/// the folder Settings → AI names, then `CRAFT_ANYTHING_ZCODE_DIR`, then the app-side folder a
/// development build or a hand-assembled package carries. `None` means there is no local copy —
/// the agent then runs only in the project's sandbox.
pub fn local_agent_dir(explicit: Option<&str>) -> Option<PathBuf> {
    let mut candidates = explicit
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .into_iter()
        .chain(std::env::var("CRAFT_ANYTHING_ZCODE_DIR").ok().map(PathBuf::from).into_iter().chain(assets().into_iter()));
    candidates.find(|dir| dir.join("zcode.cjs").is_file())
}

/// The folder the app itself carries, when a build bundled the agent (development runs).
pub fn assets() -> Option<PathBuf> {
    if let Ok(env_dir) = std::env::var("CRAFT_ANYTHING_ZCODE_DIR")
        && Path::new(&env_dir).join("zcode.cjs").is_file()
    {
        return Some(PathBuf::from(env_dir));
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    [dir.join("resources").join("zcode"), dir.join("zcode")].into_iter().find(|candidate| candidate.join("zcode.cjs").is_file())
}

/// The agent's builtin provider catalogue, beside the local copy of the agent when there is one.
pub fn builtin_config_bytes(dir: Option<&Path>) -> Option<Vec<u8>> {
    let dir = dir.map(PathBuf::from).or_else(assets)?;
    std::fs::read(dir.join("config").join("zcode-builtin.json")).ok()
}

/// A Node runtime: the one beside the local copy of the agent, else `node` on this computer.
/// `command` overrides both.
fn node_program(command: Option<&str>) -> String {
    if let Some(cmd) = command.map(str::trim).filter(|c| !c.is_empty()) {
        return cmd.to_string();
    }
    if let Some(dir) = local_agent_dir(settings_zcode_dir().as_deref()) {
        for name in ["node.exe", "node"] {
            let bundled = dir.join("node").join(name);
            if bundled.is_file() {
                return bundled.display().to_string();
            }
        }
    }
    "node".into()
}

/// Whether this computer has Z.ai credentials the bridge may use (the free built-in route).
pub fn z_ai_config_present() -> bool {
    let home = dirs::home_dir().map(|h| h.join(".z-ai-config"));
    [Some(PathBuf::from("/etc/.z-ai-config")), home].into_iter().flatten().any(|p| p.is_file())
}

/// The bridge process, started once for the whole app and kept for its lifetime.
static BRIDGE: std::sync::OnceLock<Arc<craft_anything_platform::sync::Mutex<Option<RunningProcess>>>> = std::sync::OnceLock::new();

/// Make sure the local bridge is up, when the machine's Z.ai credentials are what serves the
/// model (no key saved). Safe to call before every conversation: an answering bridge is left as
/// it is, whoever started it.
pub fn ensure_bridge() {
    if !z_ai_config_present() {
        return;
    }
    let holder = BRIDGE.get_or_init(Arc::default);
    let mut g = holder.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if g.is_some() {
        return;
    }
    // An already-running bridge (an earlier conversation's app, a hand-started one) answers.
    let agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent("CraftAnything-dev")
        .timeout_global(Some(Duration::from_secs(2)))
        .build()
        .new_agent();
    if agent.get(format!("http://127.0.0.1:{BRIDGE_PORT}/health")).call().is_ok_and(|r| r.status().as_u16() / 100 == 2) {
        return;
    }
    let Some(dir) = local_agent_dir(settings_zcode_dir().as_deref()) else { return };
    let node = node_program(None);
    if let Ok(proc) = Process::new(&node)
        .args([&dir.join("zai-bridge.cjs").display().to_string()])
        .stdin(StdinMode::Closed)
        .timeout(Duration::from_secs(24 * 3600))
        .stdout_cap(1 << 20)
        .stderr_cap(1 << 20)
        .label("zai-bridge")
        .spawn()
    {
        *g = Some(proc);
    }
}

/// One provider entry the agent understands: `provider` served at `base_url` with `api_key`,
/// model `{provider}/{model}`.
fn provider_config_json(provider: &str, model: &str, base_url: &str, api_key: &str) -> Value {
    json!({
        "schemaVersion": 1,
        "config": {
            "providerOrder": [provider],
            "providerConfigRules": { "providerRules": [{
                "providerId": provider,
                "providerName": DISPLAY,
                "enabled": true,
                "config": {
                    "group": "standard-personal",
                    "access": { "type": "api-key", "apiKey": api_key },
                    "api": { "type": "openai-chat-completions", "baseUrl": base_url },
                    "personalModelIds": [model],
                    "modelOrder": [model],
                },
            }]},
            "modelConfigRules": { "providerModelRules": [{
                "providerId": provider,
                "modelId": model,
                "config": {
                    "enabled": true,
                    "properties": {
                        "requiresMfjsToolSchema": false,
                        "contextWindow": 200000,
                        "inputFormat": { "supportsText": true, "supportsImage": false, "supportsVideo": false, "supportsAudio": false, "supportsPdf": false },
                        "outputFormat": { "supportsText": true },
                        "supportsToolCall": true,
                        "supportsJsonSchemaOutput": false,
                        "supportsNativeWebSearch": false,
                        "supportsMidConversationSystem": false,
                    },
                    "optionSpecs": {
                        "reasoningLevel": { "values": ["disabled", "enabled"], "map": "{}" },
                        "maxOutputTokens": { "max": 16000 },
                    },
                },
            }], "manualProviderModelRules": [] },
            "defaultModelSelection": { "providerId": provider, "modelId": model },
        },
    })
}

/// The provider config the agent is pointed at (`ZCODE_PERSONAL_PROVIDER_CONFIG_FILE`) and the
/// route it takes: the bridge when Z.ai credentials are on this computer and no key was saved,
/// else the direct Z.ai endpoint with the saved key. Written under the app's own config dir,
/// never logged.
pub fn ensure_provider_config(store: &KeyStore, backend_name: &str) -> Result<(PathBuf, Route), ToolError> {
    let dir = app_data_dir();
    std::fs::create_dir_all(&dir).map_err(|e| ToolError::io(dir.display(), e))?;
    let path = dir.join("provider_config.json");
    let saved_key = store.get(backend_name).map(|k| k.trim().to_string()).filter(|k| !k.is_empty());
    let route = match saved_key {
        Some(key) => Route::Direct(key),
        None if z_ai_config_present() => Route::Bridge,
        None => {
            return Err(ToolError::new(
                ErrorCode::BadSettings,
                "ZCode has no way to reach its GLM model on this computer.",
            )
            .with_remediation(format!(
                "Save a Z.ai API key for {DISPLAY} in Settings → AI, or put Z.ai credentials in ~/.z-ai-config (the app then serves them through its local bridge and no key is asked for)."
            )));
        }
    };
    craft_anything_platform::write_atomic_private(&path, provider_config_for(&route).as_bytes())?;
    Ok((path, route))
}

/// The route the agent's GLM provider takes, decided once per run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// The bridge next to the agent: the free built-in route, fed by Z.ai credentials that were
    /// found on this machine or travel to the sandbox separately.
    Bridge,
    /// The saved Z.ai key, straight to the endpoint. The key is written into the provider config
    /// alone (the app's config dir here, the sandbox's agent folder there).
    Direct(String),
}

impl Route {
    /// The model the agent is told to use: the bridge's own name, or the endpoint's real one.
    pub fn model(&self) -> &str {
        match self {
            Route::Bridge => BRIDGE_MODEL,
            Route::Direct(_) => ZAI_MODEL,
        }
    }
}

/// The provider config for a route, as the agent reads it (`ZCODE_PERSONAL_PROVIDER_CONFIG_FILE`).
pub fn provider_config_for(route: &Route) -> String {
    match route {
        Route::Bridge => provider_config_json(BRIDGE_PROVIDER, BRIDGE_MODEL, &format!("http://127.0.0.1:{BRIDGE_PORT}/llm/v1"), "builtin").to_string(),
        Route::Direct(key) => provider_config_json(BRIDGE_PROVIDER, ZAI_MODEL, ZAI_BASE_URL, key).to_string(),
    }
}

/// How the agent is driven as a registered adapter. Conversations with it go through
/// [`ZcodeConversation`] (see `conversation` below); this adapter exists so Settings, detection
/// and the model picker know it.
pub struct ZcodeAdapter;

impl AgentAdapter for ZcodeAdapter {
    fn ids(&self) -> &'static [&'static str] {
        &[AGENT_ID]
    }

    fn args(&self, cx: &ArgsCx) -> Vec<String> {
        let mut a = cx.options.args.clone();
        a.extend(["-p".into(), "--output-format".into(), "stream-json".into()]);
        a.extend(["--mode".into(), if cx.question { "plan".into() } else { "yolo".into() }]);
        a
    }

    fn answers_questions(&self) -> bool {
        true
    }

    fn streams_json(&self) -> bool {
        true
    }

    fn model_in_use(&self, options: Option<&BackendOptions>) -> Option<String> {
        Some(options.and_then(|o| o.cli_model.clone()).unwrap_or_else(|| format!("{BRIDGE_PROVIDER}/{BRIDGE_MODEL}")))
    }

    fn model_choices(&self, _config: &BackendConfig, _options: &BackendOptions) -> Result<Vec<String>, ToolError> {
        Ok(vec![format!("{BRIDGE_PROVIDER}/{BRIDGE_MODEL}")])
    }
}

/// What the protocol produced: a line to write back, an event for the app, the turn's end, or a
/// permission request the approver should decide.
#[derive(Debug)]
pub enum Out {
    Send(String),
    Event(AgentEvent),
    TurnEnded,
    Decide { call_id: String, tool: String, input: Value },
}

/// Where the conversation is in its handshake.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Waiting for the server's storage to be ready.
    Starting,
    /// A session is being created (or resumed).
    Creating,
    /// Talking.
    Ready,
    /// The server is gone; the next send restarts it.
    Dead,
}

/// The ZCode protocol, on one connection: requests out, replies and events in. Kept in a
/// `Shared` beside the process's stdin, exactly like the ACP conversation.
pub struct ZcodeCore {
    backend: String,
    workspace: String,
    mode: String,
    /// The model the session talks to, from the route the provider config takes.
    model: String,
    next_id: u64,
    phase: Phase,
    session: Option<String>,
    /// A saved session to bring back, once the server is ready.
    resume: Option<String>,
    /// The message waiting for the session to open.
    queued: Option<String>,
    /// Requests awaiting their reply: id → what it was for.
    pending: HashMap<u64, &'static str>,
    /// Permission requests on their way to the approver: request id → the server message's id.
    deciding: HashMap<String, u64>,
    input_tokens: u64,
    output_tokens: u64,
    pub failure: Option<TurnFailure>,
    pub session_missing: bool,
}

impl ZcodeCore {
    pub fn new(backend: &str, workspace: &Path, builder: bool, resume: Option<String>, model: String) -> Self {
        ZcodeCore {
            backend: backend.to_string(),
            workspace: workspace.display().to_string(),
            mode: if builder { "build".into() } else { "plan".into() },
            model,
            next_id: 0,
            phase: Phase::Starting,
            session: None,
            resume,
            queued: None,
            pending: HashMap::new(),
            deciding: HashMap::new(),
            input_tokens: 0,
            output_tokens: 0,
            failure: None,
            session_missing: false,
        }
    }

    fn id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    fn send(&mut self, method: &str, params: Value, kind: &'static str, out: &mut Vec<Out>) {
        let id = self.id();
        self.pending.insert(id, kind);
        out.push(Out::Send(json!({ "id": id, "method": method, "params": params }).to_string()));
    }

    /// The handshake: wait for the server's storage, then bring back a saved session when there
    /// is one, else open a new one.
    pub fn start(&mut self) -> Vec<Out> {
        self.phase = Phase::Starting;
        self.pending.clear();
        self.deciding.clear();
        self.session = None;
        self.input_tokens = 0;
        self.output_tokens = 0;
        Vec::new()
    }

    fn create_session(&mut self, out: &mut Vec<Out>) {
        self.phase = Phase::Creating;
        let model = json!({ "providerId": BRIDGE_PROVIDER, "modelId": self.model, "options": { "reasoningLevel": "enabled" } });
        if let Some(saved) = self.resume.take() {
            self.send("session/resume", json!({ "sessionId": saved }), "resume", out);
        } else {
            self.send(
                "session/create",
                json!({
                    "workspace": { "workspacePath": self.workspace, "workspaceKey": self.workspace },
                    "mode": self.mode,
                    "model": model,
                    "persistence": "immediate",
                }),
                "create",
                out,
            );
        }
    }

    /// Queue the person's message; it goes out once the session is there.
    pub fn queue_prompt(&mut self, text: &str, out: &mut Vec<Out>) {
        self.failure = None;
        self.session_missing = false;
        match self.phase {
            Phase::Ready => self.send_message(text, out),
            Phase::Dead => {
                self.queued = Some(text.to_string());
                self.phase = Phase::Starting;
            }
            Phase::Starting | Phase::Creating => self.queued = Some(text.to_string()),
        }
    }

    fn send_message(&mut self, text: &str, out: &mut Vec<Out>) {
        let Some(session) = self.session.clone() else {
            self.queued = Some(text.to_string());
            return;
        };
        // The model goes with every message: the registry wants the reasoning level on the
        // turn's own selection, not just on the session's (see the agent's docs).
        self.send(
            "session/send",
            json!({
                "sessionId": session,
                "content": text,
                "modelSelection": { "providerId": BRIDGE_PROVIDER, "modelId": self.model, "options": { "reasoningLevel": "enabled" } },
            }),
            "send",
            out,
        );
    }

    /// The answer to a permission request, sent the way the protocol's strict response schema
    /// wants: the decision and nothing else.
    pub fn answer(&mut self, request_id: &str, allow: bool, out: &mut Vec<Out>) {
        if let Some(id) = self.deciding.remove(request_id) {
            let decision = if allow { "allow" } else { "deny" };
            out.push(Out::Send(json!({ "id": id, "result": { "decision": decision } }).to_string()));
        }
    }

    /// Stop the running turn (out of band; it works mid-turn).
    pub fn stop(&mut self, out: &mut Vec<Out>) {
        if let Some(session) = self.session.clone() {
            out.push(Out::Send(json!({ "method": "session/stop", "params": { "sessionId": session } }).to_string()));
        }
    }

    /// One line from the server.
    pub fn line(&mut self, line: &str, out: &mut Vec<Out>) {
        let Ok(v) = serde_json::from_str::<Value>(line) else {
            // Not JSON: log lines and noise; output, never the answer.
            if !line.trim().is_empty() {
                out.push(Out::Event(AgentEvent::Output { text: line.to_string() }));
            }
            return;
        };
        // A request from the server: runtime preferences, or a permission to decide.
        if let (Some(id), Some(method)) = (v.get("id"), v.get("method").and_then(Value::as_str)) {
            match method {
                "session/requestRuntimePreferences" => {
                    out.push(Out::Send(json!({ "id": id, "result": runtime_preferences() }).to_string()));
                }
                "interaction/requestPermission" => self.permission(id.clone(), &v, out),
                _ => {
                    out.push(Out::Send(
                        json!({ "id": id, "error": { "code": -32601, "message": format!("Craft Anything doesn't handle {method}") } }).to_string(),
                    ));
                }
            }
            return;
        }
        // A reply to something we asked for.
        if let Some(id) = v.get("id").and_then(Value::as_u64) {
            self.reply(id, &v, out);
            return;
        }
        // Notifications.
        match v.get("method").and_then(Value::as_str) {
            Some("startup/storageState") => {
                if v.pointer("/params/phase").and_then(Value::as_str) == Some("ready") {
                    self.create_session(out);
                }
            }
            Some("session/event") => self.event(v.get("params").unwrap_or(&Value::Null), out),
            _ => {}
        }
    }

    fn reply(&mut self, id: u64, v: &Value, out: &mut Vec<Out>) {
        let kind = self.pending.remove(&id);
        if let Some(error) = v.get("error") {
            let message = error.get("message").and_then(Value::as_str).unwrap_or("the agent reported an error").to_string();
            if kind == Some("resume") {
                // The session it was to continue isn't there: the next message starts a new one.
                self.session_missing = true;
            }
            self.fail(message, out);
            return;
        }
        if kind == Some("create") || kind == Some("resume") {
            let session = v
                .pointer("/result/session/sessionId")
                .and_then(Value::as_str)
                .or_else(|| v.pointer("/result/sessionId").and_then(Value::as_str))
                .map(str::to_string);
            match session {
                Some(s) => {
                    self.session = Some(s.clone());
                    self.phase = Phase::Ready;
                    self.send("session/subscribe", json!({ "sessionId": s, "deliveryKind": "desktop-continuous" }), "subscribe", out);
                    if let Some(text) = self.queued.take() {
                        self.send_message(&text, out);
                    }
                }
                None => self.fail("The agent opened no session.".into(), out),
            }
        }
    }

    fn fail(&mut self, message: String, out: &mut Vec<Out>) {
        self.failure.get_or_insert(TurnFailure::new(FailureKind::Transient));
        out.push(Out::Event(AgentEvent::Error { message, remediation: None, log: None }));
        out.push(Out::TurnEnded);
        self.phase = Phase::Dead;
    }

    /// A permission request: the approver decides, or the person does through an approval card.
    fn permission(&mut self, id: Value, v: &Value, out: &mut Vec<Out>) {
        let params = v.get("params").cloned().unwrap_or(Value::Null);
        let request_id = params.get("requestId").and_then(Value::as_str).unwrap_or_default().to_string();
        let tool = params.get("toolName").and_then(Value::as_str).unwrap_or("tool").to_string();
        let input = params.get("input").cloned().unwrap_or(Value::Null);
        let numeric = id.as_u64().unwrap_or(0);
        if request_id.is_empty() {
            out.push(Out::Send(json!({ "id": numeric, "result": { "decision": "deny" } }).to_string()));
            return;
        }
        self.deciding.insert(request_id.clone(), numeric);
        out.push(Out::Decide { call_id: request_id, tool, input });
    }

    /// One `session/event`: what the agent is saying and doing.
    fn event(&mut self, params: &Value, out: &mut Vec<Out>) {
        let payload = params.get("payload").cloned().unwrap_or(Value::Null);
        let kind = payload.get("kind").and_then(Value::as_str);
        match kind {
            Some("text_delta") => {
                if let Some(delta) = payload.get("delta").and_then(Value::as_str)
                    && !delta.is_empty()
                {
                    out.push(Out::Event(AgentEvent::TextDelta { text: delta.to_string() }));
                }
            }
            Some("tool_call") => {
                let call_id = payload.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_string();
                let tool = payload.get("toolName").and_then(Value::as_str).unwrap_or("tool").to_string();
                let input = payload.get("input").cloned().unwrap_or(Value::Null);
                out.push(Out::Event(AgentEvent::ToolStarted { call_id, tool, input }));
            }
            Some("result") => {
                let call_id = payload.get("toolCallId").and_then(Value::as_str).unwrap_or_default().to_string();
                let tool = payload.get("toolName").and_then(Value::as_str).unwrap_or("tool").to_string();
                let ok = payload.pointer("/result/success").and_then(Value::as_bool).unwrap_or(false);
                let summary = payload.pointer("/result/content").and_then(Value::as_str).unwrap_or_default().to_string();
                out.push(Out::Event(AgentEvent::ToolFinished { call_id, tool, ok, summary, evidence_id: None }));
            }
            Some("started") => {
                let tool = payload.get("toolName").and_then(Value::as_str).unwrap_or("tool");
                out.push(Out::Event(AgentEvent::Status { text: format!("Running {tool}…") }));
            }
            Some("model_request_completed") => {
                if let Some(usage) = payload.get("usage") {
                    self.input_tokens += usage.get("inputTokens").or_else(|| usage.get("promptTokens")).and_then(Value::as_u64).unwrap_or(0);
                    self.output_tokens += usage.get("outputTokens").or_else(|| usage.get("completionTokens")).and_then(Value::as_u64).unwrap_or(0);
                }
            }
            _ => {
                // The turn's last word: the finished assistant message, when the stop reason says
                // the turn is over ("tool-calls" means it carries on with tools).
                let stop = payload.get("stopReason").and_then(Value::as_str);
                let main = payload.get("querySource").and_then(Value::as_str).is_some_and(|q| q == "main_turn");
                if main && stop.is_some_and(|s| s != "tool-calls") {
                    out.push(Out::TurnEnded);
                }
            }
        }
    }

    pub fn session_id(&self) -> Option<String> {
        self.session.clone()
    }
}

/// What one connection shares between the reader callback and the conversation: the protocol, the
/// way lines are written to the agent, and who decides permissions.
struct Shared {
    core: ZcodeCore,
    stdin: Option<std::process::ChildStdin>,
    remote: Option<(Envd, Arc<std::sync::atomic::AtomicU32>)>,
    approver: Option<acp::Approver>,
}

impl Shared {
    /// Write one NDJSON line to the agent, here (its stdin) or in the sandbox (an input request).
    fn write_line(&mut self, line: &str) -> bool {
        let with_newline = format!("{line}\n");
        if let Some(stdin) = self.stdin.as_mut() {
            return stdin.write_all(with_newline.as_bytes()).and_then(|_| stdin.flush()).is_ok();
        }
        if let Some((envd, pid)) = &self.remote {
            let pid = pid.load(Ordering::SeqCst);
            return pid != 0 && envd.send_input(pid, with_newline.as_bytes()).is_ok();
        }
        false
    }

    fn apply(
        &mut self,
        outs: Vec<Out>,
        shared: &Arc<craft_anything_platform::sync::Mutex<Option<Shared>>>,
        tx: &Sender<AgentEvent>,
        busy: &Arc<std::sync::atomic::AtomicBool>,
    ) {
        for out in outs {
            match out {
                Out::Send(line) => {
                    if !self.write_line(&line) {
                        let _ = tx.send(AgentEvent::Error { message: "The agent stopped taking messages.".into(), remediation: None, log: None });
                    }
                }
                Out::Event(e) => {
                    let _ = tx.send(e);
                }
                Out::TurnEnded => {
                    busy.store(false, Ordering::SeqCst);
                    let _ = tx.send(AgentEvent::TurnDone {
                        backend: self.core.backend.clone(),
                        input_tokens: self.core.input_tokens,
                        output_tokens: self.core.output_tokens,
                        cost_usd: None,
                        cost_is_estimate: true,
                    });
                }
                Out::Decide { call_id, tool, input } => {
                    let approver = self.approver.clone();
                    let shared2 = shared.clone();
                    let tx2 = tx.clone();
                    let busy2 = busy.clone();
                    let call_id2 = call_id.clone();
                    let spawned = std::thread::Builder::new().name("zcode-approval".into()).spawn(move || {
                        let allowed = approver.is_none_or(|a| a(&tool, &input));
                        let mut g = shared2.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                        if let Some(s) = g.as_mut() {
                            let mut outs = Vec::new();
                            s.core.answer(&call_id2, allowed, &mut outs);
                            s.apply(outs, &shared2, &tx2, &busy2);
                        }
                    });
                    if spawned.is_err() {
                        let mut outs = Vec::new();
                        self.core.answer(&call_id, false, &mut outs);
                        self.apply(outs, shared, tx, busy);
                    }
                }
            }
        }
    }
}

/// A conversation with the ZCode agent: one app-server process in the background, one session for
/// the whole chat, permission requests answered by `approver` (a builder) or shown as approval
/// cards (a chat).
pub struct ZcodeConversation {
    backend: String,
    agent: String,
    /// The agent's working folder: the project here, or its copy in the sandbox.
    workspace: PathBuf,
    builder: bool,
    preamble: String,
    approver: Option<acp::Approver>,
    shared: Arc<craft_anything_platform::sync::Mutex<Option<Shared>>>,
    proc: Option<RunningProcess>,
    tx: Sender<AgentEvent>,
    rx: Receiver<AgentEvent>,
    busy: Arc<std::sync::atomic::AtomicBool>,
    stop: Stop,
    /// The project's sandbox, when the agent runs in one.
    sandbox: Option<Arc<crate::sandbox::Sandbox>>,
    session: Arc<craft_anything_platform::sync::Mutex<Option<String>>>,
    lost: Arc<std::sync::atomic::AtomicBool>,
    failure: Option<TurnFailure>,
    /// The project the sandbox's files come back to.
    project_root: PathBuf,
    /// A local copy of the agent the settings name, taken once at the start.
    zcode_dir: Option<String>,
    /// The model the session talks to, from the route the run took (set when it starts).
    model: Option<String>,
}

impl ZcodeConversation {
    /// The conversation for `spec`. `approver` decides permission requests for a builder; a chat
    /// without one shows them as cards.
    pub fn new(spec: &CliSpec, approver: Option<acp::Approver>) -> Self {
        let (tx, rx) = std::sync::mpsc::channel();
        let workspace = spec.project.clone().unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| std::env::temp_dir()));
        ZcodeConversation {
            backend: spec.config.name.clone(),
            agent: spec.config.model.clone(),
            workspace: workspace.clone(),
            builder: spec.builder.is_some(),
            preamble: spec.preamble.clone().unwrap_or_else(|| crate::prompt::cli_preamble(spec.beginner)),
            approver,
            shared: Arc::default(),
            proc: None,
            tx,
            rx,
            busy: Arc::default(),
            stop: Stop::default(),
            sandbox: spec.sandbox.clone(),
            session: Arc::new(craft_anything_platform::sync::Mutex::new(spec.resume.clone())),
            lost: Arc::default(),
            failure: None,
            project_root: workspace,
            zcode_dir: settings_zcode_dir(),
            model: None,
        }
    }

    fn lock(&self) -> craft_anything_platform::sync::MutexGuard<'_, Option<Shared>> {
        self.shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The agent's command line: its Node runtime, its entry, the app-server subcommand. A run on
    /// this computer needs a local copy of the agent; the sandbox's entry is where setup puts it.
    fn argv(&self, in_sandbox: bool) -> Result<Vec<String>, ToolError> {
        let node = if in_sandbox { "node".to_string() } else { node_program(None) };
        let entry = if in_sandbox {
            format!("{}/agent/zcode.cjs", self.sandbox.as_ref().map(|s| s.remote_root.as_str()).unwrap_or("/craft-anything"))
        } else {
            let dir = local_agent_dir(self.zcode_dir.as_deref()).ok_or_else(|| {
                ToolError::new(
                    ErrorCode::EngineMissing,
                    "The ZCode agent isn't on this computer, and the app doesn't carry it.",
                )
                .with_remediation(format!(
                    "Point Settings → AI → ZCode agent folder at the folder holding zcode.cjs (it runs on Node 22+), or turn on the cloud sandbox in Settings → AI → Sandbox: the agent is set up there, and never has to be on this computer."
                ))
            })?;
            dir.join("zcode.cjs").display().to_string()
        };
        Ok(vec![node, entry, "app-server".into(), "--stdio".into()])
    }

    /// The environment the agent needs, for a run on this computer: its provider catalogue and
    /// the personal provider config the app generates. Returns the route the config took, so the
    /// session asks for the model that route serves.
    fn local_env(&self, store: &KeyStore) -> Result<(Vec<(String, String)>, Route), ToolError> {
        let dir = app_data_dir();
        let (personal, route) = ensure_provider_config(store, &self.agent)?;
        let agent_dir = local_agent_dir(self.zcode_dir.as_deref());
        let builtin = agent_dir.as_deref().and_then(|d| builtin_config_bytes(Some(d))).ok_or_else(|| {
            ToolError::new(ErrorCode::EngineMissing, "The ZCode agent's provider catalogue is missing.")
                .with_remediation(format!(
                    "The folder Settings → AI → ZCode agent folder points at needs config/zcode-builtin.json beside zcode.cjs. The sandbox route doesn't need a local copy at all: Settings → AI → Sandbox turns it on."
                ))
        })?;
        let config_dir = dir.join("config");
        std::fs::create_dir_all(&config_dir).map_err(|e| ToolError::io(config_dir.display(), e))?;
        let config_path = config_dir.join("zcode-builtin.json");
        craft_anything_platform::write_atomic_private(&config_path, &builtin)?;
        let env = vec![
            ("ZCODE_PERSONAL_PROVIDER_CONFIG_FILE".to_string(), personal.display().to_string()),
            ("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE".to_string(), config_path.display().to_string()),
            ("ZCODE_STORAGE_DIR".to_string(), dir.join("home").display().to_string()),
            (craft_anything_engine::AGENT_ENV.to_string(), "1".to_string()),
        ];
        Ok((env, route))
    }

    /// The environment for a run inside the sandbox: same names, the sandbox's own paths.
    fn sandbox_env(&self, sb: &crate::sandbox::Sandbox) -> Vec<(String, String)> {
        vec![
            ("ZCODE_PERSONAL_PROVIDER_CONFIG_FILE".to_string(), format!("{}/agent/provider_config.json", sb.remote_root)),
            ("ZCODE_BUILTIN_PROVIDER_CONFIG_FILE".to_string(), format!("{}/agent/config/zcode-builtin.json", sb.remote_root)),
            ("ZCODE_STORAGE_DIR".to_string(), format!("{}/zcode/home", sb.remote_root)),
            ("HOME".to_string(), "/root".to_string()),
            ("PATH".to_string(), format!("{}/node/bin:/usr/local/bin:/usr/bin:/bin", sb.remote_root)),
            (craft_anything_engine::AGENT_ENV.to_string(), "1".to_string()),
        ]
    }

    /// Start the app-server: here, or inside the project's sandbox.
    fn spawn(&mut self, store: &KeyStore) -> Result<(), ToolError> {
        let sandboxed = self.sandbox.is_some();
        let argv = self.argv(sandboxed)?;
        let workspace = self.sandbox.as_ref().map(|s| s.remote_root.clone()).unwrap_or_else(|| self.workspace.display().to_string());
        let resume = self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let mut outs = Vec::new();
        if let Some(sb) = self.sandbox.clone() {
            // Setup wrote the route the sandbox's provider config takes; the session asks for the
            // model it serves. Without it, setup never got to the agent.
            let model = sb.zcode_model.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone().ok_or_else(|| {
                ToolError::new(ErrorCode::EngineMissing, "The sandbox's ZCode agent setup didn't finish, so there is no model route yet.")
                    .with_remediation("Start the build again: the sandbox sets itself up on its own.")
            })?;
            self.model = Some(model.clone());
            {
                let mut g = self.lock();
                *g = Some(Shared {
                    core: ZcodeCore::new(&self.backend, Path::new(&workspace), self.builder, resume, model),
                    stdin: None,
                    remote: None,
                    approver: self.approver.clone(),
                });
            }
            let envd = sb.envd()?;
            let env: BTreeMap<String, String> = self.sandbox_env(&sb).into_iter().collect();
            let argv = argv[1..].to_vec(); // "node" is the program; the rest are its arguments
            let mut remote_proc = envd.start_process("node", &argv, &env, &format!("{}/zcode", sb.remote_root))?;
            let pid_holder = Arc::new(std::sync::atomic::AtomicU32::new(0));
            if let Some(pid) = remote_proc.pid(Duration::from_secs(60)) {
                pid_holder.store(pid, Ordering::SeqCst);
            }
            self.pipe_remote_events(remote_proc, pid_holder.clone());
            {
                let mut g = self.lock();
                if let Some(s) = g.as_mut() {
                    s.remote = Some((envd, pid_holder));
                    outs.extend(s.core.start());
                    s.apply(outs, &self.shared, &self.tx, &self.busy);
                }
            }
            return Ok(());
        }
        let (env, route) = self.local_env(store)?;
        self.model = Some(route.model().to_string());
        {
            let mut g = self.lock();
            *g = Some(Shared {
                core: ZcodeCore::new(&self.backend, Path::new(&workspace), self.builder, resume, route.model().to_string()),
                stdin: None,
                remote: None,
                approver: self.approver.clone(),
            });
        }
        // The machine's Z.ai credentials may be what serves the model: the bridge goes up first.
        ensure_bridge();
        let (shared, tx, busy) = (self.shared.clone(), self.tx.clone(), self.busy.clone());
        let mut proc = Process::new(&argv[0])
            .args(&argv[1..])
            .cwd(&self.workspace)
            .stdin(StdinMode::Piped)
            // One process serves the whole chat; a builder turn may take hours.
            .timeout(Duration::from_secs(24 * 3600))
            .stdout_cap(4 << 20)
            .stderr_cap(1 << 20);
        for (k, v) in &env {
            proc = proc.env(k, v.as_str());
        }
        let mut proc = proc
            .on_stdout_line({
                let shared = shared.clone();
                let tx = tx.clone();
                move |l| {
                    let mut g = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    if let Some(s) = g.as_mut() {
                        let mut outs = Vec::new();
                        s.core.line(l, &mut outs);
                        s.apply(outs, &shared, &tx, &busy);
                    }
                }
            })
            .on_stderr_line({
                let tx_err = tx.clone();
                move |l| {
                    if !l.trim().is_empty() {
                        let _ = tx_err.send(AgentEvent::Output { text: l.to_string() });
                    }
                }
            })
            .label(format!("{DISPLAY} server"))
            .spawn()?;
        let stdin = proc.take_stdin();
        self.proc = Some(proc);
        {
            let mut g = self.lock();
            if let Some(s) = g.as_mut() {
                s.stdin = stdin;
                outs.extend(s.core.start());
                s.apply(outs, &self.shared, &self.tx, &self.busy);
            }
        }
        Ok(())
    }

    /// Read the sandbox agent's events on their own thread, into the same handler a local
    /// process's stdout lines go through.
    fn pipe_remote_events(&mut self, proc: RemoteProcess, pid_holder: Arc<std::sync::atomic::AtomicU32>) {
        let shared = self.shared.clone();
        let tx = self.tx.clone();
        let busy = self.busy.clone();
        let _ = std::thread::Builder::new().name("zcode-sandbox".into()).spawn(move || {
            loop {
                match proc.recv_timeout(Duration::from_millis(250)) {
                    Ok(ProcEvent::Stdout(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        for line in text.split('\n').filter(|l| !l.trim().is_empty()) {
                            let mut g = shared.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                            if let Some(s) = g.as_mut() {
                                let mut outs = Vec::new();
                                s.core.line(line, &mut outs);
                                s.apply(outs, &shared, &tx, &busy);
                            }
                        }
                    }
                    Ok(ProcEvent::Stderr(bytes)) => {
                        let text = String::from_utf8_lossy(&bytes);
                        if !text.trim().is_empty() {
                            let _ = tx.send(AgentEvent::Output { text: text.trim().to_string() });
                        }
                    }
                    Ok(_) => {}
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                    Err(_) => {
                        pid_holder.store(0, Ordering::SeqCst);
                        return;
                    }
                }
            }
        });
    }

    fn send_prompt(&mut self, text: &str, _retry: bool) {
        if self.busy.swap(true, Ordering::SeqCst) {
            let _ = self.tx.send(AgentEvent::Error {
                message: "Still working on the last message.".into(),
                remediation: Some("Wait for it to finish or press stop.".into()),
                log: None,
            });
            return;
        }
        self.stop.reset();
        self.failure = None;
        let running = self.proc.as_mut().is_some_and(|p| !p.is_finished());
        if !running {
            let _ = self.tx.send(AgentEvent::Status { text: format!("Starting {DISPLAY}…") });
            let store = KeyStore::system();
            if let Err(e) = self.spawn(&store) {
                self.busy.store(false, Ordering::SeqCst);
                self.failure = Some(TurnFailure::from_process(&e));
                let _ = self.tx.send(AgentEvent::Error { message: e.message, remediation: e.remediation, log: None });
                let _ = self.tx.send(AgentEvent::TurnDone {
                    backend: self.backend.clone(),
                    input_tokens: 0,
                    output_tokens: 0,
                    cost_usd: None,
                    cost_is_estimate: false,
                });
                return;
            }
        }
        let text = format!("{}{text}", self.preamble);
        let mut g = self.lock();
        if let Some(s) = g.as_mut() {
            let mut outs = Vec::new();
            s.core.queue_prompt(&text, &mut outs);
            s.apply(outs, &self.shared, &self.tx, &self.busy);
        }
    }
}

impl Conversation for ZcodeConversation {
    fn send(&mut self, text: &str) {
        self.send_prompt(text, false);
    }

    fn poll(&mut self) -> Vec<AgentEvent> {
        let mut events = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            if matches!(e, AgentEvent::TurnDone { .. }) {
                self.after_turn();
            }
            events.push(e);
        }
        events
    }

    fn answer_consent(&mut self, call_id: &str, allow: bool) {
        let mut g = self.lock();
        if let Some(s) = g.as_mut() {
            let mut outs = Vec::new();
            s.core.answer(call_id, allow, &mut outs);
            s.apply(outs, &self.shared, &self.tx, &self.busy);
        }
    }

    fn stop(&mut self) {
        self.stop.stop();
        let mut g = self.lock();
        if let Some(s) = g.as_mut() {
            let mut outs = Vec::new();
            s.core.stop(&mut outs);
            s.apply(outs, &self.shared, &self.tx, &self.busy);
        }
    }

    fn is_busy(&self) -> bool {
        self.busy.load(Ordering::SeqCst)
    }

    fn session_id(&self) -> Option<String> {
        self.lock().as_ref().and_then(|s| s.core.session_id()).or_else(|| self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone())
    }

    fn session_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    fn last_failure(&self) -> Option<TurnFailure> {
        self.failure.or_else(|| self.lock().as_ref().and_then(|s| s.core.failure))
    }
}

impl ZcodeConversation {
    /// What follows a finished turn in the sandbox: the work comes back to this computer, and the
    /// sandbox pauses until the next turn (cheapest between turns; E2B brings it back itself).
    fn after_turn(&mut self) {
        if let Some(s) = self.lock().as_ref().and_then(|s| s.core.session_id()) {
            *self.session.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(s);
        }
        let Some(sb) = self.sandbox.clone() else { return };
        let mut status = |line: String| {
            let _ = self.tx.send(AgentEvent::Status { text: line });
        };
        match sb.sync_down(&self.project_root, &mut status) {
            Ok(summary) => {
                let _ = self.tx.send(AgentEvent::Status { text: format!("The sandbox's work is copied back ({}).", summary.describe(false)) });
            }
            Err(e) => {
                let _ = self.tx.send(AgentEvent::Status { text: format!("Couldn't copy the sandbox's work back: {}", first_line(&e.message)) });
            }
        }
        if sb.config.pause_after_turn {
            sb.pause();
            let _ = self.tx.send(AgentEvent::Status { text: "The sandbox is paused until the next turn.".into() });
        }
    }
}

fn first_line(s: &str) -> String {
    s.lines().next().unwrap_or_default().to_string()
}

impl Drop for ZcodeConversation {
    fn drop(&mut self) {
        // The local server is killed with the process handle; the sandbox's ends with the pause.
        self.proc = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn core() -> ZcodeCore {
        ZcodeCore::new("Test", Path::new("/proj"), false, None, BRIDGE_MODEL.to_string())
    }

    /// The handshake: storage ready → session/create (with the workspace and the built-in model)
    /// → subscribe → the queued message goes out.
    #[test]
    fn the_handshake_creates_a_session_and_sends() {
        let mut c = core();
        let mut out = c.start();
        assert!(out.is_empty(), "nothing to send until the server is ready");
        c.line(json!({"method": "startup/storageState", "params": {"phase": "migrating"}}).to_string().as_str(), &mut out);
        assert!(out.is_empty());
        c.queue_prompt("build me a thing", &mut out);
        c.line(json!({"method": "startup/storageState", "params": {"phase": "ready"}}).to_string().as_str(), &mut out);
        // The session is asked for first; the queued message waits for it to exist.
        assert_eq!(out.len(), 1, "the session is asked for first");
        let create = serde_json::from_str::<Value>(out[0].send_text()).unwrap();
        assert_eq!(create["method"], "session/create");
        assert_eq!(create["params"]["workspace"]["workspacePath"], "/proj");
        assert_eq!(create["params"]["model"]["providerId"], BRIDGE_PROVIDER);
        assert_eq!(create["params"]["mode"], "plan");
        // The create reply opens the session: subscribe, then the queued message.
        let mut out2 = Vec::new();
        c.line(json!({"id": create["id"], "result": {"session": {"sessionId": "sess-1"}}}).to_string().as_str(), &mut out2);
        assert_eq!(out2.len(), 2, "subscribe, then the queued message");
        let subscribe = serde_json::from_str::<Value>(out2[0].send_text()).unwrap();
        assert_eq!(subscribe["method"], "session/subscribe");
        assert_eq!(subscribe["params"]["sessionId"], "sess-1");
        let send = serde_json::from_str::<Value>(out2[1].send_text()).unwrap();
        assert_eq!(send["method"], "session/send");
        assert_eq!(send["params"]["content"], "build me a thing");
        assert_eq!(send["params"]["modelSelection"]["options"]["reasoningLevel"], "enabled");
        assert_eq!(c.session_id().as_deref(), Some("sess-1"));
    }

    /// Runtime preferences are answered with the defaults; permissions go to the approver and the
    /// answer carries nothing but the decision (the protocol's response schema is strict).
    #[test]
    fn server_requests_are_answered() {
        let mut c = core();
        let mut out = Vec::new();
        c.line(json!({"id": 7, "method": "session/requestRuntimePreferences"}).to_string().as_str(), &mut out);
        let reply = serde_json::from_str::<Value>(out[0].send_text()).unwrap();
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["memoryEnabled"], false);
        let mut out = Vec::new();
        c.line(
            json!({"id": 8, "method": "interaction/requestPermission", "params": {"requestId": "perm-1", "toolName": "Bash", "input": {"command": "ls"}}})
                .to_string()
                .as_str(),
            &mut out,
        );
        match &out[0] {
            Out::Decide { call_id, tool, input } => {
                assert_eq!(call_id, "perm-1");
                assert_eq!(tool, "Bash");
                assert_eq!(input["command"], "ls");
            }
            other => panic!("expected a decision request, got {other:?}"),
        }
        c.answer("perm-1", true, &mut out);
        let reply = serde_json::from_str::<Value>(out[1].send_text()).unwrap();
        assert_eq!(reply["id"], 8);
        assert_eq!(reply["result"], json!({"decision": "allow"}), "nothing but the decision");
    }

    /// Events become app events; the turn ends on the finished message, not on tool rounds.
    #[test]
    fn events_stream_and_the_turn_ends_once() {
        let mut c = core();
        c.session = Some("sess-1".into());
        c.phase = Phase::Ready;
        let mut out = Vec::new();
        c.line(json!({"method": "session/event", "params": {"payload": {"kind": "text_delta", "delta": "Hello"}}}).to_string().as_str(), &mut out);
        c.line(
            json!({"method": "session/event", "params": {"payload": {"kind": "tool_call", "toolCallId": "c1", "toolName": "Write", "input": {"a": 1}}}})
                .to_string()
                .as_str(),
            &mut out,
        );
        c.line(json!({"method": "session/event", "params": {"payload": {"kind": "result", "toolCallId": "c1", "toolName": "Write", "result": {"success": true, "content": "wrote it"}}}}).to_string().as_str(), &mut out);
        c.line(
            json!({"method": "session/event", "params": {"payload": {"stopReason": "tool-calls", "querySource": "main_turn", "content": "halfway"}}})
                .to_string()
                .as_str(),
            &mut out,
        );
        assert!(out.iter().all(|o| !matches!(o, Out::TurnEnded)), "a tool round is not the end");
        c.line(
            json!({"method": "session/event", "params": {"payload": {"stopReason": "stop", "querySource": "main_turn", "content": "done"}}})
                .to_string()
                .as_str(),
            &mut out,
        );
        assert!(matches!(out.last(), Some(Out::TurnEnded)));
        assert!(out.iter().any(|o| matches!(o, Out::Event(AgentEvent::TextDelta { text }) if text == "Hello")));
        assert!(out.iter().any(|o| matches!(o, Out::Event(AgentEvent::ToolStarted { tool, .. }) if tool == "Write")));
        assert!(out.iter().any(|o| matches!(o, Out::Event(AgentEvent::ToolFinished { ok: true, summary, .. }) if summary == "wrote it")));
    }

    /// A reply the protocol reports as an error fails the turn once, in words.
    #[test]
    fn an_error_reply_fails_the_turn() {
        let mut c = core();
        c.pending.insert(4, "send");
        let mut out = Vec::new();
        c.line(json!({"id": 4, "error": {"code": -32603, "message": "no model behind that name"}}).to_string().as_str(), &mut out);
        assert!(matches!(out.first(), Some(Out::Event(AgentEvent::Error { .. }))));
        assert!(matches!(out.last(), Some(Out::TurnEnded)));
        assert!(c.failure.is_some());
        assert_eq!(c.phase, Phase::Dead);
    }

    /// A resume the agent can't honour is flagged, so the caller starts fresh rather than blind.
    #[test]
    fn a_missing_resumed_session_is_flagged() {
        let mut c = ZcodeCore::new("Test", Path::new("/proj"), false, Some("sess-gone".into()), BRIDGE_MODEL.to_string());
        let mut out = c.start();
        c.line(json!({"method": "startup/storageState", "params": {"phase": "ready"}}).to_string().as_str(), &mut out);
        let resume = serde_json::from_str::<Value>(out[0].send_text()).unwrap();
        assert_eq!(resume["method"], "session/resume");
        assert_eq!(resume["params"]["sessionId"], "sess-gone");
        let mut out = Vec::new();
        c.line(json!({"id": resume["id"], "error": {"code": -32004, "message": "session unavailable"}}).to_string().as_str(), &mut out);
        assert!(c.session_missing, "the caller starts a new session");
        assert!(matches!(out.first(), Some(Out::Event(AgentEvent::Error { .. }))));
    }

    /// The provider config the agent reads is valid JSON of the shape the docs show, with the
    /// endpoint and key it was given — and the key never lands anywhere else.
    #[test]
    fn provider_config_carries_the_endpoint_and_key() {
        let v = provider_config_json(BRIDGE_PROVIDER, BRIDGE_MODEL, "http://127.0.0.1:9/v1", "test-key");
        assert_eq!(v["schemaVersion"], 1);
        let rule = &v["config"]["providerConfigRules"]["providerRules"][0];
        assert_eq!(rule["providerId"], BRIDGE_PROVIDER);
        assert_eq!(rule["config"]["api"]["baseUrl"], "http://127.0.0.1:9/v1");
        assert_eq!(rule["config"]["access"]["apiKey"], "test-key");
        let model = &v["config"]["modelConfigRules"]["providerModelRules"][0];
        assert_eq!(model["modelId"], BRIDGE_MODEL);
        assert_eq!(model["config"]["properties"]["supportsToolCall"], true);
        assert_eq!(model["config"]["properties"]["inputFormat"]["supportsText"], true);
        assert_eq!(v["config"]["defaultModelSelection"]["modelId"], BRIDGE_MODEL);
    }

    impl Out {
        fn send_text(&self) -> &str {
            match self {
                Out::Send(s) => s,
                other => panic!("expected a line to send, got {other:?}"),
            }
        }
    }

    use crate::BackendKind;

    /// The whole agent, live: the bundled `zcode.cjs` (found through `CRAFT_ANYTHING_ZCODE_DIR`)
    /// served by the local bridge when this machine has Z.ai credentials. Runs a one-message
    /// conversation and expects words back. Off unless `CRAFT_ANYTHING_ZCODE_LIVE=1`.
    #[test]
    fn live_conversation_through_the_bridge() {
        if std::env::var("CRAFT_ANYTHING_ZCODE_LIVE").ok().as_deref() != Some("1") {
            return;
        }
        assert!(assets().is_some(), "point CRAFT_ANYTHING_ZCODE_DIR at the bundled agent");
        // The app's own config dir comes from the environment (`CRAFT_ANYTHING_CONFIG_DIR`), set
        // by whoever runs this: a scratch folder away from any installed ZCode, with
        // `CRAFT_ANYTHING_NO_KEYCHAIN=1` so the key file is used instead of a keychain.
        let workspace = tempfile::tempdir().unwrap();
        let mut spec = CliSpec::new(
            BackendConfig { name: "ZCode Test".into(), kind: BackendKind::AgentCli, model: AGENT_ID.into(), base_url: None, ssh_host: None, enabled: true },
            BackendOptions::default(),
            false,
            Some(workspace.path().to_path_buf()),
        );
        spec.preamble = Some(String::new());
        let mut c = ZcodeConversation::new(&spec, None);
        c.send("Reply with exactly the word: ready");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(180);
        let mut text = String::new();
        let mut done = false;
        while std::time::Instant::now() < deadline {
            for event in c.poll() {
                match event {
                    AgentEvent::TextDelta { text: d } => text.push_str(&d),
                    AgentEvent::TurnDone { .. } => done = true,
                    AgentEvent::Error { message, .. } => panic!("the agent reported: {message}"),
                    _ => {}
                }
            }
            if done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        assert!(done, "the turn never finished; got: {text}");
        assert!(!text.trim().is_empty(), "the agent said nothing");
        assert!(c.session_id().is_some(), "the session is named, so it can continue");
    }
}
