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
///
/// For a client that only fetches catalogs. A blocking client an executor job
/// sends from uses [`job_client_builder`] and [`send_admitted`] instead.
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

/// The builder for a blocking client an executor job sends from. It sends
/// nothing the caller did not ask leave for:
///
/// - no automatic redirect: [`send_admitted`] follows a same-origin 307 or
///   308 itself, as one more attempt;
/// - no implicit retry: reqwest 0.13 otherwise resends a request on an HTTP/2
///   protocol NACK (`REFUSED_STREAM`, or a graceful `GOAWAY`) on its own
///   (`reqwest::retry` module docs). `retry::never()` is the documented way to
///   turn that off. The caller's own retry loop sees the refusal instead and
///   asks leave before resending.
///
/// hyper-util still resends a request that never left this machine because
/// the pooled connection it was given was already closed; no byte of that
/// first attempt reached the server, and reqwest exposes no setting for it.
pub(super) fn job_client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .retry(reqwest::retry::never())
}

/// Why [`send_admitted`] returned no response.
#[derive(Debug)]
pub(super) enum SendError {
    /// Leave was refused, before this attempt or a redirect hop.
    Refused(crate::llm::admission::SendRefused),
    /// The request got no response. Kept so a retry loop can classify it.
    Transport(reqwest::Error),
    /// The URL does not parse.
    InvalidUrl,
    /// More same-origin redirects than one chain may hold.
    TooManyRedirects,
}

impl SendError {
    pub(super) fn into_failure(self) -> HttpFailure {
        match self {
            Self::Refused(refused) => HttpFailure::new(refused.report(), INFERENCE),
            Self::Transport(error) => HttpFailure::request(&error, INFERENCE),
            Self::InvalidUrl => HttpFailure::new(Code::RequestRejected, INFERENCE),
            Self::TooManyRedirects => HttpFailure::new(Code::ProtocolError, INFERENCE),
        }
    }
}

/// Send `request(url)` once leave is given, and follow a same-origin 307 or
/// 308 by sending again to its `Location`, asking leave again first: a
/// redirect hop is a new transport attempt. Any other response, a redirect
/// that leaves the origin included, is returned to the caller, which reports
/// a non-success status as a rejected request. Other redirects are not
/// followed: they turn a POST into a GET without its body.
pub(super) fn send_admitted(
    url: &str,
    leave: impl Fn() -> Result<(), crate::llm::admission::SendRefused>,
    request: impl Fn(&str) -> reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, SendError> {
    let mut current = url::Url::parse(url).map_err(|_| SendError::InvalidUrl)?;
    let origin = current.origin();
    for _ in 0..=MAX_REDIRECTS {
        leave().map_err(SendError::Refused)?;
        let response = request(current.as_str())
            .send()
            .map_err(SendError::Transport)?;
        if !matches!(response.status().as_u16(), 307 | 308) {
            return Ok(response);
        }
        let next = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(|location| current.join(location).ok());
        match next {
            Some(next) if next.origin() == origin => current = next,
            _ => return Ok(response),
        }
    }
    Err(SendError::TooManyRedirects)
}

/// Whether an HTTP/2 server refused the request before processing it: a
/// `REFUSED_STREAM` reset or a graceful `GOAWAY`, the two protocol NACKs
/// reqwest's default policy would have resent on its own.
pub(super) fn is_refused_stream(error: &reqwest::Error) -> bool {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(cause) = source {
        if let Some(h2) = cause.downcast_ref::<h2::Error>() {
            return h2.is_remote()
                && ((h2.is_reset() && h2.reason() == Some(h2::Reason::REFUSED_STREAM))
                    || (h2.is_go_away() && h2.reason() == Some(h2::Reason::NO_ERROR)));
        }
        source = cause.source();
    }
    false
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
        } else if is_refused_stream(error) {
            // Not processed: the server said so. Safe to send again.
            Code::ProviderUnavailable
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

    /// An HTTP/2 server (prior knowledge, no TLS) that refuses every stream
    /// with `REFUSED_STREAM`, counting them.
    async fn spawn_refusing_h2() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::Ordering::SeqCst;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind h2 mock");
        let addr = listener.local_addr().expect("local addr");
        let streams = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counted = streams.clone();
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let counted = counted.clone();
                tokio::spawn(async move {
                    let Ok(mut connection) = h2::server::handshake(socket).await else {
                        return;
                    };
                    while let Some(Ok((_request, mut respond))) = connection.accept().await {
                        counted.fetch_add(1, SeqCst);
                        respond.send_reset(h2::Reason::REFUSED_STREAM);
                    }
                });
            }
        });
        (format!("http://{addr}/v1/chat/completions"), streams)
    }

    #[test]
    fn a_job_client_does_not_resend_a_refused_stream_on_its_own() {
        use std::sync::atomic::Ordering::SeqCst;

        let rt = tokio::runtime::Runtime::new().expect("rt");
        let send = |builder: reqwest::blocking::ClientBuilder| {
            let (url, streams) = rt.block_on(spawn_refusing_h2());
            let error = std::thread::spawn(move || {
                let client = builder
                    .http2_prior_knowledge()
                    .build()
                    .expect("build client");
                client
                    .post(&url)
                    .body("{}")
                    .send()
                    .expect_err("every stream is refused")
            })
            .join()
            .expect("client thread");
            (error, streams.load(SeqCst))
        };

        // reqwest's default policy resends a refused stream without asking:
        // what a job client must not do, since a resend skips admission.
        let (_, default_streams) = send(reqwest::blocking::Client::builder());
        assert!(default_streams > 1, "{default_streams}");

        let (error, streams) = send(job_client_builder());
        assert_eq!(streams, 1, "the refusal comes back to the caller instead");
        assert!(is_refused_stream(&error));
        assert_eq!(
            HttpFailure::request(&error, INFERENCE).to_string(),
            "runtime/provider_unavailable"
        );
    }

    #[test]
    fn a_refused_leave_sends_nothing() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};

        // Leave is asked before anything is sent; a refusal sends nothing,
        // so no server is needed (port 9 would refuse a connection).
        let asked = AtomicUsize::new(0);
        let client = job_client_builder().build().expect("build client");
        let result = send_admitted(
            "http://127.0.0.1:9/v1/chat/completions",
            || {
                asked.fetch_add(1, SeqCst);
                Err(crate::llm::admission::SendRefused::Replaced)
            },
            |url| client.post(url),
        );
        assert!(matches!(
            result,
            Err(SendError::Refused(
                crate::llm::admission::SendRefused::Replaced
            ))
        ));
        assert_eq!(asked.load(SeqCst), 1);
    }
}
