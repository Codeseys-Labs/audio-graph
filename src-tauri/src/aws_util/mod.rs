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

// ---------------------------------------------------------------------------
// UI-facing AWS error taxonomy (ag#13)
// ---------------------------------------------------------------------------

/// Structured classification of aws-sdk errors for the frontend.
///
/// The goal is to replace raw SDK strings like `DispatchFailure(...)` or
/// `Unable to refresh credentials. error=...` with a category the frontend
/// can localize and attach recovery hints to.
///
/// Kept as an `enum` (not a string) so the mapping is exhaustive at the
/// type system level — adding a new category forces an update to every
/// call-site that matches on it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "category", rename_all = "snake_case")]
pub enum UiAwsError {
    /// Access key ID is unknown to AWS. User needs to re-enter it.
    InvalidAccessKey,
    /// Secret key doesn't match the access key — signature validation failed.
    SignatureMismatch,
    /// STS session token has expired — user needs to refresh via their IdP.
    ExpiredToken,
    /// Credentials are valid but the principal lacks the required action.
    /// `permission` is the action name parsed out of the AWS message if present
    /// (e.g. `"transcribe:StartStreamTranscription"`).
    AccessDenied { permission: Option<String> },
    /// The target service isn't enabled in this region, or the region name
    /// itself was rejected.
    RegionNotSupported { region: String },
    /// Could not reach the AWS endpoint at all (DNS/TLS/connect failure).
    NetworkUnreachable,
    /// Fallback for errors that don't match any known AWS code.
    Unknown { message: String },
}

/// Classify a formatted aws-sdk error string into a [`UiAwsError`].
///
/// The aws-sdk-rust error types carry their AWS error code inside the
/// `DisplayErrorContext` wrapper, which is what `format!("{}", e)` produces
/// via the `SdkError::into_service_error()` / `ProvideErrorMetadata` trail.
/// Parsing on the displayed string keeps this classifier decoupled from the
/// SDK's concrete error types (which vary per service crate) and future-proof
/// against minor version bumps.
///
/// `region` is passed in rather than parsed out of the error because it's
/// easier to obtain at the call-site from the active `AwsCredentialSource`
/// context than to reliably pluck out of a free-form AWS message.
pub fn classify_aws_error(raw: &str, region: Option<&str>) -> UiAwsError {
    let lower = raw.to_lowercase();

    // Network/transport failures — no service response at all.
    // Check these first: a DispatchFailure will often contain the word
    // "region" too (wrong region can surface as DNS failure), but we want
    // the network classification to win when there was no HTTP response.
    if lower.contains("dispatchfailure")
        || lower.contains("dispatch failure")
        || lower.contains("io error")
        || lower.contains("connection refused")
        || lower.contains("dns error")
        || lower.contains("failed to lookup address")
        || lower.contains("timed out")
        || lower.contains("network is unreachable")
        || lower.contains("no route to host")
        || lower.contains("connection reset")
    {
        // If the message *explicitly names* the configured region, the
        // transport failure was almost certainly because the region itself
        // doesn't have the service — a made-up region like `us-fake-1`
        // surfaces as DNS lookup failure of a hostname containing that
        // region slug.
        if let Some(r) = region {
            if !r.trim().is_empty() && lower.contains(&r.to_lowercase()) {
                return UiAwsError::RegionNotSupported {
                    region: r.to_string(),
                };
            }
        }
        return UiAwsError::NetworkUnreachable;
    }

    // IAM permissions — the `User: arn:... is not authorized to perform: X`
    // pattern is emitted by aws-sdk-rust without necessarily including the
    // literal "AccessDenied" code in the Display output, so probe for the
    // phrasing before falling through to code-based matching.
    if lower.contains("not authorized to perform") {
        let permission = extract_action_from_access_denied(raw);
        return UiAwsError::AccessDenied { permission };
    }

    // Expired session token — both spellings appear in the wild
    // (StsError::ExpiredToken vs ExpiredTokenException).
    if lower.contains("expiredtoken")
        || lower.contains("expired token")
        || lower.contains("the security token included in the request is expired")
    {
        return UiAwsError::ExpiredToken;
    }

    // Access key unknown to AWS. Two codes both indicate this:
    //   InvalidClientTokenId — seen from STS
    //   InvalidAccessKeyId   — seen from most other services
    // UnrecognizedClient also sometimes surfaces with the same root cause,
    // but we keep it in the region bucket below since it also fires when
    // signing against the wrong regional endpoint.
    if lower.contains("invalidclienttokenid")
        || lower.contains("invalidaccesskeyid")
        || lower.contains("invalid access key id")
    {
        return UiAwsError::InvalidAccessKey;
    }

    // Secret key mismatch — signature didn't validate.
    if lower.contains("signaturedoesnotmatch") || lower.contains("signature does not match") {
        return UiAwsError::SignatureMismatch;
    }

    // IAM permissions — key + secret are correct but the principal can't
    // perform the action. Try to pull the action name out of the message;
    // AWS formats these as `User: arn:... is not authorized to perform: <action>`
    // or `not authorized to perform <action>`.
    if lower.contains("accessdenied") || lower.contains("access denied") {
        let permission = extract_action_from_access_denied(raw);
        return UiAwsError::AccessDenied { permission };
    }

    // UnrecognizedClient + anything explicitly mentioning "region" that got
    // this far (i.e. was not a network failure) → region not supported.
    if lower.contains("unrecognizedclient") || lower.contains("region") {
        let region_value = region.unwrap_or("").to_string();
        return UiAwsError::RegionNotSupported {
            region: region_value,
        };
    }

    UiAwsError::Unknown {
        message: raw.to_string(),
    }
}

/// Extract the `<action>` substring from an AWS AccessDenied message.
/// Returns `None` if the standard `not authorized to perform: <action>`
/// pattern isn't present — the frontend still has a generic fallback.
fn extract_action_from_access_denied(raw: &str) -> Option<String> {
    // Format variants observed from the SDK:
    //   "... is not authorized to perform: transcribe:StartStreamTranscription ..."
    //   "... not authorized to perform transcribe:StartStreamTranscription on ..."
    let needle_colon = "not authorized to perform: ";
    let needle_space = "not authorized to perform ";
    let start = raw
        .find(needle_colon)
        .map(|i| i + needle_colon.len())
        .or_else(|| raw.find(needle_space).map(|i| i + needle_space.len()))?;
    let tail = &raw[start..];
    // Action is the first whitespace-delimited token, trimmed of punctuation
    // the SDK sometimes appends (trailing "." or ",").
    let token: String = tail
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect::<String>()
        .trim_end_matches(['.', ',', ';', ':', ')', '(', '"'])
        .to_string();
    if token.is_empty() {
        None
    } else {
        Some(token)
    }
}

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

    #[test]
    fn yaml_credentials_provider_errors_on_missing_file() {
        // Point the provider at a path that does not exist. The SDK should
        // get back a CredentialsError rather than silent empty creds — this
        // is the case that, before the refreshing-provider refactor, would
        // have let the signer sign with whatever was in the static snapshot.
        let dir = unique_tempdir("missing");
        let yaml = dir.join("does-not-exist.yaml");
        assert!(!yaml.exists(), "precondition: yaml must not exist");

        let provider = YamlRefreshingCredentialsProvider::with_path("AKIAIOSFODNN7EXAMPLE", yaml);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        let err = rt
            .block_on(async { provider.provide_credentials().await })
            .expect_err("provide_credentials should fail when yaml file is missing");

        // Missing file surfaces via `not_loaded`. Anything else (e.g.
        // `invalid_configuration`) would hide the actual failure mode from
        // the SDK's retry/backoff logic, so pin the variant explicitly.
        assert!(
            matches!(err, CredentialsError::CredentialsNotLoaded(_)),
            "expected CredentialsNotLoaded, got: {:?}",
            err
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn yaml_credentials_provider_errors_on_malformed_yaml() {
        let dir = unique_tempdir("malformed");
        let yaml = dir.join("credentials.yaml");
        // Deliberately broken YAML — unterminated flow mapping.
        fs::write(&yaml, "not: [valid: yaml:").expect("write malformed yaml");

        let provider = YamlRefreshingCredentialsProvider::with_path("AKIAIOSFODNN7EXAMPLE", yaml);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        let err = rt
            .block_on(async { provider.provide_credentials().await })
            .expect_err("provide_credentials should fail on malformed yaml");

        // Malformed YAML is a configuration error (the file exists but the
        // contents are invalid), not a not-loaded error.
        assert!(
            matches!(err, CredentialsError::InvalidConfiguration(_)),
            "expected InvalidConfiguration, got: {:?}",
            err
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // -----------------------------------------------------------------------
    // ag#13 — classify_aws_error mapping coverage
    //
    // The classifier is the contract between the raw aws-sdk error strings
    // and the frontend's i18n keys. These tests lock in the mapping for the
    // five specific strings from the ag#13 issue body plus the network
    // transport failure so a regression can't silently collapse a real
    // error into the Unknown bucket.
    // -----------------------------------------------------------------------

    #[test]
    fn classify_invalid_access_key_id() {
        // STS and Transcribe both can surface this; check one of each spelling.
        let sts = "service error: InvalidClientTokenId: The security token included \
                   in the request is invalid.";
        assert_eq!(
            classify_aws_error(sts, Some("us-east-1")),
            UiAwsError::InvalidAccessKey
        );

        let transcribe = "Unhandled(Unhandled { source: ErrorMetadata { \
                          code: Some(\"InvalidAccessKeyId\"), message: ... } })";
        assert_eq!(
            classify_aws_error(transcribe, Some("us-east-1")),
            UiAwsError::InvalidAccessKey
        );
    }

    #[test]
    fn classify_signature_and_expired_token() {
        // Secret-key mismatch: different IAM keys signing against the wrong secret.
        let sig = "service error: SignatureDoesNotMatch: The request signature we \
                   calculated does not match the signature you provided.";
        assert_eq!(
            classify_aws_error(sig, Some("us-east-1")),
            UiAwsError::SignatureMismatch
        );

        // STS spelling (ExpiredToken).
        let sts_expired = "service error: ExpiredToken: The security token included \
                           in the request is expired";
        assert_eq!(
            classify_aws_error(sts_expired, Some("us-east-1")),
            UiAwsError::ExpiredToken
        );
        // Non-STS spelling (ExpiredTokenException).
        let other_expired = "code: \"ExpiredTokenException\", message: \"Token has expired\"";
        assert_eq!(
            classify_aws_error(other_expired, Some("us-east-1")),
            UiAwsError::ExpiredToken
        );
    }

    #[test]
    fn classify_access_denied_extracts_permission() {
        // Realistic message from an AWS Transcribe call made with an IAM user
        // that doesn't have transcribe:StartStreamTranscription attached.
        let raw = "User: arn:aws:iam::123456789012:user/tester is not authorized \
                   to perform: transcribe:StartStreamTranscription on resource: *";
        let err = classify_aws_error(raw, Some("us-east-1"));
        assert_eq!(
            err,
            UiAwsError::AccessDenied {
                permission: Some("transcribe:StartStreamTranscription".to_string()),
            }
        );

        // Fallback when the message just says "AccessDenied" without the
        // standard "is not authorized to perform" phrasing.
        let bare = "service error: AccessDenied: access denied.";
        assert_eq!(
            classify_aws_error(bare, Some("us-east-1")),
            UiAwsError::AccessDenied { permission: None }
        );
    }

    #[test]
    fn classify_region_and_network() {
        // UnrecognizedClient often means the client is signing against a
        // region that doesn't have the requested service.
        let region_err = "service error: UnrecognizedClientException: The security \
                          token is not recognized in this region.";
        assert_eq!(
            classify_aws_error(region_err, Some("ap-south-2")),
            UiAwsError::RegionNotSupported {
                region: "ap-south-2".to_string(),
            }
        );

        // Pure transport-layer failure — no HTTP response, no service code.
        let net_err = "dispatch failure: io error: failed to lookup address \
                       information: nodename nor servname provided";
        // When the region string doesn't appear in the error, the transport
        // layer wins and we report NetworkUnreachable rather than incorrectly
        // fingerpointing at the region.
        assert_eq!(
            classify_aws_error(net_err, Some("us-east-1")),
            UiAwsError::NetworkUnreachable
        );

        // Fallback path: non-AWS string should land in Unknown, preserving
        // the original for debugging.
        let other = "something entirely unexpected";
        match classify_aws_error(other, None) {
            UiAwsError::Unknown { message } => assert_eq!(message, other),
            other => panic!("expected Unknown, got {:?}", other),
        }
    }

    #[test]
    fn yaml_credentials_provider_errors_on_missing_secret_key() {
        let dir = unique_tempdir("no-secret");
        let yaml = dir.join("credentials.yaml");
        // Valid YAML, but no `aws_secret_key` field. The session token
        // alone is not enough to sign SIGV4 requests.
        fs::write(&yaml, "aws_session_token: SOMETOKEN\n").expect("write yaml");

        let provider = YamlRefreshingCredentialsProvider::with_path("AKIAIOSFODNN7EXAMPLE", yaml);

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime");

        let err = rt
            .block_on(async { provider.provide_credentials().await })
            .expect_err("provide_credentials should fail when secret_key is missing");

        // Missing secret is reported as not-loaded (the file parsed fine,
        // it just doesn't carry the required field).
        assert!(
            matches!(err, CredentialsError::CredentialsNotLoaded(_)),
            "expected CredentialsNotLoaded, got: {:?}",
            err
        );
        // And the underlying source message should mention the secret key
        // so the Settings UI can point the user at the right field.
        // `CredentialsError::Display` renders only the variant description
        // ("the credential provider was not enabled"), so we walk the
        // `source()` chain to reach the inner string we set in
        // `read_once` / `load_store_from_path`.
        use std::error::Error as _;
        let debug_msg = format!("{:?}", err);
        let source_msg = err.source().map(|s| s.to_string()).unwrap_or_default();
        assert!(
            debug_msg.to_lowercase().contains("secret")
                || source_msg.to_lowercase().contains("secret"),
            "error should mention the missing secret key — debug={:?}, source={:?}",
            debug_msg,
            source_msg
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
