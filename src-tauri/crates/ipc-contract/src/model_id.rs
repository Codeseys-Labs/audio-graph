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

use serde::{Deserialize, Serialize};

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

/// Where a record's model ran, written by the build that applied the model
/// rule with the endpoint it dialled in hand. A record that carries it is read
/// as written; one without it was written before the rule and is read by the
/// rule for records that cannot say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelPlacement {
    /// On this machine, or at an endpoint that could be on the user's own
    /// machine or network: the record holds a display name.
    Local,
    /// At an endpoint off the user's network: the record holds the
    /// provider's hosted identifier.
    Remote,
}

impl ModelPlacement {
    /// Where a model at `endpoint` ran, by [`endpoint_may_be_on_users_network`].
    pub fn at(endpoint: &str) -> Self {
        if endpoint_may_be_on_users_network(endpoint) {
            Self::Local
        } else {
            Self::Remote
        }
    }

    pub fn ran_remotely(self) -> bool {
        self == Self::Remote
    }
}

/// Read a stored `model_placement` leniently: a value this build does not
/// know (a later build's variant, or any other JSON) reads as absent, so the
/// record falls back to the rule for unmarked records instead of failing its
/// decode, and with it a strict reader's whole stream.
///
/// For `#[serde(default, deserialize_with = "...")]` on a stored field.
pub fn lenient_placement<'de, D>(deserializer: D) -> Result<Option<ModelPlacement>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Stored {
        Known(ModelPlacement),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Option::<Stored>::deserialize(deserializer)? {
        Some(Stored::Known(placement)) => Some(placement),
        Some(Stored::Other(_)) | None => None,
    })
}

/// A stored model id, read by its record's [`ModelPlacement`] when it has one
/// and by `legacy_remote` (what an unmarked record's other fields say) when it
/// does not. The credential screen runs either way.
pub fn stored_for_place(
    id: String,
    placement: Option<ModelPlacement>,
    legacy_remote: bool,
) -> String {
    let ran_remotely = placement.map_or(legacy_remote, ModelPlacement::ran_remotely);
    owned_for_place(id, ran_remotely)
}

/// Characters of an API key after its prefix.
fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// Whether `index` starts a token: nothing alphanumeric right before it.
fn starts_token(bytes: &[u8], index: usize) -> bool {
    index == 0 || !bytes[index - 1].is_ascii_alphanumeric()
}

/// The fewest key characters after `sk-` that read as a key: real keys run to
/// dozens, and a model name such as `sk-small` stays well under.
const MIN_KEY_RUN: usize = 16;

/// Heuristic guard: does this value contain a credential-shaped token?
///
/// Matches, case-insensitively:
/// - an `sk-` prefix that starts a token and is followed by at least
///   [`MIN_KEY_RUN`] key characters (`task-`, `mask-` or `risk-` inside a name
///   does not start a token),
/// - `bearer` starting a token and followed by any whitespace,
/// - `api_key` or `apikey` anywhere,
/// - a JWT: three base64url segments joined by dots, the first starting
///   `eyJ` (case-sensitive, as a JSON header encodes).
pub fn looks_credential_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let key_prefix = lower.match_indices("sk-").any(|(index, prefix)| {
        starts_token(bytes, index)
            && bytes[index + prefix.len()..]
                .iter()
                .take_while(|byte| is_key_char(**byte))
                .count()
                >= MIN_KEY_RUN
    });
    let bearer = lower.match_indices("bearer").any(|(index, word)| {
        starts_token(bytes, index)
            && lower[index + word.len()..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace)
    });
    key_prefix
        || bearer
        || lower.contains("api_key")
        || lower.contains("apikey")
        || contains_jwt(value)
}

/// Three base64url segments joined by dots, the first starting `eyJ` at a
/// token start and at least ten characters long.
fn contains_jwt(value: &str) -> bool {
    let bytes = value.as_bytes();
    let segment = |from: usize| {
        bytes[from..]
            .iter()
            .take_while(|byte| is_key_char(**byte))
            .count()
    };
    value.match_indices("eyJ").any(|(start, _)| {
        if start > 0 && is_key_char(bytes[start - 1]) {
            return false;
        }
        let header = segment(start);
        if header < 10 {
            return false;
        }
        let mut at = start + header;
        for _ in 0..2 {
            if bytes.get(at) != Some(&b'.') {
                return false;
            }
            let length = segment(at + 1);
            if length == 0 {
                return false;
            }
            at += 1 + length;
        }
        true
    })
}

/// Could `endpoint` be on the user's own machine or network?
///
/// True for loopback in either family (bracketed IPv6 included), the
/// unspecified address, `localhost` with or without its trailing dot and any
/// `*.localhost` name, private, link-local, unique-local and carrier-grade-NAT
/// addresses (also as IPv4-mapped IPv6), a host-less URL such as a Unix
/// socket, a single-label host name, and names under `.local`, `.lan`,
/// `.home.arpa`, `.home`, `.internal`, `.localdomain` and `.corp`. An endpoint
/// that does not parse as a URL is treated as local. No name is resolved, so a
/// public name that resolves to the user's own machine is not detected.
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
        || [
            "localhost",
            "local",
            "lan",
            "home.arpa",
            "home",
            "internal",
            "localdomain",
            "corp",
        ]
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

    /// Credential-shaped values, built from plain words so nothing here looks
    /// like a real key.
    const CREDENTIAL_SHAPED: [&str; 8] = [
        "Bearer Ym9i/c2VjcmV0",
        "bearer\twombat/c2VjcmV0",
        "Authorization: BEARER\u{a0}wombat",
        concat!("/models/s", "k-", "wombatwombatwombat.gguf"),
        concat!("org/s", "k-proj-wombat_wombat-wombat"),
        "org/apikey",
        "eyJwombatheader.wombatpayload.wombatsignature",
        "org/eyJwombatheader.wombatpayload.c2VjcmV0",
    ];

    /// Names that only contain a marker inside a word, or a short run after
    /// it: they are model names and must survive.
    const LEGITIMATE: [&str; 10] = [
        "task-classifier",
        "org/mask-rcnn",
        "risk-model-v2",
        "desk-assistant.gguf",
        "flask-app.gguf",
        concat!("s", "k-small"),
        "bearer-llm",
        "forbearer model",
        "eyJ.short.name",
        "keyJwombatheader.wombatpayload.wombatsignature",
    ];

    #[test]
    fn a_credential_shaped_identifier_is_withheld_before_anything_is_cut() {
        for id in CREDENTIAL_SHAPED {
            assert!(super::looks_credential_shaped(id), "{id:?}");
            for remote in [false, true] {
                assert_eq!(for_place(id, remote), WITHHELD_MODEL_PLACEHOLDER, "{id:?}");
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
    fn a_marker_inside_a_name_does_not_withhold_it() {
        for id in LEGITIMATE {
            assert!(!super::looks_credential_shaped(id), "{id:?}");
            assert_eq!(for_place(id, true), id);
        }
    }

    #[test]
    fn an_unknown_stored_placement_reads_as_absent() {
        use super::ModelPlacement;

        #[derive(serde::Deserialize)]
        struct Stored {
            #[serde(default, deserialize_with = "super::lenient_placement")]
            model_placement: Option<ModelPlacement>,
        }
        let read = |json: &str| {
            serde_json::from_str::<Stored>(json)
                .expect("a stored placement never fails the record")
                .model_placement
        };
        assert_eq!(
            read(r#"{"model_placement":"local"}"#),
            Some(ModelPlacement::Local)
        );
        assert_eq!(
            read(r#"{"model_placement":"remote"}"#),
            Some(ModelPlacement::Remote)
        );
        for unknown in [
            r#"{}"#,
            r#"{"model_placement":null}"#,
            r#"{"model_placement":"elsewhere"}"#,
            r#"{"model_placement":"Remote"}"#,
            r#"{"model_placement":3}"#,
            r#"{"model_placement":{"kind":"remote"}}"#,
        ] {
            assert_eq!(read(unknown), None, "{unknown}");
        }
    }

    #[test]
    fn a_marked_record_is_read_as_written_and_an_unmarked_one_by_its_fallback() {
        use super::{ModelPlacement, stored_for_place};
        let hosted = "anthropic/claude-sonnet-4".to_string();
        assert_eq!(
            stored_for_place(hosted.clone(), Some(ModelPlacement::Remote), false),
            hosted
        );
        assert_eq!(
            stored_for_place(hosted.clone(), Some(ModelPlacement::Local), true),
            "claude-sonnet-4"
        );
        assert_eq!(
            stored_for_place(hosted.clone(), None, false),
            "claude-sonnet-4"
        );
        assert_eq!(stored_for_place(hosted.clone(), None, true), hosted);
        // The credential screen runs on a marked record too.
        assert_eq!(
            stored_for_place(
                "Bearer Ym9i/c2VjcmV0".to_string(),
                Some(ModelPlacement::Remote),
                true
            ),
            WITHHELD_MODEL_PLACEHOLDER
        );
        assert_eq!(
            serde_json::to_value(ModelPlacement::Remote).unwrap(),
            "remote"
        );
        assert_eq!(
            ModelPlacement::at("https://openrouter.ai/api/v1"),
            ModelPlacement::Remote
        );
        assert_eq!(
            ModelPlacement::at("http://[::1]:8080/v1"),
            ModelPlacement::Local
        );
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
    pub(crate) const ENDPOINTS: [(&str, bool); 34] = [
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
        ("http://gpu-box.localdomain/v1", true),
        ("http://nas.home/v1", true),
        ("http://llm.example.corp/v1", true),
        ("http://[fe80::1%25eth0]/v1", true),
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
