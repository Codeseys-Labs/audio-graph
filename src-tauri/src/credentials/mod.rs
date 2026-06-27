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

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use zeroize::{Zeroize, ZeroizeOnDrop};

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
            log::debug!(
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
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
    }

    fn save(&self, state: &CredentialMigrationState) -> Result<(), String> {
        let path = self.resolved_path()?;
        let yaml = serde_yaml::to_string(state)
            .map_err(|e| format!("Failed to serialize credential migration state: {}", e))?;
        let tmp_path = path.with_extension("yaml.tmp");

        write_owner_only_temp_file(&tmp_path, &yaml)?;
        crate::fs_util::try_set_owner_only(&tmp_path)?;
        fs::rename(&tmp_path, &path)
            .map_err(|e| format!("Failed to finalize credential migration state: {}", e))?;
        crate::fs_util::try_set_owner_only(&path)?;

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
        serde_yaml::from_str::<CredentialStore>(&contents)
            .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))
    }

    fn save(&self, store: &CredentialStore) -> Result<(), String> {
        let path = self.resolved_path()?;
        let yaml = serde_yaml::to_string(store)
            .map_err(|e| format!("Failed to serialize credentials: {}", e))?;
        let tmp_path = path.with_extension("yaml.tmp");

        write_owner_only_temp_file(&tmp_path, &yaml)?;

        // Set restrictive permissions on the tmp file before rename, in case the
        // rename preserves the source file's permissions on some platforms.
        crate::fs_util::try_set_owner_only(&tmp_path)?;

        fs::rename(&tmp_path, &path)
            .map_err(|e| format!("Failed to finalize credentials: {}", e))?;

        // And again on the final file to be safe.
        crate::fs_util::try_set_owner_only(&path)?;

        log::info!("Credentials saved to {}", path.display());
        Ok(())
    }

    fn set(&self, key: &str, value: &str) -> Result<(), String> {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            log::debug!(
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
            log::debug!(
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
                    Ok(None)
                } else {
                    Ok(Some(value))
                }
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
        if self.file_backend {
            return self.yaml.set(key, value);
        }
        match self.keychain.set(key, value) {
            Ok(()) => self.state.mark_migrated(key).map_err(|e| {
                format!("OS credential saved, but failed to update migration state: {e}")
            }),
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
        if self.file_backend {
            return self.yaml.delete(key);
        }
        match self.keychain.delete(key) {
            Ok(()) => self.state.mark_deleted(key).map_err(|e| {
                format!("OS credential deleted, but failed to update migration state: {e}")
            }),
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

fn write_owner_only_temp_file(path: &Path, contents: &str) -> Result<(), String> {
    // Create the temp file with restrictive permissions FIRST so secrets are
    // never written into a world-readable file between write and chmod.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        if path.exists() {
            fs::remove_file(path)
                .map_err(|e| format!("Failed to remove stale credentials temp: {}", e))?;
        }
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
        if path.exists() {
            fs::remove_file(path)
                .map_err(|e| format!("Failed to remove stale credentials temp: {}", e))?;
        }
        let mut f = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)
            .map_err(|e| format!("Failed to create credentials temp: {}", e))?;
        // On Windows, `set_owner_only` applies an owner-only ACL via icacls.
        // Apply it before writing secret bytes so the temp file is empty during
        // the brief default-ACL window.
        crate::fs_util::try_set_owner_only(path)?;
        f.write_all(contents.as_bytes())
            .map_err(|e| format!("Failed to write credentials: {}", e))?;
    }

    Ok(())
}

pub fn save_credentials(store: &CredentialStore) -> Result<(), String> {
    default_backend().save(store)
}

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
        assert_eq!(
            fs::read_to_string(&yaml_path).expect("legacy credentials.yaml remains readable"),
            original_yaml
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
        assert!(
            fs::read_to_string(&yaml_path)
                .expect("legacy yaml remains")
                .contains("sk-yaml"),
            "first migration wave keeps credentials.yaml intact"
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
}
