//! Structured error codes for Tauri command handlers.
//!
//! Structured error boundary for Tauri commands. The goal is to give the
//! frontend a JSON payload
//! shaped like `{"code": "credential_missing", "message": {"key": "aws_secret_key"}}`
//! on rejected `invoke(...)` calls so it can:
//!
//!   1. Localize the error message via `i18n` (keyed on `code`).
//!   2. Offer recovery actions (e.g. "Open Settings" for `CredentialMissing`).
//!   3. Categorize for telemetry / user support.
//!
//! Fallible `#[tauri::command]` handlers should return `crate::error::Result<T>`
//! so legacy string errors are still serialized as `{ "code": "unknown", ... }`
//! at the invoke boundary.
//!
//! ## Serialization shape
//!
//! `#[serde(tag = "code", content = "message", rename_all = "snake_case")]`
//! produces:
//!
//! ```json
//! {"code": "io", "message": "file not found"}
//! {"code": "credential_missing", "message": {"key": "aws_secret_key"}}
//! {"code": "aws_credential_expired", "message": null}
//! ```
//!
//! Tauri's serde integration automatically serializes `Err(AppError)` into
//! this shape when the command returns `Result<T, AppError>`.

use std::{fmt, sync::OnceLock};

use regex::{Captures, Regex};

const REDACTED_SECRET: &str = "<redacted>";

/// Stable prefix marker for a provider-probe message that represents an HTTP
/// 401 (Unauthorized) rejection of the CURRENTLY SAVED credential — as opposed
/// to a generic transport/HTTP failure. `ProviderReadiness.message`
/// (`commands.rs`) is a plain `String` by the time it reaches the frontend
/// (`error.to_string()` collapses any `AppError` variant into text), so this
/// exact text prefix is the only recognizable signal across the IPC boundary.
/// The frontend keys off this prefix (`providerRecoveryAction`,
/// `ProviderReadinessPanel.tsx`) to offer a "fix your key" recovery action
/// (route to credentials settings) instead of the generic retry copy — this
/// mirrors the `Failed to parse {path}:` stable-prefix pattern that
/// `redacted_yaml_parse_error` (`credentials/mod.rs`) already uses for
/// credential-file parse failures, which the frontend's
/// `isCredentialFileParseError` keys off the same way. (audio-graph-57cc)
pub const CREDENTIAL_REJECTED_PREFIX: &str = "Credential rejected (401):";

/// Wrap a provider HTTP-error detail message with
/// [`CREDENTIAL_REJECTED_PREFIX`] when `status` is HTTP 401 (Unauthorized);
/// otherwise return `detail` unchanged. Shared by every provider
/// readiness-probe / connection-test error path (Deepgram, Soniox, the
/// generic OpenAI-compatible arm, AssemblyAI, Gemini, OpenRouter) so the
/// frontend's stable-prefix classifier works uniformly across providers
/// without each call site re-implementing the check. Scoped to 401 only —
/// 403 (Forbidden) and other 4xx/5xx stay generic, matching audio-graph-57cc's
/// literal scope ("distinguish 401 auth-rejected saved key from a generic
/// transport error").
pub fn classify_credential_rejected_message(status: reqwest::StatusCode, detail: String) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        format!("{CREDENTIAL_REJECTED_PREFIX} {detail}")
    } else {
        detail
    }
}

/// Structured application error with a stable machine-readable `code` and a
/// variant-specific `message` payload. See module docs for serialization shape.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum AppError {
    /// Generic I/O failure. Message is the underlying `io::Error` display.
    Io(String),
    /// A credential required for the current operation is not stored.
    /// `key` is the credential-store slot name (e.g. `"aws_secret_key"`).
    CredentialMissing { key: String },
    /// Writing / reading the on-disk credential file failed.
    CredentialFileError { reason: String },
    /// AWS STS reports the current credentials have expired.
    AwsCredentialExpired,
    /// A configured AWS region is empty or not a recognized region string.
    AwsRegionInvalid { region: String },
    /// Gemini API returned HTTP 429.
    GeminiRateLimited,
    /// A local model file was expected but is missing on disk.
    ModelNotFound { name: String },
    /// The selected provider was compiled out of this build.
    ProviderUnavailable {
        provider: String,
        required_feature: String,
    },
    /// Runtime privacy mode blocks a content-bearing provider call.
    PrivacyPolicyBlocked {
        mode: String,
        action: String,
        provider: String,
        data_classes: Vec<String>,
        reason: String,
    },
    /// Session state precondition violated (e.g. capture not running).
    SessionInvalid { reason: String },
    /// A network call to `service` exceeded its timeout.
    NetworkTimeout { service: String },
    /// Catch-all for errors not yet migrated to a typed variant.
    Unknown(String),
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AppError::Io(msg) => write!(f, "I/O error: {}", msg),
            AppError::CredentialMissing { key } => {
                write!(f, "Credential missing: {}", key)
            }
            AppError::CredentialFileError { reason } => {
                write!(f, "Credential file error: {}", reason)
            }
            AppError::AwsCredentialExpired => write!(f, "AWS credentials have expired"),
            AppError::AwsRegionInvalid { region } => {
                write!(f, "Invalid AWS region: {}", region)
            }
            AppError::GeminiRateLimited => write!(f, "Gemini API rate limited"),
            AppError::ModelNotFound { name } => write!(f, "Model not found: {}", name),
            AppError::ProviderUnavailable {
                provider,
                required_feature,
            } => write!(
                f,
                "{} is unavailable in this build; rebuild with {}",
                provider, required_feature
            ),
            AppError::PrivacyPolicyBlocked {
                mode,
                action,
                provider,
                data_classes,
                reason,
            } => write!(
                f,
                "Privacy policy blocked {} for {} in mode {} ({}): {}",
                action,
                provider,
                mode,
                data_classes.join(", "),
                reason
            ),
            AppError::SessionInvalid { reason } => {
                write!(f, "Invalid session state: {}", reason)
            }
            AppError::NetworkTimeout { service } => {
                write!(f, "Network timeout calling {}", service)
            }
            AppError::Unknown(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for AppError {}

impl From<std::io::Error> for AppError {
    fn from(e: std::io::Error) -> Self {
        AppError::Io(e.to_string())
    }
}

impl From<String> for AppError {
    fn from(message: String) -> Self {
        AppError::Unknown(message)
    }
}

impl From<&str> for AppError {
    fn from(message: &str) -> Self {
        AppError::Unknown(message.to_string())
    }
}

/// Lossy conversion to `String` for older helper boundaries that still need a
/// plain error message. The frontend loses the structured `code`, so prefer
/// bubbling `AppError` end-to-end where possible.
impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.to_string()
    }
}

/// Convenience alias so command bodies can write `Result<T>` instead of
/// `std::result::Result<T, AppError>`.
pub type Result<T> = std::result::Result<T, AppError>;

/// Redact secrets before returning provider diagnostics to UI-visible errors.
///
/// This keeps status codes, provider names, and response context intact while
/// preventing echoing endpoints from reflecting submitted credentials back into
/// React state or logs.
pub fn redact_known_secrets<I, S>(message: &str, secrets: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    let mut redacted = message.to_string();
    for secret in secrets {
        let secret = secret.as_ref().trim();
        if secret.len() >= 4 {
            redacted = redacted.replace(secret, REDACTED_SECRET);
        }
    }

    redacted
}

fn credential_field_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?ix)
            (
                \b(?:authorization|api[_-]?key|access[_-]?token|refresh[_-]?token|
                    id[_-]?token|client[_-]?secret|secret|token)\b
                ["']?
                \s*[:=]\s*
                (?:(?:bearer|token)\s+)?
                ["']?
            )
            [^"',;&}]+
            "#,
        )
        .expect("credential field regex compiles")
    })
}

fn auth_scheme_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)(\b(?:bearer|token)\s+)[A-Za-z0-9._~+/=-]{8,}"#)
            .expect("auth scheme regex compiles")
    })
}

fn url_query_credential_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r#"(?i)([?&](?:api[_-]?key|key|token|access[_-]?token|refresh[_-]?token|secret)=)[^&#\s"']+"#,
        )
            .expect("url query credential regex compiles")
    })
}

fn url_userinfo_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"(?i)((?:https?|wss?)://)[^/@\s"']+@"#).expect("url userinfo regex compiles")
    })
}

fn aws_access_key_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r#"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"#).expect("aws access key regex compiles")
    })
}

fn sk_token_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"\bsk-[A-Za-z0-9][A-Za-z0-9._-]{8,}\b"#).expect("sk regex"))
}

fn redact_with_prefix(input: &str, regex: &Regex) -> String {
    regex
        .replace_all(input, |caps: &Captures<'_>| {
            let prefix = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            format!("{prefix}{REDACTED_SECRET}")
        })
        .into_owned()
}

fn redact_known_secret_patterns(message: &str) -> String {
    let redacted = redact_with_prefix(message, credential_field_regex());
    let redacted = redact_with_prefix(&redacted, auth_scheme_regex());
    let redacted = redact_with_prefix(&redacted, url_query_credential_regex());
    let redacted = url_userinfo_regex()
        .replace_all(&redacted, |caps: &Captures<'_>| {
            let scheme = caps.get(1).map(|m| m.as_str()).unwrap_or_default();
            format!("{scheme}{REDACTED_SECRET}@")
        })
        .into_owned();
    let redacted = aws_access_key_regex()
        .replace_all(&redacted, REDACTED_SECRET)
        .into_owned();
    sk_token_regex()
        .replace_all(&redacted, REDACTED_SECRET)
        .into_owned()
}

/// Redact known secrets and return a bounded character excerpt for provider
/// response bodies.
pub fn redacted_error_excerpt<I, S>(body: &str, secrets: I, max_chars: usize) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    redact_known_secret_patterns(&redact_known_secrets(body, secrets))
        .chars()
        .take(max_chars)
        .collect()
}

/// Redact a bounded diagnostic string before it can reach UI-visible events or
/// logs. Use this for WebSocket close reasons, protocol errors, and transport
/// errors where the text is not strictly an HTTP response body.
pub fn redacted_provider_diagnostic<I, S>(message: &str, secrets: I) -> String
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    redacted_error_excerpt(message, secrets, 500)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_unit_variant_with_code_only() {
        // serde's internally-tagged enum with `content` omits the content key
        // for unit variants rather than emitting `"message": null`. The
        // frontend's `AppErrorPayload` treats the `message` field as optional
        // for these variants.
        let err = AppError::AwsCredentialExpired;
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "aws_credential_expired",
            })
        );
    }

    #[test]
    fn serializes_rate_limited_unit_variant() {
        let err = AppError::GeminiRateLimited;
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json, serde_json::json!({"code": "gemini_rate_limited"}));
    }

    #[test]
    fn serializes_struct_variant_with_object_message() {
        let err = AppError::CredentialMissing {
            key: "aws_secret_key".to_string(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "credential_missing",
                "message": { "key": "aws_secret_key" },
            })
        );
    }

    #[test]
    fn serializes_provider_unavailable_with_recovery_feature() {
        let err = AppError::ProviderUnavailable {
            provider: "LocalWhisper".to_string(),
            required_feature: "local-ml or asr-whisper".to_string(),
        };
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "provider_unavailable",
                "message": {
                    "provider": "LocalWhisper",
                    "required_feature": "local-ml or asr-whisper",
                },
            })
        );
    }

    #[test]
    fn serializes_newtype_variant_with_string_message() {
        let err = AppError::Io("disk full".to_string());
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "io",
                "message": "disk full",
            })
        );
    }

    #[test]
    fn io_error_converts_via_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let app_err: AppError = io_err.into();
        match app_err {
            AppError::Io(msg) => assert!(msg.contains("no such file")),
            other => panic!("expected AppError::Io, got {:?}", other),
        }
    }

    #[test]
    fn legacy_string_errors_convert_to_unknown_payload() {
        let app_err: AppError = "legacy failure".to_string().into();
        let json = serde_json::to_value(&app_err).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "code": "unknown",
                "message": "legacy failure",
            })
        );
    }

    #[test]
    fn display_formats_all_variants_readably() {
        assert_eq!(
            AppError::CredentialMissing {
                key: "gemini_api_key".to_string()
            }
            .to_string(),
            "Credential missing: gemini_api_key"
        );
        assert_eq!(
            AppError::AwsRegionInvalid {
                region: "xx-fake-1".to_string()
            }
            .to_string(),
            "Invalid AWS region: xx-fake-1"
        );
        assert_eq!(
            AppError::AwsCredentialExpired.to_string(),
            "AWS credentials have expired"
        );
        assert_eq!(
            AppError::ProviderUnavailable {
                provider: "LocalLlama".to_string(),
                required_feature: "local-ml or llm-llama".to_string(),
            }
            .to_string(),
            "LocalLlama is unavailable in this build; rebuild with local-ml or llm-llama"
        );
        // String round-trip via From<AppError> for String.
        let s: String = AppError::Unknown("boom".to_string()).into();
        assert_eq!(s, "boom");
    }

    #[test]
    fn redacts_known_secrets_from_provider_errors() {
        let body = r#"{"error":"invalid token sk-test-provider-secret-123"}"#;
        let redacted = redact_known_secrets(body, ["sk-test-provider-secret-123"]);

        assert!(!redacted.contains("sk-test-provider-secret-123"));
        assert!(redacted.contains(REDACTED_SECRET));
        assert!(redacted.contains("invalid token"));
    }

    #[test]
    fn redacted_error_excerpt_redacts_before_truncating() {
        let body = format!(
            "{}{}",
            "x".repeat(180),
            " sk-test-provider-secret-123 echoed"
        );
        let redacted = redacted_error_excerpt(&body, ["sk-test-provider-secret-123"], 200);

        assert!(!redacted.contains("sk-test-provider-secret-123"));
        assert!(redacted.contains(REDACTED_SECRET));
        assert!(redacted.chars().count() <= 200);
    }

    #[test]
    fn redacted_error_excerpt_redacts_common_provider_secret_patterns() {
        let body = concat!(
            r#"{"api_key":"sk-live-provider-secret-12345","#,
            r#""authorization":"Bearer bearer-token-secret-12345","#,
            r#""access_token":"access-token-secret-12345"} "#,
            "url=https://user:pass@example.com/v1?api_key=query-secret-12345 ",
            "ws=wss://ws-user:ws-pass@example.com/v1?token=ws-token-secret-12345 ",
            "aws=AKIA1234567890ABCDEF"
        );
        let redacted = redacted_error_excerpt(body, std::iter::empty::<&str>(), 1000);

        for leaked in [
            "sk-live-provider-secret-12345",
            "bearer-token-secret-12345",
            "access-token-secret-12345",
            "user:pass",
            "ws-user:ws-pass",
            "ws-token-secret-12345",
            "query-secret-12345",
            "AKIA1234567890ABCDEF",
        ] {
            assert!(
                !redacted.contains(leaked),
                "redacted excerpt leaked {leaked}: {redacted}"
            );
        }
        assert!(redacted.matches(REDACTED_SECRET).count() >= 5);
        assert!(redacted.contains("example.com"));
    }

    #[test]
    fn redacted_provider_diagnostic_bounds_websocket_close_reasons() {
        let key = "dg-websocket-secret";
        let message = format!(
            "Close(Some(CloseFrame {{ code: Policy, reason: \"bad token {key} Authorization: Bearer bearer-secret-12345 wss://user:pass@example.com?api_key=url-secret-12345 aws AKIA1234567890ABCDEF {}\" }}))",
            "x".repeat(700)
        );

        let redacted = redacted_provider_diagnostic(&message, [key]);

        assert!(!redacted.contains(key));
        assert!(!redacted.contains("bearer-secret-12345"));
        assert!(!redacted.contains("user:pass"));
        assert!(!redacted.contains("url-secret-12345"));
        assert!(!redacted.contains("AKIA1234567890ABCDEF"));
        assert!(redacted.contains(REDACTED_SECRET));
        assert!(redacted.chars().count() <= 500);
    }

    #[test]
    fn classify_credential_rejected_message_prefixes_401() {
        let message = classify_credential_rejected_message(
            reqwest::StatusCode::UNAUTHORIZED,
            "Deepgram returned HTTP 401 Unauthorized".to_string(),
        );

        assert!(message.starts_with(CREDENTIAL_REJECTED_PREFIX));
        // The original detail is preserved after the prefix, not discarded —
        // downstream diagnostics/tests that grep for provider/status context
        // must still find it.
        assert!(message.contains("Deepgram returned HTTP 401 Unauthorized"));
    }

    #[test]
    fn classify_credential_rejected_message_leaves_non_401_unchanged() {
        // 403 (Forbidden), 429 (rate-limited), and 5xx are NOT a rejected
        // credential in the audio-graph-57cc sense (a valid key can be
        // rate-limited or a transient 5xx can hit a fine key) — only 401 gets
        // the stable marker.
        for status in [
            reqwest::StatusCode::FORBIDDEN,
            reqwest::StatusCode::TOO_MANY_REQUESTS,
            reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        ] {
            let detail = format!("provider returned HTTP {status}");
            let message = classify_credential_rejected_message(status, detail.clone());

            assert_eq!(
                message, detail,
                "non-401 status {status} must pass the detail through unchanged"
            );
            assert!(!message.starts_with(CREDENTIAL_REJECTED_PREFIX));
        }
    }
}
