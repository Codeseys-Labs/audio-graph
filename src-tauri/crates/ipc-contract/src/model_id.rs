//! What may be stored or shown for a model. Where the model ran decides, not
//! what its identifier looks like.
//!
//! - A model at an endpoint that cannot be on the user's own machine or
//!   network keeps the provider's hosted identifier.
//! - Any other model gets a display name and never its identifier. A local
//!   server reports the path it loaded as its model id, and the same path is
//!   what the user configures as the model to request. A path names a user
//!   and a directory layout.
//! - A stored record that cannot say where its model ran is read by the
//!   local rule. A hosted id that loses its namespace is cosmetic; a path
//!   that survives is a leak.
//! - An identifier carrying a credential-shaped token is withheld whole,
//!   wherever the model ran.
//!
//! No rule here looks at the shape of the identifier to guess whether it is a
//! path: `numbat/checkpoint` and `org/model` are the same shape.
//!
//! [`endpoint_may_be_on_users_network`] answers where a model ran. It is not
//! the content-egress classifier: that one decides what the cloud-content
//! policy blocks, and a server on the user's LAN is egress there while its
//! model ids still name the user's files here.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// What stands in for a local model whose identifier leaves no name to show:
/// a bare root, a home marker, or anything that still begins with a drive.
///
/// The last case covers what older builds stored for a Windows path. They
/// dropped its backslashes before persisting it, so `C:\Users\name\m.gguf`
/// was stored as `C:Usersnamem.gguf`, which cannot be cut back to a file name.
pub const LOCAL_MODEL_PLACEHOLDER: &str = "local-model";

/// What stands in for any identifier that carries a credential-shaped token,
/// wherever the model ran. No part of such a value is kept.
pub const WITHHELD_MODEL_PLACEHOLDER: &str = "withheld-model";

/// Heuristic guard: does this value contain a credential-shaped token? Catches
/// the common API-key prefixes and bearer-token shapes so a routed-provider echo
/// can never persist a secret as "metadata". Case-insensitive.
pub fn looks_credential_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("sk-")
        || lower.contains("bearer ")
        || lower.contains("api_key")
        || lower.contains("apikey")
}

/// Could `endpoint` be on the user's own machine or network?
///
/// True for loopback in either family (bracketed IPv6 included), the
/// unspecified address, `localhost` with or without its trailing dot and any
/// `*.localhost` name, private, link-local, unique-local and carrier-grade-NAT
/// addresses (also as IPv4-mapped IPv6), a host-less URL such as a Unix
/// socket, a single-label host name, and names under `.local`, `.lan`,
/// `.home.arpa` and `.internal`. An endpoint that does not parse as a URL is
/// treated as local. No name is resolved, so a public name that resolves to
/// the user's own machine is not detected.
pub fn endpoint_may_be_on_users_network(endpoint: &str) -> bool {
    let Ok(url) = url::Url::parse(endpoint.trim()) else {
        return true;
    };
    if url.scheme().to_ascii_lowercase().contains("unix") {
        return true;
    }
    match url.host() {
        None => true,
        Some(url::Host::Ipv4(address)) => ipv4_may_be_on_users_network(address),
        Some(url::Host::Ipv6(address)) => ipv6_may_be_on_users_network(address),
        Some(url::Host::Domain(name)) => domain_may_be_on_users_network(name),
    }
}

fn ipv4_may_be_on_users_network(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    address.is_loopback()
        || address.is_unspecified()
        || address.is_private()
        || address.is_link_local()
        // Carrier-grade NAT, 100.64.0.0/10.
        || (first == 100 && (second & 0b1100_0000) == 64)
}

fn ipv6_may_be_on_users_network(address: Ipv6Addr) -> bool {
    if let Some(mapped) = address.to_ipv4_mapped() {
        return ipv4_may_be_on_users_network(mapped);
    }
    let first = address.segments()[0];
    address.is_loopback()
        || address.is_unspecified()
        // Unique local, fc00::/7.
        || (first & 0xfe00) == 0xfc00
        // Link-local, fe80::/10.
        || (first & 0xffc0) == 0xfe80
}

fn domain_may_be_on_users_network(name: &str) -> bool {
    let name = name.trim_end_matches('.').to_ascii_lowercase();
    if let Ok(address) = name.parse::<IpAddr>() {
        return match address {
            IpAddr::V4(address) => ipv4_may_be_on_users_network(address),
            IpAddr::V6(address) => ipv6_may_be_on_users_network(address),
        };
    }
    !name.contains('.')
        || ["localhost", "local", "lan", "home.arpa", "internal"]
            .iter()
            .any(|suffix| {
                name == *suffix
                    || name
                        .strip_suffix(suffix)
                        .is_some_and(|rest| rest.ends_with('.'))
            })
}

/// Providers whose every endpoint is a fixed public host, so a record that
/// names one says its model did not run on the user's network. Any other
/// provider's record is read by the local rule: OpenRouter and the generic
/// OpenAI-compatible provider take a user-chosen base URL, and their records
/// keep no endpoint.
pub fn provider_dials_only_public_hosts(provider_id: &str) -> bool {
    matches!(
        provider_id,
        "llm.cerebras"
            | "llm.sambanova"
            | "llm.aws_bedrock"
            | "decision.jev_typesafe"
            | "decision.jev_cloudflare"
            | "decision.clef_flash_cloudflare"
    )
}

/// What to store or show for a model, given where it ran.
///
/// The credential guard runs on the whole identifier, before anything is cut:
/// cutting `Bearer abc/secret` would otherwise keep `secret`.
pub fn for_place(id: &str, ran_remotely: bool) -> &str {
    if looks_credential_shaped(id) {
        WITHHELD_MODEL_PLACEHOLDER
    } else if ran_remotely {
        id
    } else {
        local_display_name(id)
    }
}

/// The display name of a local model: what follows the last separator of its
/// identifier, with a `file:` scheme and trailing separators removed first.
///
/// The result is a part of `id`, never trimmed or otherwise altered, or
/// [`LOCAL_MODEL_PLACEHOLDER`]. An empty id stays empty. Applying it again
/// changes nothing.
pub fn local_display_name(id: &str) -> &str {
    if id.is_empty() {
        return id;
    }
    let mut name = id;
    loop {
        let unschemed = match name.get(..5) {
            Some(scheme) if scheme.eq_ignore_ascii_case("file:") => &name[5..],
            _ => name,
        };
        let last = unschemed
            .trim_end_matches(['/', '\\'])
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or_default();
        if last.len() == name.len() {
            break;
        }
        name = last;
    }
    let drive_prefixed =
        matches!(name.as_bytes(), [drive, b':', ..] if drive.is_ascii_alphabetic());
    // A home marker names a user (`~numbat`); a dot segment, whitespace or a
    // bare drive names no model.
    if drive_prefixed
        || name.starts_with('~')
        || name.trim().is_empty()
        || matches!(name, "." | "..")
    {
        LOCAL_MODEL_PLACEHOLDER
    } else {
        name
    }
}

/// [`for_place`] over an owned identifier, reusing it when nothing is cut.
pub fn owned_for_place(id: String, ran_remotely: bool) -> String {
    let kept = for_place(&id, ran_remotely);
    if kept.len() == id.len() && kept.as_ptr() == id.as_ptr() {
        id
    } else {
        kept.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LOCAL_MODEL_PLACEHOLDER, WITHHELD_MODEL_PLACEHOLDER, for_place, local_display_name,
    };

    /// (identifier, what a local model shows). A remote model keeps every one
    /// of these whole.
    const CASES: [(&str, &str); 31] = [
        // Paths a local server reports or a user configures.
        ("/home/numbat/models/private.gguf", "private.gguf"),
        ("/models/mistral-7b/", "mistral-7b"),
        ("~/models/private.gguf", "private.gguf"),
        ("./numbat/private", "private"),
        ("C:\\Users\\numbat\\models\\private.gguf", "private.gguf"),
        ("C:/Users/numbat/models/private.gguf", "private.gguf"),
        (
            "\\\\numbat-host\\share\\private.safetensors",
            "private.safetensors",
        ),
        // An extended-length path and mixed separators.
        ("\\\\?\\C:\\Users\\numbat\\private.gguf", "private.gguf"),
        ("C:/Users\\numbat/models\\private.gguf", "private.gguf"),
        // The four that survived a shape test.
        ("numbat/checkpoint", "checkpoint"),
        ("numbat/private.gguf/", "private.gguf"),
        ("C:numbat/checkpoint", "checkpoint"),
        ("file:///home/numbat/checkpoint", "checkpoint"),
        // A name is not trimmed.
        ("/models/ private.gguf", " private.gguf"),
        // What older builds stored for a Windows path, and other values that
        // leave no name.
        ("C:Usersnumbatmodelsprivate.gguf", LOCAL_MODEL_PLACEHOLDER),
        ("C:private.gguf", LOCAL_MODEL_PLACEHOLDER),
        ("C:\\", LOCAL_MODEL_PLACEHOLDER),
        ("C:", LOCAL_MODEL_PLACEHOLDER),
        ("q:4b", LOCAL_MODEL_PLACEHOLDER),
        ("/", LOCAL_MODEL_PLACEHOLDER),
        ("~", LOCAL_MODEL_PLACEHOLDER),
        ("~numbat", LOCAL_MODEL_PLACEHOLDER),
        (".", LOCAL_MODEL_PLACEHOLDER),
        ("   ", LOCAL_MODEL_PLACEHOLDER),
        ("/models/   ", LOCAL_MODEL_PLACEHOLDER),
        // Hosted identifiers, which a local endpoint can serve too.
        ("org/model.gguf", "model.gguf"),
        ("org/repo/model.safetensors", "model.safetensors"),
        (
            "accounts/fireworks/models/llama-v3p1-8b-instruct",
            "llama-v3p1-8b-instruct",
        ),
        ("org/model@rev1", "model@rev1"),
        ("llama3.2:latest", "llama3.2:latest"),
        ("", ""),
    ];

    #[test]
    fn a_local_model_shows_a_name_and_a_remote_one_keeps_its_identifier() {
        for (id, shown) in CASES {
            assert_eq!(local_display_name(id), shown, "{id}");
            assert_eq!(for_place(id, false), shown, "{id}");
            assert_eq!(for_place(id, true), id, "{id}");
            assert_eq!(
                local_display_name(shown),
                shown,
                "{shown} is already a display name"
            );
            assert!(
                shown == LOCAL_MODEL_PLACEHOLDER
                    || id.trim_end_matches(['/', '\\']).ends_with(shown),
                "{shown} ends {id}"
            );
            assert!(!shown.contains(['/', '\\']), "{shown}");
            assert!(!shown.contains("numbat"), "{shown}");
        }
    }

    #[test]
    fn a_credential_shaped_identifier_is_withheld_before_anything_is_cut() {
        for id in [
            "Bearer Ym9i/c2VjcmV0",
            "/models/sk-numbat-key.gguf",
            "org/apikey",
        ] {
            for remote in [false, true] {
                assert_eq!(for_place(id, remote), WITHHELD_MODEL_PLACEHOLDER, "{id}");
            }
        }
        for remote in [false, true] {
            assert_eq!(
                for_place(WITHHELD_MODEL_PLACEHOLDER, remote),
                WITHHELD_MODEL_PLACEHOLDER
            );
        }
    }

    #[test]
    fn a_display_name_is_a_fixed_point_even_through_a_nested_scheme() {
        for id in ["numbat/file:private.gguf", "file:file:/numbat/private.gguf"] {
            let shown = local_display_name(id);
            assert_eq!(shown, "private.gguf", "{id}");
            assert_eq!(local_display_name(shown), shown);
        }
    }

    /// (endpoint, could it be on the user's own machine or network)
    pub(crate) const ENDPOINTS: [(&str, bool); 30] = [
        ("http://127.0.0.1:8080/v1", true),
        ("http://127.8.9.10/v1", true),
        ("http://[::1]:8080/v1", true),
        ("http://[::ffff:127.0.0.1]:8080/v1", true),
        ("http://0.0.0.0:8080", true),
        ("http://localhost:11434/v1", true),
        ("http://LOCALHOST.:11434/v1", true),
        ("http://models.localhost/v1", true),
        ("http://10.0.0.7:8000/v1", true),
        ("http://172.16.4.2/v1", true),
        ("http://192.168.1.20:1234/v1", true),
        ("http://169.254.10.1/v1", true),
        ("http://100.64.0.9/v1", true),
        ("http://[fd12:3456::1]/v1", true),
        ("http://[fe80::1]/v1", true),
        ("http://[::ffff:192.168.1.20]/v1", true),
        ("unix:///run/llama.sock", true),
        ("http+unix://%2Frun%2Fllama.sock/v1", true),
        ("http://gpu-box:8080/v1", true),
        ("http://gpu-box.local/v1", true),
        ("http://gpu-box.lan./v1", true),
        ("http://gpu.home.arpa/v1", true),
        ("http://llm.corp.internal/v1", true),
        ("not a url", true),
        // Public hosts, and public addresses that only look nearby.
        ("https://openrouter.ai/api/v1", false),
        ("https://api.cerebras.ai/v1", false),
        ("https://api.openai.com/v1", false),
        ("http://100.128.0.1/v1", false),
        ("http://[2606:4700::1111]/v1", false),
        ("https://notlocal.example.com/v1", false),
    ];

    #[test]
    fn an_endpoint_is_local_when_it_could_be_on_the_users_network() {
        for (endpoint, local) in ENDPOINTS {
            assert_eq!(
                super::endpoint_may_be_on_users_network(endpoint),
                local,
                "{endpoint}"
            );
        }
    }
}
