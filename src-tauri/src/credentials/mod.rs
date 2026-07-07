//! Credential management for provider API keys.
//!
//! Production desktop builds use the OS credential store by default: macOS
//! Keychain, Windows Credential Manager, and Linux Secret Service via
//! `keyring`. Legacy `credentials.yaml` is still supported as a non-destructive
//! import source and as an explicit headless/dev fallback backend selected with
//! `AUDIO_GRAPH_CREDENTIAL_BACKEND`.
//!
//! NOTE: credentials are intentionally separate from Tauri app data / model
//! cache (`app_data_dir()` = `…/com.rsac.audiograph/`, by bundle id), so
//! secrets and downloaded models don't share a tree. Secrets are zeroized in
//! memory (`ZeroizeOnDrop`) and fallback files are locked owner-only on save
//! (`fs_util::set_owner_only`).
//!
//! ## Read precedence (default keychain backend)
//!
//! When the OS keychain is the active backend, a single key is resolved in
//! this order:
//! 1. **Delete tombstone** — a key marked deleted reads as missing, even if a
//!    stale value lingers in the keychain or `credentials.yaml`.
//! 2. **Edited `credentials.yaml` override** — for a key already migrated to
//!    the keychain, a present, non-empty plaintext entry in `credentials.yaml`
//!    OVERRIDES the keychain value. This makes hand-edits to the legacy file
//!    take effect instead of being silently ignored (otherwise a stale
//!    keychain copy shadows the edit and the provider 401s). The snapshot
//!    source label for such a key is `file_override`.
//! 3. **Keychain value** — the migrated value as stored in the OS keychain.
//! 4. **Imported / fallback file value** — for keys not yet in the keychain.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use zeroize::{Zeroize, ZeroizeOnDrop};

/// Process-wide serialization lock for credential read-modify-write sequences.
///
/// The presence probe (`load_with_source`) is a hidden WRITE: it rewrites
/// `credentials-state.yaml` (`mark_present_keys`) and imports untracked YAML
/// keys into the OS keychain (`import_missing_from_yaml`). PR #70 multiplied its
/// call sites (App mount + ExpressSetup mount + Settings hydrate + Retry +
/// post-save refresh), several of which fire within the same tick. Without
/// serialization, two probes each load the state file, compute independent
/// `CredentialMigrationState` copies, and last-writer-wins on the whole-file
/// rewrite can DROP a `mark_deleted` tombstone recorded by a concurrent
/// `delete_credential` — resurrecting a just-deleted key from `credentials.yaml`
/// on the next load (audio-graph-cf22 / cred-review M1). The same shape reverts
/// a save's `mark_migrated`, re-arming YAML import over a fresh keychain value.
///
/// Holding this lock across every `DefaultCredentialBackend` load/set/delete/
/// save makes those read-modify-write sequences mutually exclusive, so a
/// tombstone (or a `mark_migrated`) can never be reverted by an interleaved
/// probe. Mirrors `settings::SETTINGS_IO_LOCK` (which does not cover credential
/// files). Recovers from poisoning for the same reason: a panic mid-write can't
/// corrupt the `()` payload, and refusing to touch credentials again after one
/// panic would be worse than proceeding.
static CREDENTIAL_IO_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// Acquire the process-wide credential I/O lock (see [`CREDENTIAL_IO_LOCK`]).
fn lock_credential_io() -> MutexGuard<'static, ()> {
    CREDENTIAL_IO_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Format a `serde_yaml` parse failure WITHOUT echoing file content.
///
/// audio-graph-4243 / cred-review M4: `credentials.yaml` holds plaintext API
/// keys by design (legacy/fallback), and a `serde_yaml::Error`'s `Display`
/// includes a snippet of the offending scalar (e.g. `invalid type: string
/// "sk-live-abc…", expected a map at line 3 column 18`). That reason flows
/// VERBATIM into `CredentialFileError.reason` → UI readiness banners, toasts,
/// `console.error`, and the app log — so a malformed hand-edit can echo a key
/// fragment into user-facing surfaces and bug-report logs.
///
/// This keeps ONLY the location (line/column, which serde_yaml computes but
/// which reveals no content) and a fixed generic message. It never includes the
/// underlying error's `Display`. Keeps the `Failed to parse {path}:` prefix so
/// callers/tests can still recognize a parse failure.
fn redacted_yaml_parse_error(path: &Path, err: &serde_yaml::Error) -> String {
    match err.location() {
        Some(loc) => format!(
            "Failed to parse {}: invalid YAML at line {} column {} (content omitted)",
            path.display(),
            loc.line(),
            loc.column()
        ),
        None => format!(
            "Failed to parse {}: invalid YAML (content omitted)",
            path.display()
        ),
    }
}

/// Canonical list of credential keys accepted by the public credential IPC
/// boundary (`save_credential_cmd`, `delete_credential_cmd`, and
/// `load_credential_presence_cmd`). This is the boundary allowlist —
/// `set_field` below performs the inner-layer match, but commands should
/// reject unknown keys up front using [`is_allowed_key`].
///
/// IMPORTANT: this must stay in sync with the frontend constant
/// `ALLOWED_CREDENTIAL_KEYS` in `src/types/index.ts` and with the match arms
/// in `set_field` and the non-secret presence/readiness helpers.
pub const ALLOWED_CREDENTIAL_KEYS: &[&str] = &[
    "openai_api_key",
    "cerebras_api_key",
    "sambanova_api_key",
    "openrouter_api_key",
    "groq_api_key",
    "together_api_key",
    "fireworks_api_key",
    "deepgram_api_key",
    "assemblyai_api_key",
    "soniox_api_key",
    "gladia_api_key",
    "speechmatics_api_key",
    "elevenlabs_api_key",
    "revai_api_key",
    "azure_speech_key",
    "gemini_api_key",
    "google_service_account_path",
    "aws_access_key",
    "aws_secret_key",
    "aws_session_token",
    "aws_profile",
    "aws_region",
];

/// Returns `true` if `key` is a recognized credential field name.
pub fn is_allowed_key(key: &str) -> bool {
    ALLOWED_CREDENTIAL_KEYS.contains(&key)
}

/// Stores API credentials for cloud providers.
///
/// # Security
///
/// This type derives [`Zeroize`] and [`ZeroizeOnDrop`] so that all secret
/// fields are overwritten with zeros when the struct goes out of scope.
/// This mitigates exposure of plaintext API keys in memory dumps, swap
/// files, and cold-boot attacks. The `serde` feature of the `zeroize`
/// crate makes the derive compatible with the existing `Serialize`/
/// `Deserialize` implementations.
#[derive(Clone, Default, Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
pub struct CredentialStore {
    // --- OpenAI-compatible API keys ---
    #[serde(default)]
    pub openai_api_key: Option<String>,
    /// Cerebras Inference API key for the first-class OpenAI-compatible LLM preset.
    #[serde(default)]
    pub cerebras_api_key: Option<String>,
    /// SambaNova Cloud API key for the first-class OpenAI-compatible LLM preset.
    /// SambaNova is an OpenAI-compatible inference provider; this dedicated slot
    /// keeps its key from shadowing a user's generic `openai_api_key`.
    #[serde(default)]
    pub sambanova_api_key: Option<String>,
    /// OpenRouter API key (separate slot from `openai_api_key` so the first-class
    /// OpenRouter provider variant can validate against `/api/v1/models` without
    /// shadowing a user's other OpenAI-compatible key).
    #[serde(default)]
    pub openrouter_api_key: Option<String>,
    #[serde(default)]
    pub groq_api_key: Option<String>,
    #[serde(default)]
    pub together_api_key: Option<String>,
    #[serde(default)]
    pub fireworks_api_key: Option<String>,

    // --- Streaming ASR provider keys ---
    #[serde(default)]
    pub deepgram_api_key: Option<String>,
    #[serde(default)]
    pub assemblyai_api_key: Option<String>,
    #[serde(default)]
    pub soniox_api_key: Option<String>,
    #[serde(default)]
    pub gladia_api_key: Option<String>,
    #[serde(default)]
    pub speechmatics_api_key: Option<String>,
    #[serde(default)]
    pub elevenlabs_api_key: Option<String>,
    #[serde(default)]
    pub revai_api_key: Option<String>,
    #[serde(default)]
    pub azure_speech_key: Option<String>,

    // --- Google ---
    #[serde(default)]
    pub gemini_api_key: Option<String>,
    #[serde(default)]
    pub google_service_account_path: Option<String>,

    // --- AWS ---
    #[serde(default)]
    pub aws_access_key: Option<String>,
    #[serde(default)]
    pub aws_secret_key: Option<String>,
    #[serde(default)]
    pub aws_session_token: Option<String>,
    #[serde(default)]
    pub aws_profile: Option<String>,
    #[serde(default)]
    pub aws_region: Option<String>,
}

pub struct CredentialSnapshot {
    pub store: CredentialStore,
    pub source: &'static str,
    pub key_sources: BTreeMap<&'static str, &'static str>,
}

impl CredentialSnapshot {
    pub(crate) fn new(store: CredentialStore, source: &'static str) -> Self {
        let key_sources = credential_source_map_from_store(&store, source);
        Self {
            store,
            source,
            key_sources,
        }
    }

    pub(crate) fn with_key_sources(
        store: CredentialStore,
        source: &'static str,
        key_sources: BTreeMap<&'static str, &'static str>,
    ) -> Self {
        Self {
            store,
            source,
            key_sources,
        }
    }

    pub fn source_for(&self, key: &str) -> &'static str {
        if !self.store.is_present(key).unwrap_or(false) {
            return "missing";
        }
        self.key_sources.get(key).copied().unwrap_or(self.source)
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct CredentialMigrationState {
    #[serde(default)]
    migrated_keys: BTreeSet<String>,
    #[serde(default)]
    deleted_keys: BTreeSet<String>,
}

impl std::fmt::Debug for CredentialStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CredentialStore")
            .field("openai_api_key", &redacted_presence(&self.openai_api_key))
            .field(
                "cerebras_api_key",
                &redacted_presence(&self.cerebras_api_key),
            )
            .field(
                "sambanova_api_key",
                &redacted_presence(&self.sambanova_api_key),
            )
            .field(
                "openrouter_api_key",
                &redacted_presence(&self.openrouter_api_key),
            )
            .field("groq_api_key", &redacted_presence(&self.groq_api_key))
            .field(
                "together_api_key",
                &redacted_presence(&self.together_api_key),
            )
            .field(
                "fireworks_api_key",
                &redacted_presence(&self.fireworks_api_key),
            )
            .field(
                "deepgram_api_key",
                &redacted_presence(&self.deepgram_api_key),
            )
            .field(
                "assemblyai_api_key",
                &redacted_presence(&self.assemblyai_api_key),
            )
            .field("soniox_api_key", &redacted_presence(&self.soniox_api_key))
            .field("gladia_api_key", &redacted_presence(&self.gladia_api_key))
            .field(
                "speechmatics_api_key",
                &redacted_presence(&self.speechmatics_api_key),
            )
            .field(
                "elevenlabs_api_key",
                &redacted_presence(&self.elevenlabs_api_key),
            )
            .field("revai_api_key", &redacted_presence(&self.revai_api_key))
            .field(
                "azure_speech_key",
                &redacted_presence(&self.azure_speech_key),
            )
            .field("gemini_api_key", &redacted_presence(&self.gemini_api_key))
            .field(
                "google_service_account_path",
                &redacted_presence(&self.google_service_account_path),
            )
            .field("aws_access_key", &redacted_presence(&self.aws_access_key))
            .field("aws_secret_key", &redacted_presence(&self.aws_secret_key))
            .field(
                "aws_session_token",
                &redacted_presence(&self.aws_session_token),
            )
            .field("aws_profile", &redacted_presence(&self.aws_profile))
            .field("aws_region", &redacted_presence(&self.aws_region))
            .finish()
    }
}

impl CredentialStore {
    pub fn get(&self, key: &str) -> Result<Option<&str>, String> {
        let value = match key {
            "openai_api_key" => self.openai_api_key.as_deref(),
            "cerebras_api_key" => self.cerebras_api_key.as_deref(),
            "sambanova_api_key" => self.sambanova_api_key.as_deref(),
            "openrouter_api_key" => self.openrouter_api_key.as_deref(),
            "groq_api_key" => self.groq_api_key.as_deref(),
            "together_api_key" => self.together_api_key.as_deref(),
            "fireworks_api_key" => self.fireworks_api_key.as_deref(),
            "deepgram_api_key" => self.deepgram_api_key.as_deref(),
            "assemblyai_api_key" => self.assemblyai_api_key.as_deref(),
            "soniox_api_key" => self.soniox_api_key.as_deref(),
            "gladia_api_key" => self.gladia_api_key.as_deref(),
            "speechmatics_api_key" => self.speechmatics_api_key.as_deref(),
            "elevenlabs_api_key" => self.elevenlabs_api_key.as_deref(),
            "revai_api_key" => self.revai_api_key.as_deref(),
            "azure_speech_key" => self.azure_speech_key.as_deref(),
            "gemini_api_key" => self.gemini_api_key.as_deref(),
            "google_service_account_path" => self.google_service_account_path.as_deref(),
            "aws_access_key" => self.aws_access_key.as_deref(),
            "aws_secret_key" => self.aws_secret_key.as_deref(),
            "aws_session_token" => self.aws_session_token.as_deref(),
            "aws_profile" => self.aws_profile.as_deref(),
            "aws_region" => self.aws_region.as_deref(),
            _ => return Err(format!("Unknown credential key: {}", key)),
        };
        Ok(value)
    }

    pub fn get_owned(&self, key: &str) -> Result<Option<String>, String> {
        Ok(self.get(key)?.map(str::to_string))
    }

    pub fn is_present(&self, key: &str) -> Result<bool, String> {
        Ok(self
            .get(key)?
            .map(str::trim)
            .is_some_and(|value| !value.is_empty()))
    }

    pub fn present_count(&self) -> usize {
        ALLOWED_CREDENTIAL_KEYS
            .iter()
            .filter(|key| self.is_present(key).unwrap_or(false))
            .count()
    }
}

fn credential_source_map_from_store(
    store: &CredentialStore,
    source: &'static str,
) -> BTreeMap<&'static str, &'static str> {
    ALLOWED_CREDENTIAL_KEYS
        .iter()
        .filter_map(|&key| {
            store
                .is_present(key)
                .ok()
                .filter(|present| *present)
                .map(|_| (key, source))
        })
        .collect()
}

impl CredentialMigrationState {
    fn is_tracked(&self, key: &str) -> bool {
        self.migrated_keys.contains(key) || self.deleted_keys.contains(key)
    }

    fn is_deleted(&self, key: &str) -> bool {
        self.deleted_keys.contains(key)
    }

    fn mark_migrated(&mut self, key: &str) {
        if is_allowed_key(key) {
            self.deleted_keys.remove(key);
            self.migrated_keys.insert(key.to_string());
        }
    }

    fn mark_deleted(&mut self, key: &str) {
        if is_allowed_key(key) {
            self.migrated_keys.remove(key);
            self.deleted_keys.insert(key.to_string());
        }
    }

    fn mark_store_snapshot(&mut self, store: &CredentialStore) {
        for &key in ALLOWED_CREDENTIAL_KEYS {
            if store.is_present(key).unwrap_or(false) {
                self.mark_migrated(key);
            } else {
                self.mark_deleted(key);
            }
        }
    }

    fn mark_present_keys(&mut self, store: &CredentialStore) {
        for &key in ALLOWED_CREDENTIAL_KEYS {
            if store.is_present(key).unwrap_or(false) {
                self.mark_migrated(key);
            }
        }
    }

    fn clear_tracked_keys(&self, store: &mut CredentialStore) -> Result<(), String> {
        for key in self.migrated_keys.iter().chain(self.deleted_keys.iter()) {
            if is_allowed_key(key) {
                set_field(store, key, None)?;
            }
        }
        Ok(())
    }

    fn clear_deleted_keys(&self, store: &mut CredentialStore) -> Result<(), String> {
        for key in &self.deleted_keys {
            if is_allowed_key(key) {
                set_field(store, key, None)?;
            }
        }
        Ok(())
    }

    fn clear_file_fallback_keys(&mut self, store: &CredentialStore) {
        for &key in ALLOWED_CREDENTIAL_KEYS {
            if store.is_present(key).unwrap_or(false) {
                self.migrated_keys.remove(key);
                self.deleted_keys.remove(key);
            }
        }
    }
}

pub fn redacted_secret_presence(value: Option<&str>) -> &'static str {
    if value.map(str::trim).is_some_and(|value| !value.is_empty()) {
        "<present>"
    } else {
        "<missing>"
    }
}

/// A stable, non-reversible fingerprint of a secret for LOG comparison only.
///
/// Emits `sha256:<8 hex chars> len=<n>` — the first 4 bytes (32 bits) of the
/// SHA-256 of the value, plus its length. This lets two log lines (the SAVE end
/// in `save_credential_cmd` and the CONNECT end in the Deepgram client) be
/// compared to answer a single question: *did the key that reached the wire
/// match the key that was just saved?* If the fingerprints differ, the in-memory
/// settings cache served a stale key (the confirmed 401 root cause); if they
/// match, a 401 is a genuine provider-side reject.
///
/// SECURITY: this is a one-way hash prefix. It reveals nothing usable about the
/// secret — never the raw key, never a first-N/last-N slice (which would leak
/// real characters). 4 bytes is ample to distinguish two distinct keys with
/// negligible collision risk. An empty/missing value returns `<missing>` so we
/// never fingerprint the empty string.
pub fn secret_fingerprint(value: Option<&str>) -> String {
    use sha2::{Digest, Sha256};

    match value.map(str::trim).filter(|v| !v.is_empty()) {
        Some(v) => {
            let digest = Sha256::digest(v.as_bytes());
            format!("sha256:{} len={}", &hex::encode(digest)[..8], v.len())
        }
        None => "<missing>".to_string(),
    }
}

fn redacted_presence(value: &Option<String>) -> &'static str {
    redacted_secret_presence(value.as_deref())
}

pub fn config_dir() -> Result<PathBuf, String> {
    let base = dirs::config_dir().ok_or_else(|| "Cannot determine config directory".to_string())?;
    let dir = base.join("audio-graph");
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create config dir: {}", e))?;
    Ok(dir)
}

pub fn credentials_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("credentials.yaml"))
}

fn credential_state_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("credentials-state.yaml"))
}

pub trait CredentialBackend {
    fn source_label(&self) -> &'static str;
    fn load(&self) -> Result<CredentialStore, String>;
    fn save(&self, store: &CredentialStore) -> Result<(), String>;

    fn load_or_default(&self) -> CredentialStore {
        match self.load() {
            Ok(store) => store,
            Err(e) => {
                log::error!(
                    "Failed to load credentials from {} ({}): using empty credential store. \
                         Backup your file and re-enter credentials in Settings.",
                    self.source_label(),
                    e
                );
                CredentialStore::default()
            }
        }
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        self.load()?.get_owned(key)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        // Empty (or whitespace-only) values are treated as "delete" to prevent
        // accidentally clobbering a valid stored credential when a user leaves a
        // form field blank after it was pre-populated from disk. Callers that
        // actually want to clear a credential should use `delete`.
        let trimmed = value.trim();
        if trimmed.is_empty() {
            // Upgraded to info-level so an empty-value skip is visible in the
            // normal log without turning on debug — this is the "silent skip"
            // path that makes credential-save non-persistence look like a
            // backend bug. Logs the key only, never the value.
            log::info!(
                "set_credential({key}): value is empty/whitespace — skipping (use delete_credential to clear)"
            );
            return Ok(());
        }
        let mut store = self.load_or_default();
        set_field(&mut store, key, Some(trimmed.to_string()))?;
        self.save(&store)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let mut store = self.load_or_default();
        set_field(&mut store, key, None)?;
        self.save(&store)
    }
}

trait KeychainStore {
    fn get_key(&self, key: &str) -> Result<Option<String>, String>;
    fn set_key(&self, key: &str, value: &str) -> Result<(), String>;
    fn delete_key(&self, key: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, Default)]
struct OsKeychainStore;

#[derive(Debug, Clone)]
struct KeychainCredentialBackend<S = OsKeychainStore> {
    store: S,
}

#[derive(Debug, Clone)]
struct DefaultCredentialBackend<S = OsKeychainStore> {
    keychain: KeychainCredentialBackend<S>,
    yaml: YamlCredentialBackend,
    state: CredentialMigrationStateBackend,
    file_backend: bool,
    fallback_to_yaml: bool,
}

#[derive(Debug, Clone, Default)]
pub struct YamlCredentialBackend {
    path: Option<PathBuf>,
}

#[derive(Debug, Clone, Default)]
struct CredentialMigrationStateBackend {
    path: Option<PathBuf>,
}

impl YamlCredentialBackend {
    pub fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    pub fn resolved_path(&self) -> Result<PathBuf, String> {
        match &self.path {
            Some(path) => Ok(path.clone()),
            None => credentials_path(),
        }
    }

    /// Remove a single key's plaintext entry from `credentials.yaml`, preserving
    /// every other key's entry (a load-modify-save that touches only `key`).
    ///
    /// audio-graph-79aa: this is the yaml side of a keychain write. After a key
    /// is migrated, `migrated_overrides_from_yaml` treats a non-empty plaintext
    /// entry as a hand-edit that BEATS the keychain — so a `credentials.yaml`
    /// value left over from before migration permanently shadows every future
    /// keychain save (the Deepgram-401 loop). A save/delete calls this in the
    /// SAME `CREDENTIAL_IO_LOCK` critical section as the keychain write, so once
    /// the fresh value lands in the keychain the stale file entry is gone and
    /// `file_override` can only ever represent a hand-edit made AFTER the save.
    ///
    /// No-op when the file does not exist or already lacks the key, so a keychain
    /// save never *creates* a plaintext credentials file (nor needlessly rewrites
    /// one, bumping its mtime) just to clear an entry that was never there.
    fn clear_key(&self, key: &str) -> Result<(), String> {
        let path = self.resolved_path()?;
        if !path.exists() {
            return Ok(());
        }
        let mut store = self.load()?;
        if store.get(key)?.is_none() {
            return Ok(());
        }
        set_field(&mut store, key, None)?;
        self.save(&store)
    }
}

impl CredentialMigrationStateBackend {
    #[cfg(test)]
    fn with_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: Some(path.into()),
        }
    }

    fn resolved_path(&self) -> Result<PathBuf, String> {
        match &self.path {
            Some(path) => Ok(path.clone()),
            None => credential_state_path(),
        }
    }

    fn load(&self) -> Result<CredentialMigrationState, String> {
        let path = self.resolved_path()?;
        if !path.exists() {
            return Ok(CredentialMigrationState::default());
        }
        let contents = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        serde_yaml::from_str::<CredentialMigrationState>(&contents)
            .map_err(|e| redacted_yaml_parse_error(&path, &e))
    }

    fn save(&self, state: &CredentialMigrationState) -> Result<(), String> {
        let path = self.resolved_path()?;
        let yaml = serde_yaml::to_string(state)
            .map_err(|e| format!("Failed to serialize credential migration state: {}", e))?;
        let tmp_path = path.with_extension("yaml.tmp");

        write_owner_only_temp_file(&tmp_path, &yaml)?;
        // BEST-EFFORT ACL hardening on the presence-STATE file (this is not a
        // secret — it records which keys are present, not their values). A
        // non-zero icacls / chmod result here must NOT abort the state write:
        // that regression spammed "Failed to finalize credential migration
        // state" every session and could drop presence-state writes. We still
        // attempt the hardening (security intent) and warn on failure.
        crate::fs_util::set_owner_only(&tmp_path);
        rename_with_retry(
            &tmp_path,
            &path,
            "Failed to finalize credential migration state",
        )?;
        crate::fs_util::set_owner_only(&path);

        Ok(())
    }

    fn update(
        &self,
        f: impl FnOnce(&mut CredentialMigrationState) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut state = self.load()?;
        f(&mut state)?;
        self.save(&state)
    }

    fn mark_migrated(&self, key: &str) -> Result<(), String> {
        self.update(|state| {
            state.mark_migrated(key);
            Ok(())
        })
    }

    fn mark_deleted(&self, key: &str) -> Result<(), String> {
        self.update(|state| {
            state.mark_deleted(key);
            Ok(())
        })
    }

    fn mark_imported_keys(&self, keys: &[&str]) -> Result<(), String> {
        if keys.is_empty() {
            return Ok(());
        }
        self.update(|state| {
            for key in keys {
                state.mark_migrated(key);
            }
            Ok(())
        })
    }

    fn mark_store_snapshot(&self, store: &CredentialStore) -> Result<(), String> {
        self.update(|state| {
            state.mark_store_snapshot(store);
            Ok(())
        })
    }

    fn mark_present_keys(&self, store: &CredentialStore) -> Result<(), String> {
        if store.present_count() == 0 {
            return Ok(());
        }
        self.update(|state| {
            state.mark_present_keys(store);
            Ok(())
        })
    }

    fn mark_file_fallback_store(&self, store: &CredentialStore) -> Result<(), String> {
        if store.present_count() == 0 {
            return Ok(());
        }
        self.update(|state| {
            state.clear_file_fallback_keys(store);
            Ok(())
        })
    }
}

impl CredentialBackend for YamlCredentialBackend {
    fn source_label(&self) -> &'static str {
        "credentials_yaml"
    }

    fn load(&self) -> Result<CredentialStore, String> {
        let path = self.resolved_path()?;
        if !path.exists() {
            // File doesn't exist — this is normal on first run, not an error.
            return Ok(CredentialStore::default());
        }
        let contents = fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
        // credentials.yaml holds plaintext keys by design — a serde_yaml error's
        // Display echoes the offending scalar, so route it through the
        // location-only formatter to keep key fragments out of the surfaced
        // reason and logs (audio-graph-4243 / cred-review M4).
        serde_yaml::from_str::<CredentialStore>(&contents)
            .map_err(|e| redacted_yaml_parse_error(&path, &e))
    }

    fn save(&self, store: &CredentialStore) -> Result<(), String> {
        let path = self.resolved_path()?;
        let yaml = serde_yaml::to_string(store)
            .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
        let tmp_path = path.with_extension("yaml.tmp");

        // The secret bytes are written under an owner-only temp file (0o600 on
        // Unix; icacls before-write on Windows) inside write_owner_only_temp_file,
        // so the file is never world-readable while it holds the secret. The two
        // set_owner_only calls below are a BEST-EFFORT belt-and-suspenders
        // re-harden (matching this code's pre-regression behaviour). A non-zero
        // icacls result must NOT abort the whole credentials save — that made a
        // transient icacls failure silently drop the user's key write. We still
        // attempt the hardening and warn on failure.
        write_owner_only_temp_file(&tmp_path, &yaml)?;

        // Set restrictive permissions on the tmp file before rename, in case the
        // rename preserves the source file's permissions on some platforms.
        crate::fs_util::set_owner_only(&tmp_path);

        rename_with_retry(&tmp_path, &path, "Failed to finalize credentials")?;

        // And again on the final file to be safe.
        crate::fs_util::set_owner_only(&path);

        log::info!("Credentials saved to {}", path.display());
        Ok(())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            // Upgraded to info-level so an empty-value skip is visible in the
            // normal log without turning on debug — this is the "silent skip"
            // path that makes credential-save non-persistence look like a
            // backend bug. Logs the key only, never the value.
            log::info!(
                "set_credential({key}): value is empty/whitespace — skipping (use delete_credential to clear)"
            );
            return Ok(());
        }
        let mut store = self.load()?;
        set_field(&mut store, key, Some(trimmed.to_string()))?;
        self.save(&store)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        let mut store = self.load()?;
        set_field(&mut store, key, None)?;
        self.save(&store)
    }
}

impl Default for KeychainCredentialBackend {
    fn default() -> Self {
        Self {
            store: OsKeychainStore,
        }
    }
}

impl OsKeychainStore {
    const SERVICE: &'static str = "audio-graph";

    fn entry(&self, key: &str) -> Result<keyring::Entry, String> {
        if !is_allowed_key(key) {
            return Err(format!("Unknown credential key: {}", key));
        }
        keyring::Entry::new(Self::SERVICE, &format!("provider:{key}"))
            .map_err(|e| format!("Failed to open OS credential entry {key}: {}", e))
    }
}

impl KeychainStore for OsKeychainStore {
    fn get_key(&self, key: &str) -> Result<Option<String>, String> {
        let entry = self.entry(key)?;
        match entry.get_password() {
            Ok(value) => Ok(Some(value)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(format!("Failed to read OS credential {key}: {}", e)),
        }
    }

    fn set_key(&self, key: &str, value: &str) -> Result<(), String> {
        let entry = self.entry(key)?;
        entry
            .set_password(value)
            .map_err(|e| format!("Failed to save OS credential {key}: {}", e))
    }

    fn delete_key(&self, key: &str) -> Result<(), String> {
        let entry = self.entry(key)?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(format!("Failed to delete OS credential {key}: {}", e)),
        }
    }
}

impl<S: KeychainStore> KeychainCredentialBackend<S> {
    #[cfg(test)]
    fn with_store(store: S) -> Self {
        Self { store }
    }

    fn import_missing_from_yaml(
        &self,
        keychain_store: &mut CredentialStore,
        yaml: &YamlCredentialBackend,
        state_backend: &CredentialMigrationStateBackend,
    ) -> Vec<&'static str> {
        let Ok(file_store) = yaml.load() else {
            return Vec::new();
        };
        let state = match state_backend.load() {
            Ok(state) => state,
            Err(e) => {
                log::warn!(
                    "Skipping credentials.yaml import because migration state is unreadable: {e}"
                );
                return Vec::new();
            }
        };
        let mut imported_keys = Vec::new();
        for (key, value) in missing_credentials_from_yaml(keychain_store, &file_store, &state) {
            if self.store.set_key(key, &value).is_ok() {
                let _ = set_field(keychain_store, key, Some(value));
                imported_keys.push(key);
            }
        }
        if let Err(e) = state_backend.mark_imported_keys(&imported_keys) {
            log::warn!("Failed to record credential import state: {e}");
        }
        imported_keys
    }
}

impl<S: KeychainStore> CredentialBackend for KeychainCredentialBackend<S> {
    fn source_label(&self) -> &'static str {
        "os_keychain"
    }

    fn load(&self) -> Result<CredentialStore, String> {
        let mut store = CredentialStore::default();
        for &key in ALLOWED_CREDENTIAL_KEYS {
            if let Some(value) = self.store.get_key(key)? {
                set_field(&mut store, key, Some(value))?;
            }
        }
        Ok(store)
    }

    fn save(&self, store: &CredentialStore) -> Result<(), String> {
        for &key in ALLOWED_CREDENTIAL_KEYS {
            match store.get(key)? {
                Some(value) if !value.trim().is_empty() => self.store.set_key(key, value)?,
                _ => self.store.delete_key(key)?,
            }
        }
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        self.store.get_key(key)
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            // Upgraded to info-level so an empty-value skip is visible in the
            // normal log without turning on debug — this is the "silent skip"
            // path that makes credential-save non-persistence look like a
            // backend bug. Logs the key only, never the value.
            log::info!(
                "set_credential({key}): value is empty/whitespace — skipping (use delete_credential to clear)"
            );
            return Ok(());
        }
        self.store.set_key(key, trimmed)
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        self.store.delete_key(key)
    }
}

impl DefaultCredentialBackend<OsKeychainStore> {
    fn new() -> Self {
        Self {
            keychain: KeychainCredentialBackend::default(),
            yaml: YamlCredentialBackend::default(),
            state: CredentialMigrationStateBackend::default(),
            file_backend: file_backend_requested(),
            fallback_to_yaml: keychain_file_fallback_requested(),
        }
    }
}

impl<S: KeychainStore> DefaultCredentialBackend<S> {
    fn load_with_source(&self) -> Result<CredentialSnapshot, String> {
        // Serialize the probe's hidden read-modify-write (mark_present_keys +
        // import_missing_from_yaml) against concurrent set/delete/save so a
        // stale state snapshot written back here can't erase a tombstone or a
        // just-recorded migration (audio-graph-cf22 / cred-review M1). Held for
        // the whole load so the state read, the keychain import, and the state
        // write are one atomic critical section w.r.t. other credential writers.
        let _io_guard = lock_credential_io();

        if self.file_backend {
            return self
                .yaml
                .load()
                .map(|store| CredentialSnapshot::new(store, self.yaml.source_label()));
        }

        match self.keychain.load() {
            Ok(mut store) => {
                let state = self.state.load()?;
                state.clear_deleted_keys(&mut store)?;
                let mut key_sources =
                    credential_source_map_from_store(&store, self.keychain.source_label());
                if let Err(e) = self.state.mark_present_keys(&store) {
                    log::warn!("Failed to record keychain credential presence state: {e}");
                }
                let imported_keys =
                    self.keychain
                        .import_missing_from_yaml(&mut store, &self.yaml, &self.state);
                for key in imported_keys {
                    if store.is_present(key).unwrap_or(false) {
                        key_sources.insert(key, "imported_file");
                    }
                }
                // A user who hand-edits credentials.yaml for a key that has
                // already been migrated to the OS keychain expects that edit to
                // take effect (BUG 7fc5). Without this, the keychain value
                // shadows the file and the edit is silently ignored -> 401. We
                // honor a non-empty plaintext entry as an explicit override of
                // the migrated keychain value. Deleted-key tombstones still win
                // (handled above via clear_deleted_keys + the is_deleted guard
                // inside migrated_overrides_from_yaml) so a delete can't be
                // resurrected by a stale file value.
                for (key, value) in self.migrated_overrides_from_yaml(&store, &state)? {
                    set_field(&mut store, key, Some(value))?;
                    key_sources.insert(key, "file_override");
                }
                Ok(CredentialSnapshot::with_key_sources(
                    store,
                    self.keychain.source_label(),
                    key_sources,
                ))
            }
            Err(e) if self.fallback_to_yaml => {
                log::warn!("OS credential store unavailable; using YAML credential fallback: {e}");
                self.load_filtered_yaml_fallback()
                    .map(|store| CredentialSnapshot::new(store, "file_fallback"))
            }
            Err(e) => Err(e),
        }
    }

    fn load_filtered_yaml_fallback(&self) -> Result<CredentialStore, String> {
        let mut store = self.yaml.load()?;
        let state = self.state.load()?;
        state.clear_tracked_keys(&mut store)?;
        Ok(store)
    }

    fn get_filtered_yaml_fallback(&self, key: &str) -> Result<Option<String>, String> {
        let state = self.state.load()?;
        if state.is_tracked(key) {
            return Ok(None);
        }
        self.yaml.get(key)
    }

    /// Collect plaintext `credentials.yaml` values that should OVERRIDE the
    /// migrated keychain value for the same key.
    ///
    /// Precedence rule (BUG 7fc5, least-surprising behavior): for a key that
    /// has already been migrated to the OS keychain, a present **non-empty**
    /// plaintext entry in `credentials.yaml` is treated as a deliberate manual
    /// edit and wins over the keychain copy. This is the only credential path
    /// where the file overrides the keychain; it exists so that editing the
    /// legacy file is never silently ignored.
    ///
    /// A **deleted** key (tombstone) is never overridden — `is_deleted` short
    /// circuits so a delete can't be resurrected by a stale file value.
    fn migrated_overrides_from_yaml(
        &self,
        keychain_store: &CredentialStore,
        state: &CredentialMigrationState,
    ) -> Result<Vec<(&'static str, String)>, String> {
        let Ok(file_store) = self.yaml.load() else {
            return Ok(Vec::new());
        };
        let mut overrides = Vec::new();
        for &key in ALLOWED_CREDENTIAL_KEYS {
            // Only migrated (not deleted) keys participate; freshly-imported or
            // never-tracked keys already reflect the file via the import path.
            if state.is_deleted(key) || !state.migrated_keys.contains(key) {
                continue;
            }
            let Some(file_value) = file_store.get(key).ok().flatten() else {
                continue;
            };
            let file_value = file_value.trim();
            if file_value.is_empty() {
                continue;
            }
            // Only surface an override when the file actually differs from the
            // keychain copy; an identical value is a no-op (and keeps the source
            // label as the keychain rather than spuriously flipping it).
            // `CredentialStore::get` already yields `Option<&str>`, so this is a
            // borrowed value (no `.as_deref()` needed) and is `Copy`, letting us
            // reuse it below without re-fetching.
            let keychain_value = keychain_store.get(key).ok().flatten();
            if keychain_value == Some(file_value) {
                continue;
            }
            // cred-review m1: if the keychain ALSO holds a non-empty value for
            // this key, the file is actively SHADOWING it. This is the
            // rotation-defeat trap: a user rotates a key through the app UI (new
            // value lands in the keychain) while a stale credentials.yaml entry
            // keeps serving the OLD key, producing repeating 401s with no
            // signal — the exact Deepgram-401 class this repo already lived
            // through, wearing a different hat. Warn prominently so it shows up
            // in the log a user attaches to a bug report; the presence probe
            // additionally surfaces `source: "file_override"` in the UI. Logs
            // the key name only, never either secret value.
            //
            // Dedupe per key: this runs on EVERY probe (App mount, Settings
            // hydrate, Retry, the a8db window-focus re-probe), so a standing
            // shadow would re-log dozens of times per session. Warn once per
            // DISTINCT condition — a non-secret fingerprint of the (keychain,
            // file) value pair — and re-arm only when either side changes (a
            // genuinely new condition). The fingerprint never contains a secret.
            if file_override_shadows_keychain_value(keychain_value) {
                let fingerprint = format!(
                    "kc={} file={}",
                    secret_fingerprint(keychain_value),
                    secret_fingerprint(Some(file_value)),
                );
                if record_shadow_warning_is_new(shadow_warn_state(), key, fingerprint) {
                    log::warn!(
                        "credential {key}: a plaintext credentials.yaml entry is SHADOWING a \
                         different value stored in the OS keychain. If you rotated this key in \
                         the app, the file edit is overriding your rotation (likely cause of \
                         repeating 401s). Remove or update the {key} entry in credentials.yaml, \
                         or re-save the key in Settings after clearing the file."
                    );
                }
            }
            overrides.push((key, file_value.to_string()));
        }
        Ok(overrides)
    }
}

impl Default for DefaultCredentialBackend<OsKeychainStore> {
    fn default() -> Self {
        Self::new()
    }
}

impl<S: KeychainStore> CredentialBackend for DefaultCredentialBackend<S> {
    fn source_label(&self) -> &'static str {
        "credential_backend"
    }

    fn load(&self) -> Result<CredentialStore, String> {
        self.load_with_source().map(|snapshot| snapshot.store)
    }

    fn save(&self, store: &CredentialStore) -> Result<(), String> {
        // Same critical section as the probe: the keychain write + the state
        // snapshot must not interleave with a concurrent probe's stale-state
        // write-back (audio-graph-cf22 / cred-review M1).
        let _io_guard = lock_credential_io();
        if self.file_backend {
            return self.yaml.save(store);
        }
        match self.keychain.save(store) {
            Ok(()) => self.state.mark_store_snapshot(store).map_err(|e| {
                format!("OS credentials saved, but failed to update migration state: {e}")
            }),
            Err(e) if self.fallback_to_yaml => {
                log::warn!(
                    "OS credential store unavailable; saving to YAML credential fallback: {e}"
                );
                self.yaml.save(store)?;
                self.state.mark_file_fallback_store(store)
            }
            Err(e) => Err(e),
        }
    }

    fn get(&self, key: &str) -> Result<Option<String>, String> {
        if self.file_backend {
            return self.yaml.get(key);
        }
        match self.keychain.get(key) {
            Ok(Some(value)) => {
                let state = self.state.load()?;
                if state.is_deleted(key) {
                    return Ok(None);
                }
                // Honor a hand-edited credentials.yaml plaintext value for a
                // migrated key (BUG 7fc5) so single-key reads match the
                // snapshot loader's precedence: file override beats keychain.
                if state.migrated_keys.contains(key)
                    && let Some(file_value) = self.yaml.get(key).ok().flatten()
                {
                    let file_value = file_value.trim();
                    if !file_value.is_empty() {
                        return Ok(Some(file_value.to_string()));
                    }
                }
                Ok(Some(value))
            }
            Ok(None) => self.get_filtered_yaml_fallback(key),
            Err(e) if self.fallback_to_yaml => {
                log::warn!(
                    "OS credential store unavailable; reading YAML credential fallback: {e}"
                );
                self.get_filtered_yaml_fallback(key)
            }
            Err(e) => Err(e),
        }
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        // Serialize the keychain write + `mark_migrated` against a concurrent
        // probe so the probe's stale-state write-back can't drop this migration
        // and re-arm a YAML import over the fresh value (cred-review M1).
        let _io_guard = lock_credential_io();
        if self.file_backend {
            return self.yaml.set(key, value);
        }
        match self.keychain.set(key, value) {
            Ok(()) => {
                self.state.mark_migrated(key).map_err(|e| {
                    format!("OS credential saved, but failed to update migration state: {e}")
                })?;
                // An empty/whitespace value is a no-op skip inside `keychain.set`
                // (it deliberately preserves the stored key rather than clearing
                // it — callers use `delete` to clear). It wrote nothing, so leave
                // the file entry untouched too: clearing on a blank-field save
                // would silently drop a user's plaintext credentials.yaml entry.
                if value.trim().is_empty() {
                    return Ok(());
                }
                // audio-graph-79aa: clear any stale plaintext entry for this key
                // from credentials.yaml in the SAME critical section as the
                // keychain write. Without this, a `credentials.yaml` value left
                // over from before migration permanently shadows the value we
                // just wrote (`migrated_overrides_from_yaml` treats a non-empty
                // file entry as a hand-edit that beats the keychain) — the exact
                // Deepgram-401 loop where a user re-saves a rotated key and reads
                // keep returning the old one. After clearing, `file_override` can
                // only ever represent a hand-edit made AFTER this save.
                self.yaml.clear_key(key).map_err(|e| {
                    format!(
                        "OS credential saved, but failed to clear the stale \
                         credentials.yaml entry (it would still shadow the new value): {e}"
                    )
                })
            }
            Err(e) if self.fallback_to_yaml => {
                log::warn!(
                    "OS credential store unavailable; saving to YAML credential fallback: {e}"
                );
                self.yaml.set(key, value)?;
                let mut fallback_store = CredentialStore::default();
                set_field(&mut fallback_store, key, Some(value.trim().to_string()))?;
                self.state.mark_file_fallback_store(&fallback_store)
            }
            Err(e) => Err(e),
        }
    }

    fn delete(&self, key: &str) -> Result<(), String> {
        // The tombstone write (`mark_deleted`) is exactly what a racing probe's
        // stale-state write-back would erase, resurrecting the key. Serialize
        // the keychain delete + tombstone against the probe (cred-review M1).
        let _io_guard = lock_credential_io();
        if self.file_backend {
            return self.yaml.delete(key);
        }
        match self.keychain.delete(key) {
            Ok(()) => {
                self.state.mark_deleted(key).map_err(|e| {
                    format!("OS credential deleted, but failed to update migration state: {e}")
                })?;
                // audio-graph-79aa (symmetric writer, per review-check-symmetric-
                // writers): clear the key's plaintext entry from credentials.yaml
                // too. The tombstone alone already masks a stale file value on
                // read (clear_deleted_keys + the is_deleted guard in
                // migrated_overrides_from_yaml), but leaving the plaintext entry
                // behind means a lost/reset credentials-state.yaml would re-import
                // and resurrect the deleted key — and leaves a deleted secret
                // sitting in plaintext on disk. Clearing it here makes the delete
                // durable and removes the leftover plaintext, in the same
                // critical section as the keychain delete + tombstone.
                self.yaml.clear_key(key).map_err(|e| {
                    format!(
                        "OS credential deleted, but failed to clear its \
                         credentials.yaml entry (a lost state file could resurrect it): {e}"
                    )
                })
            }
            Err(e) if self.fallback_to_yaml => {
                log::warn!(
                    "OS credential store unavailable; deleting from YAML credential fallback: {e}"
                );
                self.yaml.delete(key)?;
                self.state.mark_deleted(key)
            }
            Err(e) => Err(e),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CredentialBackendMode {
    OsKeychain,
    File,
    KeychainWithFileFallback,
}

fn credential_backend_mode() -> CredentialBackendMode {
    credential_backend_mode_from_env(credential_backend_env().as_deref())
}

fn credential_backend_mode_from_env(value: Option<&str>) -> CredentialBackendMode {
    match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
        Some("file" | "yaml" | "credentials_yaml" | "file_fallback") => CredentialBackendMode::File,
        Some("keychain_with_file_fallback" | "os_keychain_with_file_fallback") => {
            CredentialBackendMode::KeychainWithFileFallback
        }
        _ => CredentialBackendMode::OsKeychain,
    }
}

fn keychain_file_fallback_requested() -> bool {
    matches!(
        credential_backend_mode(),
        CredentialBackendMode::KeychainWithFileFallback
    )
}

fn file_backend_requested() -> bool {
    #[cfg(test)]
    {
        true
    }

    #[cfg(not(test))]
    {
        matches!(credential_backend_mode(), CredentialBackendMode::File)
    }
}

fn credential_backend_env() -> Option<String> {
    env::var("AUDIO_GRAPH_CREDENTIAL_BACKEND")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
}

fn default_backend() -> DefaultCredentialBackend {
    DefaultCredentialBackend::new()
}

/// cred-review m1: decide whether a plaintext `credentials.yaml` override is
/// actively SHADOWING a real (non-empty) OS-keychain value — the caller has
/// already established the file value is non-empty and differs from the
/// keychain copy, so the only remaining question is whether the keychain also
/// holds a non-empty value being masked. Returns `true` iff so, meaning a
/// rotation done through the app could be silently defeated by the file.
fn file_override_shadows_keychain_value(keychain_value: Option<&str>) -> bool {
    keychain_value.map(str::trim).is_some_and(|v| !v.is_empty())
}

/// cred-review m1 (dedupe): per-key record of the last shadow condition we
/// already warned about, so a *persistent* shadow doesn't re-log on every
/// probe.
///
/// `load_with_source` (and therefore `migrated_overrides_from_yaml`) runs on
/// every presence probe — App mount, Settings hydrate, Retry, and now the a8db
/// window-focus re-probe — so a standing shadow condition (a stale
/// `credentials.yaml` entry masking a rotated keychain key) would otherwise emit
/// the identical warning dozens of times per session, drowning the signal in a
/// bug-report log. The value stored per key is a NON-SECRET fingerprint of the
/// (keychain, file) value pair; rotating either side changes the fingerprint and
/// re-arms the warning (a genuinely new condition worth surfacing again).
static SHADOW_WARN_STATE: OnceLock<Mutex<BTreeMap<&'static str, String>>> = OnceLock::new();

fn shadow_warn_state() -> &'static Mutex<BTreeMap<&'static str, String>> {
    SHADOW_WARN_STATE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// Record that a shadow condition for `key` (identified by the non-secret
/// `fingerprint` of its keychain/file value pair) is about to be reported, and
/// return whether it is a NEW condition that should actually be logged.
///
/// Returns `true` (and records the fingerprint) the first time a given
/// condition is seen and again whenever the fingerprint changes; returns
/// `false` for an unchanged repeat so the caller can suppress a redundant warn.
/// Takes the state map explicitly so unit tests can exercise the dedupe/re-arm
/// logic on a local map without touching the process-wide static.
fn record_shadow_warning_is_new(
    state: &Mutex<BTreeMap<&'static str, String>>,
    key: &'static str,
    fingerprint: String,
) -> bool {
    let mut guard = state.lock().unwrap_or_else(|p| p.into_inner());
    match guard.get(key) {
        Some(prev) if prev == &fingerprint => false,
        _ => {
            guard.insert(key, fingerprint);
            true
        }
    }
}

fn missing_credentials_from_yaml(
    keychain_store: &CredentialStore,
    file_store: &CredentialStore,
    state: &CredentialMigrationState,
) -> Vec<(&'static str, String)> {
    ALLOWED_CREDENTIAL_KEYS
        .iter()
        .filter_map(|&key| {
            if keychain_store.is_present(key).unwrap_or(false) || state.is_tracked(key) {
                return None;
            }

            let value = file_store.get(key).ok().flatten()?;
            if value.trim().is_empty() {
                return None;
            }

            Some((key, value.to_string()))
        })
        .collect()
}

/// Ensure the parent directory of `path` exists, creating it (recursively) if
/// missing.
///
/// BUG 381c: `save()` writes a sibling `*.tmp` file and then `fs::rename`s it
/// onto the final path. If the parent directory does not exist the temp-file
/// `create_new` open — and, on Windows, the rename — fails with `os error 2`
/// (ENOENT / "The system cannot find the path specified"), which spammed a
/// WARN on every load. Production paths route through `config_dir()` which
/// already creates the dir, but custom/explicit paths (and a config dir that
/// was removed out from under a running app) do not. Ensuring the parent here
/// — in the shared temp writer — covers both `YamlCredentialBackend::save` and
/// `CredentialMigrationStateBackend::save`.
fn ensure_parent_dir(path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
        && !parent.exists()
    {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create credentials directory {}: {}",
                parent.display(),
                e
            )
        })?;
    }
    Ok(())
}

/// Rename `from` onto `to`, retrying a few times on transient failures.
///
/// On Windows an anti-virus scanner or the search indexer can briefly hold a
/// handle to the freshly-written temp file, making `fs::rename` fail with a
/// sharing violation. A short bounded retry turns those transient races into a
/// success instead of a spurious save failure / WARN.
fn rename_with_retry(from: &Path, to: &Path, context: &str) -> Result<(), String> {
    const MAX_ATTEMPTS: u32 = 5;
    let mut last_err = None;
    for attempt in 0..MAX_ATTEMPTS {
        match fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                if attempt + 1 < MAX_ATTEMPTS {
                    std::thread::sleep(std::time::Duration::from_millis(20 * (attempt + 1) as u64));
                }
            }
        }
    }
    Err(format!(
        "{context}: {}",
        last_err.expect("at least one rename attempt failed")
    ))
}

/// Remove a KNOWN-stale temp file left behind by a prior crashed / aborted
/// write so the subsequent `create_new(true)` open does not fail with
/// "The file exists (os error 80)".
///
/// We deliberately clean only this specific `.tmp` sibling and keep the
/// `create_new` + rename atomicity (BUG 381c) — this is a bounded
/// "remove-if-exists then create_new", NOT a blind truncate. If the path
/// doesn't exist this is a no-op; a removal failure is surfaced so we don't
/// silently paper over a locked / undeletable file.
fn remove_stale_temp(path: &Path) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => {
            log::warn!(
                "Removed stale credentials temp {} left by a prior aborted write",
                path.display()
            );
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!(
            "Failed to remove stale credentials temp {}: {}",
            path.display(),
            e
        )),
    }
}

fn write_owner_only_temp_file(path: &Path, contents: &str) -> Result<(), String> {
    // Ensure the destination directory exists before we create the temp file
    // (BUG 381c) — otherwise the create_new open below, and the caller's rename
    // onto a sibling path, fail with os error 2 on a missing parent.
    ensure_parent_dir(path)?;

    // A prior crashed / aborted write can leave this exact `.tmp` behind, which
    // makes the create_new(true) below fail with "file exists (os error 80)".
    // Clean that KNOWN-stale sibling first; we still use create_new + rename for
    // atomicity, so this is a bounded cleanup, not a blind truncate.
    remove_stale_temp(path)?;

    // Create the temp file with restrictive permissions FIRST so secrets are
    // never written into a world-readable file between write and chmod.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(path)
            .map_err(|e| format!("Failed to create credentials temp: {}", e))?;
        f.write_all(contents.as_bytes())
            .map_err(|e| format!("Failed to write credentials: {}", e))?;
    }

    #[cfg(not(unix))]
    {
        let mut f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("Failed to create credentials temp: {}", e))?;
        // On Windows, `set_owner_only` applies an owner-only ACL via icacls.
        // Apply it before writing secret bytes so the temp file is empty during
        // the brief default-ACL window. BEST-EFFORT: a non-zero icacls result
        // must NOT abort the write (that leaked a leftover .tmp and spammed
        // WARNs, and could drop the user's key write). We still attempt the
        // pre-write hardening and warn on failure — this is strictly better
        // than the pre-regression behaviour, which had no pre-write ACL at all.
        crate::fs_util::set_owner_only(path);
        f.write_all(contents.as_bytes())
            .map_err(|e| format!("Failed to write credentials: {}", e))?;
    }

    Ok(())
}

// NOTE (cred-review m2): a public whole-store `save_credentials(&store)`
// wrapper used to live here. It was dead (no callers) and a latent footgun —
// `KeychainCredentialBackend::save` iterates ALL allowlisted keys and
// `delete_key`s any that are absent in the passed store, so a caller that built
// a partial `CredentialStore` and saved it would wipe every other provider's
// key from the OS keychain. All live mutation goes through `set_credential` /
// `delete_credential`, which load the full store first (merge-on-save), so the
// whole-store entry point is removed rather than kept as a destructive-by-
// default API. Re-introduce a merge-on-save variant if a batch writer is ever
// needed.

pub fn load_credentials() -> CredentialStore {
    default_backend().load_or_default()
}

/// Load credentials with detailed error reporting.
/// Returns `Ok(store)` for success (including the missing-file case with an
/// empty store), and `Err(reason)` only when the file exists but cannot be
/// parsed or read.
pub fn try_load_credentials() -> Result<CredentialStore, String> {
    default_backend().load()
}

pub fn try_load_credentials_with_source() -> Result<CredentialSnapshot, String> {
    default_backend().load_with_source()
}

fn set_field(store: &mut CredentialStore, key: &str, value: Option<String>) -> Result<(), String> {
    match key {
        "openai_api_key" => store.openai_api_key = value,
        "cerebras_api_key" => store.cerebras_api_key = value,
        "sambanova_api_key" => store.sambanova_api_key = value,
        "openrouter_api_key" => store.openrouter_api_key = value,
        "groq_api_key" => store.groq_api_key = value,
        "together_api_key" => store.together_api_key = value,
        "fireworks_api_key" => store.fireworks_api_key = value,
        "deepgram_api_key" => store.deepgram_api_key = value,
        "assemblyai_api_key" => store.assemblyai_api_key = value,
        "soniox_api_key" => store.soniox_api_key = value,
        "gladia_api_key" => store.gladia_api_key = value,
        "speechmatics_api_key" => store.speechmatics_api_key = value,
        "elevenlabs_api_key" => store.elevenlabs_api_key = value,
        "revai_api_key" => store.revai_api_key = value,
        "azure_speech_key" => store.azure_speech_key = value,
        "gemini_api_key" => store.gemini_api_key = value,
        "google_service_account_path" => store.google_service_account_path = value,
        "aws_access_key" => store.aws_access_key = value,
        "aws_secret_key" => store.aws_secret_key = value,
        "aws_session_token" => store.aws_session_token = value,
        "aws_profile" => store.aws_profile = value,
        "aws_region" => store.aws_region = value,
        _ => return Err(format!("Unknown credential key: {}", key)),
    }
    Ok(())
}

pub fn set_credential(key: &str, value: &str) -> Result<(), String> {
    default_backend().set(key, value)
}

pub fn delete_credential(key: &str) -> Result<(), String> {
    default_backend().delete(key)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[test]
    fn secret_fingerprint_is_stable_non_secret_and_distinguishes_keys() {
        // Deterministic: the same key always fingerprints identically (this is
        // what lets the SAVE-end and CONNECT-end log lines be compared).
        let a = secret_fingerprint(Some("deepgram-key-one"));
        assert_eq!(a, secret_fingerprint(Some("deepgram-key-one")));

        // Two DIFFERENT keys produce DIFFERENT fingerprints — the whole point
        // of the diagnostic (stale-cache 401 detection).
        let b = secret_fingerprint(Some("deepgram-key-two-different"));
        assert_ne!(a, b);

        // Shape: `sha256:<8 hex chars> len=<n>` — exactly 8 hex chars (4 bytes)
        // and the true length. NEVER the raw key, never a slice of it.
        assert!(a.starts_with("sha256:"));
        let hex_part = a
            .strip_prefix("sha256:")
            .and_then(|s| s.split(" len=").next())
            .expect("fingerprint has the documented shape");
        assert_eq!(hex_part.len(), 8, "must be 4 bytes = 8 hex chars: {a}");
        assert!(
            hex_part.chars().all(|c| c.is_ascii_hexdigit()),
            "hex prefix must be hex only: {a}"
        );
        assert!(a.ends_with("len=16"), "must record the true length: {a}");

        // Crucially, the fingerprint must NOT contain the raw secret or any
        // contiguous slice of it — only the one-way hash prefix.
        let secret = "deepgram-key-one";
        assert!(
            !a.contains(secret),
            "fingerprint must not leak the key: {a}"
        );
        assert!(
            !a.contains(&secret[..4]),
            "fingerprint must not leak a prefix slice of the key: {a}"
        );

        // Empty / whitespace / missing all map to the missing sentinel so we
        // never fingerprint (and thus never hash-log) the empty string.
        assert_eq!(secret_fingerprint(None), "<missing>");
        assert_eq!(secret_fingerprint(Some("")), "<missing>");
        assert_eq!(secret_fingerprint(Some("   ")), "<missing>");
    }

    #[derive(Clone, Default)]
    struct FakeKeychainStore {
        values: Rc<RefCell<BTreeMap<String, String>>>,
    }

    #[derive(Clone)]
    struct FakeUnavailableKeychainStore {
        reason: &'static str,
    }

    #[derive(Clone)]
    struct ScopedOsKeychainStore {
        service: String,
        account_namespace: String,
    }

    struct ScopedOsKeychainCleanup {
        store: ScopedOsKeychainStore,
    }

    impl Default for FakeUnavailableKeychainStore {
        fn default() -> Self {
            Self {
                reason: "fake OS keychain unavailable",
            }
        }
    }

    impl FakeKeychainStore {
        fn set_initial(&self, key: &str, value: &str) {
            self.values
                .borrow_mut()
                .insert(key.to_string(), value.to_string());
        }

        fn value(&self, key: &str) -> Option<String> {
            self.values.borrow().get(key).cloned()
        }
    }

    impl KeychainStore for FakeKeychainStore {
        fn get_key(&self, key: &str) -> Result<Option<String>, String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            Ok(self.value(key))
        }

        fn set_key(&self, key: &str, value: &str) -> Result<(), String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            self.values
                .borrow_mut()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete_key(&self, key: &str) -> Result<(), String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            self.values.borrow_mut().remove(key);
            Ok(())
        }
    }

    /// Thread-safe in-memory keychain fake for concurrency tests. `FakeKeychainStore`
    /// uses `Rc<RefCell<..>>` and cannot cross thread boundaries; this variant
    /// swaps in `Arc<Mutex<..>>` so a `DefaultCredentialBackend` built on it can be
    /// shared across probe/delete threads (audio-graph-cf22 race test).
    #[derive(Clone, Default)]
    struct SharedKeychainStore {
        values: std::sync::Arc<std::sync::Mutex<BTreeMap<String, String>>>,
    }

    impl KeychainStore for SharedKeychainStore {
        fn get_key(&self, key: &str) -> Result<Option<String>, String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            Ok(self.values.lock().unwrap().get(key).cloned())
        }

        fn set_key(&self, key: &str, value: &str) -> Result<(), String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            self.values
                .lock()
                .unwrap()
                .insert(key.to_string(), value.to_string());
            Ok(())
        }

        fn delete_key(&self, key: &str) -> Result<(), String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            self.values.lock().unwrap().remove(key);
            Ok(())
        }
    }

    impl ScopedOsKeychainStore {
        fn new(service: String, account_namespace: String) -> Self {
            Self {
                service,
                account_namespace,
            }
        }

        fn entry(&self, key: &str) -> Result<keyring::Entry, String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            keyring::Entry::new(&self.service, &format!("{}:{key}", self.account_namespace))
                .map_err(|e| format!("Failed to open smoke OS credential entry {key}: {}", e))
        }
    }

    impl KeychainStore for ScopedOsKeychainStore {
        fn get_key(&self, key: &str) -> Result<Option<String>, String> {
            let entry = self.entry(key)?;
            match entry.get_password() {
                Ok(value) => Ok(Some(value)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(e) => Err(format!("Failed to read smoke OS credential {key}: {}", e)),
            }
        }

        fn set_key(&self, key: &str, value: &str) -> Result<(), String> {
            let entry = self.entry(key)?;
            entry
                .set_password(value)
                .map_err(|e| format!("Failed to save smoke OS credential {key}: {}", e))
        }

        fn delete_key(&self, key: &str) -> Result<(), String> {
            let entry = self.entry(key)?;
            match entry.delete_credential() {
                Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
                Err(e) => Err(format!("Failed to delete smoke OS credential {key}: {}", e)),
            }
        }
    }

    impl Drop for ScopedOsKeychainCleanup {
        fn drop(&mut self) {
            for key in ["openai_api_key", "deepgram_api_key", "aws_region"] {
                let _ = self.store.delete_key(key);
            }
        }
    }

    impl FakeUnavailableKeychainStore {
        fn diagnostic(&self, operation: &str, key: &str) -> String {
            format!("{operation} credential {key}: {}", self.reason)
        }
    }

    impl KeychainStore for FakeUnavailableKeychainStore {
        fn get_key(&self, key: &str) -> Result<Option<String>, String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            Err(self.diagnostic("read", key))
        }

        fn set_key(&self, key: &str, _value: &str) -> Result<(), String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            Err(self.diagnostic("save", key))
        }

        fn delete_key(&self, key: &str) -> Result<(), String> {
            if !is_allowed_key(key) {
                return Err(format!("Unknown credential key: {}", key));
            }
            Err(self.diagnostic("delete", key))
        }
    }

    #[derive(serde::Serialize)]
    struct SmokePresencePayload {
        key: &'static str,
        present: bool,
        source: &'static str,
    }

    fn serialize_smoke_presence(snapshot: &CredentialSnapshot) -> String {
        let presence: Vec<SmokePresencePayload> = ALLOWED_CREDENTIAL_KEYS
            .iter()
            .map(|&key| SmokePresencePayload {
                key,
                present: snapshot.store.is_present(key).unwrap_or(false),
                source: snapshot.source_for(key),
            })
            .collect();
        serde_json::to_string(&presence).expect("serialize non-secret smoke presence")
    }

    fn assert_payload_omits_plaintext(payload: &str, plaintext_values: &[&str]) {
        for value in plaintext_values {
            assert!(!payload.contains(value));
        }
    }

    #[test]
    fn is_allowed_key_accepts_known_credential_name() {
        assert!(is_allowed_key("openai_api_key"));
    }

    #[test]
    fn is_allowed_key_rejects_unknown_key_and_path_traversal_attempts() {
        assert!(!is_allowed_key("not_a_real_key"));
        assert!(!is_allowed_key(""));
        assert!(!is_allowed_key("../etc/passwd"));
    }

    #[test]
    fn debug_output_redacts_secret_values() {
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-openai-secret".to_string());
        store.aws_secret_key = Some("aws-secret-value".to_string());
        store.aws_session_token = Some("aws-session-token".to_string());

        let debug = format!("{store:?}");

        assert!(debug.contains("openai_api_key"));
        assert!(debug.contains("<present>"));
        assert!(!debug.contains("sk-openai-secret"));
        assert!(!debug.contains("aws-secret-value"));
        assert!(!debug.contains("aws-session-token"));
    }

    #[test]
    fn yaml_backend_round_trips_store_from_explicit_path() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-credentials-backend-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let backend = YamlCredentialBackend::with_path(path.clone());
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-openai".to_string());
        store.aws_secret_key = Some("aws-secret".to_string());

        backend.save(&store).expect("save explicit yaml backend");
        let loaded = backend.load().expect("load explicit yaml backend");

        assert_eq!(backend.source_label(), "credentials_yaml");
        assert_eq!(loaded.openai_api_key.as_deref(), Some("sk-openai"));
        assert_eq!(loaded.aws_secret_key.as_deref(), Some("aws-secret"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn yaml_backend_missing_file_returns_empty_store() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-missing-credentials-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let backend = YamlCredentialBackend::with_path(path.clone());

        let loaded = backend.load().expect("missing file is first-run empty");

        assert!(loaded.openai_api_key.is_none());
        assert!(loaded.aws_secret_key.is_none());
    }

    #[test]
    fn yaml_backend_malformed_file_surfaces_error() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-bad-credentials-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, "openai_api_key: [not valid").expect("write malformed yaml");
        let backend = YamlCredentialBackend::with_path(path.clone());

        let err = backend.load().expect_err("malformed yaml is surfaced");

        assert!(err.contains("Failed to parse"));
        assert!(err.contains("credentials"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn malformed_credentials_yaml_parse_error_never_leaks_key_fragments() {
        // audio-graph-4243 / cred-review M4: a serde_yaml Display echoes the
        // offending scalar. credentials.yaml holds plaintext keys, so a
        // malformed hand-edit could echo a key fragment into the surfaced
        // CredentialFileError.reason (UI banners + app log). The location-only
        // formatter must strip that. A bare top-level scalar deserialized into
        // the CredentialStore struct produces exactly the leaky shape
        // (`invalid type: string "<value>", expected struct ...`).
        const SENTINEL: &str = "LEAKCANARY-sk-not-real-abc123def456";
        let path = std::env::temp_dir().join(format!(
            "audio-graph-cred-leak-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, format!("\"{SENTINEL}\"\n")).expect("write leaky malformed yaml");
        let backend = YamlCredentialBackend::with_path(path.clone());

        let err = backend
            .load()
            .expect_err("bare scalar is not a valid CredentialStore");

        // The surfaced reason must NOT contain the sentinel (nor a slice of it).
        assert!(
            !err.contains(SENTINEL),
            "surfaced parse error leaked the key sentinel: {err}"
        );
        assert!(
            !err.contains("LEAKCANARY"),
            "surfaced parse error leaked a key fragment: {err}"
        );
        // It should still be recognizable as a parse failure with a location.
        assert!(err.contains("Failed to parse"), "reason: {err}");
        assert!(err.contains("content omitted"), "reason: {err}");

        // Prove the leak was real: the raw serde_yaml Display DOES echo the
        // scalar, so the redaction is load-bearing, not a no-op on this input.
        let raw = serde_yaml::from_str::<CredentialStore>(&format!("\"{SENTINEL}\"\n"))
            .expect_err("raw parse fails")
            .to_string();
        assert!(
            raw.contains(SENTINEL),
            "guard assumption broke: raw serde error no longer echoes the scalar: {raw}"
        );

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn default_backend_uses_yaml_path_in_unit_tests() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-default-credentials-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::default(),
            yaml: YamlCredentialBackend::with_path(path.clone()),
            state: CredentialMigrationStateBackend::with_path(path.with_extension("state.yaml")),
            file_backend: true,
            fallback_to_yaml: true,
        };
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-file".to_string());

        backend.save(&store).expect("save through default backend");
        let snapshot = backend.load_with_source().expect("load with source");

        assert_eq!(snapshot.source, "credentials_yaml");
        assert_eq!(snapshot.store.openai_api_key.as_deref(), Some("sk-file"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn backend_mode_defaults_to_os_keychain_without_plaintext_fallback() {
        assert_eq!(
            credential_backend_mode_from_env(None),
            CredentialBackendMode::OsKeychain
        );
        assert_eq!(
            credential_backend_mode_from_env(Some("")),
            CredentialBackendMode::OsKeychain
        );
        assert_eq!(
            credential_backend_mode_from_env(Some("os_keychain")),
            CredentialBackendMode::OsKeychain
        );
    }

    #[test]
    fn backend_mode_requires_explicit_file_or_keychain_fallback() {
        assert_eq!(
            credential_backend_mode_from_env(Some("credentials_yaml")),
            CredentialBackendMode::File
        );
        assert_eq!(
            credential_backend_mode_from_env(Some("file_fallback")),
            CredentialBackendMode::File
        );
        assert_eq!(
            credential_backend_mode_from_env(Some("keychain_with_file_fallback")),
            CredentialBackendMode::KeychainWithFileFallback
        );
        assert_eq!(
            credential_backend_mode_from_env(Some("OS_KEYCHAIN_WITH_FILE_FALLBACK")),
            CredentialBackendMode::KeychainWithFileFallback
        );
    }

    #[test]
    fn credential_snapshot_labels_imported_yaml_keys_as_imported_file() {
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-keychain".to_string());
        store.deepgram_api_key = Some("dg-imported".to_string());
        store.aws_secret_key = Some("   ".to_string());

        let mut key_sources = credential_source_map_from_store(&store, "os_keychain");
        key_sources.insert("deepgram_api_key", "imported_file");
        let snapshot = CredentialSnapshot::with_key_sources(store, "os_keychain", key_sources);

        assert_eq!(snapshot.source_for("openai_api_key"), "os_keychain");
        assert_eq!(snapshot.source_for("deepgram_api_key"), "imported_file");
        assert_eq!(snapshot.source_for("aws_secret_key"), "missing");
        assert_eq!(snapshot.source_for("revai_api_key"), "missing");
    }

    #[test]
    fn credential_snapshot_labels_filtered_fallback_keys_as_file_fallback() {
        let mut store = CredentialStore::default();
        store.deepgram_api_key = Some("dg-fallback".to_string());
        store.aws_region = Some("us-east-1".to_string());

        let snapshot = CredentialSnapshot::new(store, "file_fallback");

        assert_eq!(snapshot.source_for("deepgram_api_key"), "file_fallback");
        assert_eq!(snapshot.source_for("aws_region"), "file_fallback");
        assert_eq!(snapshot.source_for("openai_api_key"), "missing");
    }

    #[test]
    fn credential_snapshot_does_not_label_tombstoned_yaml_keys_present() {
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-stale".to_string());
        store.deepgram_api_key = Some("dg-legacy".to_string());
        store.aws_region = Some("us-east-1".to_string());

        let mut state = CredentialMigrationState::default();
        state.mark_migrated("openai_api_key");
        state.mark_deleted("aws_region");
        state
            .clear_tracked_keys(&mut store)
            .expect("filter tracked");

        let snapshot = CredentialSnapshot::new(store, "file_fallback");

        assert_eq!(snapshot.source_for("openai_api_key"), "missing");
        assert_eq!(snapshot.source_for("aws_region"), "missing");
        assert_eq!(snapshot.source_for("deepgram_api_key"), "file_fallback");
    }

    #[test]
    fn yaml_backend_set_preserves_malformed_file() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-bad-set-credentials-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let original = "openai_api_key: [not valid";
        fs::write(&path, original).expect("write malformed yaml");
        let backend = YamlCredentialBackend::with_path(path.clone());

        let err = backend
            .set("openai_api_key", "sk-new")
            .expect_err("set should reject malformed yaml");

        assert!(err.contains("Failed to parse"));
        assert_eq!(fs::read_to_string(&path).expect("read yaml"), original);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn yaml_backend_delete_preserves_malformed_file() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-bad-delete-credentials-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let original = "openai_api_key: [not valid";
        fs::write(&path, original).expect("write malformed yaml");
        let backend = YamlCredentialBackend::with_path(path.clone());

        let err = backend
            .delete("openai_api_key")
            .expect_err("delete should reject malformed yaml");

        assert!(err.contains("Failed to parse"));
        assert_eq!(fs::read_to_string(&path).expect("read yaml"), original);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn keychain_backend_rejects_unknown_key_before_opening_platform_store() {
        let err = KeychainCredentialBackend::default()
            .get("not_a_real_key")
            .expect_err("unknown key must be rejected by allowlist");

        assert!(err.contains("Unknown credential key"));
    }

    #[test]
    fn fake_unavailable_keychain_returns_diagnostics_without_fallback() {
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-fake-keychain-unavailable-{}",
            uuid::Uuid::new_v4()
        ));
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(FakeUnavailableKeychainStore::default()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state: CredentialMigrationStateBackend::with_path(state_path.clone()),
            file_backend: false,
            fallback_to_yaml: false,
        };

        let err = match backend.load_with_source() {
            Ok(_) => panic!("unavailable keychain should fail when fallback is disabled"),
            Err(err) => err,
        };

        assert!(err.contains("fake OS keychain unavailable"));
        assert!(err.contains("read credential openai_api_key"));
        assert!(!err.contains("sk-"));
        assert!(!err.contains("dummy"));
        assert!(!yaml_path.exists());
        assert!(!state_path.exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[ignore]
    fn os_keychain_smoke_save_import_delete_tombstone_and_redaction() {
        const RUN_ENV: &str = "AUDIO_GRAPH_RUN_OS_KEYCHAIN_SMOKE";
        if std::env::var(RUN_ENV).ok().as_deref() != Some("1") {
            eprintln!("skipping ignored OS keychain smoke; set {RUN_ENV}=1 to run it");
            return;
        }

        let run_id = uuid::Uuid::new_v4().simple().to_string();
        let service = format!("audio-graph-test-os-keychain-smoke-{run_id}");
        let account_namespace = format!("smoke-{run_id}:provider");
        assert_ne!(service, OsKeychainStore::SERVICE);

        let os_store = ScopedOsKeychainStore::new(service, account_namespace);
        let _cleanup = ScopedOsKeychainCleanup {
            store: os_store.clone(),
        };

        let dir = std::env::temp_dir().join(format!("audio-graph-os-keychain-smoke-{run_id}"));
        fs::create_dir_all(&dir).expect("create smoke temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");

        let keychain_secret = format!("dummy-keychain-{run_id}");
        let yaml_secret = format!("dummy-imported-{run_id}");
        let yaml_region = format!("dummy-region-{run_id}");
        let original_yaml = format!("deepgram_api_key: {yaml_secret}\naws_region: {yaml_region}\n");
        fs::write(&yaml_path, &original_yaml).expect("write smoke credentials.yaml");

        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(os_store.clone()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state: CredentialMigrationStateBackend::with_path(state_path),
            file_backend: false,
            fallback_to_yaml: false,
        };
        let plaintext_values = [
            keychain_secret.as_str(),
            yaml_secret.as_str(),
            yaml_region.as_str(),
        ];

        backend
            .set("openai_api_key", &keychain_secret)
            .expect("save key to scoped OS keychain");

        let imported = backend
            .load_with_source()
            .expect("load scoped OS keychain and import missing YAML keys");

        assert_eq!(imported.source, "os_keychain");
        assert_eq!(imported.source_for("openai_api_key"), "os_keychain");
        assert_eq!(imported.source_for("deepgram_api_key"), "imported_file");
        assert_eq!(imported.source_for("aws_region"), "imported_file");
        assert!(imported.store.openai_api_key.as_deref() == Some(keychain_secret.as_str()));
        assert!(imported.store.deepgram_api_key.as_deref() == Some(yaml_secret.as_str()));
        assert!(imported.store.aws_region.as_deref() == Some(yaml_region.as_str()));
        assert_eq!(
            fs::read_to_string(&yaml_path).expect("legacy credentials.yaml remains readable"),
            original_yaml
        );

        let presence_payload = serialize_smoke_presence(&imported);
        assert_payload_omits_plaintext(&presence_payload, &plaintext_values);

        let error = backend
            .get("not_a_real_key")
            .expect_err("unknown key should produce non-secret diagnostic");
        let error_payload =
            serde_json::to_string(&serde_json::json!({ "source": "error", "error": error }))
                .expect("serialize non-secret smoke error");
        assert_payload_omits_plaintext(&error_payload, &plaintext_values);

        backend
            .delete("deepgram_api_key")
            .expect("delete imported OS keychain credential");

        let reloaded = backend
            .load_with_source()
            .expect("reload after tombstoned delete");

        assert_eq!(reloaded.source, "os_keychain");
        assert_eq!(reloaded.source_for("openai_api_key"), "os_keychain");
        assert_eq!(reloaded.source_for("deepgram_api_key"), "missing");
        assert_eq!(reloaded.source_for("aws_region"), "os_keychain");
        assert!(reloaded.store.deepgram_api_key.is_none());
        assert!(
            os_store
                .get_key("deepgram_api_key")
                .expect("read deleted smoke OS key")
                .is_none()
        );
        // audio-graph-79aa (symmetric writer): the delete clears the deleted
        // key's plaintext entry from credentials.yaml too, so the deleted
        // secret no longer lingers on disk (and cannot resurrect if the state
        // file is lost). The unrelated aws_region entry is preserved. Assert
        // semantically via a reload rather than exact-string equality, since the
        // clear rewrites the file through the serde serializer.
        let post_delete_yaml = YamlCredentialBackend::with_path(yaml_path.clone())
            .load()
            .expect("reload smoke credentials.yaml after delete");
        assert!(
            post_delete_yaml.deepgram_api_key.is_none(),
            "delete must clear the deleted key's plaintext credentials.yaml entry"
        );
        assert_eq!(
            post_delete_yaml.aws_region.as_deref(),
            Some(yaml_region.as_str()),
            "delete must preserve unrelated credentials.yaml entries"
        );
        // The deleted plaintext secret must no longer appear anywhere in the file.
        let raw_post_delete =
            fs::read_to_string(&yaml_path).expect("legacy credentials.yaml remains readable");
        assert!(
            !raw_post_delete.contains(yaml_secret.as_str()),
            "the deleted plaintext secret must be gone from credentials.yaml"
        );

        let reloaded_presence_payload = serialize_smoke_presence(&reloaded);
        assert_payload_omits_plaintext(&reloaded_presence_payload, &plaintext_values);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fake_unavailable_keychain_fallback_labels_filtered_yaml_values() {
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-fake-keychain-fallback-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        fs::write(
            &yaml_path,
            [
                "openai_api_key: dummy-filtered-openai-key",
                "deepgram_api_key: dummy-file-fallback-deepgram-key",
                "aws_region: dummy-file-fallback-region",
            ]
            .join("\n"),
        )
        .expect("write yaml");
        let state = CredentialMigrationStateBackend::with_path(state_path.clone());
        state
            .mark_migrated("openai_api_key")
            .expect("mark migrated key");
        state.mark_deleted("aws_region").expect("mark deleted key");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(FakeUnavailableKeychainStore::default()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state,
            file_backend: false,
            fallback_to_yaml: true,
        };

        let snapshot = backend.load_with_source().expect("load fallback snapshot");

        assert_eq!(snapshot.source, "file_fallback");
        assert_eq!(snapshot.source_for("deepgram_api_key"), "file_fallback");
        assert_eq!(snapshot.source_for("openai_api_key"), "missing");
        assert_eq!(snapshot.source_for("aws_region"), "missing");
        assert_eq!(snapshot.store.present_count(), 1);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fallback_delete_tombstone_masks_recovered_keychain_value() {
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-fallback-delete-tombstone-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        fs::write(&yaml_path, "openai_api_key: dummy-yaml-key\n").expect("write yaml");

        let unavailable_backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(FakeUnavailableKeychainStore::default()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state: CredentialMigrationStateBackend::with_path(state_path.clone()),
            file_backend: false,
            fallback_to_yaml: true,
        };

        unavailable_backend
            .delete("openai_api_key")
            .expect("fallback delete should update YAML and tombstone state");

        let recovered_keychain = FakeKeychainStore::default();
        recovered_keychain.set_initial("openai_api_key", "dummy-stale-keychain-key");
        let recovered_backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(recovered_keychain),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state: CredentialMigrationStateBackend::with_path(state_path),
            file_backend: false,
            fallback_to_yaml: true,
        };

        assert_eq!(
            recovered_backend
                .get("openai_api_key")
                .expect("tombstone masks recovered keychain get"),
            None
        );
        let snapshot = recovered_backend
            .load_with_source()
            .expect("tombstone masks recovered keychain load");
        assert_eq!(snapshot.source_for("openai_api_key"), "missing");
        assert!(snapshot.store.openai_api_key.is_none());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fake_keychain_import_labels_yaml_values_as_imported_file_source() {
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-fake-keychain-source-labels-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        fs::write(
            &yaml_path,
            [
                "deepgram_api_key: dummy-imported-deepgram-key",
                "aws_region: dummy-imported-region",
            ]
            .join("\n"),
        )
        .expect("write yaml");
        let fake = FakeKeychainStore::default();
        fake.set_initial("openai_api_key", "dummy-keychain-openai-key");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake),
            yaml: YamlCredentialBackend::with_path(yaml_path),
            state: CredentialMigrationStateBackend::with_path(dir.join("credentials-state.yaml")),
            file_backend: false,
            fallback_to_yaml: true,
        };

        let snapshot = backend.load_with_source().expect("load import snapshot");

        assert_eq!(snapshot.source, "os_keychain");
        assert_eq!(snapshot.source_for("openai_api_key"), "os_keychain");
        assert_eq!(snapshot.source_for("deepgram_api_key"), "imported_file");
        assert_eq!(snapshot.source_for("aws_region"), "imported_file");
        assert_eq!(snapshot.source_for("aws_secret_key"), "missing");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn edited_credentials_yaml_overrides_migrated_keychain_value() {
        // BUG 7fc5: once a key is migrated to the keychain, hand-editing its
        // plaintext value in credentials.yaml must NOT be silently ignored.
        // A present, non-empty file value overrides the stale keychain copy on
        // both the snapshot loader and the single-key getter.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-migrated-yaml-override-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");

        // deepgram_api_key lives in the keychain with the old value and is
        // marked migrated; the user has since edited the file to a new value.
        fs::write(&yaml_path, "deepgram_api_key: dg-edited-in-file\n").expect("write yaml");
        let state = CredentialMigrationStateBackend::with_path(state_path.clone());
        state
            .mark_migrated("deepgram_api_key")
            .expect("mark migrated key");

        let fake = FakeKeychainStore::default();
        fake.set_initial("deepgram_api_key", "dg-stale-keychain");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake),
            yaml: YamlCredentialBackend::with_path(yaml_path),
            state,
            file_backend: false,
            fallback_to_yaml: false,
        };

        let snapshot = backend.load_with_source().expect("load override snapshot");
        assert_eq!(
            snapshot.store.deepgram_api_key.as_deref(),
            Some("dg-edited-in-file"),
            "edited credentials.yaml value must win over the migrated keychain copy"
        );
        assert_eq!(snapshot.source_for("deepgram_api_key"), "file_override");

        assert_eq!(
            backend
                .get("deepgram_api_key")
                .expect("single-key get honors file override"),
            Some("dg-edited-in-file".to_string()),
            "single-key get must match the snapshot loader's file-override precedence"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_after_preexisting_yaml_entry_clears_shadow_and_returns_new_value() {
        // audio-graph-79aa (the Deepgram-401 loop): a key that lived in
        // credentials.yaml BEFORE migration leaves a stale plaintext entry.
        // `migrated_overrides_from_yaml` makes that non-empty file entry BEAT the
        // keychain, so a user who re-saves a rotated key writes the new value to
        // the keychain but every read still returns the stale file value -> a
        // permanent 401 with a "successful" save. `set` must now clear the stale
        // file entry in the same critical section as the keychain write.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-save-clears-yaml-shadow-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");

        // Pre-migration state: the stale key sits in credentials.yaml, the same
        // value was migrated into the keychain, and state marks it migrated.
        fs::write(
            &yaml_path,
            "deepgram_api_key: dg-stale-preexisting\nopenai_api_key: sk-other-untouched\n",
        )
        .expect("write yaml");
        let state = CredentialMigrationStateBackend::with_path(state_path.clone());
        state
            .mark_migrated("deepgram_api_key")
            .expect("mark migrated key");

        let fake = FakeKeychainStore::default();
        fake.set_initial("deepgram_api_key", "dg-stale-preexisting");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake.clone()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state,
            file_backend: false,
            fallback_to_yaml: false,
        };

        // The user rotates the key in the app: save a fresh value.
        backend
            .set("deepgram_api_key", "dg-fresh-rotated")
            .expect("save rotated key");

        // The keychain holds the fresh value...
        assert_eq!(
            fake.value("deepgram_api_key").as_deref(),
            Some("dg-fresh-rotated")
        );
        // ...and the stale plaintext entry is gone from credentials.yaml, so it
        // can no longer shadow the fresh value.
        let raw_yaml = YamlCredentialBackend::with_path(yaml_path.clone());
        assert_eq!(
            raw_yaml
                .load()
                .expect("reload yaml")
                .deepgram_api_key
                .as_deref(),
            None,
            "the stale credentials.yaml entry must be cleared by the save"
        );
        // Sibling keys in the same file are preserved (clear touches only the
        // saved key).
        assert_eq!(
            raw_yaml
                .load()
                .expect("reload yaml")
                .openai_api_key
                .as_deref(),
            Some("sk-other-untouched"),
            "unrelated credentials.yaml entries must be preserved"
        );

        // The single-key get returns the NEW value, sourced from the keychain
        // (not a file_override), which is the whole point.
        assert_eq!(
            backend.get("deepgram_api_key").expect("get after rotate"),
            Some("dg-fresh-rotated".to_string()),
            "get must return the freshly saved value, not the stale shadow"
        );
        let snapshot = backend.load_with_source().expect("load after rotate");
        assert_eq!(
            snapshot.store.deepgram_api_key.as_deref(),
            Some("dg-fresh-rotated")
        );
        assert_eq!(
            snapshot.source_for("deepgram_api_key"),
            "os_keychain",
            "with the shadow cleared, the source is the keychain, not file_override"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn repeated_probe_after_save_does_not_resurrect_stale_yaml_value() {
        // The readiness probe (`load_with_source`) reads through the same
        // file-override precedence and re-imports untracked YAML keys. After a
        // save clears the stale entry, no number of probes may bring it back
        // (the 401 loop was the probe validating the stale key on every mount).
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-probe-no-resurrect-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        fs::write(&yaml_path, "deepgram_api_key: dg-stale-preexisting\n").expect("write yaml");
        let state = CredentialMigrationStateBackend::with_path(state_path.clone());
        state
            .mark_migrated("deepgram_api_key")
            .expect("mark migrated key");
        let fake = FakeKeychainStore::default();
        fake.set_initial("deepgram_api_key", "dg-stale-preexisting");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake),
            yaml: YamlCredentialBackend::with_path(yaml_path),
            state,
            file_backend: false,
            fallback_to_yaml: false,
        };

        backend
            .set("deepgram_api_key", "dg-fresh-rotated")
            .expect("save rotated key");

        for _ in 0..5 {
            let snapshot = backend.load_with_source().expect("probe reload");
            assert_eq!(
                snapshot.store.deepgram_api_key.as_deref(),
                Some("dg-fresh-rotated"),
                "no probe may resurrect the cleared stale value"
            );
            assert_eq!(snapshot.source_for("deepgram_api_key"), "os_keychain");
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn hand_edit_after_save_still_overrides_keychain() {
        // The 7fc5 feature must survive the 79aa fix: a hand-edit made AFTER a
        // save (a genuinely newer plaintext value) must still override the
        // keychain. The fix only clears the file DURING a save, so a later edit
        // is once again honored as a deliberate override.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-hand-edit-after-save-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        fs::write(&yaml_path, "deepgram_api_key: dg-stale-preexisting\n").expect("write yaml");
        let state = CredentialMigrationStateBackend::with_path(state_path.clone());
        state
            .mark_migrated("deepgram_api_key")
            .expect("mark migrated key");
        let fake = FakeKeychainStore::default();
        fake.set_initial("deepgram_api_key", "dg-stale-preexisting");
        let yaml = YamlCredentialBackend::with_path(yaml_path.clone());
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake),
            yaml: yaml.clone(),
            state,
            file_backend: false,
            fallback_to_yaml: false,
        };

        // Save through the app: clears the stale file entry.
        backend
            .set("deepgram_api_key", "dg-saved-via-app")
            .expect("save via app");
        assert_eq!(
            backend.get("deepgram_api_key").expect("get after save"),
            Some("dg-saved-via-app".to_string())
        );

        // Now the user deliberately hand-edits credentials.yaml to a NEWER value
        // (e.g. debugging with a temporary key). That edit must win again.
        yaml.set("deepgram_api_key", "dg-hand-edited-newer")
            .expect("hand-edit the file after the save");

        assert_eq!(
            backend
                .get("deepgram_api_key")
                .expect("get after hand-edit"),
            Some("dg-hand-edited-newer".to_string()),
            "a hand-edit made AFTER a save must still override the keychain (7fc5 preserved)"
        );
        let snapshot = backend.load_with_source().expect("load after hand-edit");
        assert_eq!(snapshot.source_for("deepgram_api_key"), "file_override");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_clears_yaml_and_survives_lost_state_file() {
        // audio-graph-79aa symmetric writer: delete must clear the plaintext
        // credentials.yaml entry too, not just tombstone in state. If it only
        // tombstoned, a lost/reset credentials-state.yaml would let the import
        // path resurrect the deleted key from its leftover plaintext value.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-delete-clears-yaml-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        fs::write(
            &yaml_path,
            "deepgram_api_key: dg-stale-preexisting\nopenai_api_key: sk-keep-me\n",
        )
        .expect("write yaml");
        let state = CredentialMigrationStateBackend::with_path(state_path.clone());
        state
            .mark_migrated("deepgram_api_key")
            .expect("mark migrated key");
        let fake = FakeKeychainStore::default();
        fake.set_initial("deepgram_api_key", "dg-stale-preexisting");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake.clone()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state,
            file_backend: false,
            fallback_to_yaml: false,
        };

        backend
            .delete("deepgram_api_key")
            .expect("delete migrated key");

        // Keychain entry gone, plaintext entry gone, sibling preserved.
        assert!(fake.value("deepgram_api_key").is_none());
        let raw_yaml = YamlCredentialBackend::with_path(yaml_path.clone());
        assert_eq!(
            raw_yaml
                .load()
                .expect("reload yaml")
                .deepgram_api_key
                .as_deref(),
            None,
            "delete must clear the plaintext credentials.yaml entry"
        );
        assert_eq!(
            raw_yaml
                .load()
                .expect("reload yaml")
                .openai_api_key
                .as_deref(),
            Some("sk-keep-me"),
            "delete must not touch unrelated credentials.yaml entries"
        );

        // Simulate a lost/reset state file: the tombstone is gone. With the
        // plaintext entry cleared there is nothing left to resurrect.
        let _ = fs::remove_file(&state_path);
        let fresh_state = CredentialMigrationStateBackend::with_path(state_path.clone());
        let recovered_backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake),
            yaml: YamlCredentialBackend::with_path(yaml_path),
            state: fresh_state,
            file_backend: false,
            fallback_to_yaml: false,
        };
        assert_eq!(
            recovered_backend
                .get("deepgram_api_key")
                .expect("get after state loss"),
            None,
            "a deleted key must not resurrect from yaml even after the state file is lost"
        );
        let snapshot = recovered_backend
            .load_with_source()
            .expect("load after state loss");
        assert_eq!(snapshot.source_for("deepgram_api_key"), "missing");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_backend_save_keeps_yaml_as_primary_store() {
        // Invariant (1) from audio-graph-79aa: in file_backend mode the yaml IS
        // the primary store. A save writes the value INTO credentials.yaml and
        // must never clear it — clearing here would drop the only copy.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-file-backend-keeps-yaml-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(FakeKeychainStore::default()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state: CredentialMigrationStateBackend::with_path(state_path),
            file_backend: true,
            fallback_to_yaml: false,
        };

        backend
            .set("deepgram_api_key", "dg-file-backend-value")
            .expect("save in file_backend mode");

        // The value lives in credentials.yaml (the primary store) and reads back.
        let raw_yaml = YamlCredentialBackend::with_path(yaml_path);
        assert_eq!(
            raw_yaml
                .load()
                .expect("reload yaml")
                .deepgram_api_key
                .as_deref(),
            Some("dg-file-backend-value"),
            "file_backend mode must keep the value in credentials.yaml"
        );
        assert_eq!(
            backend
                .get("deepgram_api_key")
                .expect("get in file_backend"),
            Some("dg-file-backend-value".to_string())
        );

        // And a delete in file_backend mode clears it from the yaml store.
        backend
            .delete("deepgram_api_key")
            .expect("delete in file_backend mode");
        assert_eq!(
            backend.get("deepgram_api_key").expect("get after delete"),
            None
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fallback_save_keeps_yaml_when_keychain_unavailable() {
        // Invariant (1) continued: in keychain-with-file-fallback mode, when the
        // keychain is unavailable the save goes to yaml (the fallback primary
        // store). The clear_key path only runs on a SUCCESSFUL keychain write,
        // so the fallback value must survive.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-fallback-keeps-yaml-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(FakeUnavailableKeychainStore::default()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state: CredentialMigrationStateBackend::with_path(state_path),
            file_backend: false,
            fallback_to_yaml: true,
        };

        backend
            .set("deepgram_api_key", "dg-fallback-value")
            .expect("save via yaml fallback");

        let raw_yaml = YamlCredentialBackend::with_path(yaml_path);
        assert_eq!(
            raw_yaml
                .load()
                .expect("reload yaml")
                .deepgram_api_key
                .as_deref(),
            Some("dg-fallback-value"),
            "keychain-unavailable fallback save must keep the value in credentials.yaml"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yaml_clear_key_is_noop_when_file_absent() {
        // clear_key must never CREATE a plaintext credentials file: a keychain
        // save on a machine that never had a credentials.yaml must not write one
        // out just to clear a non-existent entry.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-clear-key-noop-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let yaml = YamlCredentialBackend::with_path(yaml_path.clone());

        yaml.clear_key("deepgram_api_key")
            .expect("clear on absent file is a no-op");
        assert!(
            !yaml_path.exists(),
            "clear_key must not create a plaintext credentials.yaml"
        );

        // And clearing a key that isn't in an existing file leaves it untouched.
        fs::write(&yaml_path, "openai_api_key: sk-keep\n").expect("write yaml");
        yaml.clear_key("deepgram_api_key")
            .expect("clear on absent key is a no-op");
        assert_eq!(
            yaml.load().expect("reload yaml").openai_api_key.as_deref(),
            Some("sk-keep"),
            "clearing an absent key must not disturb the file"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_source_and_present_count_stay_coherent() {
        // cred-review n3: `source_for` returns "missing" for an absent OR
        // present-but-whitespace key, and a real source only for a genuinely
        // present key — and `present_count` must agree with that split. This
        // coherence was previously only exercised by the ignored OS-keychain
        // smoke test's payload builder; pin it directly here.
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-real".to_string()); // present
        store.deepgram_api_key = Some("   ".to_string()); // whitespace → absent
        // gemini_api_key left None → absent

        let snapshot = CredentialSnapshot::new(store, "credentials_yaml");

        assert_eq!(
            snapshot.source_for("openai_api_key"),
            "credentials_yaml",
            "a genuinely present key reports its source"
        );
        assert_eq!(
            snapshot.source_for("deepgram_api_key"),
            "missing",
            "a whitespace-only key is not present, so its source is 'missing'"
        );
        assert_eq!(
            snapshot.source_for("gemini_api_key"),
            "missing",
            "an absent key reports 'missing'"
        );

        // present_count counts exactly the keys source_for calls non-missing.
        assert_eq!(
            snapshot.store.present_count(),
            1,
            "only openai_api_key is genuinely present"
        );
        let non_missing = ALLOWED_CREDENTIAL_KEYS
            .iter()
            .filter(|&&key| snapshot.source_for(key) != "missing")
            .count();
        assert_eq!(
            non_missing,
            snapshot.store.present_count(),
            "source_for's non-missing set must equal present_count"
        );
    }

    #[test]
    fn file_override_shadow_detection() {
        // cred-review m1: the shadow warning fires only when the keychain holds
        // a real (non-empty) value being masked by the file override. The
        // caller has already filtered to "file value non-empty AND differs from
        // keychain", so this predicate only inspects the keychain side.
        assert!(
            file_override_shadows_keychain_value(Some("dg-rotated-in-app")),
            "a non-empty keychain value shadowed by the file is the rotation-defeat trap"
        );
        assert!(
            !file_override_shadows_keychain_value(None),
            "no keychain value means the file is the sole source, not a shadow"
        );
        assert!(
            !file_override_shadows_keychain_value(Some("")),
            "an empty keychain value is not being meaningfully shadowed"
        );
        assert!(
            !file_override_shadows_keychain_value(Some("   ")),
            "a whitespace-only keychain value is not being meaningfully shadowed"
        );
    }

    #[test]
    fn shadow_warning_dedupes_per_key_and_rearms_on_rotation() {
        // cred-review m1 (dedupe): the shadow warning must fire once per
        // DISTINCT condition, not once per probe. `record_shadow_warning_is_new`
        // returns true the first time a (key, fingerprint) pair is seen, false
        // for an unchanged repeat, and true again when the fingerprint changes
        // (either side rotated). Uses a local state map so the process-wide
        // static and other tests are untouched.
        let state = Mutex::new(BTreeMap::new());

        let fp_v1 = "kc=sha256:aaaaaaaa len=10 file=sha256:bbbbbbbb len=10".to_string();
        // First sighting of the condition: warn.
        assert!(
            record_shadow_warning_is_new(&state, "deepgram_api_key", fp_v1.clone()),
            "the first time a shadow condition is seen it must warn"
        );
        // Same condition on the next probe: suppressed.
        assert!(
            !record_shadow_warning_is_new(&state, "deepgram_api_key", fp_v1.clone()),
            "an unchanged shadow condition must not re-warn on the next probe"
        );
        assert!(
            !record_shadow_warning_is_new(&state, "deepgram_api_key", fp_v1),
            "still suppressed on a third identical probe"
        );

        // Rotation changes the fingerprint → re-arm and warn again.
        let fp_v2 = "kc=sha256:cccccccc len=12 file=sha256:bbbbbbbb len=10".to_string();
        assert!(
            record_shadow_warning_is_new(&state, "deepgram_api_key", fp_v2.clone()),
            "a rotated keychain value is a new condition and must re-warn"
        );
        assert!(
            !record_shadow_warning_is_new(&state, "deepgram_api_key", fp_v2),
            "the new condition is then itself deduped"
        );

        // A different key is tracked independently.
        let other_fp = "kc=sha256:dddddddd len=8 file=sha256:eeeeeeee len=8".to_string();
        assert!(
            record_shadow_warning_is_new(&state, "openai_api_key", other_fp),
            "a distinct key has its own dedupe slot and warns on first sighting"
        );
    }

    #[test]
    fn migrated_yaml_override_does_not_resurrect_deleted_key() {
        // The override must never defeat a delete tombstone: a deleted key with
        // a stale plaintext value in credentials.yaml stays gone.
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-migrated-override-tombstone-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        fs::write(&yaml_path, "deepgram_api_key: dg-stale-file\n").expect("write yaml");
        let state = CredentialMigrationStateBackend::with_path(state_path.clone());
        state
            .mark_deleted("deepgram_api_key")
            .expect("mark deleted key");

        let fake = FakeKeychainStore::default();
        // Keychain still has a residual value the tombstone must mask.
        fake.set_initial("deepgram_api_key", "dg-residual-keychain");
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake),
            yaml: YamlCredentialBackend::with_path(yaml_path),
            state,
            file_backend: false,
            fallback_to_yaml: false,
        };

        let snapshot = backend.load_with_source().expect("load tombstone snapshot");
        assert!(snapshot.store.deepgram_api_key.is_none());
        assert_eq!(snapshot.source_for("deepgram_api_key"), "missing");
        assert_eq!(
            backend.get("deepgram_api_key").expect("get tombstoned key"),
            None,
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn concurrent_probe_and_delete_keeps_tombstone_and_does_not_resurrect_key() -> Result<(), String>
    {
        // audio-graph-cf22 / cred-review M1: the presence probe
        // (`load_with_source`) is a hidden WRITE — it rewrites
        // credentials-state.yaml and imports untracked YAML keys. Before the
        // CREDENTIAL_IO_LOCK, a probe that loaded state (tombstone absent), had a
        // delete write the tombstone under it, then wrote its stale state back,
        // would ERASE the tombstone -> the deleted key resurrects from
        // credentials.yaml on the next load. Hammer probe-vs-delete on N threads
        // and assert the key STAYS deleted afterward.
        use std::sync::Arc;

        let dir = std::env::temp_dir().join(format!(
            "audio-graph-cred-probe-delete-race-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        let state_path = dir.join("credentials-state.yaml");
        // A legacy plaintext value that WOULD resurrect the key if the tombstone
        // were dropped (the import path re-imports untracked YAML keys).
        fs::write(
            &yaml_path,
            "openai_api_key: sk-legacy-should-stay-deleted\n",
        )
        .expect("write yaml");

        let make_backend = || {
            let shared = SharedKeychainStore::default();
            // Seed a keychain value so the key is present + migrated at start.
            shared
                .set_key("openai_api_key", "sk-keychain-initial")
                .expect("seed keychain");
            DefaultCredentialBackend {
                keychain: KeychainCredentialBackend::with_store(shared),
                yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
                state: CredentialMigrationStateBackend::with_path(state_path.clone()),
                file_backend: false,
                fallback_to_yaml: false,
            }
        };

        // First load migrates + imports so the key is tracked as migrated.
        let seed = make_backend();
        seed.state.mark_migrated("openai_api_key")?;
        let backend = Arc::new(make_backend());

        let mut handles = Vec::new();
        // Fire a wave of concurrent probes racing the single authoritative
        // delete. Each probe does the full read-modify-write of the state file.
        for _ in 0..16 {
            let b = Arc::clone(&backend);
            handles.push(std::thread::spawn(move || {
                for _ in 0..25 {
                    let _ = b.load_with_source();
                }
            }));
        }
        {
            let b = Arc::clone(&backend);
            handles.push(std::thread::spawn(move || {
                // Let a few probes run first so the delete lands mid-storm.
                std::thread::sleep(std::time::Duration::from_millis(1));
                b.delete("openai_api_key")
                    .expect("delete under probe storm");
            }));
        }
        for h in handles {
            h.join().expect("thread joined");
        }

        // A final settle-probe (the delete may have raced ahead of some probes).
        let snapshot = backend.load_with_source().expect("final load");
        assert!(
            snapshot.store.openai_api_key.is_none(),
            "deleted key must NOT resurrect after a concurrent probe+delete storm"
        );
        assert_eq!(snapshot.source_for("openai_api_key"), "missing");
        assert_eq!(
            backend.get("openai_api_key").expect("get after race"),
            None,
            "single-key get must also see the surviving tombstone"
        );

        let _ = fs::remove_dir_all(&dir);
        Ok(())
    }

    #[test]
    fn fake_keychain_imports_missing_yaml_without_overwriting_existing_values() {
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-fake-keychain-import-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        fs::write(
            &yaml_path,
            [
                "openai_api_key: sk-yaml",
                "deepgram_api_key: dg-yaml",
                "aws_region: us-east-1",
            ]
            .join("\n"),
        )
        .expect("write yaml");
        let yaml = YamlCredentialBackend::with_path(yaml_path.clone());
        let state = CredentialMigrationStateBackend::with_path(dir.join("credentials-state.yaml"));
        let fake = FakeKeychainStore::default();
        fake.set_initial("openai_api_key", "sk-keychain");
        let keychain = KeychainCredentialBackend::with_store(fake.clone());
        let mut keychain_store = keychain.load().expect("load fake keychain");

        let imported = keychain.import_missing_from_yaml(&mut keychain_store, &yaml, &state);

        assert_eq!(fake.value("openai_api_key").as_deref(), Some("sk-keychain"));
        assert_eq!(fake.value("deepgram_api_key").as_deref(), Some("dg-yaml"));
        assert_eq!(fake.value("aws_region").as_deref(), Some("us-east-1"));
        assert!(imported.contains(&"deepgram_api_key"));
        assert!(imported.contains(&"aws_region"));
        assert!(!imported.contains(&"openai_api_key"));
        assert_eq!(
            keychain_store.openai_api_key.as_deref(),
            Some("sk-keychain")
        );
        assert_eq!(keychain_store.deepgram_api_key.as_deref(), Some("dg-yaml"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn fake_default_backend_delete_tombstone_blocks_yaml_resurrection() {
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-fake-keychain-delete-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        let yaml_path = dir.join("credentials.yaml");
        fs::write(&yaml_path, "openai_api_key: sk-yaml\n").expect("write yaml");
        let fake = FakeKeychainStore::default();
        let backend = DefaultCredentialBackend {
            keychain: KeychainCredentialBackend::with_store(fake.clone()),
            yaml: YamlCredentialBackend::with_path(yaml_path.clone()),
            state: CredentialMigrationStateBackend::with_path(dir.join("credentials-state.yaml")),
            file_backend: false,
            fallback_to_yaml: false,
        };

        let imported = backend.load_with_source().expect("first import");
        assert_eq!(imported.source_for("openai_api_key"), "imported_file");
        assert_eq!(fake.value("openai_api_key").as_deref(), Some("sk-yaml"));

        backend
            .delete("openai_api_key")
            .expect("delete imported key");

        let reloaded = backend
            .load_with_source()
            .expect("reload after tombstoned delete");
        assert_eq!(fake.value("openai_api_key"), None);
        assert_eq!(reloaded.source_for("openai_api_key"), "missing");
        assert!(reloaded.store.openai_api_key.is_none());
        // audio-graph-79aa (symmetric writer): the delete now ALSO clears the
        // key's plaintext entry from credentials.yaml, not just the tombstone.
        // Previously the plaintext `sk-yaml` was left behind and only the state
        // tombstone masked it on read — a latent resurrection risk if the state
        // file were ever lost/reset. Deleting the plaintext too makes the delete
        // durable and removes the leftover secret from disk.
        assert!(
            !fs::read_to_string(&yaml_path)
                .expect("legacy yaml remains readable")
                .contains("sk-yaml"),
            "delete must clear the plaintext credentials.yaml entry, not just tombstone it"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yaml_import_candidates_never_overwrite_existing_keychain_values() {
        let mut keychain_store = CredentialStore::default();
        keychain_store.openai_api_key = Some("sk-keychain".to_string());
        keychain_store.aws_region = Some("  ".to_string());

        let mut file_store = CredentialStore::default();
        file_store.openai_api_key = Some("sk-yaml".to_string());
        file_store.deepgram_api_key = Some("dg-yaml".to_string());
        file_store.aws_region = Some("us-east-1".to_string());
        file_store.aws_secret_key = Some("   ".to_string());
        let state = CredentialMigrationState::default();

        let candidates = missing_credentials_from_yaml(&keychain_store, &file_store, &state);

        assert!(!candidates.iter().any(|(key, _)| *key == "openai_api_key"));
        assert!(
            candidates
                .iter()
                .any(|(key, value)| *key == "deepgram_api_key" && value == "dg-yaml")
        );
        assert!(
            candidates
                .iter()
                .any(|(key, value)| *key == "aws_region" && value == "us-east-1")
        );
        assert!(!candidates.iter().any(|(key, _)| *key == "aws_secret_key"));
    }

    #[test]
    fn yaml_import_candidates_skip_migrated_and_deleted_keys() {
        let keychain_store = CredentialStore::default();
        let mut file_store = CredentialStore::default();
        file_store.openai_api_key = Some("sk-yaml".to_string());
        file_store.deepgram_api_key = Some("dg-yaml".to_string());
        file_store.aws_region = Some("us-east-1".to_string());

        let mut state = CredentialMigrationState::default();
        state.mark_migrated("openai_api_key");
        state.mark_deleted("aws_region");

        let candidates = missing_credentials_from_yaml(&keychain_store, &file_store, &state);

        assert!(!candidates.iter().any(|(key, _)| *key == "openai_api_key"));
        assert!(!candidates.iter().any(|(key, _)| *key == "aws_region"));
        assert!(
            candidates
                .iter()
                .any(|(key, value)| *key == "deepgram_api_key" && value == "dg-yaml")
        );
    }

    #[test]
    fn migration_state_filters_tracked_keys_from_yaml_fallback() {
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-stale".to_string());
        store.deepgram_api_key = Some("dg-legacy".to_string());
        store.aws_region = Some("us-east-1".to_string());

        let mut state = CredentialMigrationState::default();
        state.mark_migrated("openai_api_key");
        state.mark_deleted("aws_region");

        state
            .clear_tracked_keys(&mut store)
            .expect("filter tracked");

        assert!(store.openai_api_key.is_none());
        assert_eq!(store.deepgram_api_key.as_deref(), Some("dg-legacy"));
        assert!(store.aws_region.is_none());
    }

    #[test]
    fn migration_state_keeps_new_file_fallback_writes_visible() {
        let mut state = CredentialMigrationState::default();
        state.mark_migrated("openai_api_key");
        state.mark_deleted("aws_region");

        let mut fallback_store = CredentialStore::default();
        fallback_store.openai_api_key = Some("sk-new-file".to_string());
        fallback_store.aws_region = Some("us-west-2".to_string());

        state.clear_file_fallback_keys(&fallback_store);

        assert!(!state.is_tracked("openai_api_key"));
        assert!(!state.is_tracked("aws_region"));

        let mut filtered = fallback_store.clone();
        state
            .clear_tracked_keys(&mut filtered)
            .expect("filter tracked");

        assert_eq!(filtered.openai_api_key.as_deref(), Some("sk-new-file"));
        assert_eq!(filtered.aws_region.as_deref(), Some("us-west-2"));
    }

    #[test]
    fn migration_state_save_creates_missing_parent_dir() {
        // BUG 381c: save() must create the parent dir before rename, so a
        // state path under a non-existent directory succeeds instead of
        // failing with os error 2 (ENOENT) and spamming a WARN on every load.
        let base = std::env::temp_dir().join(format!(
            "audio-graph-state-missing-parent-{}",
            uuid::Uuid::new_v4()
        ));
        // Intentionally do NOT create `base`; the state file lives under a
        // nested, not-yet-existing subdirectory.
        let state_path = base.join("nested").join("credentials-state.yaml");
        assert!(!state_path.parent().unwrap().exists());

        let backend = CredentialMigrationStateBackend::with_path(state_path.clone());
        backend
            .mark_migrated("openai_api_key")
            .expect("save into a non-existent parent dir must succeed (BUG 381c)");

        assert!(state_path.exists(), "state file should have been written");
        let loaded = backend.load().expect("reload saved state");
        assert!(loaded.migrated_keys.contains("openai_api_key"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn yaml_backend_save_creates_missing_parent_dir() {
        // BUG 381c companion: the YAML credential save path shares the same
        // temp-write + rename idiom and must also create a missing parent dir.
        let base = std::env::temp_dir().join(format!(
            "audio-graph-yaml-missing-parent-{}",
            uuid::Uuid::new_v4()
        ));
        let yaml_path = base.join("nested").join("credentials.yaml");
        assert!(!yaml_path.parent().unwrap().exists());

        let backend = YamlCredentialBackend::with_path(yaml_path.clone());
        let mut store = CredentialStore::default();
        store.openai_api_key = Some("sk-missing-parent".to_string());
        backend
            .save(&store)
            .expect("yaml save into a non-existent parent dir must succeed (BUG 381c)");

        assert!(yaml_path.exists());
        let loaded = backend.load().expect("reload saved yaml");
        assert_eq!(loaded.openai_api_key.as_deref(), Some("sk-missing-parent"));

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn migration_state_backend_round_trips_without_secret_values() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-credential-state-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let backend = CredentialMigrationStateBackend::with_path(path.clone());

        backend
            .mark_migrated("openai_api_key")
            .expect("mark migrated");
        backend.mark_deleted("aws_region").expect("mark deleted");

        let loaded = backend.load().expect("load state");
        assert!(loaded.migrated_keys.contains("openai_api_key"));
        assert!(loaded.deleted_keys.contains("aws_region"));

        let serialized = fs::read_to_string(&path).expect("read state file");
        assert!(serialized.contains("openai_api_key"));
        assert!(serialized.contains("aws_region"));
        assert!(!serialized.contains("sk-"));

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn owner_only_temp_writer_writes_contents() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-credentials-{}.yaml.tmp",
            uuid::Uuid::new_v4()
        ));

        write_owner_only_temp_file(&path, "openai_api_key: sk-test\n").expect("write temp file");

        let contents = fs::read_to_string(&path).expect("read temp file");
        assert_eq!(contents, "openai_api_key: sk-test\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn owner_only_temp_writer_replaces_stale_permissive_file_before_secret_write() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-credentials-stale-{}.yaml.tmp",
            uuid::Uuid::new_v4()
        ));
        fs::write(&path, "stale").expect("write stale temp file");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644))
                .expect("make stale temp permissive");
        }

        write_owner_only_temp_file(&path, "openai_api_key: sk-test\n")
            .expect("replace stale temp file");

        let contents = fs::read_to_string(&path).expect("read temp file");
        assert_eq!(contents, "openai_api_key: sk-test\n");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = fs::remove_file(&path);
    }

    /// FIX 2(a): the ACL-hardening step at the save sites is BEST-EFFORT. This
    /// pins the invariant that a hardening failure can never abort the save:
    /// `set_owner_only` swallows a guaranteed `try_set_owner_only` failure
    /// (a missing path) and returns `()`, so the `save` paths that call it can
    /// never propagate an ACL error via `?`.
    #[test]
    fn set_owner_only_is_best_effort_and_never_aborts() {
        let missing = std::env::temp_dir().join(format!(
            "audio-graph-owner-only-best-effort-{}.tmp",
            uuid::Uuid::new_v4()
        ));
        // A missing path makes the strict variant fail...
        assert!(
            crate::fs_util::try_set_owner_only(&missing).is_err(),
            "strict hardening should fail on a missing path"
        );
        // ...but the best-effort variant used by the save paths returns unit and
        // must not panic — proving an ACL failure cannot abort a save.
        crate::fs_util::set_owner_only(&missing);
    }

    /// FIX 2(a): a full credentials save completes even though the ACL step is
    /// only best-effort. The saved secret is still readable back, confirming the
    /// save is not aborted by (and does not depend on) the hardening result.
    #[test]
    fn save_completes_with_best_effort_hardening() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-credentials-besteffort-save-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let backend = YamlCredentialBackend::with_path(path.clone());
        let mut store = CredentialStore::default();
        store.deepgram_api_key = Some("dg-key".to_string());

        backend
            .save(&store)
            .expect("save must complete even if ACL is best-effort");
        let loaded = backend.load().expect("load saved store");
        assert_eq!(loaded.deepgram_api_key.as_deref(), Some("dg-key"));

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("yaml.tmp"));
    }

    /// FIX 2(b): a pre-existing stale `.tmp` (left by a prior crashed / aborted
    /// write) must NOT block a subsequent full save with "file exists (os error
    /// 80)". The save path removes the known-stale sibling before create_new.
    #[test]
    fn save_succeeds_despite_pre_existing_stale_tmp() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-credentials-stale-blocks-{}.yaml",
            uuid::Uuid::new_v4()
        ));
        let tmp_path = path.with_extension("yaml.tmp");
        // Simulate the leftover from a prior aborted write.
        fs::write(&tmp_path, "leftover-from-crash").expect("seed stale tmp");
        assert!(tmp_path.exists());

        let backend = YamlCredentialBackend::with_path(path.clone());
        let mut store = CredentialStore::default();
        store.deepgram_api_key = Some("dg-after-stale".to_string());

        backend
            .save(&store)
            .expect("stale .tmp must not block the save");

        let loaded = backend.load().expect("load saved store");
        assert_eq!(loaded.deepgram_api_key.as_deref(), Some("dg-after-stale"));
        // The tmp was consumed by the create_new + rename; it should not linger.
        assert!(
            !tmp_path.exists(),
            "stale/temp file should be gone after save"
        );

        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(&tmp_path);
    }

    /// FIX 2(b): `remove_stale_temp` is a no-op when there is nothing to remove,
    /// and cleans a leftover when present.
    #[test]
    fn remove_stale_temp_handles_missing_and_present() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-remove-stale-{}.yaml.tmp",
            uuid::Uuid::new_v4()
        ));
        // Missing -> Ok (no-op).
        remove_stale_temp(&path).expect("missing stale tmp is a no-op");
        // Present -> removed.
        fs::write(&path, "leftover").expect("seed leftover");
        remove_stale_temp(&path).expect("present stale tmp is removed");
        assert!(!path.exists());
    }
}
