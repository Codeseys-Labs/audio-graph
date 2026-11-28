//! Classify HTTP failures without retaining a request, headers, body, or source.

use std::fmt;

use audio_graph_ipc_contract::credential_contract::{
    CredentialError, CredentialErrorCode, CredentialSafeRecoveryAction,
};
use audio_graph_ipc_contract::runtime_diagnostic::{
    RuntimeDiagnostic, RuntimeDiagnosticContext, RuntimeErrorCode as Code, RuntimeErrorDiagnostic,
    RuntimeOperation, RuntimeSafeRecoveryAction as Action, RuntimeStatusClass, RuntimeTransport,
};
use reqwest::StatusCode;

pub(super) const INFERENCE: RuntimeDiagnosticContext =
    http_context(RuntimeOperation::LanguageModelInference);

pub(super) const fn http_context(operation: RuntimeOperation) -> RuntimeDiagnosticContext {
    RuntimeDiagnosticContext::new(operation).with_transport(RuntimeTransport::Http)
}

pub(super) const fn diagnostic(code: Code, context: RuntimeDiagnosticContext) -> RuntimeDiagnostic {
    let (retryable, action) = match code {
        Code::NetworkUnreachable => (true, Action::CheckNetwork),
        Code::Timeout | Code::InvalidResponse => (true, Action::Retry),
        Code::RateLimited | Code::CapacityExhausted | Code::ProviderUnavailable => {
            (true, Action::RetryAfterDelay)
        }
        Code::ProtocolError | Code::RequestRejected => (false, Action::ReviewConfiguration),
        Code::Unsupported => (false, Action::ChooseSupportedProvider),
        Code::PolicyBlocked => (false, Action::ReviewPolicy),
        Code::Cancelled | Code::Internal => (false, Action::None),
    };
    RuntimeDiagnostic::Runtime(RuntimeErrorDiagnostic::new(
        code, retryable, action, context,
    ))
}

pub(super) const fn with_status(
    context: RuntimeDiagnosticContext,
    status: u16,
) -> RuntimeDiagnosticContext {
    match status {
        300..=399 => context.with_status_class(RuntimeStatusClass::Redirect),
        400..=499 => context.with_status_class(RuntimeStatusClass::ClientError),
        500..=599 => context.with_status_class(RuntimeStatusClass::ServerError),
        _ => context,
    }
}

/// Category for an HTTP status, whether it arrived on a response line or as a
/// structured error code. `None` is not a failure status this module knows.
pub(super) const fn status_code(status: u16) -> Option<Code> {
    Some(match status {
        408 => Code::Timeout,
        429 => Code::RateLimited,
        500..=599 => Code::ProviderUnavailable,
        400..=499 => Code::RequestRejected,
        _ => return None,
    })
}

/// Structural evidence that a provider refused the caller's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AuthFailure {
    /// HTTP 401 on a request that carried a credential.
    CredentialRejected,
    /// HTTP 403, or a modeled access denial.
    PermissionDenied,
    /// A modeled expired-credential signal.
    CredentialExpired,
}

/// The only place LLM auth evidence becomes a diagnostic; every HTTP and SDK
/// path routes through it.
///
/// The contract has no provider-auth codes yet. `credential/access_denied`
/// stands in for a rejected credential and the other two remain ordinary
/// rejections; swap these arms when those codes exist.
pub(super) fn auth_diagnostic(
    failure: AuthFailure,
    context: RuntimeDiagnosticContext,
) -> RuntimeDiagnostic {
    match failure {
        AuthFailure::CredentialRejected => CredentialError {
            code: CredentialErrorCode::AccessDenied,
            retryable: false,
            recovery_action: CredentialSafeRecoveryAction::ReenterCredential,
            set_id: None,
        }
        .into(),
        AuthFailure::PermissionDenied | AuthFailure::CredentialExpired => {
            diagnostic(Code::RequestRejected, context)
        }
    }
}

/// Private recovery evidence for the projection schema downgrade. Never
/// rendered or serialized as part of the public diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SchemaRejection {
    Request,
    Model,
}

#[derive(Debug)]
pub(super) struct HttpFailure {
    pub(super) diagnostic: RuntimeDiagnostic,
    pub(super) schema_rejection: Option<SchemaRejection>,
}

impl HttpFailure {
    pub(super) const fn new(code: Code, context: RuntimeDiagnosticContext) -> Self {
        Self {
            diagnostic: diagnostic(code, context),
            schema_rejection: None,
        }
    }

    /// `credential_sent` is whether the request carried a provider credential:
    /// without one a 401 is an ordinary rejection, not a rejected credential.
    pub(super) fn status(
        status: StatusCode,
        context: RuntimeDiagnosticContext,
        credential_sent: bool,
    ) -> Self {
        let context = with_status(context, status.as_u16());
        let diagnostic = match status.as_u16() {
            401 if credential_sent => auth_diagnostic(AuthFailure::CredentialRejected, context),
            403 => auth_diagnostic(AuthFailure::PermissionDenied, context),
            other => diagnostic(status_code(other).unwrap_or(Code::ProtocolError), context),
        };
        Self {
            diagnostic,
            schema_rejection: match status.as_u16() {
                400 | 422 => Some(SchemaRejection::Request),
                404 => Some(SchemaRejection::Model),
                _ => None,
            },
        }
    }

    pub(super) fn request(error: &reqwest::Error, context: RuntimeDiagnosticContext) -> Self {
        let code = if error.is_timeout() {
            Code::Timeout
        } else if error.is_connect() {
            Code::NetworkUnreachable
        } else if error.is_builder() {
            Code::RequestRejected
        } else if error.is_redirect() {
            Code::ProtocolError
        } else {
            Code::Internal
        };
        let mut failure = Self::new(code, context);
        // A pre-header timeout may already have incurred a remote effect.
        if code == Code::Timeout {
            failure.diagnostic =
                RuntimeErrorDiagnostic::new(code, false, Action::None, context).into();
        }
        failure
    }

    pub(super) fn response(error: &reqwest::Error, context: RuntimeDiagnosticContext) -> Self {
        Self::new(
            if error.is_timeout() {
                Code::Timeout
            } else {
                Code::InvalidResponse
            },
            context,
        )
    }
}

impl fmt::Display for HttpFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.diagnostic, f)
    }
}

impl From<HttpFailure> for String {
    fn from(error: HttpFailure) -> Self {
        error.to_string()
    }
}

/// Log capture for canary tests.
///
/// A test binary has one global `log` logger, so this reuses the capture
/// `tts::test_support` installs instead of competing with it; a third harness
/// must join that logger too. Capture is per thread, which keeps parallel tests
/// apart: run the code under test on the capturing thread.
#[cfg(test)]
pub(super) mod log_canary {
    use crate::tts::test_support::{capture_tts_logs, take_tts_logs};

    pub(crate) struct Capture(());

    impl Capture {
        pub(crate) fn start() -> Self {
            capture_tts_logs();
            Self(())
        }

        /// End the capture and return every line logged on this thread.
        pub(crate) fn finish(self) -> String {
            take_tts_logs().join("\n")
        }
    }

    /// Assert that no canary reached a returned error, terminal frame, or log.
    pub(crate) fn assert_absent(output: &str, canaries: &[&str]) {
        for canary in canaries {
            assert!(!output.contains(canary), "leaked {canary}: {output}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_401_is_a_credential_failure_only_for_a_credential_that_was_sent() {
        let token = |status: u16, credential_sent| {
            HttpFailure::status(
                StatusCode::from_u16(status).unwrap(),
                INFERENCE,
                credential_sent,
            )
            .to_string()
        };
        assert_eq!(token(401, true), "credential/access_denied");
        assert_eq!(token(401, false), "runtime/request_rejected");
        // No provider-auth code exists yet for a denied but valid credential.
        assert_eq!(token(403, true), "runtime/request_rejected");
        assert_eq!(token(429, true), "runtime/rate_limited");
        assert_eq!(token(302, true), "runtime/protocol_error");
    }
}
