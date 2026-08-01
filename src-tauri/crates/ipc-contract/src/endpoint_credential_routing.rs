//! Endpoint → credential-slot routing and audience authorization.
//!
//! [`credential_key_for_endpoint`] is the legacy UI/form-grouping helper. It
//! intentionally preserves the historical fallback behavior so draft inputs
//! can be cached by form group, but it is **not an authorization API**.
//!
//! Security-bearing code must use [`classify_endpoint_audience`] or
//! [`saved_credential_key_for_endpoint`]. Those functions parse the URL and
//! authorize saved credentials only for exact, Rust-owned HTTPS origins.
//!
//! ## Single source of truth
//!
//! [`ENDPOINT_CREDENTIAL_ROUTING`] is the one table. Two consumers derive from
//! it and must never diverge:
//!
//!  * the Rust runtime [`credential_key_for_endpoint`] iterates the table, and
//!  * the frontend table + matcher in `src/generated/endpointCredentialRouting.ts`
//!    are generated verbatim from it by
//!    [`endpoint_credential_routing_typescript_module`] (the `export_endpoint_credential_routing`
//!    bin), with a Rust drift test that fails CI if the committed TS diverges.
//!
//! Before this table the routing was hand-maintained twice (Rust + TS) and only
//! a shared-vector contract test kept them lockstep; generating one from the
//! other makes drift impossible rather than merely tested.

/// Cerebras Cloud's OpenAI-compatible base URL.
pub const CEREBRAS_BASE_URL: &str = "https://api.cerebras.ai/v1";

/// SambaNova Cloud's OpenAI-compatible base URL.
///
/// Confirmed from SambaNova docs (docs.sambanova.ai "API keys and URLs" +
/// the published OpenAPI spec `servers: - url: https://api.sambanova.ai/v1`).
pub const SAMBANOVA_BASE_URL: &str = "https://api.sambanova.ai/v1";

/// One exact HTTPS origin authorized to receive a saved credential.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedEndpointAudience {
    pub origin: &'static str,
    pub credential_key: &'static str,
}

/// Exact built-in origins that may receive saved credentials.
///
/// Paths are deliberately absent: authorization is by normalized origin
/// `(scheme, host, effective port)`, while request builders append their own
/// provider paths. Unknown/custom origins remain draft-or-anonymous until the
/// protected custom-origin binding workstream lands (Seed audio-graph-98a9).
pub const SAVED_ENDPOINT_AUDIENCES: &[SavedEndpointAudience] = &[
    SavedEndpointAudience {
        origin: "https://api.openai.com",
        credential_key: "openai_api_key",
    },
    SavedEndpointAudience {
        origin: "https://api.cerebras.ai",
        credential_key: "cerebras_api_key",
    },
    SavedEndpointAudience {
        origin: "https://api.sambanova.ai",
        credential_key: "sambanova_api_key",
    },
    SavedEndpointAudience {
        origin: "https://openrouter.ai",
        credential_key: "openrouter_api_key",
    },
    SavedEndpointAudience {
        origin: "https://generativelanguage.googleapis.com",
        credential_key: "gemini_api_key",
    },
    SavedEndpointAudience {
        origin: "https://api.groq.com",
        credential_key: "groq_api_key",
    },
    SavedEndpointAudience {
        origin: "https://api.together.xyz",
        credential_key: "together_api_key",
    },
    SavedEndpointAudience {
        origin: "https://api.fireworks.ai",
        credential_key: "fireworks_api_key",
    },
];

/// Parsed audience class for an OpenAI-compatible HTTP endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointAudience {
    /// Exact built-in HTTPS origin; the named saved slot is authorized.
    Saved(SavedEndpointAudience),
    /// Valid endpoint that may use only an invocation draft or no credential.
    DraftOrAnonymous {
        normalized_origin: String,
        loopback: bool,
    },
}

/// Content-free endpoint-policy failure. Display never echoes the supplied URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointAudienceError {
    InvalidUrl,
    UnsupportedScheme,
    MissingHost,
    EmbeddedCredentials,
    QueryNotAllowed,
    FragmentNotAllowed,
    TrailingDotHost,
    InsecureRemote,
    NonDefaultRemotePort,
}

impl std::fmt::Display for EndpointAudienceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let message = match self {
            Self::InvalidUrl => "invalid endpoint URL",
            Self::UnsupportedScheme => "endpoint scheme must be http or https",
            Self::MissingHost => "endpoint URL must include a host",
            Self::EmbeddedCredentials => "endpoint URL must not contain userinfo",
            Self::QueryNotAllowed => "endpoint base URL must not contain a query",
            Self::FragmentNotAllowed => "endpoint base URL must not contain a fragment",
            Self::TrailingDotHost => "endpoint host must not use a trailing dot",
            Self::InsecureRemote => "remote endpoint must use HTTPS",
            Self::NonDefaultRemotePort => "remote HTTPS endpoint must use port 443",
        };
        f.write_str(message)
    }
}

impl std::error::Error for EndpointAudienceError {}

fn is_loopback_host(host: url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    }
}

/// Parse and classify an endpoint before any saved credential is selected.
///
/// Valid custom HTTPS endpoints are draft-only; exact loopback HTTP(S)
/// endpoints may use a draft or remain anonymous. This classifier records the
/// origin class; the command-side resolver enforces the credential mode.
/// Remote cleartext, ambiguous authority components, and non-default remote
/// ports fail closed before a request is built.
pub fn classify_endpoint_audience(
    endpoint: &str,
) -> Result<EndpointAudience, EndpointAudienceError> {
    let parsed = url::Url::parse(endpoint.trim()).map_err(|_| EndpointAudienceError::InvalidUrl)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(EndpointAudienceError::UnsupportedScheme);
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(EndpointAudienceError::EmbeddedCredentials);
    }
    if parsed.query().is_some() {
        return Err(EndpointAudienceError::QueryNotAllowed);
    }
    if parsed.fragment().is_some() {
        return Err(EndpointAudienceError::FragmentNotAllowed);
    }

    let host = parsed.host().ok_or(EndpointAudienceError::MissingHost)?;
    if matches!(host, url::Host::Domain(domain) if domain.ends_with('.')) {
        return Err(EndpointAudienceError::TrailingDotHost);
    }
    let loopback = is_loopback_host(host);
    if parsed.scheme() == "http" && !loopback {
        return Err(EndpointAudienceError::InsecureRemote);
    }
    if !loopback && parsed.port_or_known_default() != Some(443) {
        return Err(EndpointAudienceError::NonDefaultRemotePort);
    }

    let normalized_origin = parsed.origin().ascii_serialization();
    if let Some(audience) = SAVED_ENDPOINT_AUDIENCES
        .iter()
        .find(|audience| audience.origin == normalized_origin)
    {
        return Ok(EndpointAudience::Saved(*audience));
    }

    Ok(EndpointAudience::DraftOrAnonymous {
        normalized_origin,
        loopback,
    })
}

/// Return the saved slot authorized for `endpoint`, or `None` for custom,
/// loopback, malformed, or otherwise denied endpoints.
///
/// Call [`classify_endpoint_audience`] when denial must be distinguished from
/// a valid draft-or-anonymous endpoint.
pub fn saved_credential_key_for_endpoint(endpoint: &str) -> Option<&'static str> {
    match classify_endpoint_audience(endpoint).ok()? {
        EndpointAudience::Saved(audience) => Some(audience.credential_key),
        EndpointAudience::DraftOrAnonymous { .. } => None,
    }
}

/// Legacy UI grouping slot for an endpoint with no dedicated display rule.
/// Never use this fallback to authorize a saved credential.
pub const DEFAULT_ENDPOINT_CREDENTIAL_KEY: &str = "openai_api_key";

/// How an [`EndpointCredentialRoute`] decides whether an endpoint matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointMatch {
    /// Matches when the endpoint, normalized (trimmed, trailing slashes
    /// stripped, lowercased), equals the base URL. Used for hosts whose
    /// generic name could otherwise capture look-alike proxies.
    ExactHost(&'static str),
    /// Matches when the lowercased endpoint contains any of these substrings.
    SubstringAny(&'static [&'static str]),
}

/// One ordered routing rule mapping matching endpoints to a credential slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointCredentialRoute {
    pub credential_key: &'static str,
    pub matcher: EndpointMatch,
}

/// Ordered endpoint → credential-slot routing rules; first match wins. Any
/// endpoint matching no rule falls through to [`DEFAULT_ENDPOINT_CREDENTIAL_KEY`].
///
/// This is the single source of truth for endpoint credential routing; the
/// generated TypeScript table mirrors it exactly.
pub const ENDPOINT_CREDENTIAL_ROUTING: &[EndpointCredentialRoute] = &[
    EndpointCredentialRoute {
        credential_key: "cerebras_api_key",
        matcher: EndpointMatch::ExactHost(CEREBRAS_BASE_URL),
    },
    EndpointCredentialRoute {
        credential_key: "sambanova_api_key",
        matcher: EndpointMatch::ExactHost(SAMBANOVA_BASE_URL),
    },
    EndpointCredentialRoute {
        credential_key: "openrouter_api_key",
        matcher: EndpointMatch::SubstringAny(&["openrouter"]),
    },
    EndpointCredentialRoute {
        credential_key: "gemini_api_key",
        matcher: EndpointMatch::SubstringAny(&["generativelanguage.googleapis.com", "gemini"]),
    },
    EndpointCredentialRoute {
        credential_key: "groq_api_key",
        matcher: EndpointMatch::SubstringAny(&["groq"]),
    },
    EndpointCredentialRoute {
        credential_key: "together_api_key",
        matcher: EndpointMatch::SubstringAny(&["together"]),
    },
    EndpointCredentialRoute {
        credential_key: "fireworks_api_key",
        matcher: EndpointMatch::SubstringAny(&["fireworks"]),
    },
];

/// Normalize an endpoint for exact-host comparison: trim, strip trailing
/// slashes, lowercase. The base-URL constants are already in this normal form.
fn normalize_endpoint(endpoint: &str) -> String {
    endpoint.trim().trim_end_matches('/').to_ascii_lowercase()
}

impl EndpointMatch {
    fn matches(&self, endpoint: &str, lowercased: &str) -> bool {
        match self {
            EndpointMatch::ExactHost(base) => normalize_endpoint(endpoint) == *base,
            EndpointMatch::SubstringAny(patterns) => {
                patterns.iter().any(|pattern| lowercased.contains(*pattern))
            }
        }
    }
}

/// Pick the UI/form grouping slot for an OpenAI-compatible endpoint.
///
/// This preserves historical draft-input behavior and is not safe for saved
/// credential selection. Security-bearing code must use
/// [`saved_credential_key_for_endpoint`].
pub fn credential_key_for_endpoint(endpoint: &str) -> &'static str {
    let lower = endpoint.to_ascii_lowercase();
    for route in ENDPOINT_CREDENTIAL_ROUTING {
        if route.matcher.matches(endpoint, &lower) {
            return route.credential_key;
        }
    }
    DEFAULT_ENDPOINT_CREDENTIAL_KEY
}

/// True when `endpoint` is Cerebras Cloud's OpenAI-compatible base URL.
pub fn is_cerebras_endpoint(endpoint: &str) -> bool {
    normalize_endpoint(endpoint) == CEREBRAS_BASE_URL
}

/// True when `endpoint` is SambaNova Cloud's OpenAI-compatible base URL.
pub fn is_sambanova_endpoint(endpoint: &str) -> bool {
    normalize_endpoint(endpoint) == SAMBANOVA_BASE_URL
}

/// The union of every credential slot the table can route to, including the
/// default fallback, in table order (default last if not already present).
fn credential_key_union() -> Vec<&'static str> {
    let mut slots: Vec<&'static str> = ENDPOINT_CREDENTIAL_ROUTING
        .iter()
        .map(|route| route.credential_key)
        .collect();
    if !slots.contains(&DEFAULT_ENDPOINT_CREDENTIAL_KEY) {
        slots.push(DEFAULT_ENDPOINT_CREDENTIAL_KEY);
    }
    slots
}

/// The TS identifier for a base URL used by an `ExactHost` rule, so the
/// generated table references the exported constant instead of re-inlining the
/// URL string.
fn base_url_const_name(base: &str) -> Option<&'static str> {
    match base {
        CEREBRAS_BASE_URL => Some("CEREBRAS_BASE_URL"),
        SAMBANOVA_BASE_URL => Some("SAMBANOVA_BASE_URL"),
        _ => None,
    }
}

/// The generated TypeScript module consumed by the frontend
/// (`src/generated/endpointCredentialRouting.ts`). The routing table, the slot
/// union, and the matcher are all derived from [`ENDPOINT_CREDENTIAL_ROUTING`],
/// so the frontend router is byte-for-byte a projection of the Rust source.
pub fn endpoint_credential_routing_typescript_module() -> String {
    let union = credential_key_union()
        .iter()
        .map(|slot| format!("  | \"{slot}\""))
        .collect::<Vec<_>>()
        .join("\n");

    let mut routes = String::new();
    for route in ENDPOINT_CREDENTIAL_ROUTING {
        let (kind, patterns): (&str, Vec<String>) = match &route.matcher {
            EndpointMatch::ExactHost(base) => (
                "exact_host",
                vec![
                    base_url_const_name(base)
                        .map(String::from)
                        .unwrap_or_else(|| format!("\"{base}\"")),
                ],
            ),
            EndpointMatch::SubstringAny(patterns) => (
                "substring_any",
                patterns.iter().map(|p| format!("\"{p}\"")).collect(),
            ),
        };
        routes.push_str(&format!(
            "  {{\n    credential_key: \"{key}\",\n    match_kind: \"{kind}\",\n    patterns: [{patterns}],\n  }},\n",
            key = route.credential_key,
            patterns = patterns.join(", "),
        ));
    }

    let saved_audiences = SAVED_ENDPOINT_AUDIENCES
        .iter()
        .map(|audience| {
            format!(
                "  {{ origin: \"{}\", credential_key: \"{}\" }},",
                audience.origin, audience.credential_key
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    format!(
        r#"// @generated by src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs. Do not edit manually.

// Endpoint credential grouping + saved-key audience authorization.
//
// Single source of truth: ENDPOINT_CREDENTIAL_ROUTING in the Rust module
// src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs. Both this
// table and the Rust runtime credential_key_for_endpoint are derived from that
// one table, so the TS and Rust routers can never drift.

export const CEREBRAS_BASE_URL = "{cerebras}";
export const SAMBANOVA_BASE_URL = "{sambanova}";

export type EndpointCredentialKey =
{union};

/**
 * Legacy form-grouping slot used when an endpoint matches no display rule.
 * This value is not saved-key authority; security-bearing consumers must use
 * savedCredentialKeyForEndpoint, which has no fallback.
 */
export const DEFAULT_ENDPOINT_CREDENTIAL_KEY: EndpointCredentialKey =
  "{default_key}";

export type EndpointMatchKind = "exact_host" | "substring_any";

export interface EndpointCredentialRoute {{
  credential_key: EndpointCredentialKey;
  match_kind: EndpointMatchKind;
  patterns: readonly string[];
}}

export interface SavedEndpointAudience {{
  origin: string;
  credential_key: EndpointCredentialKey;
}}

/** Exact normalized HTTPS origins authorized to receive saved credentials. */
export const SAVED_ENDPOINT_AUDIENCES: readonly SavedEndpointAudience[] = [
{saved_audiences}
];

export type EndpointAudience =
  | {{
      kind: "saved";
      normalized_origin: string;
      credential_key: EndpointCredentialKey;
    }}
  | {{
      kind: "draft_or_anonymous";
      normalized_origin: string;
      loopback: boolean;
    }}
  | {{ kind: "denied" }};

/**
 * Ordered endpoint → credential-slot routing rules; first match wins. An
 * `exact_host` rule matches when the endpoint, normalized (trimmed, trailing
 * slashes stripped, lowercased), equals one of its patterns; a `substring_any`
 * rule matches when the lowercased endpoint contains any of its patterns.
 */
export const ENDPOINT_CREDENTIAL_ROUTING: readonly EndpointCredentialRoute[] = [
{routes}];

{matcher}"#,
        cerebras = CEREBRAS_BASE_URL,
        sambanova = SAMBANOVA_BASE_URL,
        union = union,
        default_key = DEFAULT_ENDPOINT_CREDENTIAL_KEY,
        routes = routes,
        saved_audiences = saved_audiences,
        matcher = NORMALIZE_AND_MATCHER_TS,
    )
}

/// The static (non-table-derived) tail of the generated module: the normalizer,
/// the endpoint→slot matcher, and the two exact-host predicates. Kept as a raw
/// literal so its braces need no escaping.
const NORMALIZE_AND_MATCHER_TS: &str = r#"function normalizeEndpoint(endpoint: string): string {
  return endpoint.trim().replace(/\/+$/, "").toLowerCase();
}

/**
 * Map an OpenAI-compatible endpoint URL to the credential-store slot its API
 * key is saved under. Mirrors the backend's per-endpoint credential routing so
 * the UI can resolve the right saved key for whatever endpoint is selected.
 */
export function endpointCredentialKey(
  endpoint: string,
): EndpointCredentialKey {
  const normalized = normalizeEndpoint(endpoint);
  const lower = endpoint.toLowerCase();
  for (const route of ENDPOINT_CREDENTIAL_ROUTING) {
    const matched =
      route.match_kind === "exact_host"
        ? route.patterns.some((pattern) => normalized === pattern)
        : route.patterns.some((pattern) => lower.includes(pattern));
    if (matched) {
      return route.credential_key;
    }
  }
  return DEFAULT_ENDPOINT_CREDENTIAL_KEY;
}

function isLoopbackHostname(hostname: string): boolean {
  const normalized = hostname.toLowerCase().replace(/^\[|\]$/g, "");
  if (normalized === "localhost" || normalized === "::1") return true;
  const ipv4 = normalized.split(".");
  return (
    ipv4.length === 4 &&
    ipv4.every((part) => /^\d+$/.test(part) && Number(part) <= 255) &&
    Number(ipv4[0]) === 127
  );
}

/**
 * Parse an endpoint and classify its saved-key authority. Unknown/custom
 * HTTPS endpoints are draft-only; exact loopback HTTP(S) endpoints may use a
 * draft or remain anonymous. Malformed or insecure remote endpoints are denied.
 */
export function classifyEndpointAudience(endpoint: string): EndpointAudience {
  const input = endpoint.trim();
  let parsed: URL;
  try {
    parsed = new URL(input);
  } catch {
    return { kind: "denied" };
  }

  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return { kind: "denied" };
  }
  if (parsed.username !== "" || parsed.password !== "") {
    return { kind: "denied" };
  }
  if (input.includes("?") || input.includes(String.fromCharCode(35))) {
    return { kind: "denied" };
  }
  if (parsed.hostname.endsWith(".")) {
    return { kind: "denied" };
  }

  const loopback = isLoopbackHostname(parsed.hostname);
  if (parsed.protocol === "http:" && !loopback) {
    return { kind: "denied" };
  }
  if (!loopback && parsed.port !== "") {
    return { kind: "denied" };
  }

  const normalizedOrigin = parsed.origin.toLowerCase();
  const saved = SAVED_ENDPOINT_AUDIENCES.find(
    (audience) => audience.origin === normalizedOrigin,
  );
  if (saved) {
    return {
      kind: "saved",
      normalized_origin: saved.origin,
      credential_key: saved.credential_key,
    };
  }

  return {
    kind: "draft_or_anonymous",
    normalized_origin: normalizedOrigin,
    loopback,
  };
}

/** Nullable security-bearing saved-key lookup; never falls back. */
export function savedCredentialKeyForEndpoint(
  endpoint: string,
): EndpointCredentialKey | null {
  const audience = classifyEndpointAudience(endpoint);
  return audience.kind === "saved" ? audience.credential_key : null;
}

export function isCerebrasEndpoint(endpoint: string): boolean {
  return normalizeEndpoint(endpoint) === CEREBRAS_BASE_URL;
}

export function isSambanovaEndpoint(endpoint: string): boolean {
  return normalizeEndpoint(endpoint) === SAMBANOVA_BASE_URL;
}
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_covers_known_openai_compatible_hosts() {
        for (endpoint, key) in [
            ("https://api.openai.com/v1", "openai_api_key"),
            (CEREBRAS_BASE_URL, "cerebras_api_key"),
            ("https://api.cerebras.ai/v1/", "cerebras_api_key"),
            (SAMBANOVA_BASE_URL, "sambanova_api_key"),
            ("https://api.sambanova.ai/v1/", "sambanova_api_key"),
            ("https://openrouter.ai/api/v1", "openrouter_api_key"),
            ("https://api.groq.com/openai/v1", "groq_api_key"),
            ("https://api.together.xyz/v1", "together_api_key"),
            ("https://api.fireworks.ai/inference/v1", "fireworks_api_key"),
            (
                "https://generativelanguage.googleapis.com/v1beta/openai",
                "gemini_api_key",
            ),
        ] {
            assert_eq!(
                credential_key_for_endpoint(endpoint),
                key,
                "{endpoint} should route to {key}"
            );
        }
    }

    #[test]
    fn exact_host_rules_are_not_substring_matches() {
        // A look-alike host must fall through to the generic slot, never
        // capture the dedicated exact-host slot.
        assert_eq!(
            credential_key_for_endpoint("https://api.cerebras.ai.evil.com/v1"),
            "openai_api_key"
        );
        assert_eq!(
            credential_key_for_endpoint("https://cerebras-proxy.internal/v1"),
            "openai_api_key"
        );
        assert!(is_cerebras_endpoint("https://api.cerebras.ai/v1/"));
        assert!(is_sambanova_endpoint("HTTPS://API.SAMBANOVA.AI/V1"));
        assert!(!is_cerebras_endpoint(SAMBANOVA_BASE_URL));
    }

    #[test]
    fn saved_audience_requires_an_exact_normalized_builtin_https_origin() {
        for (endpoint, key, origin) in [
            (
                "https://API.OPENAI.COM:443/v1",
                "openai_api_key",
                "https://api.openai.com",
            ),
            (
                CEREBRAS_BASE_URL,
                "cerebras_api_key",
                "https://api.cerebras.ai",
            ),
            (
                SAMBANOVA_BASE_URL,
                "sambanova_api_key",
                "https://api.sambanova.ai",
            ),
            (
                "https://openrouter.ai/api/v1",
                "openrouter_api_key",
                "https://openrouter.ai",
            ),
            (
                "https://api.groq.com/openai/v1",
                "groq_api_key",
                "https://api.groq.com",
            ),
        ] {
            let EndpointAudience::Saved(audience) =
                classify_endpoint_audience(endpoint).expect("trusted endpoint")
            else {
                panic!("{endpoint} should be a saved audience")
            };
            assert_eq!(audience.credential_key, key);
            assert_eq!(audience.origin, origin);
            assert_eq!(saved_credential_key_for_endpoint(endpoint), Some(key));
        }
    }

    #[test]
    fn custom_and_loopback_endpoints_never_infer_a_saved_slot() {
        for endpoint in [
            "https://api.example.test/v1",
            "https://openrouter.example.test/v1?name=openrouter",
            "http://localhost:11434/v1",
            "http://127.1:8000/v1",
            "http://[::1]:8000/v1",
        ] {
            assert_eq!(
                saved_credential_key_for_endpoint(endpoint),
                None,
                "{endpoint}"
            );
        }

        for endpoint in [
            "https://api.openai.com.evil.test/v1",
            "https://evil.test/api.openai.com/v1",
            "https://evil.test/v1?provider=openrouter",
            "https://openrouter.ai.evil.test/api/v1",
            "https://xn--openai-9za.example/v1",
        ] {
            assert_eq!(
                saved_credential_key_for_endpoint(endpoint),
                None,
                "{endpoint}"
            );
            assert!(matches!(
                classify_endpoint_audience(endpoint),
                Ok(EndpointAudience::DraftOrAnonymous { .. }) | Err(_)
            ));
        }
    }

    #[test]
    fn ambiguous_or_insecure_base_urls_fail_closed() {
        for (endpoint, expected) in [
            (
                "https://user@example.com/v1",
                EndpointAudienceError::EmbeddedCredentials,
            ),
            (
                "https://api.openai.com/v1?x=1",
                EndpointAudienceError::QueryNotAllowed,
            ),
            (
                "https://api.openai.com/v1#x",
                EndpointAudienceError::FragmentNotAllowed,
            ),
            (
                "https://api.openai.com./v1",
                EndpointAudienceError::TrailingDotHost,
            ),
            (
                "https://api.openai.com:444/v1",
                EndpointAudienceError::NonDefaultRemotePort,
            ),
            (
                "http://api.openai.com/v1",
                EndpointAudienceError::InsecureRemote,
            ),
            (
                "ftp://api.openai.com/v1",
                EndpointAudienceError::UnsupportedScheme,
            ),
        ] {
            assert_eq!(
                classify_endpoint_audience(endpoint),
                Err(expected),
                "{endpoint}"
            );
            assert_eq!(
                saved_credential_key_for_endpoint(endpoint),
                None,
                "{endpoint}"
            );
        }
    }

    #[test]
    fn generated_typescript_module_contains_core_symbols() {
        let module = endpoint_credential_routing_typescript_module();
        assert!(module.contains(
            "@generated by src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs"
        ));
        assert!(module.contains("Do not edit manually"));
        assert!(module.contains("export function endpointCredentialKey"));
        assert!(module.contains("export function savedCredentialKeyForEndpoint"));
        assert!(module.contains("export function classifyEndpointAudience"));
        assert!(module.contains("export const SAVED_ENDPOINT_AUDIENCES"));
        assert!(module.contains("export const ENDPOINT_CREDENTIAL_ROUTING"));
        assert!(module.contains("credential_key: \"cerebras_api_key\","));
        assert!(module.contains("patterns: [CEREBRAS_BASE_URL],"));
        assert!(module.contains("patterns: [\"generativelanguage.googleapis.com\", \"gemini\"],"));
        // Every routed slot plus the default must appear in the union type.
        for slot in credential_key_union() {
            assert!(
                module.contains(&format!("| \"{slot}\"")),
                "union missing {slot}"
            );
        }
    }
}
