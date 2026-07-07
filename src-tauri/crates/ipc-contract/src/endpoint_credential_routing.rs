//! Endpoint → credential-slot routing (seed audio-graph-ed48).
//!
//! OpenAI-compatible HTTP providers only store routing details (endpoint/model)
//! in settings; the secret lives in `credentials.yaml` under a per-provider
//! slot. At runtime the endpoint URL is the stable discriminator we have, so
//! this table maps an endpoint to the credential slot its key is saved under.
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

/// Credential slot used for any OpenAI-compatible endpoint that matches no
/// dedicated routing rule. OpenAI, Anthropic-compatible shims, vLLM with auth,
/// and unknown OpenAI-compatible endpoints share this generic bearer slot.
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

/// Pick the credential slot for an OpenAI-compatible HTTP provider endpoint by
/// walking [`ENDPOINT_CREDENTIAL_ROUTING`] in order.
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
                vec![base_url_const_name(base)
                    .map(String::from)
                    .unwrap_or_else(|| format!("\"{base}\""))],
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

    format!(
        r#"// @generated by src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs. Do not edit manually.

// Endpoint → credential-slot routing (seed audio-graph-ed48).
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
 * Credential slot used for any OpenAI-compatible endpoint that matches no
 * dedicated routing rule (OpenAI, Anthropic-compatible shims, vLLM with auth,
 * and unknown OpenAI-compatible endpoints share this generic bearer slot).
 */
export const DEFAULT_ENDPOINT_CREDENTIAL_KEY: EndpointCredentialKey =
  "{default_key}";

export type EndpointMatchKind = "exact_host" | "substring_any";

export interface EndpointCredentialRoute {{
  credential_key: EndpointCredentialKey;
  match_kind: EndpointMatchKind;
  patterns: readonly string[];
}}

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
    fn generated_typescript_module_contains_core_symbols() {
        let module = endpoint_credential_routing_typescript_module();
        assert!(module.contains(
            "@generated by src-tauri/crates/ipc-contract/src/endpoint_credential_routing.rs"
        ));
        assert!(module.contains("Do not edit manually"));
        assert!(module.contains("export function endpointCredentialKey"));
        assert!(module.contains("export const ENDPOINT_CREDENTIAL_ROUTING"));
        assert!(module.contains("credential_key: \"cerebras_api_key\","));
        assert!(module.contains("patterns: [CEREBRAS_BASE_URL],"));
        assert!(module.contains(
            "patterns: [\"generativelanguage.googleapis.com\", \"gemini\"],"
        ));
        // Every routed slot plus the default must appear in the union type.
        for slot in credential_key_union() {
            assert!(
                module.contains(&format!("| \"{slot}\"")),
                "union missing {slot}"
            );
        }
    }
}
