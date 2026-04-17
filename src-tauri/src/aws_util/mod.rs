//! Shared AWS SDK configuration helpers.
//!
//! Centralizes `aws_config::SdkConfig` construction so every call site
//! (test_aws_credentials, aws_preflight_probe, start_transcribe → AWS
//! Transcribe) gets the same credential-refresh behavior.
//!
//! # Why this module exists
//!
//! AWS Transcribe streams run for many minutes, sometimes hours. When the
//! user authenticates via STS (Assume Role, SSO, MFA) the session token has
//! a short TTL — one hour is the default. If we build an SDK config once
//! at session start and cache the creds, the SIGV4 signer will keep signing
//! with the stale token until AWS rejects it mid-stream with a cryptic
//! `Signature expired` error and the EventStream dies.
//!
//! `DefaultChain` and `Profile` modes don't have this problem — the SDK's
//! default credential chain handles refresh internally (the profile provider
//! re-reads `~/.aws/credentials` on expiry, SSO refreshes the token, etc.).
//!
//! `AccessKeys` mode *does* have this problem. Before this module, it built
//! a static `Credentials::new(...)` once. This module replaces that with
//! [`YamlRefreshingCredentialsProvider`], which re-reads
//! `~/.config/audio-graph/credentials.yaml` on every SDK credential request
//! whenever a session token is present. Long-term IAM user keys (no session
//! token) still use static credentials since they don't expire.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use aws_config::{BehaviorVersion, SdkConfig};
use aws_credential_types::provider::{
    error::CredentialsError, future, ProvideCredentials, SharedCredentialsProvider,
};
use aws_credential_types::Credentials;

use crate::credentials::{credentials_path, CredentialStore};
use crate::settings::AwsCredentialSource;

/// Build an `SdkConfig` for the requested region + credential source.
///
/// Callers in `commands.rs` and `asr::aws_transcribe` should use this
/// instead of inlining `aws_config::defaults(...)` so they all benefit
/// from the refreshing-credentials behavior in `AccessKeys` mode.
pub async fn build_aws_sdk_config(
    region: &str,
    source: AwsCredentialSource,
) -> Result<SdkConfig, String> {
    let region = aws_config::Region::new(region.to_string());
    match source {
        AwsCredentialSource::DefaultChain => Ok(aws_config::defaults(BehaviorVersion::latest())
            .region(region)
            .load()
            .await),
        AwsCredentialSource::Profile { name } => {
            Ok(aws_config::defaults(BehaviorVersion::latest())
                .profile_name(&name)
                .region(region)
                .load()
                .await)
        }
        AwsCredentialSource::AccessKeys { access_key } => {
            // We need to decide between static (long-term IAM user) and
            // refreshing (STS session token) creds. Peek at the store once
            // to pick the right provider shape. The static case still
            // re-uses the key material captured here. The refreshing case
            // throws away the snapshot and re-reads on every SDK call.
            let store = crate::credentials::load_credentials();
            let secret = store
                .aws_secret_key
                .clone()
                .ok_or_else(|| "AWS secret key not found in credentials store".to_string())?;

            let provider: SharedCredentialsProvider = if store.aws_session_token.is_some() {
                // STS / short-TTL creds: wrap in the refreshing provider so
                // each SDK call picks up the latest yaml contents.
                SharedCredentialsProvider::new(YamlRefreshingCredentialsProvider::new(access_key))
            } else {
                // Long-term IAM user creds: static is fine and matches
                // prior behavior exactly.
                SharedCredentialsProvider::new(Credentials::new(
                    access_key,
                    secret,
                    None,
                    None,
                    "audio-graph",
                ))
            };

            Ok(aws_config::defaults(BehaviorVersion::latest())
                .credentials_provider(provider)
                .region(region)
                .load()
                .await)
        }
    }
}

/// A `ProvideCredentials` implementation that re-reads
/// `~/.config/audio-graph/credentials.yaml` every time the SDK asks for
/// credentials.
///
/// This is the minimum-viable refresh strategy for `AccessKeys` mode with
/// an STS session token: whenever the user updates the yaml on disk (via
/// the Settings UI or `aws sts get-session-token | yq ...`), the next
/// SDK call picks up the new values without tearing down the streaming
/// Transcribe session.
///
/// The `access_key_id` is passed in at construction because it lives in
/// `settings.json`, not `credentials.yaml`. The secret and session token
/// are read from yaml on every call.
#[derive(Debug, Clone)]
pub struct YamlRefreshingCredentialsProvider {
    access_key_id: Arc<String>,
    /// Override the credentials-yaml path for tests. `None` = use the
    /// real `crate::credentials::credentials_path()` resolver.
    yaml_path: Option<Arc<PathBuf>>,
}

impl YamlRefreshingCredentialsProvider {
    /// Construct a provider that reads from the real credentials.yaml.
    pub fn new(access_key_id: impl Into<String>) -> Self {
        Self {
            access_key_id: Arc::new(access_key_id.into()),
            yaml_path: None,
        }
    }

    /// Test-only: point at a specific yaml file instead of the
    /// user's real `~/.config/audio-graph/credentials.yaml`.
    #[cfg(test)]
    pub fn with_path(access_key_id: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Self {
            access_key_id: Arc::new(access_key_id.into()),
            yaml_path: Some(Arc::new(path.into())),
        }
    }

    fn resolve_path(&self) -> Result<PathBuf, String> {
        match &self.yaml_path {
            Some(p) => Ok((**p).clone()),
            None => credentials_path(),
        }
    }

    fn read_once(&self) -> Result<Credentials, CredentialsError> {
        let path = self.resolve_path().map_err(CredentialsError::not_loaded)?;
        load_store_from_path(&path).and_then(|store| {
            let secret = store.aws_secret_key.clone().ok_or_else(|| {
                CredentialsError::not_loaded("AWS secret key not found in credentials.yaml")
            })?;
            let session_token = store.aws_session_token.clone();
            Ok(Credentials::new(
                self.access_key_id.as_str(),
                secret,
                session_token,
                // No expiry is set: a sensible expiry would require
                // parsing the STS response, and we don't have it.
                // Since the SDK will just call us again on the next
                // request, "no expiry" is safe — the next call
                // re-reads disk anyway.
                None,
                "audio-graph-yaml-refresh",
            ))
        })
    }
}

impl ProvideCredentials for YamlRefreshingCredentialsProvider {
    fn provide_credentials<'a>(&'a self) -> future::ProvideCredentials<'a>
    where
        Self: 'a,
    {
        future::ProvideCredentials::ready(self.read_once())
    }
}

/// Read a `CredentialStore` from an explicit path. Mirrors the logic in
/// `credentials::load_credentials` but without the fallback-to-default
/// behavior: if the file is missing or malformed, we surface a real
/// error so the SDK can report it to the user instead of silently
/// signing with empty creds.
fn load_store_from_path(path: &Path) -> Result<CredentialStore, CredentialsError> {
    let contents = std::fs::read_to_string(path).map_err(|e| {
        CredentialsError::not_loaded(format!("Failed to read {}: {}", path.display(), e))
    })?;
    serde_yaml::from_str::<CredentialStore>(&contents).map_err(|e| {
        CredentialsError::invalid_configuration(format!(
            "Failed to parse {}: {}",
            path.display(),
            e
        ))
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Build a unique tempdir for this test. We don't use the `tempfile`
    /// crate because it's not a declared dependency of audio-graph.
    fn unique_tempdir(label: &str) -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!(
            "audio-graph-aws-util-{}-{}-{}-{}",
            label, pid, nanos, n
        ));
        fs::create_dir_all(&dir).expect("create tempdir");
        dir
    }

    fn write_yaml(path: &Path, secret: &str, session_token: Option<&str>) {
        let mut yaml = String::new();
        yaml.push_str(&format!("aws_secret_key: {}\n", secret));
        if let Some(tok) = session_token {
            yaml.push_str(&format!("aws_session_token: {}\n", tok));
        }
        fs::write(path, yaml).expect("write credentials.yaml");
    }

    #[test]
    fn yaml_credentials_provider_reads_latest_disk_value() {
        let dir = unique_tempdir("refresh");
        let yaml = dir.join("credentials.yaml");

        // First write: initial creds.
        write_yaml(&yaml, "SECRET_ONE", Some("TOKEN_ONE"));

        let provider =
            YamlRefreshingCredentialsProvider::with_path("AKIAIOSFODNN7EXAMPLE", yaml.clone());

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        let first = rt
            .block_on(async { provider.provide_credentials().await })
            .expect("first provide_credentials");
        assert_eq!(first.access_key_id(), "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(first.secret_access_key(), "SECRET_ONE");
        assert_eq!(first.session_token(), Some("TOKEN_ONE"));

        // Rewrite yaml with rotated creds; provider should see them on
        // the next call without any cache invalidation from us.
        write_yaml(&yaml, "SECRET_TWO", Some("TOKEN_TWO"));

        let second = rt
            .block_on(async { provider.provide_credentials().await })
            .expect("second provide_credentials");
        assert_eq!(second.access_key_id(), "AKIAIOSFODNN7EXAMPLE");
        assert_eq!(second.secret_access_key(), "SECRET_TWO");
        assert_eq!(second.session_token(), Some("TOKEN_TWO"));

        // Housekeeping (best-effort, never panics on CI quirks).
        let _ = fs::remove_dir_all(&dir);
    }
}
