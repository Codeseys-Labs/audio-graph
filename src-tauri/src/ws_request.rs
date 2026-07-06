//! Shared WebSocket upgrade-request builder for hand-rolled streaming clients.
//!
//! tungstenite's `IntoClientRequest for http::Request` is the identity function:
//! a hand-built `http::Request` passed to `connect_async` gets **no**
//! auto-injected upgrade headers, and `generate_request` then hard-fails on the
//! five mandatory WebSocket headers
//! (`Host` / `Connection` / `Upgrade` / `Sec-WebSocket-Version` /
//! `Sec-WebSocket-Key`) with `Protocol(InvalidHeader("sec-websocket-key"))`
//! **before any TCP/TLS**. Clients that hand-built a `Request` with only their
//! auth header (AssemblyAI, Gemini Live) were therefore 100% non-functional in
//! production — see the 2026-07-05 provider-connections review (B1/B2) and
//! seed `audio-graph-7086`.
//!
//! This helper always starts from `url.into_client_request()` — which injects
//! the five mandatory headers — and then layers the provider auth/content
//! headers on top. It is the same shape `openai_realtime.rs` already uses
//! correctly; centralizing it means the mandatory-header set cannot silently
//! diverge between clients again.
//!
//! **Security**: credentials must be passed as header pairs here, never smuggled
//! into `url`'s query string. URLs are logged by DNS, proxies, firewalls, and
//! cert monitoring — defeating TLS protection — whereas headers are not logged
//! by default (see the Gemini header-not-query comment at `gemini/mod.rs`).

use tokio_tungstenite::tungstenite::{
    client::IntoClientRequest,
    http::{HeaderName, HeaderValue, Request},
};

/// Build a WebSocket client upgrade `Request` for `url` with the five mandatory
/// upgrade headers (injected by `IntoClientRequest`) plus the supplied
/// auth/content header pairs layered on top.
///
/// `url` MUST carry no secret material in its query string; pass credentials as
/// `headers` entries instead.
///
/// Returns a redaction-safe error string on an invalid URL / header value —
/// the URL parse error carries only the (non-secret) URL, and header-value
/// parse errors from `http` do not echo the value; callers still route the
/// message through the provider redactor for defense in depth.
pub(crate) fn build_ws_upgrade_request<I>(url: &str, headers: I) -> Result<Request<()>, String>
where
    I: IntoIterator<Item = (HeaderName, HeaderValue)>,
{
    let mut request = url
        .into_client_request()
        .map_err(|e| format!("Failed to build WebSocket request: {e}"))?;
    let map = request.headers_mut();
    for (name, value) in headers {
        map.insert(name, value);
    }
    Ok(request)
}

/// Test-only fixture support for asserting the exact PRODUCTION upgrade-request
/// shape on the wire. Shared by the AssemblyAI and Gemini production-path
/// regression tests (audio-graph-7086 / review B1+B2) so the header-capturing
/// server exists exactly once.
#[cfg(test)]
pub(crate) mod test_support {
    /// URI + lowercased header `(name, value)` pairs captured from the
    /// client's WebSocket upgrade request.
    pub(crate) type CapturedHandshake = (String, Vec<(String, String)>);

    /// Bind a local listener, accept ONE WebSocket handshake, and capture the
    /// client's upgrade request (URI + headers). Returns the bound address and
    /// a `JoinHandle` resolving to the captured handshake once the server side
    /// completes.
    ///
    /// If the client request is missing any of the five mandatory upgrade
    /// headers the handshake never completes and the join panics — which is
    /// exactly the regression the callers pin.
    pub(crate) async fn spawn_header_capturing_ws_server() -> (
        std::net::SocketAddr,
        tokio::task::JoinHandle<CapturedHandshake>,
    ) {
        use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind local websocket server");
        let addr = listener.local_addr().expect("local addr");

        let handle = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept websocket");
            let mut captured: Option<CapturedHandshake> = None;
            // The closure signature (including the large `ErrorResponse` Err
            // variant) is dictated by tungstenite's `Callback` trait; it cannot
            // be boxed or shrunk on our side. Test-only code.
            #[allow(clippy::result_large_err)]
            let callback = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                let uri = req.uri().to_string();
                let headers = req
                    .headers()
                    .iter()
                    .map(|(name, value)| {
                        (
                            name.as_str().to_ascii_lowercase(),
                            String::from_utf8_lossy(value.as_bytes()).into_owned(),
                        )
                    })
                    .collect::<Vec<_>>();
                captured = Some((uri, headers));
                Ok(resp)
            };
            let _ws = tokio_tungstenite::accept_hdr_async(stream, callback)
                .await
                .expect("server completes websocket handshake with production request");
            captured.expect("handshake callback captured the client request")
        });

        (addr, handle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio_tungstenite::tungstenite::http::header;

    /// The five headers `tungstenite::handshake::client::generate_request` hard-
    /// requires. If any is absent the connect fails with
    /// `Protocol(InvalidHeader("sec-websocket-key"))` before any network I/O.
    const MANDATORY_WS_HEADERS: [&str; 5] = [
        "host",
        "connection",
        "upgrade",
        "sec-websocket-version",
        "sec-websocket-key",
    ];

    #[test]
    fn builds_request_with_mandatory_ws_headers_and_auth() {
        // Obviously-fake sentinel; never a real credential.
        let key = "test-key-not-real";
        let request = build_ws_upgrade_request(
            "wss://example.com/v1/stream?foo=bar",
            [
                (header::AUTHORIZATION, HeaderValue::from_str(key).unwrap()),
                (
                    HeaderName::from_static("x-goog-api-key"),
                    HeaderValue::from_str(key).unwrap(),
                ),
            ],
        )
        .expect("request builds");

        let headers = request.headers();
        for mandatory in MANDATORY_WS_HEADERS {
            assert!(
                headers.contains_key(mandatory),
                "production upgrade request is missing mandatory `{mandatory}` header"
            );
        }
        // The mandatory headers must carry usable values, not just be present.
        assert_eq!(
            headers
                .get("sec-websocket-version")
                .and_then(|v| v.to_str().ok()),
            Some("13")
        );
        assert!(
            headers
                .get("sec-websocket-key")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|k| !k.is_empty()),
            "sec-websocket-key must be generated and non-empty"
        );

        // The layered auth/content headers are present with the exact values.
        assert_eq!(
            headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok()),
            Some(key)
        );
        assert_eq!(
            headers.get("x-goog-api-key").and_then(|v| v.to_str().ok()),
            Some(key)
        );

        // The key must never be smuggled into the URL / query string.
        assert!(
            !request.uri().to_string().contains(key),
            "credential must not appear in the request URI"
        );
        assert_eq!(request.uri().query(), Some("foo=bar"));
    }

    #[test]
    fn rejects_invalid_url() {
        let err = build_ws_upgrade_request("not a url", Vec::<(HeaderName, HeaderValue)>::new())
            .expect_err("an unparseable URL must fail request construction");
        assert!(err.contains("Failed to build WebSocket request"), "{err}");
    }
}
