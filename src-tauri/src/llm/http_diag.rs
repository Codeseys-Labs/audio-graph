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
/// For a client that only fetches catalogs or runs a readiness check. A
/// blocking client an executor job sends from uses [`job_client_builder`] and
/// [`send_admitted`] instead.
pub(crate) fn same_origin_redirects() -> reqwest::redirect::Policy {
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
    /// The chain's time ran out before a send. `sent` is whether any request
    /// of the chain had gone out by then.
    DeadlineExceeded { sent: bool },
}

impl SendError {
    pub(super) fn into_failure(self) -> HttpFailure {
        match self {
            Self::Refused(refused) => HttpFailure::new(refused.report(), INFERENCE),
            Self::Transport(error) => HttpFailure::request(&error, INFERENCE),
            Self::InvalidUrl => HttpFailure::new(Code::RequestRejected, INFERENCE),
            Self::TooManyRedirects => HttpFailure::new(Code::ProtocolError, INFERENCE),
            // Classified as a timeout after a send is in
            // `HttpFailure::request`: the remote effect is unknown.
            Self::DeadlineExceeded { sent: true } => HttpFailure::sent_timeout(INFERENCE),
            Self::DeadlineExceeded { sent: false } => HttpFailure::new(Code::Timeout, INFERENCE),
        }
    }
}

/// Send `request(url)` once leave is given, and follow a same-origin 307 or
/// 308 by sending again to its `Location`, asking leave again first: a
/// redirect hop is a new transport attempt.
///
/// A hop is followed only to the same origin and the same userinfo (user
/// name and password) as `url`: reqwest turns userinfo into an
/// `Authorization` header, so a `Location` that adds, changes or removes it
/// would change the credentials the request carries. A redirect that fails
/// either test, like any other response, is returned to the caller, which
/// reports a non-success status as a rejected request. 301, 302 and 303 are
/// not followed: they turn a POST into a GET without its body.
///
/// Every hop of the chain, from connection to response headers, shares one
/// `budget`: each hop is given the time that remains once leave is given and
/// its request is built. Reading the final response body is not inside it:
/// the blocking client gives that read its own timeout, as it did with the
/// client-level timeout before. A retry by the caller starts a new
/// chain. As under reqwest's policy, at most `MAX_REDIRECTS` requests are
/// sent, the first and nine hops; a tenth redirect ends the chain as too many.
///
/// A hop that an HTTP/2 server refuses unprocessed (see [`is_refused_stream`])
/// is sent again in place, once, after asking leave again and within the same
/// budget; reqwest's own resend stays off. A second refusal is returned.
pub(super) fn send_admitted(
    url: &str,
    budget: std::time::Duration,
    leave: impl Fn() -> Result<(), crate::llm::admission::SendRefused>,
    request: impl Fn(&str) -> reqwest::blocking::RequestBuilder,
) -> Result<reqwest::blocking::Response, SendError> {
    let deadline = std::time::Instant::now() + budget;
    let remaining = || deadline.saturating_duration_since(std::time::Instant::now());
    let mut current = url::Url::parse(url).map_err(|_| SendError::InvalidUrl)?;
    let origin = current.origin();
    let userinfo = (
        current.username().to_owned(),
        current.password().map(str::to_owned),
    );
    let mut sent = false;
    for _ in 0..MAX_REDIRECTS {
        let mut resent = false;
        let response = loop {
            if remaining().is_zero() {
                return Err(SendError::DeadlineExceeded { sent });
            }
            leave().map_err(SendError::Refused)?;
            let builder = request(current.as_str());
            // Leave can wait on the slot's mutex: what is left is measured
            // after it, just before the send.
            let left = remaining();
            if left.is_zero() {
                return Err(SendError::DeadlineExceeded { sent });
            }
            let outcome = builder.timeout(left).send();
            sent = true;
            #[cfg(test)]
            after_send::run();
            match outcome {
                Ok(response) => break response,
                Err(error) if !resent && is_refused_stream(&error) => resent = true,
                Err(error) => return Err(SendError::Transport(error)),
            }
        };
        if !matches!(response.status().as_u16(), 307 | 308) {
            return Ok(response);
        }
        let next = response
            .headers()
            .get(reqwest::header::LOCATION)
            .and_then(|location| location.to_str().ok())
            .and_then(|location| current.join(location).ok());
        match next {
            Some(next)
                if next.origin() == origin
                    && next.username() == userinfo.0
                    && next.password() == userinfo.1.as_deref() =>
            {
                current = next;
            }
            _ => return Ok(response),
        }
    }
    Err(SendError::TooManyRedirects)
}

/// A hook a test runs on the sending thread after each send, to unwind a
/// dial after its request went out.
#[cfg(test)]
pub(crate) mod after_send {
    use std::cell::RefCell;

    thread_local! {
        static HOOK: RefCell<Option<Box<dyn FnMut()>>> = const { RefCell::new(None) };
    }

    pub(crate) fn set(hook: impl FnMut() + 'static) {
        HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
    }

    pub(super) fn run() {
        let hook = HOOK.with(|slot| slot.borrow_mut().take());
        if let Some(mut hook) = hook {
            hook();
            HOOK.with(|slot| *slot.borrow_mut() = Some(hook));
        }
    }
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
        if code == Code::Timeout {
            return Self::sent_timeout(context);
        }
        Self::new(code, context)
    }

    /// A timeout once a request was sent: a remote effect may already have
    /// happened, so it is not offered as safe to retry.
    pub(super) fn sent_timeout(context: RuntimeDiagnosticContext) -> Self {
        Self {
            diagnostic: RuntimeErrorDiagnostic::new(Code::Timeout, false, Action::None, context)
                .into(),
            schema_rejection: None,
        }
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
            std::time::Duration::from_secs(5),
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

    /// An HTTP/1.1 server that answers each connection with the next
    /// `(status, Location)` of `script` after `delay`, and keeps each
    /// request's head. `{addr}` in a location is replaced by its own address.
    async fn spawn_redirect_mock(
        script: Vec<(u16, Option<&'static str>)>,
        delay: std::time::Duration,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind redirect mock");
        let addr = listener.local_addr().expect("local addr").to_string();
        let heads = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = heads.clone();
        let own = addr.clone();
        tokio::spawn(async move {
            for (status, location) in script {
                let Ok((mut stream, _)) = listener.accept().await else {
                    break;
                };
                let mut head = Vec::new();
                let mut chunk = [0u8; 1024];
                while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                    match stream.read(&mut chunk).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => head.extend_from_slice(&chunk[..n]),
                    }
                }
                kept.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&head).into_owned());
                tokio::time::sleep(delay).await;
                let location = location
                    .map(|location| format!("Location: {}\r\n", location.replace("{addr}", &own)))
                    .unwrap_or_default();
                let response = format!(
                    "HTTP/1.1 {status} Scripted\r\n{location}Content-Length: 0\r\nConnection: close\r\n\r\n"
                );
                let _ = stream.write_all(response.as_bytes()).await;
                let _ = stream.shutdown().await;
            }
        });
        (addr, heads)
    }

    /// Send `url` through [`send_admitted`] from a keyless job client, on a
    /// thread of its own (a blocking client cannot run on the mock's runtime).
    fn send_chain(url: String, budget: std::time::Duration) -> Result<u16, SendError> {
        std::thread::spawn(move || {
            let client = job_client_builder().build().expect("build client");
            send_admitted(&url, budget, || Ok(()), |url| client.post(url).body("{}"))
                .map(|response| response.status().as_u16())
        })
        .join()
        .expect("client thread")
    }

    fn authorization_values(head: &str) -> Vec<&str> {
        head.lines()
            .filter(|line| line.to_ascii_lowercase().starts_with("authorization:"))
            .collect()
    }

    #[test]
    fn a_redirect_that_changes_the_userinfo_is_not_followed() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let budget = std::time::Duration::from_secs(10);
        for (base_userinfo, location) in [
            // Adds userinfo.
            ("", "http://plainuser:plainword@{addr}/moved"),
            // Changes it.
            (
                "plainuser:plainword@",
                "http://otheruser:otherword@{addr}/moved",
            ),
            // Changes only the password.
            (
                "plainuser:plainword@",
                "http://plainuser:otherword@{addr}/moved",
            ),
            // Removes it.
            ("plainuser:plainword@", "http://{addr}/moved"),
        ] {
            let (addr, heads) = rt.block_on(spawn_redirect_mock(
                vec![(307, Some(location)), (200, None)],
                std::time::Duration::ZERO,
            ));
            let url = format!("http://{base_userinfo}{addr}/v1/chat/completions");
            let status = send_chain(url, budget).expect("the 307 is returned");
            assert_eq!(status, 307, "{location}");
            assert_eq!(heads.lock().unwrap().len(), 1, "{location}");
        }
    }

    #[test]
    fn a_relative_redirect_on_a_userinfo_base_keeps_the_same_headers() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let (addr, heads) = rt.block_on(spawn_redirect_mock(
            vec![(307, Some("/moved/chat/completions")), (200, None)],
            std::time::Duration::ZERO,
        ));
        let url = format!("http://plainuser:plainword@{addr}/v1/chat/completions");
        let status = send_chain(url, std::time::Duration::from_secs(10)).expect("followed");
        assert_eq!(status, 200);
        let heads = heads.lock().unwrap();
        assert_eq!(heads.len(), 2);
        let first = authorization_values(&heads[0]);
        assert_eq!(first.len(), 1, "{}", heads[0]);
        assert_eq!(authorization_values(&heads[1]), first);
    }

    #[test]
    fn a_redirect_chain_shares_one_deadline() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let (addr, heads) = rt.block_on(spawn_redirect_mock(
            vec![(307, Some("/moved/chat/completions")), (200, None)],
            std::time::Duration::from_millis(650),
        ));
        let url = format!("http://{addr}/v1/chat/completions");
        let error = send_chain(url, std::time::Duration::from_secs(1))
            .expect_err("two 650 ms answers do not fit one second");
        assert_eq!(error.into_failure().to_string(), "runtime/timeout");
        assert!(heads.lock().unwrap().len() <= 2);
    }

    #[test]
    fn a_redirect_chain_sends_at_most_ten_requests() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let (addr, heads) = rt.block_on(spawn_redirect_mock(
            vec![(307, Some("/again")); MAX_REDIRECTS + 2],
            std::time::Duration::ZERO,
        ));
        let url = format!("http://{addr}/v1/chat/completions");
        let error =
            send_chain(url, std::time::Duration::from_secs(10)).expect_err("too many redirects");
        assert!(matches!(error, SendError::TooManyRedirects));
        // The first request and nine hops, as reqwest's policy allowed.
        assert_eq!(heads.lock().unwrap().len(), 10);
    }

    /// What [`spawn_scripted_h2`] does with one stream.
    #[derive(Clone, Copy)]
    enum H2Answer {
        Refuse,
        Status(u16, Option<&'static str>),
    }

    /// An HTTP/2 server (prior knowledge, no TLS) that answers its streams by
    /// `script`, in order, and keeps each stream's path.
    async fn spawn_scripted_h2(
        script: Vec<H2Answer>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind h2 mock");
        let addr = listener.local_addr().expect("local addr");
        let paths = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let kept = paths.clone();
        let script = std::sync::Arc::new(std::sync::Mutex::new(script.into_iter()));
        tokio::spawn(async move {
            while let Ok((socket, _)) = listener.accept().await {
                let (kept, script) = (kept.clone(), script.clone());
                tokio::spawn(async move {
                    let Ok(mut connection) = h2::server::handshake(socket).await else {
                        return;
                    };
                    while let Some(Ok((request, mut respond))) = connection.accept().await {
                        kept.lock().unwrap().push(request.uri().path().to_string());
                        let answer = script.lock().unwrap().next();
                        match answer {
                            Some(H2Answer::Status(status, location)) => {
                                let mut response = tauri::http::Response::builder().status(status);
                                if let Some(location) = location {
                                    response = response.header("location", location);
                                }
                                let _ = respond.send_response(response.body(()).unwrap(), true);
                            }
                            Some(H2Answer::Refuse) | None => {
                                respond.send_reset(h2::Reason::REFUSED_STREAM);
                            }
                        }
                    }
                });
            }
        });
        (format!("http://{addr}/v1/chat/completions"), paths)
    }

    /// Send `url` from an HTTP/2 job client through [`send_admitted`], with
    /// `leave` answering each ask by its number (0 first).
    fn send_h2(
        url: String,
        leave: impl Fn(usize) -> Result<(), crate::llm::admission::SendRefused> + Send + 'static,
    ) -> (Result<u16, SendError>, usize) {
        std::thread::spawn(move || {
            let client = job_client_builder()
                .http2_prior_knowledge()
                .build()
                .expect("build client");
            let asked = std::cell::Cell::new(0);
            let result = send_admitted(
                &url,
                std::time::Duration::from_secs(10),
                || {
                    let ask = asked.get();
                    asked.set(ask + 1);
                    leave(ask)
                },
                |url| client.post(url).body("{}"),
            )
            .map(|response| response.status().as_u16());
            (result, asked.get())
        })
        .join()
        .expect("client thread")
    }

    #[test]
    fn a_refused_stream_is_sent_again_once_after_asking_leave() {
        let rt = tokio::runtime::Runtime::new().expect("rt");

        // Refused, then accepted: two streams, two asks.
        let (url, paths) = rt.block_on(spawn_scripted_h2(vec![
            H2Answer::Refuse,
            H2Answer::Status(200, None),
        ]));
        let (result, asked) = send_h2(url, |_| Ok(()));
        assert_eq!(result.expect("the resend is answered"), 200);
        assert_eq!(asked, 2);
        assert_eq!(paths.lock().unwrap().len(), 2);

        // Refused twice: the second refusal is returned, as before.
        let (url, paths) = rt.block_on(spawn_scripted_h2(vec![H2Answer::Refuse; 3]));
        let (result, asked) = send_h2(url, |_| Ok(()));
        let Err(SendError::Transport(error)) = result else {
            panic!("a second refusal is returned");
        };
        assert!(is_refused_stream(&error));
        assert_eq!((asked, paths.lock().unwrap().len()), (2, 2));

        // A save between the refusal and the resend stops the resend.
        let (url, paths) = rt.block_on(spawn_scripted_h2(vec![
            H2Answer::Refuse,
            H2Answer::Status(200, None),
        ]));
        let (result, asked) = send_h2(url, |ask| match ask {
            0 => Ok(()),
            _ => Err(crate::llm::admission::SendRefused::Replaced),
        });
        let error = result.expect_err("the resend is refused");
        assert_eq!(error.into_failure().to_string(), "runtime/policy_blocked");
        assert_eq!((asked, paths.lock().unwrap().len()), (2, 1));
    }

    #[test]
    fn a_refused_redirect_hop_is_sent_again_not_the_original_request() {
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let (url, paths) = rt.block_on(spawn_scripted_h2(vec![
            H2Answer::Status(307, Some("/moved/chat/completions")),
            H2Answer::Refuse,
            H2Answer::Status(200, None),
        ]));
        let (result, asked) = send_h2(url, |_| Ok(()));
        assert_eq!(result.expect("the resent hop is answered"), 200);
        assert_eq!(asked, 3);
        assert_eq!(
            *paths.lock().unwrap(),
            [
                "/v1/chat/completions",
                "/moved/chat/completions",
                "/moved/chat/completions"
            ]
        );
    }

    #[test]
    fn an_expired_chain_is_safe_to_retry_only_if_nothing_was_sent() {
        let retryable = |error: SendError| match error.into_failure().diagnostic {
            RuntimeDiagnostic::Runtime(detail) => {
                assert_eq!(detail.code, Code::Timeout);
                detail.retryable
            }
            other => panic!("{other:?}"),
        };
        let rt = tokio::runtime::Runtime::new().expect("rt");
        let budget = std::time::Duration::from_millis(200);
        let slow_leave = || std::thread::sleep(std::time::Duration::from_millis(300));

        // The first ask outlasts the budget: measured after leave, nothing sent.
        let (addr, heads) = rt.block_on(spawn_redirect_mock(
            vec![(200, None)],
            std::time::Duration::ZERO,
        ));
        let error = std::thread::spawn(move || {
            let client = job_client_builder().build().expect("build client");
            send_admitted(
                &format!("http://{addr}/v1/chat/completions"),
                budget,
                || {
                    slow_leave();
                    Ok(())
                },
                |url| client.post(url),
            )
            .expect_err("no time is left to send")
        })
        .join()
        .expect("client thread");
        assert!(matches!(error, SendError::DeadlineExceeded { sent: false }));
        assert!(retryable(error));
        assert!(heads.lock().unwrap().is_empty());

        // The POST was answered with a redirect, then the hop's ask outlasts
        // the budget: something was sent, so it is not offered as safe to retry.
        let (addr, heads) = rt.block_on(spawn_redirect_mock(
            vec![(307, Some("/moved")), (200, None)],
            std::time::Duration::ZERO,
        ));
        let error = std::thread::spawn(move || {
            let client = job_client_builder().build().expect("build client");
            let asked = std::cell::Cell::new(0);
            send_admitted(
                &format!("http://{addr}/v1/chat/completions"),
                budget,
                || {
                    if asked.replace(asked.get() + 1) > 0 {
                        slow_leave();
                    }
                    Ok(())
                },
                |url| client.post(url),
            )
            .expect_err("no time is left for the hop")
        })
        .join()
        .expect("client thread");
        assert!(matches!(error, SendError::DeadlineExceeded { sent: true }));
        assert!(!retryable(error));
        assert_eq!(heads.lock().unwrap().len(), 1);
    }
}
