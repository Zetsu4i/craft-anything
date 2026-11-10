//! API keys: the OS keychain first, then a clearly labelled owner-only fallback file, then the
//! usual environment variables. Keys are never logged or written to project files.

use crate::BackendKind;
use craft_anything_engine::tools::asset_tools::AssetKeys;
use craft_anything_engines::assets::{AssetProvider, AssetProviders};
use craft_anything_evidence::{ErrorCode, ToolError};
use craft_anything_platform::sync::Mutex;
use std::path::PathBuf;
use std::time::Duration;

/// Service name in the OS keychain.
pub const SERVICE: &str = "ai.storyteller.craft-anything";
/// Set to `1` to skip the OS keychain (tests, headless servers).
pub const NO_KEYCHAIN_ENV: &str = "CRAFT_ANYTHING_NO_KEYCHAIN";
/// The keychain can block (a locked desktop keyring); never wait longer than this.
const KEYCHAIN_TIMEOUT: Duration = Duration::from_secs(3);

const FILE_HEADER: &str = "# Craft Anything AI keys: FALLBACK STORE.\n# These keys are here because no OS keychain was available when they were saved.\n# This file is readable only by you (0600). Delete a line to forget a key.\n";

/// Serializes every fallback-file read-modify-write, across backend names: two saves for different
/// backends must not read the file together and overwrite each other's entry.
static FILE_LOCK: Mutex<()> = Mutex::new(());

/// Where the fal.ai key for generated game assets is stored.
pub const FAL_KEY: &str = "assets:fal";

/// Where the cloud sandbox provider's key is stored (Settings → AI → Sandbox).
pub const SANDBOX_KEY: &str = "sandbox:e2b";

/// The sandbox's key: stored, else `E2B_API_KEY`.
pub fn sandbox_key(store: &KeyStore) -> Option<String> {
    store.get(SANDBOX_KEY).or_else(|| std::env::var("E2B_API_KEY").ok()).map(|k| k.trim().to_string()).filter(|k| !k.is_empty())
}

/// Where an asset provider's key is stored (`assets:<provider id>`).
pub fn asset_key_name(provider: &str) -> String {
    format!("assets:{provider}")
}

/// An asset provider's key: stored, else the provider's own environment variable.
pub fn asset_key(store: &KeyStore, provider: &dyn AssetProvider) -> Option<String> {
    store.get(&asset_key_name(provider.id())).or_else(|| provider.key_env().and_then(|v| std::env::var(v).ok())).filter(|k| !k.trim().is_empty())
}

/// Every provider's key that is set up, for the builder's tool server.
pub fn asset_keys(store: &KeyStore, providers: &AssetProviders) -> AssetKeys {
    AssetKeys(
        providers.ids().into_iter().filter_map(|id| providers.get(id)).filter_map(|p| asset_key(store, p.as_ref()).map(|k| (p.id().to_string(), k))).collect(),
    )
}

/// After a key changed: keys a run already loaded into `session` follow, so a removed one stops being
/// spent at once. A session with none loaded stays that way. Reads the keychain, so call it off the UI
/// thread; a tool passes the session it holds, because locking it again would wait forever.
pub fn refresh_loaded(session: &mut craft_anything_engine::Session, store: &KeyStore) {
    if session.ext.get::<AssetKeys>().is_some() {
        let providers = session.ext.get::<AssetProviders>().map(|p| p.as_ref().clone()).unwrap_or_default();
        session.ext.insert(std::sync::Arc::new(asset_keys(store, &providers)));
    }
}

/// The fal.ai key for game assets: stored, else fal's environment variable.
pub fn fal_key(store: &KeyStore) -> Option<String> {
    asset_key(store, &craft_anything_engines::assets::fal::Fal::default())
}

/// The environment variable conventionally holding a key for this kind.
pub fn env_var(kind: BackendKind) -> Option<&'static str> {
    match kind {
        BackendKind::Anthropic => Some("ANTHROPIC_API_KEY"),
        BackendKind::Openai => Some("OPENAI_API_KEY"),
        BackendKind::Openrouter => Some("OPENROUTER_API_KEY"),
        BackendKind::Gemini => Some("GEMINI_API_KEY"),
        BackendKind::OpenaiCompatible | BackendKind::AgentCli => None,
    }
}

/// What a keychain delete told us.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeychainDelete {
    /// The entry was removed, or was already missing.
    Confirmed,
    /// There is no keychain on this computer to hold a key, so the fallback file is all there is.
    NoKeychain,
    /// A platform failure: the keychain exists but the delete didn't get through. A read-back
    /// settles whether the key is still there.
    Unreachable,
    /// The keychain is there but locked, denied or timed out: the key may still be in it.
    Unconfirmed,
}

/// What a keychain read found. `Unavailable` is not `Missing`: the key may still be there.
#[derive(Debug, Clone, PartialEq, Eq)]
enum KeychainGet {
    Found(String),
    Missing,
    Unavailable,
}

/// Where keys live.
#[derive(Debug, Clone)]
pub struct KeyStore {
    file: PathBuf,
    use_keychain: bool,
}

impl KeyStore {
    /// The default store: keychain + `<config dir>/ai-keys.toml`.
    pub fn system() -> Self {
        let no_chain = std::env::var(NO_KEYCHAIN_ENV).is_ok_and(|v| v == "1");
        KeyStore { file: craft_anything_engine::settings::config_dir().join("ai-keys.toml"), use_keychain: !no_chain }
    }

    /// A file-only store (tests).
    pub fn file_only(file: PathBuf) -> Self {
        KeyStore { file, use_keychain: false }
    }

    fn read_file(&self) -> toml::Table {
        std::fs::read_to_string(&self.file).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    /// The file's keys before changing them: none when it doesn't exist, an error when it can't be
    /// read, so a save never replaces a file it couldn't read and drops the other keys (#262).
    fn read_file_to_change(&self) -> Result<toml::Table, ToolError> {
        let unreadable = |why: String| {
            ToolError::new(ErrorCode::FileError, format!("Couldn't read the key file {}: {why}", self.file.display()))
                .with_remediation("Fix or move that file (it was left untouched), then save the key again.")
        };
        match std::fs::read(&self.file) {
            Ok(b) => {
                let text = String::from_utf8(b).map_err(|_| unreadable("it isn't valid UTF-8".into()))?;
                toml::from_str(&text).map_err(|e| unreadable(e.to_string()))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
            Err(e) => Err(unreadable(e.to_string())),
        }
    }

    fn write_file(&self, t: &toml::Table) -> Result<(), ToolError> {
        let body = toml::to_string(t).map_err(|e| ToolError::internal(e.to_string()))?;
        craft_anything_platform::write_atomic_private(&self.file, format!("{FILE_HEADER}{body}").as_bytes())
    }

    /// The stored key for a backend (keychain, then fallback file).
    pub fn get(&self, backend: &str) -> Option<String> {
        if self.use_keychain
            && let Some(k) = keychain::get(backend)
        {
            return Some(k);
        }
        self.read_file().get(backend).and_then(|v| v.as_str()).map(str::to_string).filter(|k| !k.is_empty())
    }

    /// The key to use: stored, else the kind's environment variable.
    pub fn resolve(&self, backend: &str, kind: BackendKind) -> Option<String> {
        self.get(backend).or_else(|| env_var(kind).and_then(|v| std::env::var(v).ok()).filter(|k| !k.trim().is_empty()))
    }

    /// Save a key. Returns where it went: `"keychain"` or `"file"`.
    pub fn set(&self, backend: &str, key: &str) -> Result<&'static str, ToolError> {
        if key.trim().is_empty() {
            return Err(ToolError::invalid("The key is empty."));
        }
        if self.use_keychain && keychain::set(backend, key) {
            // Don't leave a stale copy in the fallback file (the keychain's copy is read first).
            let _file = FILE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Ok(mut t) = self.read_file_to_change()
                && t.remove(backend).is_some()
            {
                self.write_file(&t)?;
            }
            return Ok("keychain");
        }
        let _file = FILE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut t = self.read_file_to_change()?;
        t.insert(backend.to_string(), toml::Value::String(key.to_string()));
        self.write_file(&t).map_err(|e| ToolError::new(ErrorCode::NoPermission, format!("Couldn't save the key: {}", e.message)))?;
        Ok("file")
    }

    pub fn delete(&self, backend: &str) -> Result<(), ToolError> {
        // A delete that couldn't be confirmed must not read as success: the key would still be
        // used. A keychain that couldn't be reached is settled by reading the key back (a computer
        // with no keychain finds nothing); a locked, denied or timed-out one fails closed here.
        let result = if self.use_keychain { keychain::delete(backend) } else { KeychainDelete::Confirmed };
        let read_back = if result == KeychainDelete::Unreachable { keychain::get_result(backend) } else { KeychainGet::Missing };
        let keychain_done = keychain_delete_confirmed(result, read_back);
        // A known keychain failure leaves both copies untouched for a retry.
        if result == KeychainDelete::Unconfirmed {
            return Err(ToolError::new(ErrorCode::NoPermission, "The system keychain still has this key, so it wasn't removed.")
                .with_remediation("Unlock the keychain and try again, or delete this AI's entry in your keychain app."));
        }
        {
            let _file = FILE_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let mut t = self.read_file_to_change()?;
            if t.remove(backend).is_some() {
                self.write_file(&t)?;
            }
        }
        if !keychain_done {
            return Err(ToolError::new(
                ErrorCode::NoPermission,
                "The saved key file was cleared, but the system keychain couldn't confirm this key was removed.",
            )
            .with_remediation("Unlock the keychain and try again, or delete this AI's entry in your keychain app."));
        }
        Ok(())
    }
}

/// Whether a keychain delete is confirmed: its own result, or, when the keychain couldn't be
/// reached, a read-back that finds nothing. A locked or timed-out keychain is never settled by the
/// read-back, which fails the same way. A headless computer with no keychain reads back nothing,
/// so a fallback-file-only delete passes.
fn keychain_delete_confirmed(result: KeychainDelete, read_back: KeychainGet) -> bool {
    match result {
        KeychainDelete::Confirmed | KeychainDelete::NoKeychain => true,
        KeychainDelete::Unreachable => read_back == KeychainGet::Missing,
        KeychainDelete::Unconfirmed => false,
    }
}

/// Run a keychain operation on a helper thread with a deadline.
fn bounded<T: Send + 'static>(f: impl FnOnce() -> Option<T> + Send + 'static) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("keychain".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .ok()?;
    rx.recv_timeout(KEYCHAIN_TIMEOUT).ok().flatten()
}

#[cfg(any(target_os = "linux", target_os = "macos", windows))]
mod keychain {
    use super::{KeychainDelete, KeychainGet, SERVICE, bounded};

    pub fn get(backend: &str) -> Option<String> {
        match get_result(backend) {
            KeychainGet::Found(key) => Some(key),
            KeychainGet::Missing | KeychainGet::Unavailable => None,
        }
    }

    /// Read the entry and say whether it was missing or the keychain couldn't answer. The two are
    /// not interchangeable when a delete is being confirmed: an unavailable read may hide a key.
    pub fn get_result(backend: &str) -> KeychainGet {
        let b = backend.to_string();
        bounded(move || match keyring::Entry::new(SERVICE, &b) {
            Ok(entry) => Some(match entry.get_password() {
                Ok(key) => KeychainGet::Found(key),
                Err(keyring::Error::NoEntry) => KeychainGet::Missing,
                Err(_) => KeychainGet::Unavailable,
            }),
            Err(_) => Some(KeychainGet::Unavailable),
        })
        .unwrap_or(KeychainGet::Unavailable)
    }

    pub fn set(backend: &str, key: &str) -> bool {
        let (b, k) = (backend.to_string(), key.to_string());
        bounded(move || keyring::Entry::new(SERVICE, &b).ok()?.set_password(&k).ok()).is_some()
    }

    /// Delete the entry and say how far it got. A missing entry is confirmed; a platform with no
    /// keychain to hold one is [`KeychainDelete::NoKeychain`]; a platform failure is
    /// [`KeychainDelete::Unreachable`] for the caller to settle with a read-back; a locked, denied
    /// or timed-out keychain is unconfirmed.
    pub fn delete(backend: &str) -> KeychainDelete {
        let b = backend.to_string();
        bounded(move || {
            match keyring::Entry::new(SERVICE, &b) {
                Ok(entry) => Some(delete_result(entry.delete_credential())),
                // No keychain backend on this computer: there is nothing there to delete.
                Err(_) => Some(KeychainDelete::NoKeychain),
            }
        })
        .unwrap_or(KeychainDelete::Unconfirmed)
    }

    /// A delete is confirmed only when the entry was removed or was already gone. A platform
    /// failure is the maybe-no-keychain case (the caller reads back); a locked store is not.
    fn delete_result(result: Result<(), keyring::Error>) -> KeychainDelete {
        match result {
            Ok(()) | Err(keyring::Error::NoEntry) => KeychainDelete::Confirmed,
            Err(keyring::Error::PlatformFailure(_)) => KeychainDelete::Unreachable,
            Err(_) => KeychainDelete::Unconfirmed,
        }
    }

    #[cfg(test)]
    mod tests {
        use super::delete_result;
        use crate::keys::KeychainDelete::{Confirmed, Unconfirmed, Unreachable};

        #[test]
        fn only_a_missing_or_removed_entry_is_confirmed() {
            assert_eq!(delete_result(Ok(())), Confirmed);
            assert_eq!(delete_result(Err(keyring::Error::NoEntry)), Confirmed);
            assert_eq!(delete_result(Err(keyring::Error::PlatformFailure(Box::new(std::io::Error::other("present but failing"))))), Unreachable);
            assert_eq!(delete_result(Err(keyring::Error::NoStorageAccess(Box::new(std::io::Error::other("locked"))))), Unconfirmed);
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
mod keychain {
    use super::{KeychainDelete, KeychainGet};

    pub fn get(_: &str) -> Option<String> {
        None
    }

    pub fn get_result(_: &str) -> KeychainGet {
        KeychainGet::Missing
    }

    pub fn set(_: &str, _: &str) -> bool {
        false
    }

    pub fn delete(_: &str) -> KeychainDelete {
        KeychainDelete::NoKeychain
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A second provider with no environment variable, so nothing here reads the real one.
    struct Other;

    impl AssetProvider for Other {
        fn id(&self) -> &'static str {
            "other"
        }
        fn name(&self) -> &'static str {
            "Other"
        }
        fn generate(&self, _key: &str, _req: &craft_anything_engines::assets::AssetRequest) -> Result<craft_anything_engines::assets::Generated, ToolError> {
            Err(ToolError::invalid("not here"))
        }
    }

    /// A failed keychain delete is settled by reading the key back: gone is confirmed, present is
    /// not. A machine with no keychain reads back nothing, so the file-only delete still passes.
    #[test]
    fn a_failed_keychain_delete_is_settled_by_the_read_back() {
        use KeychainGet::{Found, Missing, Unavailable};
        assert!(keychain_delete_confirmed(KeychainDelete::Confirmed, Missing), "the delete itself succeeded");
        assert!(keychain_delete_confirmed(KeychainDelete::NoKeychain, Unavailable), "there is no keychain to hold a key");
        assert!(keychain_delete_confirmed(KeychainDelete::Unreachable, Missing), "the key is gone");
        assert!(!keychain_delete_confirmed(KeychainDelete::Unreachable, Found("still here".into())), "the keychain still has it");
        assert!(!keychain_delete_confirmed(KeychainDelete::Unreachable, Unavailable), "a read error or timeout can't confirm the delete");
        assert!(!keychain_delete_confirmed(KeychainDelete::Unconfirmed, Missing), "a locked or timed-out delete fails closed");
    }

    /// Saves for different backends share one fallback file: a read-modify-write from one must not
    /// overwrite another's entry.
    #[test]
    fn fallback_writes_from_different_backends_dont_lose_entries() {
        let d = tempfile::tempdir().unwrap();
        let store = KeyStore::file_only(d.path().join("keys.toml"));
        let mut handles = Vec::new();
        for i in 0..8 {
            let store = store.clone();
            handles.push(std::thread::spawn(move || {
                for j in 0..5 {
                    store.set(&format!("backend-{i}-{j}"), "sk-test").unwrap();
                }
            }));
        }
        for handle in handles {
            handle.join().unwrap();
        }
        let keys = store.read_file();
        for i in 0..8 {
            for j in 0..5 {
                assert_eq!(keys.get(&format!("backend-{i}-{j}")).and_then(|v| v.as_str()), Some("sk-test"), "backend-{i}-{j}");
            }
        }
    }

    #[test]
    fn each_asset_provider_has_its_own_key() {
        let d = tempfile::tempdir().unwrap();
        let s = KeyStore::file_only(d.path().join("keys.toml"));
        let mut providers = AssetProviders::built_in();
        providers.register(std::sync::Arc::new(Other));
        s.set(&asset_key_name("other"), "o-1").unwrap();
        let keys = asset_keys(&s, &providers);
        assert_eq!(keys.get("other"), Some("o-1"));
        // fal's is stored where it always was, apart from the other provider's.
        assert_eq!(asset_key_name("fal"), FAL_KEY);
        s.set(FAL_KEY, "f-1").unwrap();
        let keys = asset_keys(&s, &providers);
        assert_eq!((keys.get("fal"), keys.get("other")), (Some("f-1"), Some("o-1")));
        s.delete(&asset_key_name("other")).unwrap();
        assert_eq!(asset_keys(&s, &providers).get("other"), None);
        assert_eq!(fal_key(&s).as_deref(), Some("f-1"));
    }

    /// #923: a key removed in Settings must stop being spent, in a run that already loaded it too.
    #[test]
    fn keys_loaded_by_a_run_follow_a_change_and_none_appear_otherwise() {
        let d = tempfile::tempdir().unwrap();
        let s = KeyStore::file_only(d.path().join("keys.toml"));
        let mut providers = AssetProviders::built_in();
        providers.register(std::sync::Arc::new(Other));
        s.set(&asset_key_name("other"), "o-1").unwrap();
        let mut session = craft_anything_engine::testing::session();
        session.ext.insert(std::sync::Arc::new(providers));
        refresh_loaded(&mut session, &s);
        assert!(session.ext.get::<AssetKeys>().is_none(), "no run loaded any");

        session.ext.insert(std::sync::Arc::new(AssetKeys([("other".to_string(), "old".to_string())].into())));
        refresh_loaded(&mut session, &s);
        assert_eq!(session.ext.get::<AssetKeys>().unwrap().get("other"), Some("o-1"), "a changed key");
        s.delete(&asset_key_name("other")).unwrap();
        refresh_loaded(&mut session, &s);
        assert_eq!(session.ext.get::<AssetKeys>().unwrap().get("other"), None, "a removed key");
    }

    /// Its key can't be removed: the AI stays in the list, so removing it can be tried again.
    #[test]
    fn an_ai_whose_key_cannot_be_removed_stays_to_try_again() {
        let d = tempfile::tempdir().unwrap();
        let file = d.path().join("keys.toml");
        let engine = std::sync::Arc::new(craft_anything_engine::Engine::new(craft_anything_engine::testing::session()));
        let hub = crate::AiHub::with_stores(engine, d.path().join("settings.toml"), KeyStore::file_only(file.clone()));
        let mut cfg = hub.config();
        cfg.upsert(crate::BackendEntry {
            config: crate::BackendConfig {
                name: "Mine".into(),
                kind: BackendKind::Openrouter,
                model: "m".into(),
                base_url: None,
                ssh_host: None,
                enabled: true,
            },
            options: Default::default(),
        });
        hub.engine.session().settings.ai = cfg.to_table().unwrap();
        std::fs::write(&file, "not [ toml").unwrap();
        assert!(hub.remove_backend("Mine").is_err());
        assert!(hub.config().get("Mine").is_some(), "it can still be picked to try again");
        std::fs::remove_file(&file).unwrap();
        assert!(hub.remove_backend("Mine").unwrap());
        assert!(hub.config().get("Mine").is_none());
    }

    /// #262: a key file that can't be read is left as it is, with every other key in it.
    #[test]
    fn an_unreadable_key_file_is_never_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("ai-keys.toml");
        let s = KeyStore::file_only(file.clone());
        let valid = b"FakeBackend = \"sk-test-backend\"\n\"assets:fal\" = \"sk-test-old\"\n".to_vec();
        let broken_toml = b"FakeBackend = \"sk-test-backend\"\n\"assets:fal\" = \"sk-test-old\n".to_vec();
        let broken_utf8 = [valid.clone(), b"\xff".to_vec()].concat();
        for bad in [broken_toml, broken_utf8] {
            std::fs::write(&file, &bad).unwrap();
            let e = s.set(FAL_KEY, "sk-test-new").unwrap_err();
            assert!(e.message.contains("Couldn't read the key file"), "{e:?}");
            assert!(s.delete("FakeBackend").is_err());
            assert_eq!(std::fs::read(&file).unwrap(), bad, "the file was changed");
        }
        std::fs::write(&file, &valid).unwrap();
        assert_eq!(s.set(FAL_KEY, "sk-test-new").unwrap(), "file");
        assert_eq!((s.get("FakeBackend").as_deref(), s.get(FAL_KEY).as_deref()), (Some("sk-test-backend"), Some("sk-test-new")));
    }

    #[test]
    fn file_fallback_is_labelled_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let s = KeyStore::file_only(dir.path().join("ai-keys.toml"));
        assert_eq!(s.set("Claude", "sk-test").unwrap(), "file");
        assert_eq!(s.get("Claude").as_deref(), Some("sk-test"));
        let text = std::fs::read_to_string(dir.path().join("ai-keys.toml")).unwrap();
        assert!(text.starts_with("# Craft Anything AI keys: FALLBACK STORE."));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            assert_eq!(std::fs::metadata(dir.path().join("ai-keys.toml")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        s.delete("Claude").unwrap();
        assert!(s.get("Claude").is_none());
        assert!(s.set("x", " ").is_err());
        assert_eq!(env_var(BackendKind::Openrouter), Some("OPENROUTER_API_KEY"));
        assert_eq!(env_var(BackendKind::Gemini), Some("GEMINI_API_KEY"));
    }
}
