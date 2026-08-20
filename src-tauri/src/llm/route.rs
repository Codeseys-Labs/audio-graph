//! The single-skin named LLM route table (ADR-0038).
//!
//! Routes are **named entities**, not provider strings assembled at the call
//! site. Every content-bearing LLM dispatch resolves exactly one route, gates it
//! against ADR-0033's product-enablement boundary, and stamps the resolved route
//! id into provenance from trusted code.
//!
//! # There is no fallback mechanism here, on purpose
//!
//! ADR-0038:196-197 and :328-330 record that the accepted option *configured
//! with empty fallback lists* IS the pinned-route option. An empty list plus a
//! live chain walker is a configuration default someone can flip; this module
//! therefore carries **no `authorized_fallbacks` field and no walker**. Adding an
//! authorized alternate requires a new ADR **and** new dispatch code — not a
//! config change.
//!
//! # Why this lives in the app crate and not in a generator-visible crate
//!
//! With no fallback mechanism there is no persisted authorization record and no
//! new IPC shape, so nothing crosses the `crates/ipc-contract` or
//! `crates/provider-registry` generator boundary. The table also needs
//! [`crate::settings::LlmProvider`] and [`crate::provider_registry`], which the
//! lightweight crates deliberately do not link. When a future ticket authorizes a
//! fallback entry and must persist the authorization, that ticket moves the record
//! shape into `ipc-contract` under ADR-0027.

use crate::error::AppError;
use crate::settings::LlmProvider;

// ---------------------------------------------------------------------------
// Wire skin
// ---------------------------------------------------------------------------

/// A provider wire skin. Chat Completions is the only MVP-admitted skin
/// (ADR-0038:120-122).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireSkin {
    ChatCompletions,
    /// Reserved. Not dispatchable — see [`WireSkin::admitted`].
    Messages,
    /// Reserved. Not dispatchable — see [`WireSkin::admitted`].
    Responses,
}

/// An MVP-admitted wire skin. The only way to obtain one is
/// [`WireSkin::admitted`], and this enum has exactly one variant, so every
/// dispatcher that takes an `AdmittedSkin` is exhaustive without an arm for the
/// reserved skins: `Messages` / `Responses` have **no dispatch code path at
/// all**, not a rejected one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmittedSkin {
    ChatCompletions,
}

impl AdmittedSkin {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => WireSkin::ChatCompletions.as_str(),
        }
    }
}

impl WireSkin {
    pub fn admitted(self) -> Option<AdmittedSkin> {
        match self {
            Self::ChatCompletions => Some(AdmittedSkin::ChatCompletions),
            // Admission requires a parity probe for the *selected* endpoint.
            // ADR-0038:49-54 records that the preceding research deliberately
            // left Messages/Responses parity unproven, and no probe exists in
            // this repository. The variants exist only so a future probe can
            // name the skin it is proving.
            Self::Messages | Self::Responses => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ChatCompletions => "chat_completions",
            Self::Messages => "messages",
            Self::Responses => "responses",
        }
    }
}

// ---------------------------------------------------------------------------
// Per-endpoint capability
// ---------------------------------------------------------------------------

/// How strongly an endpoint constrains generation to a schema.
///
/// Graded and recorded, **never** a dispatch gate (ADR-0038 sub-decision 5,
/// :161-165): applied literally as a hard gate it rejects both designated proof
/// routes. The runtime validator stays the sole admission authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstrainedDecodingGrade {
    /// The endpoint documents schema-constrained generation.
    GuaranteedConstrained,
    /// The endpoint advertises a schema parameter; enforcement varies by
    /// upstream even with `strict: true`.
    AdvertisedHint,
    /// JSON mode or free text only.
    Unconstrained,
}

/// Capability facts for ONE endpoint. Never an aggregate model record:
/// ADR-0038:54-56 and :123-125 pin the selected Cerebras endpoint at 131,072
/// context / 40,960 max completion against an aggregate record of 262,144.
///
/// Both token fields are `Option` because only the Cerebras row has an in-repo
/// citation. `None` means "not documented in this repository", and the clamp is
/// then a no-op — a fabricated number would be worse than an absent one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EndpointCapability {
    /// Recorded, never enforced: nothing in this repository measures prompt
    /// tokens, and no count is fabricated to compare against this.
    pub max_context_tokens: Option<u32>,
    pub max_completion_tokens: Option<u32>,
    pub constrained_decoding: ConstrainedDecodingGrade,
}

impl EndpointCapability {
    const fn undocumented(constrained_decoding: ConstrainedDecodingGrade) -> Self {
        Self {
            max_context_tokens: None,
            max_completion_tokens: None,
            constrained_decoding,
        }
    }
}

// ---------------------------------------------------------------------------
// Route descriptors
// ---------------------------------------------------------------------------

/// Which blocking backend handle serves a route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockingBackend {
    NativeLlama,
    OpenAiCompatible,
    OpenRouter,
    MistralRs,
}

/// A named LLM egress route.
#[derive(Debug)]
pub struct RouteDescriptor {
    /// Stable route id stamped into provenance by trusted code. Namespaced
    /// `route.*` so it is never confused with a registry provider id (`llm.*`)
    /// or a settings variant tag.
    pub id: &'static str,
    pub display_name: &'static str,
    /// The registry descriptor this route is authorized against (ADR-0033:48-52).
    ///
    /// `route.cerebras_via_openrouter` and `route.openrouter` deliberately share
    /// `llm.openrouter`: that is the descriptor the endpoint authenticates to,
    /// and no `llm.cerebras_via_openrouter` descriptor exists. Minting one is a
    /// product-picker change governed by ADR-0033's promotion path and would
    /// churn the generated `src/generated/providerRegistry.ts`. The two routes
    /// stay distinct on id, capability row, and provenance — which is what
    /// ADR-0038 sub-decision 3 requires.
    pub provider_id: &'static str,
    pub skin: WireSkin,
    pub capability: EndpointCapability,
    /// `None` means the route has no blocking implementation.
    pub blocking_backend: Option<BlockingBackend>,
}

impl RouteDescriptor {
    /// Clamp a requested completion budget to this endpoint's documented
    /// maximum. Returns `(budget, clamped)`.
    ///
    /// Structural, not load-bearing today: the shipped completion defaults are
    /// 2048 (or 512 when `llm_api_config` is stale), both far under 40,960, so
    /// this does not fire in the shipped configuration. An endpoint with no
    /// documented maximum is returned unchanged.
    pub fn clamp_completion_budget(&self, requested: u32) -> (u32, bool) {
        match self.capability.max_completion_tokens {
            Some(max) if requested > max => (max, true),
            _ => (requested, false),
        }
    }
}

/// The route table. Every shipped row declares
/// [`WireSkin::ChatCompletions`]; a test pins that, so there is not even a live
/// reserved route.
pub const LLM_ROUTES: &[RouteDescriptor] = &[
    RouteDescriptor {
        id: "route.local_llama",
        display_name: "Local llama.cpp",
        provider_id: "llm.local_llama",
        skin: WireSkin::ChatCompletions,
        capability: EndpointCapability::undocumented(ConstrainedDecodingGrade::Unconstrained),
        blocking_backend: Some(BlockingBackend::NativeLlama),
    },
    RouteDescriptor {
        id: "route.mistralrs",
        display_name: "mistral.rs",
        provider_id: "llm.mistralrs",
        skin: WireSkin::ChatCompletions,
        // mistral.rs constrains generation with a schemars-derived grammar, but
        // that is a local runtime guarantee documented only by its own API, not
        // an endpoint record in this repository — graded as an advertised hint.
        capability: EndpointCapability::undocumented(ConstrainedDecodingGrade::AdvertisedHint),
        blocking_backend: Some(BlockingBackend::MistralRs),
    },
    RouteDescriptor {
        id: "route.cerebras_direct",
        display_name: "Cerebras (direct)",
        provider_id: "llm.cerebras",
        skin: WireSkin::ChatCompletions,
        capability: EndpointCapability {
            // The ONLY numbers in this table with an in-repo citation:
            // ADR-0038:54-56 records the selected Cerebras endpoint at 131,072
            // context / 40,960 max completion, against an aggregate model
            // record of 262,144 that must never appear here.
            max_context_tokens: Some(131_072),
            max_completion_tokens: Some(40_960),
            constrained_decoding: ConstrainedDecodingGrade::GuaranteedConstrained,
        },
        blocking_backend: Some(BlockingBackend::OpenAiCompatible),
    },
    RouteDescriptor {
        id: "route.sambanova_direct",
        display_name: "SambaNova (direct)",
        provider_id: "llm.sambanova",
        skin: WireSkin::ChatCompletions,
        capability: EndpointCapability::undocumented(ConstrainedDecodingGrade::Unconstrained),
        blocking_backend: Some(BlockingBackend::OpenAiCompatible),
    },
    RouteDescriptor {
        id: "route.openai_compatible",
        display_name: "OpenAI-compatible endpoint",
        provider_id: "llm.api",
        skin: WireSkin::ChatCompletions,
        // This row covers arbitrary user-supplied endpoints (Ollama, vLLM,
        // OpenAI, loopback), so no per-endpoint token record exists for it.
        capability: EndpointCapability::undocumented(ConstrainedDecodingGrade::Unconstrained),
        blocking_backend: Some(BlockingBackend::OpenAiCompatible),
    },
    RouteDescriptor {
        id: "route.openrouter",
        display_name: "OpenRouter",
        provider_id: "llm.openrouter",
        skin: WireSkin::ChatCompletions,
        // Unpinned OpenRouter's upstream varies per request, so its endpoint
        // token limits are not a fixed fact. `strict: true` is honored by some
        // upstreams and not others, which is exactly "advertised hint".
        capability: EndpointCapability::undocumented(ConstrainedDecodingGrade::AdvertisedHint),
        blocking_backend: Some(BlockingBackend::OpenRouter),
    },
    RouteDescriptor {
        id: "route.cerebras_via_openrouter",
        display_name: "Cerebras via OpenRouter",
        provider_id: "llm.openrouter",
        skin: WireSkin::ChatCompletions,
        // A distinct processing path (AudioGraph → OpenRouter → Cerebras) and a
        // distinct ADR-0034 producer row (ADR-0038:341-345). The token limits of
        // the OpenRouter-fronted Cerebras endpoint are not documented in this
        // repository — only the direct endpoint's are — so they stay `None`
        // rather than being copied across a different processing path.
        capability: EndpointCapability::undocumented(
            ConstrainedDecodingGrade::GuaranteedConstrained,
        ),
        blocking_backend: Some(BlockingBackend::OpenRouter),
    },
    RouteDescriptor {
        id: "route.aws_bedrock",
        display_name: "AWS Bedrock",
        provider_id: "llm.aws_bedrock",
        skin: WireSkin::ChatCompletions,
        capability: EndpointCapability::undocumented(ConstrainedDecodingGrade::Unconstrained),
        // Bedrock serves streaming chat only (`llm/bedrock.rs` +
        // `llm/streaming.rs`); it has no blocking client, and
        // `api_config_from_runtime_settings` returns `None` for it. Declaring
        // that here turns today's misleading "API LLM client is not configured"
        // into an honest terminal error.
        blocking_backend: None,
    },
];

fn route_by_id(id: &'static str) -> &'static RouteDescriptor {
    LLM_ROUTES
        .iter()
        .find(|route| route.id == id)
        .unwrap_or_else(|| panic!("LLM_ROUTES is missing the {id} row"))
}

/// The single resolver from a settings provider variant to a named route.
pub fn resolve_route(provider: &LlmProvider) -> &'static RouteDescriptor {
    match provider {
        LlmProvider::LocalLlama => route_by_id("route.local_llama"),
        LlmProvider::MistralRs { .. } => route_by_id("route.mistralrs"),
        LlmProvider::Api { endpoint, .. } => route_for_api_endpoint(endpoint),
        // Resolved from the settings variant alone this is the unpinned route.
        // The dispatch site re-resolves from the live client's routing policy
        // (see `route_for_openrouter_policy`); both share `provider_id`, so the
        // refinement can never widen egress.
        LlmProvider::OpenRouter { .. } => route_by_id("route.openrouter"),
        LlmProvider::AwsBedrock { .. } => route_by_id("route.aws_bedrock"),
    }
}

/// Resolve the OpenAI-compatible route from an endpoint URL. Endpoint-sensitive,
/// so it must be applied to the endpoint that is actually dispatched to — not to
/// a settings snapshot taken earlier (see [`AuthorizedRoute::ensure_serves`]).
pub fn route_for_api_endpoint(endpoint: &str) -> &'static RouteDescriptor {
    if crate::settings::is_cerebras_endpoint(endpoint) {
        route_by_id("route.cerebras_direct")
    } else if crate::settings::is_sambanova_endpoint(endpoint) {
        route_by_id("route.sambanova_direct")
    } else {
        route_by_id("route.openai_compatible")
    }
}

/// Resolve the OpenRouter route from the routing policy that will ride the
/// request.
///
/// `route.cerebras_via_openrouter` requires a genuine **singleton** pin, not
/// merely a first-entry match. `buildOpenRouterRoutingPolicy`'s
/// `strict_accelerator` preset fills `order` AND `only` from a provider *list*,
/// and `allow_fallbacks: false` still permits OpenRouter to serve from any
/// provider in that list. So `order = ["cerebras", "groq"]` would let Groq serve
/// while trusted code stamped the Cerebras route id — a false authorization
/// identity written by trusted code. Only a one-entry pin yields the distinct
/// route; anything wider is honestly `route.openrouter`.
pub fn route_for_openrouter_policy(
    policy: Option<&super::openrouter::OpenRouterRoutingPolicy>,
    legacy_provider_order: Option<&[String]>,
) -> &'static RouteDescriptor {
    let effective = policy
        .cloned()
        .or_else(|| {
            super::openrouter::OpenRouterRoutingPolicy::from_provider_order(legacy_provider_order)
        })
        .unwrap_or_default();

    if effective.allow_fallbacks != Some(false) {
        // ADR-0038:153-157: OpenRouter defaults `allow_fallbacks` to true, so an
        // unpinned config is genuinely NOT Cerebras-via-OpenRouter.
        return route_by_id("route.openrouter");
    }

    let pinned = singleton_pin(&effective.order).or_else(|| singleton_pin(&effective.only));
    // `only` is an OpenRouter ALLOWLIST, not a second pin slot: it is
    // compatible with the Cerebras-via-OpenRouter route only when it is empty
    // (no allowlist restriction) or itself a singleton pin that normalizes to
    // `cerebras` — a singleton naming any OTHER provider (e.g. `only:
    // ["groq"]`) means Groq, not Cerebras, actually serves the request, so
    // stamping the Cerebras route id would be a trusted-code falsehood.
    let only_is_compatible =
        effective.only.is_empty() || singleton_pin(&effective.only).as_deref() == Some("cerebras");
    match pinned {
        Some(provider) if provider == "cerebras" && only_is_compatible => {
            route_by_id("route.cerebras_via_openrouter")
        }
        _ => route_by_id("route.openrouter"),
    }
}

/// The single normalized provider name of a one-entry pin list, else `None`.
fn singleton_pin(list: &[String]) -> Option<String> {
    match list {
        [single] => {
            let normalized = super::openrouter::normalize_provider_name(single);
            (!normalized.is_empty()).then_some(normalized)
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The ADR-0033 gate
// ---------------------------------------------------------------------------

/// Proof that a route's registry descriptor was resolved and admitted for a
/// content-bearing dispatch (ADR-0033:48-52).
///
/// The private `_seal` field makes [`authorize_route_dispatch`] the ONLY
/// constructor, in this module or any other, so a dispatch site that skipped the
/// gate cannot be written — it does not compile. That is why every backend
/// attempt function takes `&AuthorizedRoute`.
#[derive(Debug)]
pub struct AuthorizedRoute {
    route: &'static RouteDescriptor,
    skin: AdmittedSkin,
    completion_budget: Option<u32>,
    completion_budget_clamped: bool,
    _seal: (),
}

impl AuthorizedRoute {
    pub fn descriptor(&self) -> &'static RouteDescriptor {
        self.route
    }

    pub fn id(&self) -> &'static str {
        self.route.id
    }

    pub fn provider_id(&self) -> &'static str {
        self.route.provider_id
    }

    pub fn skin(&self) -> AdmittedSkin {
        self.skin
    }

    /// The per-endpoint-clamped completion budget, when the route documents a
    /// maximum and the caller supplied a request. `None` when either is absent.
    pub fn completion_budget(&self) -> Option<u32> {
        self.completion_budget
    }

    pub fn completion_budget_clamped(&self) -> bool {
        self.completion_budget_clamped
    }

    /// Record the completion budget this dispatch will declare, clamped to the
    /// route's documented per-endpoint maximum.
    pub fn with_completion_budget(mut self, requested: u32) -> Self {
        let (budget, clamped) = self.route.clamp_completion_budget(requested);
        self.completion_budget = Some(budget);
        self.completion_budget_clamped = clamped;
        self
    }

    /// Re-resolve the stamped route from the client actually being dispatched to,
    /// failing closed when that client no longer authorizes to the same registry
    /// descriptor.
    ///
    /// This exists because the job's `LlmProvider` is a **snapshot** taken at
    /// session start, while egress goes through the shared client handle that
    /// `sync_llm_api_client_from_settings_cache` /
    /// `sync_openrouter_client_from_settings_cache` rebuild on every settings
    /// save. Without it a session authorized as `route.cerebras_direct` could
    /// egress to a re-pointed endpoint while trusted code still stamped
    /// `route.cerebras_direct` and applied the Cerebras capability row — a
    /// trusted-stamp falsification, and the same saved/stale-settings class
    /// ADR-0033 exists to stop.
    ///
    /// Two outcomes, and the distinction is the security property:
    /// - **Refinement** is allowed when the live route shares this route's
    ///   `provider_id`. That is how `route.openrouter` sharpens into
    ///   `route.cerebras_via_openrouter` once the live routing policy is read:
    ///   the ADR-0033 gate already admitted `llm.openrouter`, so sharpening the
    ///   recorded identity cannot widen egress.
    /// - **Refusal** otherwise. A re-pointed `Api` endpoint moves between
    ///   `llm.api` / `llm.cerebras` / `llm.sambanova`, which is a different
    ///   authorization, so it is rejected with a content-free error naming route
    ///   ids only.
    ///
    /// Residual, stated rather than hidden: an endpoint edit that stays inside one
    /// registry descriptor (`localhost:11434` → `api.openai.com`, both `llm.api`)
    /// is not an authorization change and is accepted. The privacy dimension of
    /// that edit is carried by the rebuilt client's own
    /// `content_egress_policy`, which `sync_llm_api_client_from_settings_cache`
    /// recomputes from `requires_cloud_content_transfer()`.
    pub fn refine_within_authorization(
        &self,
        live: &'static RouteDescriptor,
    ) -> Result<&'static RouteDescriptor, String> {
        if live.provider_id == self.route.provider_id {
            return Ok(live);
        }
        Err(format!(
            "authorized route {} (gated on {}) does not authorize the configured client route {} \
             (gated on {}); refusing dispatch — re-authorization required",
            self.route.id, self.route.provider_id, live.id, live.provider_id
        ))
    }
}

/// Resolve, gate, and admit a route for a content-bearing dispatch.
pub fn authorize_route_dispatch(provider: &LlmProvider) -> Result<AuthorizedRoute, AppError> {
    authorize_descriptor(resolve_route(provider))
}

fn authorize_descriptor(route: &'static RouteDescriptor) -> Result<AuthorizedRoute, AppError> {
    crate::provider_registry::ensure_provider_id_start_enabled(route.provider_id)?;
    let skin = route.skin.admitted().ok_or_else(|| {
        AppError::Unknown(format!(
            "wire skin {} is reserved and not admitted for dispatch (ADR-0038)",
            route.skin.as_str()
        ))
    })?;
    Ok(AuthorizedRoute {
        route,
        skin,
        completion_budget: None,
        completion_budget_clamped: false,
        _seal: (),
    })
}

// ---------------------------------------------------------------------------
// Terminal status
// ---------------------------------------------------------------------------

/// The one normalized terminal status for an LLM route attempt (ADR-0038:126-127).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalStatus {
    Completed,
    Truncated,
    Refused,
    Failed,
    TransportLost,
}

impl TerminalStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Truncated => "Truncated",
            Self::Refused => "Refused",
            Self::Failed => "Failed",
            Self::TransportLost => "TransportLost",
        }
    }
}

/// Normalize a provider `finish_reason` into a [`TerminalStatus`].
///
/// Trim + ASCII-lowercase first, mirroring `bedrock::map_stop_reason`.
///
/// Two mappings deserve a note because the code cannot show why:
/// - `tool_calls` / `function_call` are `Failed`, not `Completed`: no request in
///   this repository sends tools, so a tool call is not a usable completion.
/// - Any **unrecognized** non-empty token is `Completed`, matching the absent
///   case below and `bedrock::map_stop_reason`'s fallback to `"stop"` for
///   exactly the same reason: "so the stream still terminates cleanly rather
///   than surfacing a raw SDK token" — this function claims to mirror that
///   fallback, so it must not invert it. A vendor-specific *success* token this
///   table has not enumerated yet (Together AI's `"eos"`, a future SDK
///   addition) is not evidence of an unusable completion; ADR-0038's own rule is
///   that "the runtime validator must remain the sole admission authority," so
///   an unrecognized token defers to the validator instead of discarding a
///   perfectly valid patch before the JSON is even parsed.
///
/// Residual limitation, stated rather than papered over: an absent, null, or
/// unrecognized `finish_reason` maps to `Completed`, because many
/// OpenAI-compatible servers omit the field and vendor vocabularies are not
/// exhaustively enumerable here. A provider that truncates without reporting it
/// is still only caught by the runtime validator.
pub fn terminal_status_from_finish_reason(finish_reason: Option<&str>) -> TerminalStatus {
    let Some(raw) = finish_reason else {
        return TerminalStatus::Completed;
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "length" | "max_tokens" | "model_length" => TerminalStatus::Truncated,
        "content_filter" | "guardrail_intervened" | "refusal" => TerminalStatus::Refused,
        // No request in this repository sends tools, so a tool call is not a
        // usable completion — this is a KNOWN-unusable token, unlike the
        // unrecognized-token fallback below.
        "tool_calls" | "function_call" => TerminalStatus::Failed,
        // "" / "stop" / "end_turn" / "stop_sequence", and every other
        // unrecognized non-empty token.
        _ => TerminalStatus::Completed,
    }
}

// ---------------------------------------------------------------------------
// Retry classification
// ---------------------------------------------------------------------------

/// Four classes, classification only.
///
/// There is deliberately **no "never dispatched" class, and it must not grow
/// one**: a route layer cannot know whether the socket closed before or after the
/// provider began work. That fact lives in `audio-graph-5e41`'s durable
/// scheduler record (`DurableQueued`), and provably-absent is 5e41's
/// `AbsentRetryAuthorized`. A consumer that wants "definitely-not-dispatched"
/// must read its own durable record (ADR-0038:235-240).
///
/// Retry **progression** is `audio-graph-3b48`'s; this record owns classification
/// only (ADR-0038:241-244).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RetryClass {
    /// The provider deterministically rejected the request (401/403, 400/422,
    /// 404). A retry cannot change the answer.
    PermanentRejection,
    /// The provider signalled temporary unavailability BEFORE producing a
    /// completion: 408/409/429/5xx, or a **connect-phase** transport failure
    /// (connect failed ⇒ nothing was sent).
    TransientAvailability,
    /// The provider produced a terminal response we cannot use: `Truncated`,
    /// `Refused`, an undecodable 2xx body, or a draft the validator rejects. The
    /// remote effect is KNOWN to have happened and been billed.
    UnusableCompletion,
    /// The route layer cannot tell whether the provider performed work: a
    /// post-send timeout, a mid-flight socket loss, `TransportLost`. **Never**
    /// auto-retried.
    ///
    /// Named `ExternalEffectUnknown`, not `OutcomeUncertain`: `audio-graph-5e41`
    /// (closed and accepted) already assigned `OutcomeUncertain` to a
    /// canonical-write state, and the collision would land inside the one
    /// taxonomy that reads both vocabularies (ADR-0038:128-135).
    ExternalEffectUnknown,
}

impl RetryClass {
    /// Whether this class may be auto-reissued without fresh authorization.
    pub fn auto_retry_permitted(self) -> bool {
        match self {
            Self::TransientAvailability => true,
            // `UnusableCompletion` is not auto-reissued by the route layer; the
            // in-client post-2xx body-decode retry is the one bounded reissue
            // that survives, and it is classified here rather than hidden.
            Self::PermanentRejection | Self::UnusableCompletion | Self::ExternalEffectUnknown => {
                false
            }
        }
    }
}

/// Classify a terminal status for retry purposes. `None` for `Completed`: a
/// completed attempt has nothing to reissue, and inventing a class for it would
/// invite a reader to treat the field as an instruction.
pub fn retry_class_for_terminal_status(status: TerminalStatus) -> Option<RetryClass> {
    match status {
        TerminalStatus::Completed => None,
        // The provider produced these and billed for them, so the remote effect
        // is KNOWN — that is what separates them from `ExternalEffectUnknown`.
        TerminalStatus::Truncated | TerminalStatus::Refused | TerminalStatus::Failed => {
            Some(RetryClass::UnusableCompletion)
        }
        TerminalStatus::TransportLost => Some(RetryClass::ExternalEffectUnknown),
    }
}

// ---------------------------------------------------------------------------
// Served-route evidence
// ---------------------------------------------------------------------------

/// Which side of the wire the recorded model id came from. Stamped by trusted
/// code; a model- or config-echoed id is never recorded without this marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelIdentitySource {
    /// The response reported the model that served the request.
    Served,
    /// The response omitted a model echo, so the requested id is recorded.
    /// Default so records written before this contract are read honestly.
    #[default]
    Requested,
}

/// Content-free evidence read off one wire response. Every string field passes
/// [`sanitize_route_metadata`], so no prompt, reply, or credential text can ride
/// through even from a hostile upstream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WireOutcome {
    pub terminal_status: TerminalStatus,
    /// Response top-level `model` echo, sanitized. `None` when the provider
    /// omits it.
    pub served_model: Option<String>,
    /// OpenRouter's response top-level `provider`, sanitized. `None` elsewhere.
    pub served_upstream_provider: Option<String>,
    /// The grade actually achieved by the request form that was sent — a strict
    /// request downgraded to JSON mode records `Unconstrained`, not the route's
    /// declared grade.
    pub constrained_decoding: ConstrainedDecodingGrade,
}

impl WireOutcome {
    /// A wire outcome from a response that carried no route metadata at all.
    pub fn plain(
        terminal_status: TerminalStatus,
        constrained_decoding: ConstrainedDecodingGrade,
    ) -> Self {
        Self {
            terminal_status,
            served_model: None,
            served_upstream_provider: None,
            constrained_decoding,
        }
    }
}

/// Which structured-output request form a dispatch will send.
///
/// Chosen from the route's [`ConstrainedDecodingGrade`], never from a host
/// substring: `prefers_vllm_structured_outputs()` remains the one endpoint
/// heuristic, and it selects only the vLLM-specific body field.
#[derive(Debug, Clone, PartialEq)]
pub enum RequestOutputForm {
    /// No `response_format` at all (free-text chat).
    Unconstrained,
    /// `{"type": "json_object"}` — generic JSON mode, no schema constraint.
    JsonObject,
    /// `{"type": "json_schema", "json_schema": {name, strict: true, schema}}`.
    StrictJsonSchema {
        name: String,
        schema: serde_json::Value,
    },
    /// vLLM's non-standard `structured_outputs: {json: schema}` body field.
    VllmStructuredOutputs { schema: serde_json::Value },
}

impl RequestOutputForm {
    /// The grade this request form actually achieves on a route whose endpoint
    /// declares `route_grade`.
    ///
    /// A strict schema request can be no stronger than the endpoint's own
    /// guarantee, and JSON mode is `Unconstrained` regardless of what the
    /// endpoint could have done — so a downgrade records the truth, not the
    /// route's declared ambition.
    pub fn achieved_grade(
        &self,
        route_grade: ConstrainedDecodingGrade,
    ) -> ConstrainedDecodingGrade {
        match self {
            Self::Unconstrained | Self::JsonObject => ConstrainedDecodingGrade::Unconstrained,
            Self::StrictJsonSchema { .. } => route_grade,
            // vLLM's field is a documented server extension, not a first-party
            // generation guarantee for the model behind it.
            Self::VllmStructuredOutputs { .. } => ConstrainedDecodingGrade::AdvertisedHint,
        }
    }
}

/// The content-free route record persisted with a projection patch: one per
/// patch, never multiplied per materialized item.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RouteRecord {
    /// Stamped route id (`route.cerebras_via_openrouter`, …).
    pub route_id: String,
    /// Registry provider id the route was authorized against (ADR-0033).
    pub provider_id: String,
    pub wire_skin: String,
    pub terminal_status: TerminalStatus,
    /// `None` when the attempt completed — see
    /// [`retry_class_for_terminal_status`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_class: Option<RetryClass>,
    pub constrained_decoding: ConstrainedDecodingGrade,
    /// Whether the declared completion budget was clamped to the endpoint's
    /// documented maximum.
    #[serde(default)]
    pub completion_budget_clamped: bool,
    /// The upstream provider that actually served the request, when the response
    /// reported one. Sanitized metadata only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub served_upstream_provider: Option<String>,
}

// ---------------------------------------------------------------------------
// Metadata sanitization (shared with the OpenRouter telemetry path)
// ---------------------------------------------------------------------------

/// Maximum length of a retained provider/model metadata token. Real provider
/// names (`"Cerebras"`, `"Amazon Bedrock"`) and model slugs
/// (`"anthropic/claude-sonnet-4.5"`) sit well under this; the cap is tight enough
/// that a full prompt or reply cannot survive as metadata.
pub(crate) const MAX_METADATA_LEN: usize = 64;

/// Sanitize a free-form provider/model metadata string into a bounded, non-secret
/// token, or drop it entirely.
///
/// Redaction defense-in-depth (seed audio-graph-76bd), unchanged from the
/// implementation this replaced in `openrouter.rs`:
/// 1. Reject anything longer than [`MAX_METADATA_LEN`] outright — a provider name
///    or model slug is short, so an over-length value is prompt/reply spill and is
///    dropped rather than truncated.
/// 2. Reject values carrying a credential-shaped token (`sk-…`, `Bearer …`) so a
///    hostile upstream that echoes a key into the `provider`/`model` field cannot
///    smuggle it into persisted route evidence.
/// 3. Keep only `[A-Za-z0-9 ._:/-]`, trim, and drop if nothing survives.
pub(crate) fn sanitize_route_metadata(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_METADATA_LEN {
        return None;
    }
    if looks_credential_shaped(trimmed) {
        return None;
    }

    let sanitized: String = trimmed
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || matches!(ch, ' ' | '-' | '_' | '.' | ':' | '/'))
        .collect();
    let sanitized = sanitized.trim();
    if sanitized.is_empty() {
        None
    } else {
        Some(sanitized.to_string())
    }
}

/// Heuristic guard: does this value contain a credential-shaped token? Catches
/// the common API-key prefixes and bearer-token shapes so a routed-provider echo
/// can never persist a secret as "metadata". Case-insensitive.
pub(crate) fn looks_credential_shaped(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("sk-")
        || lower.contains("bearer ")
        || lower.contains("api_key")
        || lower.contains("apikey")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::openrouter::{DEFAULT_BASE_URL, OpenRouterRoutingPolicy};

    fn openrouter_provider() -> LlmProvider {
        LlmProvider::OpenRouter {
            model: "openai/gpt-5.2".to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            provider_order: None,
            include_usage_in_stream: true,
            api_key: String::new(),
        }
    }

    // ----- table integrity --------------------------------------------------

    #[test]
    fn route_ids_are_unique_and_namespaced() {
        let mut ids: Vec<&str> = LLM_ROUTES.iter().map(|route| route.id).collect();
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "route ids must be unique");
        for route in LLM_ROUTES {
            assert!(
                route.id.starts_with("route."),
                "{} must live in the route.* namespace so it is never mistaken for a \
                 registry provider id",
                route.id
            );
        }
    }

    #[test]
    fn every_shipped_route_declares_the_only_admitted_skin() {
        for route in LLM_ROUTES {
            assert_eq!(
                route.skin,
                WireSkin::ChatCompletions,
                "{} declares a non-admitted wire skin",
                route.id
            );
            assert!(route.skin.admitted().is_some());
        }
    }

    #[test]
    fn reserved_skins_are_not_admitted() {
        assert!(WireSkin::Messages.admitted().is_none());
        assert!(WireSkin::Responses.admitted().is_none());
    }

    #[test]
    fn every_llm_route_provider_id_resolves_and_is_gated() {
        for route in LLM_ROUTES {
            // Panics if the registry has no such descriptor.
            let descriptor = crate::provider_registry::descriptor_by_id(route.provider_id);
            assert_eq!(descriptor.id, route.provider_id);
            // The gate is called for every row; all seven llm.* ids are
            // MVP-selectable today, so every row admits.
            authorize_descriptor(route)
                .unwrap_or_else(|e| panic!("{} failed the ADR-0033 gate: {e}", route.id));
        }
    }

    #[test]
    fn every_settings_variant_resolves_through_the_gate() {
        let providers = [
            LlmProvider::LocalLlama,
            LlmProvider::MistralRs {
                model_id: "m.gguf".to_string(),
            },
            LlmProvider::Api {
                endpoint: "http://localhost:11434/v1".to_string(),
                api_key: String::new(),
                model: "llama3.2".to_string(),
            },
            LlmProvider::Api {
                endpoint: crate::settings::CEREBRAS_BASE_URL.to_string(),
                api_key: String::new(),
                model: "gpt-oss-120b".to_string(),
            },
            LlmProvider::Api {
                endpoint: crate::settings::SAMBANOVA_BASE_URL.to_string(),
                api_key: String::new(),
                model: "x".to_string(),
            },
            openrouter_provider(),
            LlmProvider::AwsBedrock {
                region: "us-east-1".to_string(),
                model_id: "anthropic.claude".to_string(),
                credential_source: Default::default(),
            },
        ];
        let ids: Vec<&str> = providers
            .iter()
            .map(|provider| {
                authorize_route_dispatch(provider)
                    .expect("every MVP LLM provider admits")
                    .id()
            })
            .collect();
        assert_eq!(
            ids,
            vec![
                "route.local_llama",
                "route.mistralrs",
                "route.openai_compatible",
                "route.cerebras_direct",
                "route.sambanova_direct",
                "route.openrouter",
                "route.aws_bedrock",
            ]
        );
    }

    #[test]
    fn bedrock_has_no_blocking_route() {
        assert!(
            route_by_id("route.aws_bedrock").blocking_backend.is_none(),
            "Bedrock serves streaming chat only"
        );
    }

    // ----- capability -------------------------------------------------------

    #[test]
    fn no_route_declares_the_aggregate_262144() {
        for route in LLM_ROUTES {
            assert_ne!(
                route.capability.max_context_tokens,
                Some(262_144),
                "{} declares the AGGREGATE model record, not a per-endpoint fact",
                route.id
            );
            assert_ne!(route.capability.max_completion_tokens, Some(262_144));
        }
    }

    #[test]
    fn cerebras_completion_budget_clamps_to_endpoint_not_aggregate_model_record() {
        let cerebras = route_by_id("route.cerebras_direct");
        assert_eq!(cerebras.capability.max_context_tokens, Some(131_072));
        assert_eq!(cerebras.clamp_completion_budget(262_144), (40_960, true));
        // The shipped defaults are far under the cap, so the clamp is structural.
        assert_eq!(cerebras.clamp_completion_budget(2_048), (2_048, false));
        assert_eq!(cerebras.clamp_completion_budget(512), (512, false));
    }

    #[test]
    fn undocumented_capability_makes_the_clamp_a_no_op() {
        let generic = route_by_id("route.openai_compatible");
        assert_eq!(generic.capability.max_completion_tokens, None);
        assert_eq!(generic.clamp_completion_budget(999_999), (999_999, false));
    }

    #[test]
    fn authorized_route_records_the_clamp_decision() {
        let cerebras = authorize_descriptor(route_by_id("route.cerebras_direct"))
            .expect("cerebras admits")
            .with_completion_budget(262_144);
        assert_eq!(cerebras.completion_budget(), Some(40_960));
        assert!(cerebras.completion_budget_clamped());

        let generic = authorize_descriptor(route_by_id("route.openai_compatible"))
            .expect("generic admits")
            .with_completion_budget(2_048);
        assert_eq!(generic.completion_budget(), Some(2_048));
        assert!(!generic.completion_budget_clamped());
    }

    // ----- the Cerebras-via-OpenRouter split --------------------------------

    #[test]
    fn cerebras_via_openrouter_is_a_distinct_route_sharing_one_gate_descriptor() {
        let direct = route_by_id("route.cerebras_direct");
        let via = route_by_id("route.cerebras_via_openrouter");
        let plain = route_by_id("route.openrouter");

        assert_ne!(direct.id, via.id);
        assert_ne!(via.id, plain.id);
        assert_ne!(
            via.capability, plain.capability,
            "the via-OpenRouter row must not be a copy of the unpinned row"
        );
        assert_ne!(via.capability, direct.capability);
        // Both OpenRouter-fronted routes authenticate to the same registry
        // descriptor; no llm.cerebras_via_openrouter descriptor exists.
        assert_eq!(via.provider_id, "llm.openrouter");
        assert_eq!(plain.provider_id, "llm.openrouter");
        assert_eq!(direct.provider_id, "llm.cerebras");
    }

    #[test]
    fn only_a_singleton_cerebras_pin_yields_the_via_openrouter_route() {
        let pinned = OpenRouterRoutingPolicy {
            order: vec!["cerebras".to_string()],
            only: vec!["cerebras".to_string()],
            allow_fallbacks: Some(false),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            route_for_openrouter_policy(Some(&pinned), None).id,
            "route.cerebras_via_openrouter"
        );

        // A multi-provider `strict_accelerator` list satisfies a first-entry
        // discriminator but still permits Groq to serve, so stamping the Cerebras
        // route id would be a trusted-code falsehood.
        let multi = OpenRouterRoutingPolicy {
            order: vec!["cerebras".to_string(), "groq".to_string()],
            only: vec!["cerebras".to_string(), "groq".to_string()],
            allow_fallbacks: Some(false),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            route_for_openrouter_policy(Some(&multi), None).id,
            "route.openrouter",
            "a multi-provider pin is not Cerebras-via-OpenRouter"
        );

        // A `custom` policy whose `order` singleton pins Cerebras but whose
        // `only` allowlist names a DIFFERENT provider: Groq actually serves the
        // request (OpenRouter's `only` is an allowlist), so stamping the
        // Cerebras-via-OpenRouter route id would be a trusted-code falsehood.
        let divergent_singleton = OpenRouterRoutingPolicy {
            order: vec!["cerebras".to_string()],
            only: vec!["groq".to_string()],
            allow_fallbacks: Some(false),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            route_for_openrouter_policy(Some(&divergent_singleton), None).id,
            "route.openrouter",
            "an `only` singleton that names a different provider than the `order` pin \
             must not be stamped Cerebras-via-OpenRouter"
        );

        // Without allow_fallbacks=false OpenRouter may route anywhere.
        let unpinned = OpenRouterRoutingPolicy {
            order: vec!["cerebras".to_string()],
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            route_for_openrouter_policy(Some(&unpinned), None).id,
            "route.openrouter"
        );

        // A different singleton provider is the plain route.
        let groq = OpenRouterRoutingPolicy {
            order: vec!["groq".to_string()],
            allow_fallbacks: Some(false),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            route_for_openrouter_policy(Some(&groq), None).id,
            "route.openrouter"
        );

        // No policy at all, and a legacy provider_order (which never sets
        // allow_fallbacks), are both the plain route.
        assert_eq!(
            route_for_openrouter_policy(None, None).id,
            "route.openrouter"
        );
        let legacy = vec!["cerebras".to_string()];
        assert_eq!(
            route_for_openrouter_policy(None, Some(&legacy)).id,
            "route.openrouter"
        );
    }

    #[test]
    fn singleton_pin_folds_display_names_and_separators() {
        let pinned = OpenRouterRoutingPolicy {
            order: vec!["Cerebras".to_string()],
            allow_fallbacks: Some(false),
            ..OpenRouterRoutingPolicy::default()
        };
        assert_eq!(
            route_for_openrouter_policy(Some(&pinned), None).id,
            "route.cerebras_via_openrouter"
        );
    }

    // ----- live-config consistency ------------------------------------------

    #[test]
    fn refinement_rejects_a_client_that_moved_off_the_authorized_descriptor() {
        let authorized = authorize_route_dispatch(&LlmProvider::Api {
            endpoint: crate::settings::CEREBRAS_BASE_URL.to_string(),
            api_key: "sk-cerebras-secret".to_string(),
            model: "gpt-oss-120b".to_string(),
        })
        .expect("cerebras admits");
        assert_eq!(authorized.id(), "route.cerebras_direct");
        assert_eq!(
            authorized
                .refine_within_authorization(route_for_api_endpoint(
                    crate::settings::CEREBRAS_BASE_URL
                ))
                .expect("the unchanged endpoint still serves this route")
                .id,
            "route.cerebras_direct"
        );

        // Mid-session the user re-points the endpoint; the shared client is
        // rebuilt, but the queued job still carries the Cerebras snapshot.
        let err = authorized
            .refine_within_authorization(route_for_api_endpoint("https://api.openai.com/v1"))
            .expect_err("a re-pointed endpoint must fail closed");
        assert!(err.contains("route.cerebras_direct") && err.contains("route.openai_compatible"));
        assert!(
            !err.contains("api.openai.com")
                && !err.contains("sk-cerebras-secret")
                && !err.contains("gpt-oss-120b"),
            "the refusal must be content-free: route ids only, got: {err}"
        );
    }

    #[test]
    fn openrouter_refinement_sharpens_the_stamp_without_widening_authorization() {
        let authorized =
            authorize_route_dispatch(&openrouter_provider()).expect("openrouter admits");
        assert_eq!(authorized.id(), "route.openrouter");

        let pinned = OpenRouterRoutingPolicy {
            order: vec!["cerebras".to_string()],
            only: vec!["cerebras".to_string()],
            allow_fallbacks: Some(false),
            ..OpenRouterRoutingPolicy::default()
        };
        let refined = authorized
            .refine_within_authorization(route_for_openrouter_policy(Some(&pinned), None))
            .expect("both OpenRouter routes gate on llm.openrouter");
        assert_eq!(refined.id, "route.cerebras_via_openrouter");
        assert_eq!(refined.provider_id, authorized.provider_id());

        // Direct Cerebras is a different authorized egress route, so it can never
        // be reached by refining an OpenRouter authorization.
        assert!(
            authorized
                .refine_within_authorization(route_by_id("route.cerebras_direct"))
                .is_err()
        );
    }

    #[test]
    fn provider_deferred_rejection_carries_no_endpoint_or_credential() {
        // No LLM descriptor is deferred today, so exercise the gate through a
        // descriptor that is (the same helper every route row uses).
        let err = crate::provider_registry::ensure_provider_id_start_enabled("asr.local_whisper")
            .expect_err("asr.local_whisper is deferred for the MVP");
        let rendered = err.to_string();
        assert!(rendered.contains("asr.local_whisper"));
        assert!(
            !rendered.contains("http") && !rendered.contains("sk-"),
            "ADR-0033 errors carry registry identity only, got: {rendered}"
        );
    }

    // ----- terminal status --------------------------------------------------

    #[test]
    fn terminal_status_from_finish_reason_table() {
        for reason in ["stop", "end_turn", "stop_sequence", "STOP", "  stop  ", ""] {
            assert_eq!(
                terminal_status_from_finish_reason(Some(reason)),
                TerminalStatus::Completed,
                "{reason:?}"
            );
        }
        assert_eq!(
            terminal_status_from_finish_reason(None),
            TerminalStatus::Completed
        );
        for reason in ["length", "max_tokens", "model_length", "LENGTH"] {
            assert_eq!(
                terminal_status_from_finish_reason(Some(reason)),
                TerminalStatus::Truncated,
                "{reason:?}"
            );
        }
        for reason in ["content_filter", "guardrail_intervened", "refusal"] {
            assert_eq!(
                terminal_status_from_finish_reason(Some(reason)),
                TerminalStatus::Refused,
                "{reason:?}"
            );
        }
        // No request in this repo sends tools, so a tool call is a KNOWN-bad,
        // unusable completion.
        for reason in ["tool_calls", "function_call"] {
            assert_eq!(
                terminal_status_from_finish_reason(Some(reason)),
                TerminalStatus::Failed,
                "{reason:?}"
            );
        }
        // An UNRECOGNIZED non-empty token — a vendor-specific success marker
        // this table has not enumerated (Together AI's "eos"), or a future SDK
        // addition — is not evidence of an unusable completion: it defers to
        // the runtime validator instead, mirroring `bedrock::map_stop_reason`'s
        // fallback to a clean stop for the same reason.
        for reason in ["COMPLETE", "eos", "wat"] {
            assert_eq!(
                terminal_status_from_finish_reason(Some(reason)),
                TerminalStatus::Completed,
                "{reason:?}"
            );
        }
    }

    // ----- retry classification ---------------------------------------------

    #[test]
    fn the_uncertain_class_is_never_auto_retried_and_is_not_named_outcome_uncertain() {
        assert!(!RetryClass::ExternalEffectUnknown.auto_retry_permitted());
        assert!(!RetryClass::PermanentRejection.auto_retry_permitted());
        assert!(!RetryClass::UnusableCompletion.auto_retry_permitted());
        assert!(RetryClass::TransientAvailability.auto_retry_permitted());
        // audio-graph-5e41 owns `OutcomeUncertain` for a canonical-write state.
        let json = serde_json::to_string(&RetryClass::ExternalEffectUnknown).expect("serialize");
        assert_eq!(json, "\"external_effect_unknown\"");
        assert!(!json.contains("outcome_uncertain"));
    }

    #[test]
    fn transport_lost_is_the_uncertain_class_and_truncated_is_a_known_effect() {
        assert_eq!(
            retry_class_for_terminal_status(TerminalStatus::TransportLost),
            Some(RetryClass::ExternalEffectUnknown)
        );
        for status in [
            TerminalStatus::Truncated,
            TerminalStatus::Refused,
            TerminalStatus::Failed,
        ] {
            assert_eq!(
                retry_class_for_terminal_status(status),
                Some(RetryClass::UnusableCompletion),
                "{status:?} was produced and billed"
            );
        }
        assert_eq!(
            retry_class_for_terminal_status(TerminalStatus::Completed),
            None,
            "a completed attempt has nothing to reissue"
        );
    }

    // ----- redaction --------------------------------------------------------

    #[test]
    fn route_metadata_sanitizer_drops_credentials_and_overlong_prose() {
        assert_eq!(
            sanitize_route_metadata(" Cerebras "),
            Some("Cerebras".to_string())
        );
        assert_eq!(
            sanitize_route_metadata("anthropic/claude-sonnet-4.5"),
            Some("anthropic/claude-sonnet-4.5".to_string())
        );
        assert_eq!(sanitize_route_metadata("Together sk-live-abc"), None);
        assert_eq!(sanitize_route_metadata("Bearer abc"), None);
        assert_eq!(
            sanitize_route_metadata(&"x".repeat(MAX_METADATA_LEN + 1)),
            None
        );
        assert_eq!(sanitize_route_metadata("   "), None);
        assert_eq!(sanitize_route_metadata("<>{}"), None);
    }

    #[test]
    fn route_record_serializes_content_free_fields_only() {
        let record = RouteRecord {
            route_id: "route.cerebras_via_openrouter".to_string(),
            provider_id: "llm.openrouter".to_string(),
            wire_skin: WireSkin::ChatCompletions.as_str().to_string(),
            terminal_status: TerminalStatus::Truncated,
            retry_class: Some(RetryClass::UnusableCompletion),
            constrained_decoding: ConstrainedDecodingGrade::GuaranteedConstrained,
            completion_budget_clamped: true,
            served_upstream_provider: Some("Cerebras".to_string()),
        };
        let json = serde_json::to_string(&record).expect("serialize");
        for field in [
            "route_id",
            "provider_id",
            "wire_skin",
            "terminal_status",
            "retry_class",
            "constrained_decoding",
            "completion_budget_clamped",
            "served_upstream_provider",
        ] {
            assert!(json.contains(field), "missing {field} in {json}");
        }
        // The struct has no field capable of carrying prompt, reply, or key text;
        // this pins the field set so a future addition has to face this test.
        let parsed: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(
            parsed.as_object().expect("object").len(),
            8,
            "RouteRecord grew a field; prove it cannot carry content: {json}"
        );
    }
}
