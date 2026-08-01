//! Shared HTTP diagnostic helpers for the OpenRouter / generic-API LLM clients.
//!
//! OpenRouter and generic-API LLM clients use this helper to extract a
//! redaction-safe request id. Request URLs and paths are deliberately omitted
//! from exportable diagnostics because custom path components may be private.

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
