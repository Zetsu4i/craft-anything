//! The E2B cloud sandbox: a per-project Linux box in the cloud a coding agent can build in, with
//! all its usual tools, instead of this computer. Lifecycle through E2B's REST API; files and
//! processes through the sandbox's own gateway with the access token the API hands out. The API
//! key is the person's own (Settings → AI → Sandbox); it is never logged or written to a project.
//!
//! Everything here is plain HTTPS, so no SDK is involved. Verified against the live API: create
//! (`POST /v2/sandboxes`, `timeout` in seconds), resume-or-attach (`POST /v2/sandboxes/{id}/connect`),
//! pause, timeout, kill, files (`GET/POST /files?path=…`) and processes (`/process.Process/*`,
//! connect-RPC: one 5-byte-framed JSON message per side for the streaming calls, plain JSON for
//! unary ones). `autoPause` + `autoResume` are E2B's own restore-on-timeout: a sandbox nobody
//! uses is snapshotted, and the next request to it brings it back.

use base64::Engine as _;
use craft_anything_evidence::{ErrorCode, ToolError};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

/// The port the sandbox's own API (envd) answers on, behind E2B's public gateway.
pub const ENVD_PORT: u16 = 49983;
/// The default E2B template: Debian with Node, Python, git and the usual build tools.
pub const DEFAULT_TEMPLATE: &str = "base";
/// Where a project's synced copy lives inside every sandbox.
pub const REMOTE_ROOT: &str = "/craft-anything";
/// Where the sandbox's copy of our CLI tool server lives.
pub const REMOTE_BIN: &str = "/craft-anything/bin";
/// The default sandbox timeout (the idle window E2B applies on its side).
pub const DEFAULT_TIMEOUT_SECS: u64 = 30 * 60;
/// The API base of E2B's control plane for `domain`.
pub fn api_base(domain: &str) -> String {
    format!("https://api.{domain}")
}
/// The public gateway that routes to a sandbox's own API for `domain`.
pub fn gateway_base(domain: &str) -> String {
    format!("https://sandbox.{domain}")
}

/// What an error from E2B said, as our error contract.
fn e2b_error(what: &str, status: u16, body: &str) -> ToolError {
    let code = match status {
        401 | 403 => ErrorCode::NoPermission,
        404 => ErrorCode::EngineMissing,
        408 | 504 => ErrorCode::TimedOut,
        _ => ErrorCode::EngineMissing,
    };
    let detail = body.trim().chars().take(300).collect::<String>();
    ToolError::new(code, format!("E2B {what} answered {status}: {detail}"))
        .with_remediation("Check the E2B API key in Settings → AI → Sandbox, and your connection.")
}

fn agent(global: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent("CraftAnything-dev")
        .timeout_connect(Some(Duration::from_secs(30)))
        .timeout_global(Some(global))
        .build()
        .new_agent()
}

/// A sandbox E2B is holding for a project: its id and the token its own API wants.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SandboxInfo {
    pub sandbox_id: String,
    pub envd_token: String,
}

/// A sandbox's state as E2B's API reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxState {
    Running,
    Paused,
    Gone,
}

/// The client for one person's E2B account. Cheap to clone; every call is blocking, so keep it off
/// the UI thread.
#[derive(Clone)]
pub struct E2B {
    /// The person's own API key (Settings → AI → Sandbox, or `E2B_API_KEY`).
    pub key: String,
    pub domain: String,
}

impl E2B {
    pub fn new(key: impl Into<String>, domain: impl Into<String>) -> Self {
        E2B { key: key.into(), domain: domain.into() }
    }

    fn api(&self, method: &str, path: &str, body: Option<Value>, timeout: Duration) -> Result<(u16, String), ToolError> {
        let url = format!("{}{path}", api_base(&self.domain));
        let agent = agent(timeout);
        let sent = match (method, &body) {
            ("GET", _) => agent.get(&url).header("X-API-KEY", &self.key).call(),
            ("DELETE", _) => agent.delete(&url).header("X-API-KEY", &self.key).call(),
            (_, Some(b)) => agent.post(&url).header("X-API-KEY", &self.key).header("Content-Type", "application/json").send(b.to_string()),
            _ => agent.post(&url).header("X-API-KEY", &self.key).send_empty(),
        };
        let mut resp = sent.map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't reach E2B ({what}): {e}", what = path)))?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        Ok((status, text))
    }

    fn parse_info(&self, what: &str, status: u16, text: &str) -> Result<SandboxInfo, ToolError> {
        let v: Value = serde_json::from_str(text).map_err(|e| e2b_error(what, status, &format!("{text} ({e})")))?;
        let id = v.get("sandboxID").and_then(Value::as_str).unwrap_or_default().to_string();
        let token = v.get("envdAccessToken").and_then(Value::as_str).unwrap_or_default().to_string();
        if id.is_empty() || token.is_empty() {
            return Err(e2b_error(what, status, text));
        }
        Ok(SandboxInfo { sandbox_id: id, envd_token: token })
    }

    /// Start a new sandbox. `auto_pause` snapshots it instead of killing it when its timeout runs
    /// out; `auto_resume` brings it back on the next request, so a paused project's next turn
    /// simply continues.
    pub fn create(&self, template: &str, timeout_secs: u64, auto_pause: bool, auto_resume: bool) -> Result<SandboxInfo, ToolError> {
        let body = json!({
            "templateID": template,
            "timeout": timeout_secs,
            "autoPause": auto_pause,
            "autoResume": { "enabled": auto_resume },
        });
        let (status, text) = self.api("POST", "/v2/sandboxes", Some(body), Duration::from_secs(120))?;
        if status != 200 && status != 201 {
            return Err(e2b_error("starting a sandbox", status, &text));
        }
        self.parse_info("starting a sandbox", status, &text)
    }

    /// Attach to a running sandbox, or resume a paused one. This is how a project's sandbox is
    /// brought back after a pause or a timeout.
    pub fn connect(&self, sandbox_id: &str, timeout_secs: u64) -> Result<SandboxInfo, ToolError> {
        let body = json!({ "timeout": timeout_secs });
        let (status, text) = self.api("POST", &format!("/v2/sandboxes/{sandbox_id}/connect"), Some(body), Duration::from_secs(180))?;
        if status != 200 && status != 201 {
            return Err(e2b_error("bringing the sandbox back", status, &text));
        }
        self.parse_info("bringing the sandbox back", status, &text)
    }

    /// Where the sandbox is: running, paused (its files are kept and it can be brought back), or
    /// gone (killed, or it never existed).
    pub fn state(&self, sandbox_id: &str) -> Result<SandboxState, ToolError> {
        let (status, text) = self.api("GET", &format!("/sandboxes/{sandbox_id}"), None, Duration::from_secs(30))?;
        match status {
            200 => {
                let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                // The API reports a paused sandbox in its own list; a `state` field says which.
                let running = v.get("state").and_then(Value::as_str).map(|s| s != "paused").unwrap_or(true);
                Ok(if running { SandboxState::Running } else { SandboxState::Paused })
            }
            404 => Ok(SandboxState::Gone),
            _ => Err(e2b_error("checking the sandbox", status, &text)),
        }
    }

    /// Pause: E2B snapshots the sandbox's disk (and memory) and stops it. Nothing is lost; the
    /// next `connect` brings it back.
    pub fn pause(&self, sandbox_id: &str) -> Result<(), ToolError> {
        let (status, text) = self.api("POST", &format!("/sandboxes/{sandbox_id}/pause"), Some(json!({})), Duration::from_secs(180))?;
        if status == 204 || status == 200 { Ok(()) } else { Err(e2b_error("pausing the sandbox", status, &text)) }
    }

    /// Stop the sandbox for good; its disk is deleted.
    pub fn kill(&self, sandbox_id: &str) -> Result<bool, ToolError> {
        let (status, text) = self.api("DELETE", &format!("/sandboxes/{sandbox_id}"), None, Duration::from_secs(60))?;
        match status {
            204 | 200 => Ok(true),
            404 => Ok(false),
            _ => Err(e2b_error("stopping the sandbox", status, &text)),
        }
    }

    /// Extend the sandbox's idle timeout from now.
    pub fn set_timeout(&self, sandbox_id: &str, timeout_secs: u64) -> Result<(), ToolError> {
        let (status, text) =
            self.api("POST", &format!("/sandboxes/{sandbox_id}/timeout"), Some(json!({ "timeout": timeout_secs })), Duration::from_secs(30))?;
        if status == 204 || status == 200 { Ok(()) } else { Err(e2b_error("setting the sandbox's timeout", status, &text)) }
    }

    /// Whether the key opens E2B at all (the settings panel's "Test").
    pub fn check_key(&self) -> Result<(), ToolError> {
        let (status, text) = self.api("GET", "/sandboxes?status=running", None, Duration::from_secs(30))?;
        if status == 200 { Ok(()) } else { Err(e2b_error("checking the API key", status, &text)) }
    }

    /// How many sandboxes are running on this account right now.
    pub fn running_count(&self) -> Result<usize, ToolError> {
        let (status, text) = self.api("GET", "/sandboxes?status=running", None, Duration::from_secs(30))?;
        if status != 200 {
            return Err(e2b_error("listing the sandboxes", status, &text));
        }
        Ok(serde_json::from_str::<Value>(&text).ok().and_then(|v| v.as_array().map(|a| a.len())).unwrap_or(0))
    }
}

/// One sandbox's own API (files, processes), reached through E2B's public gateway with the access
/// token from [`E2B::create`] or [`E2B::connect`].
#[derive(Clone)]
pub struct Envd {
    pub info: SandboxInfo,
    pub domain: String,
}

impl Envd {
    pub fn new(info: SandboxInfo, domain: impl Into<String>) -> Self {
        Envd { info, domain: domain.into() }
    }

    /// The sandbox's routing and access headers, on every request to its own API.
    fn headers(&self) -> [(&'static str, String); 3] {
        [("E2b-Sandbox-Id", self.info.sandbox_id.clone()), ("E2b-Sandbox-Port", ENVD_PORT.to_string()), ("X-Access-Token", self.info.envd_token.clone())]
    }

    /// Health: whether the sandbox answers at all (useful right after a resume).
    pub fn health(&self) -> Result<bool, ToolError> {
        let url = format!("{}/health", gateway_base(&self.domain));
        let mut r = agent(Duration::from_secs(20)).get(&url);
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let resp = r.call().map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't reach the sandbox: {e}")))?;
        Ok(resp.status().as_u16() / 100 == 2)
    }

    /// Write a file (any missing parent folders are made). Returns its path as the sandbox saw it.
    pub fn write_file(&self, path: &str, bytes: &[u8]) -> Result<(), ToolError> {
        let url = format!("{}{}path={}", gateway_base(&self.domain), "/files?", urlencode(path));
        let mut r = agent(upload_timeout(bytes.len())).post(&url).header("Content-Type", "application/octet-stream");
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let mut resp = r.send(bytes).map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't upload {path}: {e}")))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(e2b_error(&format!("uploading {path}"), status, &text));
        }
        Ok(())
    }

    /// Read a file. `None` when it isn't there.
    pub fn read_file(&self, path: &str) -> Result<Option<Vec<u8>>, ToolError> {
        let url = format!("{}{}path={}", gateway_base(&self.domain), "/files?", urlencode(path));
        let mut r = agent(Duration::from_secs(120)).get(&url);
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let mut resp = r.call().map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't download {path}: {e}")))?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Ok(None);
        }
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(e2b_error(&format!("downloading {path}"), status, &text));
        }
        let mut bytes = Vec::new();
        resp.into_body().into_reader().read_to_end(&mut bytes).map_err(|e| ToolError::io(path, e))?;
        Ok(Some(bytes))
    }

    /// Delete a file. `Ok(false)` when it wasn't there.
    pub fn remove_file(&self, path: &str) -> Result<bool, ToolError> {
        let url = format!("{}{}path={}", gateway_base(&self.domain), "/files?", urlencode(path));
        let mut r = agent(Duration::from_secs(30)).delete(&url);
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let mut resp = r.call().map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't remove {path}: {e}")))?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Ok(false);
        }
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(e2b_error(&format!("removing {path}"), status, &text));
        }
        Ok(true)
    }

    /// Start a process inside the sandbox. Its output arrives as [`ProcEvent`]s on the returned
    /// channel until it ends. Write to its stdin with [`RemoteProcess::write_input`]; a one-shot
    /// prompt that reads to the end gets [`RemoteProcess::close_input`] after its last write.
    pub fn start_process(&self, program: &str, args: &[String], env: &BTreeMap<String, String>, cwd: &str) -> Result<RemoteProcess, ToolError> {
        let mut process = json!({ "cmd": program, "args": args });
        if !env.is_empty() {
            process["env"] = json!(env);
        }
        if !cwd.is_empty() {
            process["cwd"] = json!(cwd);
        }
        let payload = encode_frame(&json!({ "process": process }));
        let mut r = agent(Duration::from_secs(24 * 3600))
            .post(&format!("{}{}", gateway_base(&self.domain), "/process.Process/Start"))
            .header("Content-Type", "application/connect+json");
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let mut resp = r.send(payload).map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't start {program} in the sandbox: {e}")))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(e2b_error(&format!("starting {program} in the sandbox"), status, &text));
        }
        // The answer is a stream of framed events; read it on its own thread so a quiet process
        // never holds up the caller, and events flow as they come.
        let reader = resp.into_body().into_reader();
        let (tx, rx) = std::sync::mpsc::channel::<ProcEvent>();
        std::thread::Builder::new()
            .name("e2b-proc".into())
            .spawn(move || read_events(reader, &tx))
            .map_err(|e| ToolError::internal(format!("Couldn't start reading the sandbox's output: {e}")))?;
        let remote = RemoteProcess { events: rx, pid: None, envd: self.clone() };
        Ok(remote)
    }

    /// Close a process's stdin, so a one-shot program that reads to the end runs and exits.
    pub fn close_stdin(&self, pid: u32) -> Result<(), ToolError> {
        let body = json!({ "process": { "pid": pid } });
        let mut r = agent(Duration::from_secs(30))
            .post(&format!("{}{}", gateway_base(&self.domain), "/process.Process/CloseStdin"))
            .header("Content-Type", "application/json");
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let mut resp = r.send(body.to_string()).map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't close the process's input: {e}")))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(e2b_error("closing the process's input", status, &text));
        }
        Ok(())
    }

    /// Write to a process's stdin (ordering kept: each call is its own request).
    pub fn send_input(&self, pid: u32, bytes: &[u8]) -> Result<(), ToolError> {
        let body = json!({ "process": { "pid": pid }, "input": { "stdin": b64(bytes) } });
        let mut r = agent(Duration::from_secs(30))
            .post(&format!("{}{}", gateway_base(&self.domain), "/process.Process/SendInput"))
            .header("Content-Type", "application/json");
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let mut resp = r.send(body.to_string()).map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't write to the process: {e}")))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(e2b_error("writing to the process", status, &text));
        }
        Ok(())
    }

    /// Signal a process (`SIGTERM`, then `SIGKILL` if it stays).
    pub fn send_signal(&self, pid: u32, signal: &str) -> Result<(), ToolError> {
        let body = json!({ "process": { "pid": pid }, "signal": signal });
        let mut r = agent(Duration::from_secs(30))
            .post(&format!("{}{}", gateway_base(&self.domain), "/process.Process/SendSignal"))
            .header("Content-Type", "application/json");
        for (k, v) in self.headers() {
            r = r.header(k, v);
        }
        let mut resp = r.send(body.to_string()).map_err(|e| ToolError::new(ErrorCode::TimedOut, format!("Couldn't signal the process: {e}")))?;
        let status = resp.status().as_u16();
        if !(200..300).contains(&status) {
            let text = resp.body_mut().read_to_string().unwrap_or_default();
            return Err(e2b_error("signalling the process", status, &text));
        }
        Ok(())
    }
}

fn upload_timeout(len: usize) -> Duration {
    // A slow connection uploads roughly a megabyte a second at worst; give every megabyte a
    // second, at least a minute.
    Duration::from_secs((len as u64 / (1024 * 1024)).saturating_add(60))
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => out.push(b as char),
            other => out.push_str(&format!("%{other:02X}")),
        }
    }
    out
}

fn b64(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.encode(bytes)
}

fn un64(s: &str) -> Vec<u8> {
    use base64::engine::general_purpose::STANDARD;
    STANDARD.decode(s).unwrap_or_default()
}

/// One framed connect-RPC message: a flag byte, a 4-byte big-endian length, then the JSON.
fn encode_frame(v: &Value) -> Vec<u8> {
    let payload = v.to_string().into_bytes();
    let mut out = Vec::with_capacity(payload.len() + 5);
    out.push(0);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    out
}

/// One event from a process started in the sandbox.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcEvent {
    /// The process named itself.
    Started(u32),
    /// Bytes from its stdout, or its stderr.
    Stdout(Vec<u8>),
    Stderr(Vec<u8>),
    /// It ended: the exit code when it exited by itself.
    Ended(Option<i32>),
}

/// Read the framed event stream to the end, forwarding events; ends when the sandbox ends the
/// stream (the process exited, or we signalled it).
fn read_events<R: Read>(mut reader: R, tx: &std::sync::mpsc::Sender<ProcEvent>) {
    let mut header = [0u8; 5];
    loop {
        if read_exact_or_eof(&mut reader, &mut header).is_none() {
            return;
        }
        let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]) as usize;
        // A nonsense length would try to allocate forever; the sandbox frames small JSON here.
        if len > 64 * 1024 * 1024 {
            return;
        }
        let mut payload = vec![0u8; len];
        if read_exact_or_eof(&mut reader, &mut payload).is_none() {
            return;
        }
        let Ok(v) = serde_json::from_slice::<Value>(&payload) else { continue };
        // Flag 0x02 is connect's end-of-stream marker (metadata only); 0 is an event.
        if header[0] != 0 {
            continue;
        }
        for event in decode_event(&v) {
            if tx.send(event).is_err() {
                return;
            }
        }
    }
}

/// `read_exact` that says "ended" instead of erroring on a half frame.
fn read_exact_or_eof<R: Read>(reader: &mut R, buf: &mut [u8]) -> Option<()> {
    let mut filled = 0;
    while filled < buf.len() {
        match reader.read(&mut buf[filled..]) {
            Ok(0) => return None,
            Ok(n) => filled += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => return None,
        }
    }
    Some(())
}

fn decode_event(v: &Value) -> Vec<ProcEvent> {
    // Events arrive wrapped: {"event": {"start": …}}.
    let v = v.get("event").unwrap_or(v);
    let mut out = Vec::new();
    if let Some(pid) = v.get("start").and_then(|s| s.get("pid")).and_then(Value::as_u64) {
        out.push(ProcEvent::Started(pid.min(u32::MAX as u64) as u32));
    }
    if let Some(data) = v.get("data") {
        if let Some(s) = data.get("stdout").and_then(Value::as_str) {
            out.push(ProcEvent::Stdout(un64(s)));
        }
        if let Some(s) = data.get("stderr").and_then(Value::as_str) {
            out.push(ProcEvent::Stderr(un64(s)));
        }
    }
    if let Some(end) = v.get("end") {
        out.push(ProcEvent::Ended(exit_code(end)));
    }
    out
}

/// The end event's exit code: an explicit `exitCode`, else the one inside `status`
/// ("exit status 0" — protojson leaves a 0 `exitCode` out), else nothing.
fn exit_code(end: &Value) -> Option<i32> {
    if let Some(code) = end.get("exitCode").and_then(Value::as_i64) {
        return Some(code.min(i32::MAX as i64) as i32);
    }
    let status = end.get("status").and_then(Value::as_str).unwrap_or_default();
    status.strip_prefix("exit status ").and_then(|n| n.parse::<i32>().ok()).or_else(|| end.get("exited").and_then(Value::as_bool).filter(|e| *e).map(|_| 0))
}

/// A process running inside the sandbox: its id, and where its output comes from. The id is
/// learned from the first event; [`RemoteProcess::pid`] waits for it.
pub struct RemoteProcess {
    events: Receiver<ProcEvent>,
    pid: Option<u32>,
    envd: Envd,
}

impl RemoteProcess {
    /// The process's id, once the sandbox reported it (kept for later writes).
    pub fn pid(&mut self, wait: Duration) -> Option<u32> {
        if let Some(pid) = self.pid {
            return Some(pid);
        }
        let deadline = Instant::now() + wait;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            match self.events.recv_timeout(left.min(Duration::from_millis(100))) {
                Ok(ProcEvent::Started(pid)) => {
                    self.pid = Some(pid);
                    return Some(pid);
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return None,
            }
        }
    }

    /// Write to the process's stdin (ordering kept: each call is its own request).
    pub fn write_input(&self, bytes: &[u8]) -> Result<(), ToolError> {
        match self.pid {
            Some(pid) => self.envd.send_input(pid, bytes),
            None => Err(ToolError::new(ErrorCode::EngineMissing, "The sandbox process never named itself, so nothing can be written to it.")),
        }
    }

    /// Close the process's stdin, so a one-shot program that reads to the end runs and exits.
    pub fn close_input(&self) -> Result<(), ToolError> {
        match self.pid {
            Some(pid) => self.envd.close_stdin(pid),
            None => Err(ToolError::new(ErrorCode::EngineMissing, "The sandbox process never named itself, so its input can't be closed.")),
        }
    }

    /// The next event, waiting at most `wait`.
    pub fn recv_timeout(&self, wait: Duration) -> Result<ProcEvent, std::sync::mpsc::RecvTimeoutError> {
        self.events.recv_timeout(wait)
    }
}

/// What a process's output lines go to.
type LineSink = Box<dyn FnMut(&str) + Send>;

/// A builder-shaped run of a program in the sandbox, matching the surface of
/// `craft_anything_platform::process::Process` that the agent runner uses: the same callbacks, the
/// same wall-clock and idle deadlines, the same cancellation, the same [`Output`].
pub struct SandboxProcess {
    envd: Envd,
    program: String,
    args: Vec<String>,
    env: BTreeMap<String, String>,
    cwd: String,
    stdin: Option<Vec<u8>>,
    timeout: Option<Duration>,
    idle_timeout: Option<Duration>,
    cancel: Option<Box<dyn Fn() -> bool + Send>>,
    on_stdout: Option<LineSink>,
    on_stderr: Option<LineSink>,
}

/// What a run came back with: the same shape as the platform's own `Output`.
#[derive(Debug)]
pub struct Output {
    /// Exit code, when the process exited by itself.
    pub code: Option<i32>,
    pub success: bool,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SandboxProcess {
    pub fn new(envd: Envd, program: impl Into<String>) -> Self {
        SandboxProcess {
            envd,
            program: program.into(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: String::new(),
            stdin: None,
            timeout: None,
            idle_timeout: None,
            cancel: None,
            on_stdout: None,
            on_stderr: None,
        }
    }

    pub fn args(mut self, args: &[String]) -> Self {
        self.args = args.to_vec();
        self
    }

    pub fn env(mut self, key: &str, value: &str) -> Self {
        self.env.insert(key.to_string(), value.to_string());
        self
    }

    pub fn cwd(mut self, dir: &str) -> Self {
        self.cwd = dir.to_string();
        self
    }

    pub fn stdin_bytes(mut self, bytes: Vec<u8>) -> Self {
        self.stdin = Some(bytes);
        self
    }

    pub fn timeout(mut self, limit: Duration) -> Self {
        self.timeout = Some(limit);
        self
    }

    pub fn idle_timeout(mut self, limit: Duration) -> Self {
        self.idle_timeout = Some(limit);
        self
    }

    pub fn cancel_when(mut self, f: impl Fn() -> bool + Send + 'static) -> Self {
        self.cancel = Some(Box::new(f));
        self
    }

    pub fn on_stdout_line(mut self, f: impl FnMut(&str) + Send + 'static) -> Self {
        self.on_stdout = Some(Box::new(f));
        self
    }

    pub fn on_stderr_line(mut self, f: impl FnMut(&str) + Send + 'static) -> Self {
        self.on_stderr = Some(Box::new(f));
        self
    }

    /// Run it to the end: the process exits, a deadline runs out, or the run is cancelled.
    pub fn run(mut self) -> Result<Output, ToolError> {
        let started = Instant::now();
        let mut proc = self.envd.start_process(&self.program, &self.args, &self.env, &self.cwd)?;
        // Name it, hand it the prompt, then close its input so a one-shot run finishes on its own.
        let pid = proc.pid(Duration::from_secs(30));
        if let Some(input) = self.stdin.as_deref() {
            if pid.is_some() {
                proc.write_input(input)?;
            }
            proc.close_input()?;
        }
        let mut last_activity = Instant::now();
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut partial_out = String::new();
        let mut partial_err = String::new();
        let mut ended: Option<Option<i32>> = None;
        let mut stop: Option<ToolError> = None;
        let mut signalled_at: Option<Instant> = None;
        let limit = self.timeout.unwrap_or(Duration::from_secs(24 * 3600));
        let idle = self.idle_timeout.unwrap_or(Duration::from_secs(24 * 3600));
        loop {
            // Stop conditions, checked between events: cancelled, wall clock, or quiet too long.
            if stop.is_none()
                && let Some(f) = &self.cancel
                && f()
            {
                stop = Some(ToolError::cancelled());
            }
            if stop.is_none() && last_activity.elapsed() > idle {
                stop = Some(ToolError::new(ErrorCode::TimedOut, "The run in the sandbox went quiet for too long."));
            }
            if stop.is_none() && started.elapsed() > limit {
                stop = Some(ToolError::new(ErrorCode::TimedOut, "The run in the sandbox ran out of time."));
            }
            // Ask first, then kill the hard way once a short grace has passed.
            if let (Some(_), Some(at)) = (&stop, signalled_at)
                && at.elapsed() > Duration::from_secs(5)
                && let Some(pid) = pid
            {
                let _ = self.envd.send_signal(pid, "SIGNAL_SIGKILL");
                signalled_at = Some(Instant::now() + Duration::from_secs(3600));
            }
            if stop.is_some() && signalled_at.is_none() {
                signalled_at = Some(Instant::now());
                if let Some(pid) = pid {
                    let _ = self.envd.send_signal(pid, "SIGNAL_SIGTERM");
                }
            }
            match proc.recv_timeout(Duration::from_millis(120)) {
                Ok(ProcEvent::Started(_)) => last_activity = Instant::now(),
                Ok(ProcEvent::Stdout(bytes)) => {
                    last_activity = Instant::now();
                    stdout.extend_from_slice(&bytes);
                    feed_lines(&bytes, &mut partial_out, &mut self.on_stdout);
                }
                Ok(ProcEvent::Stderr(bytes)) => {
                    last_activity = Instant::now();
                    stderr.extend_from_slice(&bytes);
                    feed_lines(&bytes, &mut partial_err, &mut self.on_stderr);
                }
                Ok(ProcEvent::Ended(code)) => {
                    ended = Some(code);
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
            }
            if ended.is_some() {
                break;
            }
        }
        if let Some(e) = stop {
            if let Some(pid) = proc.pid(Duration::from_secs(1)) {
                let _ = self.envd.send_signal(pid, "SIGNAL_SIGKILL");
            }
            return Err(e);
        }
        if ended.is_none() {
            // The stream ended without an `end` event (the sandbox died under us).
            return Err(ToolError::new(ErrorCode::EngineMissing, "The sandbox stopped answering while the process was running."));
        }
        flush_lines(&mut partial_out, &mut self.on_stdout);
        flush_lines(&mut partial_err, &mut self.on_stderr);
        let code = ended.unwrap_or(None);
        Ok(Output { code, success: code.is_some_and(|c| c == 0), stdout, stderr })
    }
}

/// Feed whole lines to `f`, keeping the unfinished tail in `partial`.
fn feed_lines(bytes: &[u8], partial: &mut String, f: &mut Option<LineSink>) {
    if f.is_none() && bytes.is_empty() {
        return;
    }
    partial.push_str(&String::from_utf8_lossy(bytes));
    while let Some(pos) = partial.find('\n') {
        let line: String = partial.drain(..=pos).collect();
        let line = line.trim_end_matches('\n').trim_end_matches('\r');
        if let Some(f) = f {
            f(line);
        }
    }
}

fn flush_lines(partial: &mut String, f: &mut Option<LineSink>) {
    if !partial.is_empty()
        && let Some(f) = f
    {
        f(partial.as_str());
    }
    partial.clear();
}

/// One file in a sync manifest: its size and modification time.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SyncEntry {
    pub size: u64,
    /// Milliseconds since the Unix epoch.
    pub mtime: u64,
}

/// The sync's tally, for a status line in the app.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub uploaded: usize,
    pub downloaded: usize,
    pub removed: usize,
    pub skipped: usize,
    pub bytes: u64,
}

impl SyncSummary {
    pub fn describe(&self, up: bool) -> String {
        let (moved, word) = if up { (self.uploaded, "to") } else { (self.downloaded, "from") };
        match (moved, self.removed) {
            (0, 0) => "everything already matches".to_string(),
            (n, 0) => format!("{n} file{} {word} the sandbox", if n == 1 { "" } else { "s" }),
            (0, n) => format!("{n} file{} cleaned up", if n == 1 { "" } else { "s" }),
            (n, m) => format!("{n} file{} {word} the sandbox, {m} cleaned up", if n == 1 { "" } else { "s" }),
        }
    }
}

/// Folders and files that never travel: they are rebuilt inside the sandbox (or are not wanted
/// there), and they never come back either.
pub const DEFAULT_IGNORES: &[&str] =
    &[".git", "node_modules", "__pycache__", ".venv", "venv", "target", ".gradle", ".cache", ".next", ".nuxt", "dist-cache", ".pnpm-store"];

/// Whether a relative path (slash-separated) is skipped by the ignore list. A pattern without a
/// slash matches any part of the path (like .gitignore: `node_modules` skips the whole tree,
/// `secrets.json` every file of that name); a pattern with a slash matches that exact path.
pub fn ignored(rel: &str, extra: &[String]) -> bool {
    rel.split('/').any(|part| DEFAULT_IGNORES.contains(&part) || extra.iter().any(|i| !i.contains('/') && i == part))
        || extra.iter().any(|i| i.contains('/') && i == rel)
}

/// Read the local tree under `root` (relative, slash-separated paths → size and mtime).
pub fn local_manifest(root: &Path, extra_ignores: &[String]) -> BTreeMap<String, SyncEntry> {
    let mut out = BTreeMap::new();
    walk_local(root, root, &mut out, extra_ignores, 0);
    out
}

fn walk_local(root: &Path, dir: &Path, out: &mut BTreeMap<String, SyncEntry>, extra: &[String], depth: u8) {
    if depth > 32 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(rel) = path.strip_prefix(root) else { continue };
        let rel = rel.to_string_lossy().replace('\\', "/");
        if ignored(&rel, extra) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if meta.is_dir() {
            walk_local(root, &path, out, extra, depth + 1);
        } else if meta.is_file() {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_millis().min(u64::MAX as u128) as u64)
                .unwrap_or(0);
            out.insert(rel, SyncEntry { size: meta.len(), mtime });
        }
    }
}

/// The sandbox's manifest, from one `find` inside it (fast, exact, and it sees what the agent did).
pub fn remote_manifest(envd: &Envd, root: &str, extra_ignores: &[String]) -> Result<BTreeMap<String, SyncEntry>, ToolError> {
    let mut find = format!("cd \"{root}\" 2>/dev/null || exit 7; find . -type f -printf '%p\\t%s\\t%T@\\n' 2>/dev/null | sort");
    for dir in DEFAULT_IGNORES.iter().copied().chain(extra_ignores.iter().map(String::as_str)) {
        find.push_str(&format!(" | grep -v -E '(^|/){dir}(/|$)'", dir = shell_safe(dir)));
    }
    let proc = envd.start_process("bash", &["-c".to_string(), find], &BTreeMap::new(), "")?;
    let mut out = String::new();
    let mut stderr = String::new();
    loop {
        match proc.recv_timeout(Duration::from_secs(60)) {
            Ok(ProcEvent::Stdout(b)) => out.push_str(&String::from_utf8_lossy(&b)),
            Ok(ProcEvent::Stderr(b)) => stderr.push_str(&String::from_utf8_lossy(&b)),
            Ok(ProcEvent::Ended(code)) => {
                if code == Some(7) {
                    return Ok(BTreeMap::new());
                }
                break;
            }
            Ok(ProcEvent::Started(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Err(ToolError::new(ErrorCode::TimedOut, "Listing the sandbox's files took too long.")),
            Err(_) => break,
        }
    }
    if !stderr.trim().is_empty() {
        log::warn!("the sandbox's file listing had something to say: {}", stderr.trim());
    }
    let mut map = BTreeMap::new();
    for line in out.lines() {
        let mut parts = line.splitn(3, '\t');
        let (Some(path), Some(size), Some(mtime)) = (parts.next(), parts.next(), parts.next()) else { continue };
        let rel = path.trim_start_matches("./");
        if rel.is_empty() {
            continue;
        }
        let Ok(size) = size.parse::<u64>() else { continue };
        let mtime = mtime.split('.').next().and_then(|s| s.parse::<u64>().ok()).unwrap_or(0) * 1000;
        map.insert(rel.to_string(), SyncEntry { size, mtime });
    }
    Ok(map)
}

fn shell_safe(s: &str) -> String {
    s.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-' || *c == '.').collect()
}

/// Copy the project to the sandbox: only what changed travels, folders the ignore list names
/// never do. `progress` gets a line now and then, for the app's status.
pub fn sync_up(
    envd: &Envd,
    local_root: &Path,
    remote_root: &str,
    extra_ignores: &[String],
    mut progress: impl FnMut(String),
) -> Result<SyncSummary, ToolError> {
    progress("Comparing the project with the sandbox…".into());
    let local = local_manifest(local_root, extra_ignores);
    let remote = remote_manifest(envd, remote_root, extra_ignores)?;
    let mut summary = SyncSummary::default();
    for (rel, entry) in &local {
        match remote.get(rel) {
            Some(r) if r.size == entry.size && r.mtime / 1000 == entry.mtime / 1000 => {
                summary.skipped += 1;
            }
            _ => {
                let bytes = std::fs::read(local_root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR))).map_err(|e| ToolError::io(rel, e))?;
                let remote_path = format!("{remote_root}/{rel}");
                envd.write_file(&remote_path, &bytes)?;
                summary.uploaded += 1;
                summary.bytes += bytes.len() as u64;
                if summary.uploaded % 25 == 1 {
                    progress(format!("Sending the project to the sandbox… {} files so far", summary.uploaded));
                }
            }
        }
    }
    // Files the project no longer has, the sandbox shouldn't keep either.
    for rel in remote.keys() {
        if !local.contains_key(rel) {
            let _ = envd.remove_file(&format!("{remote_root}/{rel}"));
            summary.removed += 1;
        }
    }
    Ok(summary)
}

/// Copy the sandbox's project back: what the agent built travels to this computer, minus the
/// folders that stay in the sandbox.
pub fn sync_down(
    envd: &Envd,
    local_root: &Path,
    remote_root: &str,
    extra_ignores: &[String],
    mut progress: impl FnMut(String),
) -> Result<SyncSummary, ToolError> {
    progress("Collecting what the sandbox built…".into());
    let local = local_manifest(local_root, extra_ignores);
    let remote = remote_manifest(envd, remote_root, extra_ignores)?;
    let mut summary = SyncSummary::default();
    for (rel, entry) in &remote {
        match local.get(rel) {
            Some(l) if l.size == entry.size && l.mtime / 1000 == entry.mtime / 1000 => {
                summary.skipped += 1;
            }
            _ => {
                let remote_path = format!("{remote_root}/{rel}");
                let Some(bytes) = envd.read_file(&remote_path)? else { continue };
                let path = local_root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR));
                if let Some(parent) = path.parent()
                    && std::fs::create_dir_all(parent).is_err()
                {
                    continue;
                }
                std::fs::write(&path, &bytes).map_err(|e| ToolError::io(rel, e))?;
                summary.downloaded += 1;
                summary.bytes += bytes.len() as u64;
                if summary.downloaded % 25 == 1 {
                    progress(format!("Copying the sandbox's work back… {} files so far", summary.downloaded));
                }
            }
        }
    }
    // A file the agent deleted in the sandbox is deleted here too, but only one a previous sync
    // carried (so a file that was never ours is never removed).
    for rel in local.keys() {
        if !remote.contains_key(rel) {
            let _ = std::fs::remove_file(local_root.join(rel.replace('/', std::path::MAIN_SEPARATOR_STR)));
            summary.removed += 1;
        }
    }
    Ok(summary)
}

/// Run a short command in the sandbox and collect its output (for setup steps: installing Node,
/// unpacking the agent). Returns stdout, or an error with stderr in it when it failed.
pub fn run_command(envd: &Envd, command: &str, timeout: Duration) -> Result<String, ToolError> {
    let proc = envd.start_process("bash", &["-c".to_string(), command.to_string()], &BTreeMap::new(), "")?;
    let mut out = String::new();
    let mut err = String::new();
    let started = Instant::now();
    loop {
        let left = timeout.checked_sub(started.elapsed()).unwrap_or_default();
        if left.is_zero() {
            return Err(ToolError::new(ErrorCode::TimedOut, format!("The sandbox took too long on: {command}")));
        }
        match proc.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(ProcEvent::Stdout(b)) => out.push_str(&String::from_utf8_lossy(&b)),
            Ok(ProcEvent::Stderr(b)) => err.push_str(&String::from_utf8_lossy(&b)),
            Ok(ProcEvent::Ended(code)) => {
                if code.is_some_and(|c| c == 0) {
                    return Ok(out);
                }
                return Err(ToolError::new(
                    ErrorCode::EngineMissing,
                    format!(
                        "The sandbox's command failed ({}): {}",
                        code.map(|c| c.to_string()).unwrap_or_else(|| "no code".into()),
                        err.trim().chars().take(400).collect::<String>()
                    ),
                ));
            }
            Ok(ProcEvent::Started(_)) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => return Err(ToolError::new(ErrorCode::EngineMissing, format!("The sandbox stopped while running: {command}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_are_flag_length_payload() {
        let f = encode_frame(&json!({"process":{"cmd":"bash"}}));
        assert_eq!(f[0], 0);
        let len = u32::from_be_bytes([f[1], f[2], f[3], f[4]]) as usize;
        assert_eq!(f.len(), len + 5);
        let v: Value = serde_json::from_slice(&f[5..]).unwrap();
        assert_eq!(v["process"]["cmd"], "bash");
    }

    #[test]
    fn events_decode_from_connect_json() {
        let out = decode_event(&json!({"event":{"start":{"pid":42}}}));
        assert_eq!(out, vec![ProcEvent::Started(42)]);
        let out = decode_event(&json!({"event":{"data":{"stdout": b64(b"hello"), "stderr": b64(b"boo")}}}));
        assert_eq!(out, vec![ProcEvent::Stdout(b"hello".to_vec()), ProcEvent::Stderr(b"boo".to_vec())]);
        let out = decode_event(&json!({"event":{"end":{"exitCode":3,"exited":true,"status":"exit status 3"}}}));
        assert_eq!(out, vec![ProcEvent::Ended(Some(3))]);
        // protojson leaves a 0 exitCode out; it's in the status line.
        let out = decode_event(&json!({"event":{"end":{"exited":true,"status":"exit status 0"}}}));
        assert_eq!(out, vec![ProcEvent::Ended(Some(0))]);
    }

    #[test]
    fn framed_events_are_read_to_the_end() {
        let mut stream = Vec::new();
        stream.extend_from_slice(&encode_frame(&json!({"event":{"start":{"pid":1}}})));
        stream.extend_from_slice(&encode_frame(&json!({"event":{"data":{"stdout": b64(b"line one\n")}}})));
        stream.extend_from_slice(&encode_frame(&json!({"event":{"end":{"exited":true,"status":"exit status 0"}}})));
        stream.extend_from_slice(&[2, 0, 0, 0, 2, b'{', b'}']);
        let (tx, rx) = std::sync::mpsc::channel();
        read_events(std::io::Cursor::new(stream), &tx);
        drop(tx);
        let events: Vec<ProcEvent> = rx.into_iter().collect();
        assert_eq!(events, vec![ProcEvent::Started(1), ProcEvent::Stdout(b"line one\n".to_vec()), ProcEvent::Ended(Some(0))]);
    }

    #[test]
    fn a_trailing_half_frame_is_dropped_not_a_hang() {
        let mut stream = encode_frame(&json!({"start":{"pid":1}}));
        stream.extend_from_slice(&[0, 0, 0, 0, 9, b'{']);
        let (tx, rx) = std::sync::mpsc::channel();
        read_events(std::io::Cursor::new(stream), &tx);
        drop(tx);
        let events: Vec<ProcEvent> = rx.into_iter().collect();
        assert_eq!(events, vec![ProcEvent::Started(1)]);
    }

    #[test]
    fn urls_encode_paths() {
        assert_eq!(urlencode("/craft-anything/app/a b/c+d.txt"), "/craft-anything/app/a%20b/c%2Bd.txt");
    }

    #[test]
    fn ignore_rules_skip_folders_and_named_files() {
        assert!(ignored("app/node_modules/x.js", &[]));
        assert!(ignored(".git/config", &[]));
        assert!(!ignored("app/src/main.rs", &[]));
        assert!(ignored("app/secrets.json", &["secrets.json".to_string()]));
        assert!(ignored("app/src/secrets.json", &["secrets.json".to_string()]), "a name matches anywhere, like .gitignore");
        assert!(ignored("app/secrets.json", &["app/secrets.json".to_string()]), "a path with a slash matches exactly");
        assert!(!ignored("other/secrets.json", &["app/secrets.json".to_string()]));
    }

    #[test]
    fn local_manifest_walks_and_skips_ignored_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("app/src")).unwrap();
        std::fs::create_dir_all(root.join("app/node_modules/left-pad")).unwrap();
        std::fs::write(root.join("craft.json"), "{}").unwrap();
        std::fs::write(root.join("app/src/main.txt"), "hi").unwrap();
        std::fs::write(root.join("app/node_modules/left-pad/index.js"), "x").unwrap();
        let m = local_manifest(root, &[]);
        assert!(m.contains_key("craft.json"));
        assert!(m.contains_key("app/src/main.txt"));
        assert!(!m.contains_key("app/node_modules/left-pad/index.js"));
        assert!(m.iter().all(|(_, e)| e.size > 0));
    }

    #[test]
    fn lines_feed_whole_and_partial() {
        let mut partial = String::new();
        let lines: std::sync::Arc<craft_anything_platform::sync::Mutex<Vec<String>>> = Default::default();
        {
            let keep = lines.clone();
            let mut sink: Option<LineSink> = Some(Box::new(move |l: &str| keep.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(l.to_string())));
            feed_lines(b"one\ntwo\nth", &mut partial, &mut sink);
            feed_lines(b"ree\n", &mut partial, &mut sink);
            flush_lines(&mut partial, &mut sink);
        }
        assert_eq!(*lines.lock().unwrap_or_else(std::sync::PoisonError::into_inner), vec!["one", "two", "three"]);
    }

    #[test]
    fn remote_manifest_parses_find_output() {
        // Parsing is done in `remote_manifest` over a live sandbox; here, the shape it expects.
        let line = "./app/src/main.rs\t123\t1730000000.5";
        let mut parts = line.splitn(3, '\t');
        let path = parts.next().unwrap().trim_start_matches("./");
        let size: u64 = parts.next().unwrap().parse().unwrap();
        let mtime = parts.next().unwrap().split('.').next().unwrap().parse::<u64>().unwrap() * 1000;
        assert_eq!(path, "app/src/main.rs");
        assert_eq!(size, 123);
        assert_eq!(mtime, 1_730_000_000_000);
    }

    /// The runner's builder defaults hold nothing back.
    #[test]
    fn sandbox_process_defaults() {
        let info = SandboxInfo { sandbox_id: "s".into(), envd_token: "t".into() };
        let p = SandboxProcess::new(Envd::new(info, "e2b.app"), "bash");
        assert_eq!(p.program, "bash");
        assert!(p.args.is_empty() && p.env.is_empty() && p.cwd.is_empty());
    }

    /// A run against the real service, when `E2B_API_KEY` names a key: the whole loop — create,
    /// files both ways, a process with stdin, pause, bring back, stop. Skipped without a key.
    #[test]
    fn live_round_trip_against_e2b() {
        let Ok(key) = std::env::var("E2B_API_KEY") else { return };
        if key.trim().is_empty() {
            return;
        }
        let e2b = E2B::new(key.trim(), "e2b.app");
        let info = e2b.create(DEFAULT_TEMPLATE, 600, true, true).expect("create");
        let envd = Envd::new(info.clone(), "e2b.app");
        // Files: write, read back, nested.
        envd.write_file("/craft-anything/app/deep/f.txt", b"hello").expect("write");
        assert_eq!(envd.read_file("/craft-anything/app/deep/f.txt").expect("read").as_deref(), Some(b"hello".as_slice()));
        // A process with a line of stdin, and its output.
        let out = SandboxProcess::new(envd.clone(), "bash")
            .args(&["-c".to_string(), "tr a-z A-Z".to_string()])
            .stdin_bytes(b"quiet please\n".to_vec())
            .run()
            .expect("process");
        assert!(out.success, "{out:?}");
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "QUIET PLEASE");
        // Sync up: one local file lands in the sandbox.
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("app/src")).unwrap();
        std::fs::write(dir.path().join("app/src/main.txt"), "built here").unwrap();
        std::fs::create_dir_all(dir.path().join("app/node_modules/x")).unwrap();
        std::fs::write(dir.path().join("app/node_modules/x/i.js"), "ignored").unwrap();
        let summary = sync_up(&envd, dir.path(), REMOTE_ROOT, &[], |_| {}).expect("sync up");
        assert_eq!(summary.uploaded, 1, "{summary:?}");
        let listing = run_command(&envd, &format!("cat {REMOTE_ROOT}/app/src/main.txt"), Duration::from_secs(60)).expect("cat");
        assert_eq!(listing.trim(), "built here");
        // Pause, bring it back: the files are still there.
        e2b.pause(&info.sandbox_id).expect("pause");
        let again = e2b.connect(&info.sandbox_id, 600).expect("connect after pause");
        let envd2 = Envd::new(again, "e2b.app");
        let back = run_command(&envd2, &format!("cat {REMOTE_ROOT}/app/deep/f.txt"), Duration::from_secs(120)).expect("cat after resume");
        assert_eq!(back.trim(), "hello");
        // Sync down: the sandbox's copy comes back, the ignored folder doesn't travel.
        std::fs::write(dir.path().join("app/src/main.txt"), "changed in sandbox").unwrap();
        let down = sync_down(&envd2, dir.path(), REMOTE_ROOT, &[], |_| {}).expect("sync down");
        assert_eq!(down.downloaded, 0, "only mtime moved, nothing to fetch");
        e2b.kill(&info.sandbox_id).expect("kill");
        assert_eq!(e2b.state(&info.sandbox_id).expect("state"), SandboxState::Gone);
    }
}
