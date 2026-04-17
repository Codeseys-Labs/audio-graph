//! Structured error codes for Tauri command handlers.
//!
//! Pilot for loop10 MEDIUM #8. The goal is to give the frontend a JSON payload
//! shaped like `{"code": "credential_missing", "message": {"key": "aws_secret_key"}}`
//! on rejected `invoke(...)` calls so it can:
//!
//!   1. Localize the error message via `i18n` (keyed on `code`).
//!   2. Offer recovery actions (e.g. "Open Settings" for `CredentialMissing`).
//!   3. Categorize for telemetry / user support.
//!
//! This is a **pilot**: only `save_credential_cmd` and `start_transcribe` use
//! it today. Bulk migration of the other ~35 commands is scoped for later loops.
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

use std::fmt;

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

/// Lossy conversion to `String` for call sites that still have
/// `Result<_, String>` signatures in the outer chain. The frontend loses
/// the structured `code`, so prefer bubbling `AppError` end-to-end where
/// possible.
impl From<AppError> for String {
    fn from(e: AppError) -> Self {
        e.to_string()
    }
}

/// Convenience alias so command bodies can write `Result<T>` instead of
/// `std::result::Result<T, AppError>`.
pub type Result<T> = std::result::Result<T, AppError>;

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
        // String round-trip via From<AppError> for String.
        let s: String = AppError::Unknown("boom".to_string()).into();
        assert_eq!(s, "boom");
    }
}
