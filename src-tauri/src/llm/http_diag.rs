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

    pub(super) fn status(status: StatusCode, context: RuntimeDiagnosticContext) -> Self {
        let code = match status.as_u16() {
            408 => Code::Timeout,
            429 => Code::RateLimited,
            500..=599 => Code::ProviderUnavailable,
            400..=499 => Code::RequestRejected,
            _ => Code::ProtocolError,
        };
        let mut failure = Self::new(code, with_status(context, status.as_u16()));
        failure.schema_rejection = match status.as_u16() {
            400 | 422 => Some(SchemaRejection::Request),
            404 => Some(SchemaRejection::Model),
            _ => None,
        };
        failure
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

#[cfg(test)]
pub(super) mod log_canary {
    use std::cell::RefCell;
    use std::sync::Once;

    thread_local! {
        static LOG: RefCell<Option<String>> = const { RefCell::new(None) };
    }

    struct Logger;

    impl log::Log for Logger {
        fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
            metadata.target().starts_with("audio_graph::llm::")
        }

        fn log(&self, record: &log::Record<'_>) {
            use std::fmt::Write as _;
            if self.enabled(record.metadata()) {
                LOG.with_borrow_mut(|buffer| {
                    if let Some(buffer) = buffer {
                        writeln!(buffer, "{}", record.args()).unwrap();
                    }
                });
            }
        }

        fn flush(&self) {}
    }

    pub(crate) struct Capture;

    impl Capture {
        pub(crate) fn start() -> Self {
            static INIT: Once = Once::new();
            INIT.call_once(|| {
                log::set_logger(&Logger).unwrap();
                log::set_max_level(log::LevelFilter::Trace);
            });
            LOG.with_borrow_mut(|buffer| {
                assert!(buffer.is_none(), "nested log capture");
                *buffer = Some(String::new());
            });
            Self
        }

        pub(crate) fn assert_absent(&self, canaries: &[&str]) {
            LOG.with_borrow(|buffer| {
                let logs = buffer.as_deref().unwrap();
                for canary in canaries {
                    assert!(!logs.contains(canary), "log leaked {canary}: {logs}");
                }
            });
        }

        pub(crate) fn contents(&self) -> String {
            LOG.with_borrow(|buffer| buffer.as_ref().unwrap().clone())
        }
    }

    impl Drop for Capture {
        fn drop(&mut self) {
            LOG.with_borrow_mut(|buffer| *buffer = None);
        }
    }
}
