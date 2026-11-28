//! Classify HTTP failures without retaining a request, headers, body, or source.

use std::fmt;

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

/// reqwest's own default bound on one redirect chain.
const MAX_REDIRECTS: usize = 10;

/// Follow a redirect only within the origin the request was addressed to.
///
/// reqwest drops `Authorization` on a cross-origin hop, so a status from there
/// says nothing about the credential this client sent, and a 307 or 308 would
/// carry the prompt to a host the egress policy never authorized. Stopping
/// hands the 3xx back to the caller, which reports a rejected request.
pub(super) fn same_origin_redirects() -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(|attempt| {
        let same_origin = attempt
            .previous()
            .first()
            .is_some_and(|origin| origin.origin() == attempt.url().origin());
        if !same_origin {
            attempt.stop()
        } else if attempt.previous().len() >= MAX_REDIRECTS {
            attempt.error("too many redirects")
        } else {
            attempt.follow()
        }
    })
}

pub(super) const fn diagnostic(code: Code, context: RuntimeDiagnosticContext) -> RuntimeDiagnostic {
    let (retryable, action) = match code {
        Code::NetworkUnreachable => (true, Action::CheckNetwork),
        Code::Timeout | Code::InvalidResponse => (true, Action::Retry),
        Code::RateLimited | Code::CapacityExhausted | Code::ProviderUnavailable => {
            (true, Action::RetryAfterDelay)
        }
        Code::CredentialRejected => (false, Action::ReenterCredential),
        Code::CredentialExpired => (false, Action::RefreshCredential),
        Code::ProtocolError
        | Code::RequestRejected
        | Code::PermissionDenied
        | Code::InvalidConfiguration => (false, Action::ReviewConfiguration),
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

    /// Classified by the contract's shared status mapping. `credential_sent`
    /// is whether the request carried a provider credential: without one a
    /// 401 is an ordinary rejection, not a rejected credential.
    pub(super) fn status(
        status: StatusCode,
        context: RuntimeDiagnosticContext,
        credential_sent: bool,
    ) -> Self {
        let diagnostic =
            RuntimeErrorDiagnostic::from_http_status(status.as_u16(), credential_sent, context);
        Self {
            diagnostic: diagnostic.into(),
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

/// Canary check for what crosses a returned error, terminal frame, or log.
#[cfg(test)]
pub(super) mod canary {
    pub(crate) fn assert_absent(output: &str, canaries: &[&str]) {
        for canary in canaries {
            assert!(!output.contains(canary), "leaked {canary}: {output}");
        }
    }
}

/// Lets a test fail the next HTTP client builds on its thread, which reqwest
/// gives no way to provoke.
#[cfg(test)]
pub(crate) mod client_build {
    use std::cell::Cell;

    thread_local! {
        static FAILURES: Cell<u32> = const { Cell::new(0) };
    }

    pub(crate) fn fail_next(count: u32) {
        FAILURES.set(count);
    }

    pub(in crate::llm) fn take_failure() -> bool {
        let pending = FAILURES.get();
        FAILURES.set(pending.saturating_sub(1));
        pending > 0
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
        assert_eq!(token(401, true), "runtime/credential_rejected");
        assert_eq!(token(401, false), "runtime/request_rejected");
        assert_eq!(token(403, true), "runtime/permission_denied");
        assert_eq!(token(429, true), "runtime/rate_limited");
    }
}
