//! Shared HTTP diagnostic helpers for the OpenRouter / generic-API LLM clients.
//!
//! The blocking OpenRouter client (`openrouter.rs`) and the SSE streaming client
//! (`streaming.rs`) both need to (a) extract a redaction-safe request-id from
//! response headers for diagnostics and (b) reduce a full request URL to just its
//! path (never the query string, which can carry credentials/routing metadata).
//! These were byte-identical copies in both files (review n1); centralizing them
//! keeps the header allow-list and the sanitizer filter from silently drifting
//! apart between the two transports.

/// Reduce a request URL to just its path for diagnostics.
///
/// Deliberately drops scheme/host/**query string** — the query can carry API
/// keys or routing metadata that must never reach a log or UI-visible error.
/// An unparseable URL yields a fixed non-secret placeholder rather than echoing
/// the raw (possibly secret-bearing) string back.
pub(crate) fn diagnostic_path(url: &str) -> String {
    reqwest::Url::parse(url)
        .map(|parsed| parsed.path().to_string())
        .unwrap_or_else(|_| "<unparseable>".to_string())
}

/// Extract a provider request-id from response headers, sanitized for safe
/// logging.
///
/// Checks the known id headers in priority order, keeps only
/// `[A-Za-z0-9-_.:]` (dropping anything a header could smuggle that isn't
/// id-shaped), and caps the length at 128 chars. Returns the first non-empty
/// match, or `None` when no id header is present.
pub(crate) fn response_request_id(headers: &reqwest::header::HeaderMap) -> Option<String> {
    for name in [
        "x-request-id",
        "request-id",
        "x-openrouter-request-id",
        "cf-ray",
    ] {
        let Some(value) = headers.get(name).and_then(|value| value.to_str().ok()) else {
            continue;
        };
        let sanitized: String = value
            .chars()
            .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.' | ':'))
            .take(128)
            .collect();
        if !sanitized.is_empty() {
            return Some(sanitized);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::{HeaderMap, HeaderValue};

    #[test]
    fn diagnostic_path_keeps_path_drops_query() {
        assert_eq!(
            diagnostic_path("https://openrouter.ai/api/v1/chat/completions?key=secret"),
            "/api/v1/chat/completions"
        );
    }

    #[test]
    fn diagnostic_path_placeholder_on_unparseable() {
        assert_eq!(diagnostic_path("not a url"), "<unparseable>");
    }

    #[test]
    fn response_request_id_prefers_priority_order() {
        let mut headers = HeaderMap::new();
        headers.insert("cf-ray", HeaderValue::from_static("ray-999"));
        headers.insert("x-request-id", HeaderValue::from_static("req-123"));
        // `x-request-id` outranks `cf-ray`.
        assert_eq!(response_request_id(&headers), Some("req-123".to_string()));
    }

    #[test]
    fn response_request_id_sanitizes_and_caps() {
        let mut headers = HeaderMap::new();
        // Spaces / disallowed chars are stripped; only id-shaped chars survive.
        headers.insert("x-request-id", HeaderValue::from_static("ab cd/e\tf!g"));
        assert_eq!(response_request_id(&headers), Some("abcdefg".to_string()));

        let long = "a".repeat(300);
        let mut headers = HeaderMap::new();
        headers.insert("request-id", HeaderValue::from_str(&long).unwrap());
        assert_eq!(response_request_id(&headers).map(|s| s.len()), Some(128));
    }

    #[test]
    fn response_request_id_none_when_absent() {
        let headers = HeaderMap::new();
        assert_eq!(response_request_id(&headers), None);
    }
}
