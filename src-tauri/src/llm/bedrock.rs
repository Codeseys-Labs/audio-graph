//! AWS Bedrock `ConverseStream` streaming-chat adapter.
//!
//! This is Bedrock's first working inference path. The provider was previously
//! a settings/readiness-probe-only stub with no chat route at all (see the
//! discover note for `audio-graph-2f4a`). The adapter drives the
//! `aws_sdk_bedrockruntime` `ConverseStream` event stream into the
//! provider-neutral [`TokenDelta`] contract that the rest of the streaming
//! pipeline already speaks.
//!
//! # Shape
//!
//! Mirrors the AWS Transcribe streaming adapter (`asr::aws_transcribe`): build
//! an [`aws_config::SdkConfig`] via [`crate::aws_util::build_aws_sdk_config`]
//! (so STS/profile session-token refresh is inherited for free), then
//! `aws_sdk_bedrockruntime::Client::new(&sdk_config)`, then `.converse_stream()`.
//!
//! # Testability
//!
//! The event → [`TokenDelta`] mapping lives behind a small intermediate enum
//! ([`BedrockStreamEvent`]) plus pure helper functions so unit tests can feed
//! synthetic `ContentBlockDelta` / `MessageStop` / `Metadata` / error events and
//! assert the resulting `TokenDelta` sequence and `finish_reason` mapping with
//! NO live AWS credentials. The live SDK path translates concrete SDK event
//! types into [`BedrockStreamEvent`] (see [`bedrock_event_from_sdk`]); the
//! drainer ([`drive_bedrock_events`]) is what the tests exercise directly.
//!
//! # Secrets
//!
//! Errors are routed through [`crate::aws_util::classify_aws_error`] and never
//! interpolate credential material. We follow the same redaction discipline as
//! the rest of the AWS code: no access keys, secrets, or session tokens are
//! ever logged or surfaced in error strings.

use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::llm::engine::ChatMessage;
use crate::llm::stream_contract::{StreamContextMetadata, StreamTerminalEvent, StreamUsage};
use crate::llm::streaming::TokenDelta;
use crate::settings::AwsCredentialSource;

/// Provider-neutral identifier used by [`crate::aws_util::classify_aws_error`]
/// hints and log lines. Never carries secret material.
const PROVIDER_NAME: &str = "aws_bedrock";

/// Provider tag used in content-egress policy error strings. Matches the
/// `llm.aws_bedrock` tag the streaming router already passes so blocked-mode
/// errors are uniform regardless of which layer caught the egress attempt.
const POLICY_PROVIDER: &str = "llm.aws_bedrock";

/// Default `privacy_mode` label baked into a fail-closed
/// [`crate::asr::ProviderContentEgressPolicy`] when no policy was threaded in.
const EXPLICIT_POLICY_REQUIRED: &str = "explicit_policy_required";

/// One decoded event from the Bedrock `ConverseStream`, decoupled from the
/// concrete `aws_sdk_bedrockruntime` event types.
///
/// Keeping this intermediate enum SDK-free lets the drainer and its tests stay
/// ungated: tests build `BedrockStreamEvent` values directly and never need to
/// construct SDK event structs (which are awkward to fabricate and pull in the
/// whole signing/transport stack).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BedrockStreamEvent {
    /// A `contentBlockDelta` carrying a chunk of generated text.
    TextDelta(String),
    /// `messageStop` with the raw Bedrock `stopReason` (e.g. `"end_turn"`,
    /// `"max_tokens"`). Mapped to an OpenAI-style `finish_reason` via
    /// [`map_stop_reason`].
    MessageStop { stop_reason: String },
    /// `metadata` token-usage block.
    Usage(StreamUsage),
}

/// Map a Bedrock `stopReason` to the OpenAI-style `finish_reason` the rest of
/// the streaming pipeline (and the frontend) already understands.
///
/// Bedrock reasons (`StopReason`): `end_turn`, `tool_use`, `max_tokens`,
/// `stop_sequence`, `guardrail_intervened`, `content_filtered`. Anything else
/// (including future SDK additions) falls back to `"stop"` so the stream still
/// terminates cleanly rather than surfacing a raw SDK token.
pub fn map_stop_reason(stop_reason: &str) -> String {
    match stop_reason.trim().to_ascii_lowercase().as_str() {
        "max_tokens" => "length",
        "tool_use" => "tool_calls",
        "content_filtered" | "guardrail_intervened" => "content_filter",
        // `end_turn`, `stop_sequence`, and anything unknown -> "stop".
        _ => "stop",
    }
    .to_string()
}

/// Build a [`StreamUsage`] from Bedrock metadata token counts.
///
/// Bedrock reports `input_tokens` / `output_tokens` / `total_tokens` as signed
/// integers; negatives are nonsensical here so we clamp to zero before the
/// `u32` cast (mirrors the defensive `usage_total_to_u32` style used elsewhere).
pub fn stream_usage_from_counts(input: i32, output: i32, total: i32) -> StreamUsage {
    StreamUsage {
        prompt_tokens: Some(input.max(0) as u32),
        completion_tokens: Some(output.max(0) as u32),
        total_tokens: Some(total.max(0) as u32),
    }
}

/// Drive a stream of decoded [`BedrockStreamEvent`]s into the [`TokenDelta`]
/// channel, honoring cancellation between events.
///
/// Contract (identical to the local-llama and SSE adapters): emits zero or more
/// [`TokenDelta::Delta`] frames followed by EXACTLY ONE terminal frame
/// (`Done` / `Error` / `Cancelled`). The terminal frame is built through
/// [`StreamTerminalEvent`] so Done/Error/Cancelled semantics stay shared.
///
/// `events` yields `Ok(event)` for a decoded stream event, or `Err(message)`
/// for a transport/decode error (already classified + redacted by the caller).
/// `None` means the stream closed without a `messageStop`, which we treat as a
/// clean end (`finish_reason = "stop"`) using whatever text was accumulated.
pub async fn drive_bedrock_events<S>(
    mut events: S,
    tx: mpsc::Sender<TokenDelta>,
    cancel: CancellationToken,
    metadata: StreamContextMetadata,
) where
    S: BedrockEventSource,
{
    if cancel.is_cancelled() {
        send_terminal(&tx, StreamTerminalEvent::cancelled(String::new(), metadata)).await;
        return;
    }

    let mut full_text = String::new();
    let mut usage: Option<StreamUsage> = None;

    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                // Drop the stream (the `events` source is moved into this fn and
                // dropped on return) and emit the single Cancelled terminator.
                send_terminal(
                    &tx,
                    StreamTerminalEvent::cancelled(std::mem::take(&mut full_text), metadata),
                )
                .await;
                return;
            }
            next = events.next() => next,
        };

        match next {
            Some(Ok(BedrockStreamEvent::TextDelta(content))) => {
                if content.is_empty() {
                    continue;
                }
                full_text.push_str(&content);
                if tx
                    .send(TokenDelta::Delta {
                        content,
                        finish_reason: None,
                    })
                    .await
                    .is_err()
                {
                    // Receiver dropped; abandon without a terminal frame (no one
                    // is listening).
                    return;
                }
            }
            Some(Ok(BedrockStreamEvent::Usage(reported))) => {
                // Keep the last usage block that actually reports a total, so a
                // trailing empty metadata frame can't clobber a real count.
                if reported.has_reported_total() || usage.is_none() {
                    usage = Some(reported);
                }
            }
            Some(Ok(BedrockStreamEvent::MessageStop { stop_reason })) => {
                send_terminal(
                    &tx,
                    StreamTerminalEvent::done(
                        std::mem::take(&mut full_text),
                        usage.take(),
                        map_stop_reason(&stop_reason),
                        metadata,
                    ),
                )
                .await;
                return;
            }
            Some(Err(message)) => {
                send_terminal(
                    &tx,
                    StreamTerminalEvent::error(message, std::mem::take(&mut full_text), metadata),
                )
                .await;
                return;
            }
            None => {
                // Stream closed without an explicit messageStop. Emit a Done with
                // whatever we accumulated (some providers/edge cases do this).
                send_terminal(
                    &tx,
                    StreamTerminalEvent::done(
                        std::mem::take(&mut full_text),
                        usage.take(),
                        "stop".to_string(),
                        metadata,
                    ),
                )
                .await;
                return;
            }
        }
    }
}

async fn send_terminal(tx: &mpsc::Sender<TokenDelta>, event: StreamTerminalEvent) {
    let _ = tx.send(TokenDelta::from_terminal_event(event)).await;
}

/// Async source of decoded Bedrock stream events.
///
/// Abstracted as a trait so [`drive_bedrock_events`] can be unit-tested with a
/// synthetic, in-memory source (no SDK, no AWS creds) while the live adapter
/// wraps the real `aws_sdk_bedrockruntime` event receiver.
pub trait BedrockEventSource: Send {
    /// Yield the next event. `Ok(event)` for a decoded event, `Err(message)`
    /// for an already-redacted/classified terminal error, `None` for clean EOF.
    fn next(
        &mut self,
    ) -> impl std::future::Future<Output = Option<Result<BedrockStreamEvent, String>>> + Send;
}

/// In-memory event source over a pre-built `Vec`, used by unit tests.
pub struct VecBedrockEventSource {
    events: std::collections::VecDeque<Result<BedrockStreamEvent, String>>,
}

impl VecBedrockEventSource {
    pub fn new(events: Vec<Result<BedrockStreamEvent, String>>) -> Self {
        Self {
            events: events.into(),
        }
    }
}

impl BedrockEventSource for VecBedrockEventSource {
    async fn next(&mut self) -> Option<Result<BedrockStreamEvent, String>> {
        self.events.pop_front()
    }
}

// ---------------------------------------------------------------------------
// Live SDK adapter
// ---------------------------------------------------------------------------

/// Live AWS Bedrock `ConverseStream` adapter.
///
/// Built on demand from the `AwsBedrock` settings — no persistent backend
/// handle is needed (`StreamBackendHandles::has_handle_for` returning `false`
/// for `AwsBedrock` is intentional).
pub struct BedrockConverseStreamAdapter {
    region: String,
    model_id: String,
    credential_source: AwsCredentialSource,
    history: Vec<ChatMessage>,
    graph_context: String,
    max_tokens: u32,
    temperature: f32,
    /// Defense-in-depth content-egress guard. The streaming router
    /// (`stream_chat_with_request`) already applies the same gate before it
    /// constructs this adapter; carrying the policy *inside* the adapter gives
    /// a SECOND layer so a direct `BedrockConverseStreamAdapter::new(..).run()`
    /// caller that bypasses the router still cannot ship prompt/graph content to
    /// AWS in a blocked privacy mode. Defaults to `block` (fail closed) so any
    /// construction path that forgets to thread the policy refuses egress.
    content_egress_policy: crate::asr::ProviderContentEgressPolicy,
}

impl BedrockConverseStreamAdapter {
    pub fn new(
        region: String,
        model_id: String,
        credential_source: AwsCredentialSource,
        history: Vec<ChatMessage>,
        graph_context: String,
        max_tokens: u32,
        temperature: f32,
    ) -> Self {
        Self {
            region,
            model_id,
            credential_source,
            history,
            graph_context,
            max_tokens,
            temperature,
            content_egress_policy: crate::asr::ProviderContentEgressPolicy::block(
                EXPLICIT_POLICY_REQUIRED,
            ),
        }
    }

    /// Thread the runtime content-egress policy into the adapter (builder).
    ///
    /// Callers that route a real cloud request must pass the policy derived from
    /// the active `PrivacyMode` so the adapter can refuse content egress in a
    /// blocked mode independently of the router-level check.
    pub fn with_content_egress_policy(
        mut self,
        policy: crate::asr::ProviderContentEgressPolicy,
    ) -> Self {
        self.content_egress_policy = policy;
        self
    }

    /// Run the streaming request to completion, emitting [`TokenDelta`] frames
    /// on `tx` and honoring `cancel` between events. Always emits exactly one
    /// terminal frame.
    pub async fn run(
        self,
        tx: mpsc::Sender<TokenDelta>,
        cancel: CancellationToken,
        metadata: StreamContextMetadata,
    ) {
        use aws_sdk_bedrockruntime::types::InferenceConfiguration;

        if cancel.is_cancelled() {
            send_terminal(&tx, StreamTerminalEvent::cancelled(String::new(), metadata)).await;
            return;
        }

        // Defense-in-depth content-egress gate (second layer). The streaming
        // router already applied this same check before constructing the
        // adapter; re-checking here means a direct caller that bypasses the
        // router still cannot ship prompt/graph content to AWS in a blocked
        // privacy mode. We refuse BEFORE building the SDK config/client so no
        // request — not even a handshake — leaves the machine. The error string
        // is the policy helper's redacted message (no prompt/graph/secret text).
        if let Err(message) = self
            .content_egress_policy
            .check_prompt(POLICY_PROVIDER)
            .and_then(|()| self.content_egress_policy.check_json(POLICY_PROVIDER))
        {
            send_terminal(
                &tx,
                StreamTerminalEvent::error(message, String::new(), metadata),
            )
            .await;
            return;
        }

        let region = self.region.clone();

        // Build SDK config + client — the exact two-line pattern from
        // asr::aws_transcribe. Inherits STS/profile refresh for free.
        let sdk_config =
            match crate::aws_util::build_aws_sdk_config(&self.region, self.credential_source).await
            {
                Ok(cfg) => cfg,
                Err(message) => {
                    send_terminal(
                        &tx,
                        StreamTerminalEvent::error(
                            classify_bedrock_error(&message, Some(&region)),
                            String::new(),
                            metadata,
                        ),
                    )
                    .await;
                    return;
                }
            };
        let client = aws_sdk_bedrockruntime::Client::new(&sdk_config);

        // Split the synthesized system prompt out of the message history into
        // Bedrock's dedicated `system` slot; map the rest to Converse messages.
        // The system slot carries the byte-stable graph-context prefix plus a
        // `cachePoint` breakpoint for cache-capable models (ADR-0025 §2d / seed
        // audio-graph-8925) so subsequent turns read the prefix from cache.
        let system_prompt = build_system_prompt(&self.graph_context);
        let system_blocks = build_system_content_blocks(system_prompt, &self.model_id);
        let messages = match build_converse_messages(&self.history) {
            Ok(messages) => messages,
            Err(message) => {
                send_terminal(
                    &tx,
                    StreamTerminalEvent::error(message, String::new(), metadata),
                )
                .await;
                return;
            }
        };

        let inference_config = InferenceConfiguration::builder()
            .max_tokens(i32::try_from(self.max_tokens).unwrap_or(i32::MAX))
            .temperature(self.temperature)
            .build();

        let send_future = client
            .converse_stream()
            .model_id(&self.model_id)
            .set_system(Some(system_blocks))
            .set_messages(Some(messages))
            .inference_config(inference_config)
            .send();

        let output = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                send_terminal(&tx, StreamTerminalEvent::cancelled(String::new(), metadata)).await;
                return;
            }
            result = send_future => match result {
                Ok(output) => output,
                Err(err) => {
                    send_terminal(
                        &tx,
                        StreamTerminalEvent::error(
                            classify_bedrock_error(&render_sdk_error(&err), Some(&region)),
                            String::new(),
                            metadata,
                        ),
                    )
                    .await;
                    return;
                }
            }
        };

        // Wrap the live SDK event receiver and drain it through the shared,
        // unit-tested event loop.
        let source = SdkBedrockEventSource {
            stream: output.stream,
            region,
        };
        drive_bedrock_events(source, tx, cancel, metadata).await;
    }
}

/// Wrap the live `aws_sdk_bedrockruntime` event receiver as a
/// [`BedrockEventSource`], translating SDK event types into the SDK-free
/// [`BedrockStreamEvent`] and classifying/redacting transport errors.
struct SdkBedrockEventSource {
    stream: aws_sdk_bedrockruntime::primitives::event_stream::EventReceiver<
        aws_sdk_bedrockruntime::types::ConverseStreamOutput,
        aws_sdk_bedrockruntime::types::error::ConverseStreamOutputError,
    >,
    region: String,
}

impl BedrockEventSource for SdkBedrockEventSource {
    async fn next(&mut self) -> Option<Result<BedrockStreamEvent, String>> {
        loop {
            match self.stream.recv().await {
                Ok(Some(event)) => match bedrock_event_from_sdk(event) {
                    // A None means a non-text structural event (messageStart,
                    // contentBlockStart/stop, empty deltas) we simply skip.
                    Some(decoded) => return Some(Ok(decoded)),
                    None => continue,
                },
                Ok(None) => return None,
                Err(err) => {
                    let raw = render_sdk_error(&err);
                    return Some(Err(classify_bedrock_error(&raw, Some(&self.region))));
                }
            }
        }
    }
}

/// Translate one concrete `aws_sdk_bedrockruntime` `ConverseStreamOutput`
/// variant into the SDK-free [`BedrockStreamEvent`]. Returns `None` for
/// structural events (message/content-block start/stop) that carry no payload
/// the [`TokenDelta`] contract cares about.
fn bedrock_event_from_sdk(
    event: aws_sdk_bedrockruntime::types::ConverseStreamOutput,
) -> Option<BedrockStreamEvent> {
    use aws_sdk_bedrockruntime::types::{ContentBlockDelta, ConverseStreamOutput};

    match event {
        ConverseStreamOutput::ContentBlockDelta(delta_event) => {
            match delta_event.delta {
                Some(ContentBlockDelta::Text(text)) if !text.is_empty() => {
                    Some(BedrockStreamEvent::TextDelta(text))
                }
                // Tool-use / reasoning deltas / empty text: nothing to surface
                // as chat text.
                _ => None,
            }
        }
        ConverseStreamOutput::MessageStop(stop_event) => Some(BedrockStreamEvent::MessageStop {
            stop_reason: stop_event.stop_reason.as_str().to_string(),
        }),
        ConverseStreamOutput::Metadata(metadata_event) => metadata_event.usage.map(|usage| {
            BedrockStreamEvent::Usage(stream_usage_from_counts(
                usage.input_tokens,
                usage.output_tokens,
                usage.total_tokens,
            ))
        }),
        // messageStart, contentBlockStart, contentBlockStop, and any future
        // non-exhaustive variant: structural, no payload to forward.
        _ => None,
    }
}

/// Render an `aws-sdk` error plus its full `std::error::Error` source chain
/// into a single diagnostic string.
///
/// This is the same expansion `aws_smithy_types::error::display::DisplayErrorContext`
/// performs (top-level message + `: <source>` for each cause), reproduced here
/// so the adapter does not need a direct dependency on `aws-smithy-types`. The
/// source chain is where the AWS service code (`ExpiredToken`,
/// `AccessDeniedException`, `DispatchFailure`, ...) lives, which
/// [`crate::aws_util::classify_aws_error`] keys on. SDK error strings carry
/// service codes / transport detail only — never credential material.
fn render_sdk_error<E: std::error::Error>(err: &E) -> String {
    let mut rendered = err.to_string();
    let mut source = err.source();
    while let Some(cause) = source {
        rendered.push_str(": ");
        rendered.push_str(&cause.to_string());
        source = cause.source();
    }
    rendered
}

/// Map an error string into a redacted, user-facing terminal message using the
/// shared AWS error taxonomy. Never interpolates credential material — the
/// `raw` string comes from [`render_sdk_error`] (service codes / transport
/// detail) or from `build_aws_sdk_config` (credential-store status, already
/// redaction-safe).
fn classify_bedrock_error(raw: &str, region: Option<&str>) -> String {
    let classified = crate::aws_util::classify_aws_error(raw, region);
    format!("Bedrock ConverseStream error (provider={PROVIDER_NAME}): {classified:?}")
}

/// Build the graph-context system prompt. Mirrors the system prompt the SSE
/// path synthesizes (`streaming::build_messages`) so Bedrock replies are
/// comparable to the other providers'.
fn build_system_prompt(graph_context: &str) -> String {
    format!(
        "You are a knowledge graph assistant analyzing a live audio conversation. \
         Here is the current knowledge graph context:\n\n{}\n\n\
         Answer the user's question about the conversation, people, topics, or relationships discussed.",
        graph_context
    )
}

/// Whether a Bedrock model supports the Converse `cachePoint` prompt-caching
/// block (ADR-0025 §2d / seed audio-graph-8925).
///
/// The stable-prefix caching seed (`d77e`, PR #77) shipped the OpenRouter
/// `cache_control:ephemeral` passthrough; this is the Bedrock-native analog.
/// Bedrock's `cachePoint` is a **best-effort per-model** capability, and sending
/// the block to a model that does not support it is a hard `ValidationException`
/// (not a silent no-op — that soft-miss behavior applies only to a too-small
/// prefix). So this gate is deliberately a **conservative allowlist**: a model
/// not matched here gets no `cachePoint` block and keeps the current uncached
/// behavior, which is the "providers without cache support are unaffected"
/// contract from the seed.
///
/// The allowlist is the set of Converse-cache-capable models AWS documents
/// (Amazon Bedrock user guide, "Prompt caching for faster model inference"):
/// Anthropic Claude 3.5 Haiku, 3.5 Sonnet **v2** (`20241022`), 3.7 Sonnet, the
/// Claude 4 family (Opus / Sonnet / Haiku, incl. the 4.5 / 4.6 point releases),
/// and Amazon Nova Micro / Lite / Pro. Matching is case-insensitive substring so
/// a cross-region inference-profile id (e.g. `us.anthropic.claude-3-7-sonnet-…`)
/// still resolves. The original Claude 3.5 Sonnet **v1** (`20240620`) and the
/// first-generation Claude 3 models are intentionally NOT matched — they are not
/// on the supported list, so a bare `claude-3-5-sonnet` (no date) stays uncached
/// by design rather than risking a validation error.
pub fn model_supports_cache_point(model_id: &str) -> bool {
    // AWS-documented Converse prompt-caching allowlist. Each entry is matched as
    // a lowercase substring against the (possibly region-prefixed) model id.
    const CACHE_CAPABLE_MODEL_MARKERS: &[&str] = &[
        // Amazon Nova (explicit prompt caching).
        "nova-micro",
        "nova-lite",
        "nova-pro",
        // Anthropic Claude.
        "claude-3-5-haiku",
        "claude-3-7-sonnet",
        // 3.5 Sonnet v2 ONLY (the 20240620 v1 build is unsupported).
        "claude-3-5-sonnet-20241022",
        // Claude 4 family — the bare markers also cover the 4.5 / 4.6 point
        // releases (e.g. "claude-opus-4-5", "claude-sonnet-4-6").
        "claude-opus-4",
        "claude-sonnet-4",
        "claude-haiku-4",
    ];
    let id = model_id.to_ascii_lowercase();
    CACHE_CAPABLE_MODEL_MARKERS
        .iter()
        .any(|marker| id.contains(marker))
}

/// Build the Bedrock Converse `system` slot, appending a `cachePoint` block after
/// the stable-prefix system prompt for cache-capable models (ADR-0025 §2d / seed
/// audio-graph-8925).
///
/// The Converse cache order is `tools → system → messages`; the synthesized
/// graph-context system prompt is the largest byte-stable block reused across a
/// session's turns, so the breakpoint rides at the end of the `system` slot —
/// the direct analog of the OpenRouter path placing the `cache_control`
/// breakpoint on the last stable-prefix message. Dynamic per-turn content (the
/// user's question + conversation history) lives in `messages`, after the
/// breakpoint, so it never busts the cached prefix.
///
/// For a model NOT on the [`model_supports_cache_point`] allowlist the slot is
/// the single `Text` block exactly as before (no behavior change). The
/// `cachePoint` uses `type: default` with no explicit TTL, so the model's
/// default (5-minute) caching window applies — matching the ephemeral semantics
/// the OpenRouter passthrough uses.
fn build_system_content_blocks(
    system_prompt: String,
    model_id: &str,
) -> Vec<aws_sdk_bedrockruntime::types::SystemContentBlock> {
    use aws_sdk_bedrockruntime::types::{CachePointBlock, CachePointType, SystemContentBlock};

    let mut blocks = vec![SystemContentBlock::Text(system_prompt)];
    if model_supports_cache_point(model_id) {
        // `build()` only fails when the required `type` field is unset, which we
        // always set. On the impossible error path we degrade gracefully: skip
        // the breakpoint (no caching) rather than fail the whole request.
        if let Ok(cache_point) = CachePointBlock::builder()
            .r#type(CachePointType::Default)
            .build()
        {
            blocks.push(SystemContentBlock::CachePoint(cache_point));
        }
    }
    blocks
}

/// Map the chat history into Bedrock Converse `Message`s. The synthesized
/// graph-context system prompt is carried separately in Bedrock's dedicated
/// `system` slot, so this only maps the user/assistant turns. A stray `system`
/// role (which shouldn't appear here) is mapped to `user` defensively so we
/// never silently drop a turn. Empty history is rejected because Bedrock
/// requires at least one message.
fn build_converse_messages(
    history: &[ChatMessage],
) -> Result<Vec<aws_sdk_bedrockruntime::types::Message>, String> {
    use aws_sdk_bedrockruntime::types::{ContentBlock, ConversationRole, Message};

    let mut messages = Vec::with_capacity(history.len());
    for msg in history {
        // Bedrock only accepts `user` / `assistant` conversation roles; the
        // synthesized graph-context system prompt rides in the `system` slot.
        let role = match msg.role.as_str() {
            "assistant" => ConversationRole::Assistant,
            // `user`, `system` (shouldn't appear here), and anything else map to
            // user so we never drop a turn.
            _ => ConversationRole::User,
        };
        let built = Message::builder()
            .role(role)
            .content(ContentBlock::Text(msg.content.clone()))
            .build()
            .map_err(|e| {
                format!("Failed to build Bedrock Converse message (provider={PROVIDER_NAME}): {e}")
            })?;
        messages.push(built);
    }

    if messages.is_empty() {
        return Err(format!(
            "Bedrock ConverseStream requires at least one user message (provider={PROVIDER_NAME})"
        ));
    }
    Ok(messages)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::LlmProvider;

    fn test_metadata() -> StreamContextMetadata {
        StreamContextMetadata::from_provider(&LlmProvider::AwsBedrock {
            region: "us-east-1".to_string(),
            model_id: "anthropic.claude-3-5-sonnet".to_string(),
            credential_source: AwsCredentialSource::DefaultChain,
        })
    }

    async fn drain(events: Vec<Result<BedrockStreamEvent, String>>) -> Vec<TokenDelta> {
        let (tx, mut rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let source = VecBedrockEventSource::new(events);
        tokio::spawn(drive_bedrock_events(source, tx, cancel, test_metadata()));
        let mut frames = Vec::new();
        while let Some(frame) = rx.recv().await {
            frames.push(frame);
        }
        frames
    }

    #[test]
    fn stop_reason_maps_to_openai_finish_reason() {
        assert_eq!(map_stop_reason("end_turn"), "stop");
        assert_eq!(map_stop_reason("stop_sequence"), "stop");
        assert_eq!(map_stop_reason("max_tokens"), "length");
        assert_eq!(map_stop_reason("tool_use"), "tool_calls");
        assert_eq!(map_stop_reason("content_filtered"), "content_filter");
        assert_eq!(map_stop_reason("guardrail_intervened"), "content_filter");
        // Case-insensitive + unknown fallback.
        assert_eq!(map_stop_reason("END_TURN"), "stop");
        assert_eq!(map_stop_reason("something_new_from_aws"), "stop");
    }

    #[test]
    fn usage_counts_clamp_negatives() {
        let usage = stream_usage_from_counts(7, 4, 11);
        assert_eq!(usage.prompt_tokens, Some(7));
        assert_eq!(usage.completion_tokens, Some(4));
        assert_eq!(usage.total_tokens, Some(11));

        // Defensive: negative counts (shouldn't happen) clamp to 0 rather than
        // wrapping into a huge u32.
        let clamped = stream_usage_from_counts(-1, -5, -3);
        assert_eq!(clamped.prompt_tokens, Some(0));
        assert_eq!(clamped.completion_tokens, Some(0));
        assert_eq!(clamped.total_tokens, Some(0));
    }

    #[tokio::test]
    async fn drains_text_deltas_then_done_with_usage_and_finish_reason() {
        let frames = drain(vec![
            Ok(BedrockStreamEvent::TextDelta("Hel".to_string())),
            Ok(BedrockStreamEvent::TextDelta("lo ".to_string())),
            Ok(BedrockStreamEvent::TextDelta("world".to_string())),
            Ok(BedrockStreamEvent::Usage(stream_usage_from_counts(3, 2, 5))),
            Ok(BedrockStreamEvent::MessageStop {
                stop_reason: "end_turn".to_string(),
            }),
        ])
        .await;

        let mut deltas = Vec::new();
        let mut done = None;
        for frame in frames {
            match frame {
                TokenDelta::Delta {
                    content,
                    finish_reason,
                } => {
                    assert_eq!(finish_reason, None);
                    deltas.push(content);
                }
                TokenDelta::Done {
                    full_text,
                    usage,
                    finish_reason,
                } => done = Some((full_text, usage, finish_reason)),
                other => panic!("unexpected frame: {other:?}"),
            }
        }

        assert_eq!(deltas, vec!["Hel", "lo ", "world"]);
        let (full_text, usage, finish_reason) = done.expect("done terminal frame");
        assert_eq!(full_text, "Hello world");
        assert_eq!(finish_reason, "stop");
        let usage = usage.expect("usage on done");
        assert_eq!(usage.total_tokens, Some(5));
        assert_eq!(usage.prompt_tokens, Some(3));
        assert_eq!(usage.completion_tokens, Some(2));
    }

    #[tokio::test]
    async fn max_tokens_stop_reason_maps_to_length() {
        let frames = drain(vec![
            Ok(BedrockStreamEvent::TextDelta("truncated".to_string())),
            Ok(BedrockStreamEvent::MessageStop {
                stop_reason: "max_tokens".to_string(),
            }),
        ])
        .await;

        match frames.last().expect("terminal frame") {
            TokenDelta::Done { finish_reason, .. } => {
                assert_eq!(finish_reason, "length");
            }
            other => panic!("expected Done with length, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn transport_error_maps_to_terminal_error_with_partial_text() {
        // The event source (SdkBedrockEventSource) classifies + redacts the raw
        // SDK error into the wrapped message BEFORE it reaches the drainer; the
        // drainer forwards that message verbatim on the Error terminal frame.
        let classified =
            classify_bedrock_error("dispatch failure: io error: connection reset", None);
        let frames = drain(vec![
            Ok(BedrockStreamEvent::TextDelta("partial".to_string())),
            Err(classified.clone()),
        ])
        .await;

        // First the partial delta, then exactly one Error terminal frame.
        assert!(matches!(frames.first(), Some(TokenDelta::Delta { .. })));
        match frames.last().expect("terminal frame") {
            TokenDelta::Error { message, full_text } => {
                assert_eq!(full_text, "partial");
                assert_eq!(
                    message, &classified,
                    "the drainer must forward the already-classified error verbatim"
                );
                assert!(message.contains("Bedrock ConverseStream error"));
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }
        assert_eq!(frames.len(), 2, "exactly one delta + one terminal");
    }

    #[test]
    fn classify_bedrock_error_wraps_with_provider_context_and_taxonomy() {
        // Network/transport failure -> NetworkUnreachable, wrapped with the
        // Bedrock provider context. The raw SDK string is never echoed back
        // (we surface the classified taxonomy, not the free-form message).
        let network = classify_bedrock_error("dispatch failure: io error: connection reset", None);
        assert!(network.contains("Bedrock ConverseStream error"));
        assert!(network.contains("provider=aws_bedrock"));
        assert!(
            network.contains("NetworkUnreachable"),
            "transport failure must classify as NetworkUnreachable: {network}"
        );

        // Expired STS session token -> ExpiredToken.
        let expired = classify_bedrock_error(
            "service error: ExpiredTokenException: The security token included in the request is expired",
            Some("us-east-1"),
        );
        assert!(expired.contains("ExpiredToken"), "got: {expired}");
    }

    #[test]
    fn render_sdk_error_walks_source_chain() {
        use std::fmt;

        #[derive(Debug)]
        struct Inner;
        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "InvalidAccessKeyId: bad key")
            }
        }
        impl std::error::Error for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);
        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "service error")
            }
        }
        impl std::error::Error for Outer {
            fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
                Some(&self.0)
            }
        }

        let rendered = render_sdk_error(&Outer(Inner));
        assert_eq!(rendered, "service error: InvalidAccessKeyId: bad key");
        // And feeding the expanded chain into the classifier resolves the code
        // that lives in the source, not the top-level message.
        assert!(classify_bedrock_error(&rendered, None).contains("InvalidAccessKey"));
    }

    #[tokio::test]
    async fn stream_end_without_message_stop_emits_done() {
        let frames = drain(vec![Ok(BedrockStreamEvent::TextDelta("hi".to_string()))]).await;
        match frames.last().expect("terminal frame") {
            TokenDelta::Done {
                full_text,
                finish_reason,
                ..
            } => {
                assert_eq!(full_text, "hi");
                assert_eq!(finish_reason, "stop");
            }
            other => panic!("expected Done on clean EOF, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cancel_before_first_event_emits_cancelled() {
        let (tx, mut rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let source = VecBedrockEventSource::new(vec![Ok(BedrockStreamEvent::TextDelta(
            "should-not-arrive".to_string(),
        ))]);
        drive_bedrock_events(source, tx, cancel, test_metadata()).await;

        match rx.recv().await.expect("terminal frame") {
            TokenDelta::Cancelled { full_text } => assert!(full_text.is_empty()),
            other => panic!("expected Cancelled, got {other:?}"),
        }
        assert!(
            rx.recv().await.is_none(),
            "cancelled stream must end after exactly one terminal frame"
        );
    }

    #[tokio::test]
    async fn empty_text_deltas_are_skipped() {
        let frames = drain(vec![
            Ok(BedrockStreamEvent::TextDelta(String::new())),
            Ok(BedrockStreamEvent::TextDelta("real".to_string())),
            Ok(BedrockStreamEvent::MessageStop {
                stop_reason: "end_turn".to_string(),
            }),
        ])
        .await;

        let deltas: Vec<_> = frames
            .iter()
            .filter_map(|f| match f {
                TokenDelta::Delta { content, .. } => Some(content.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(deltas, vec!["real"], "empty deltas must not be forwarded");
    }

    #[tokio::test]
    async fn trailing_empty_usage_does_not_clobber_real_usage() {
        let frames = drain(vec![
            Ok(BedrockStreamEvent::Usage(stream_usage_from_counts(
                7, 4, 11,
            ))),
            Ok(BedrockStreamEvent::Usage(StreamUsage::default())),
            Ok(BedrockStreamEvent::MessageStop {
                stop_reason: "end_turn".to_string(),
            }),
        ])
        .await;

        match frames.last().expect("terminal frame") {
            TokenDelta::Done { usage, .. } => {
                let usage = usage.as_ref().expect("usage on done");
                assert_eq!(
                    usage.total_tokens,
                    Some(11),
                    "real usage must survive a trailing empty metadata frame"
                );
            }
            other => panic!("expected Done, got {other:?}"),
        }
    }

    #[test]
    fn build_converse_messages_rejects_empty_history() {
        let err = build_converse_messages(&[]).expect_err("empty history must be rejected");
        assert!(err.contains("at least one user message"), "got: {err}");
    }

    #[test]
    fn build_converse_messages_maps_roles() {
        let messages = build_converse_messages(&[
            ChatMessage {
                role: "user".to_string(),
                content: "hi".to_string(),
            },
            ChatMessage {
                role: "assistant".to_string(),
                content: "hello".to_string(),
            },
        ])
        .expect("messages build");
        assert_eq!(messages.len(), 2);
    }

    #[test]
    fn system_prompt_includes_graph_context() {
        let prompt = build_system_prompt("GRAPH_CONTEXT_MARKER");
        assert!(prompt.contains("GRAPH_CONTEXT_MARKER"));
        assert!(prompt.contains("knowledge graph assistant"));
    }

    // -----------------------------------------------------------------------
    // Converse `cachePoint` stable-prefix caching (ADR-0025 §2d / seed
    // audio-graph-8925). The system slot carries the byte-stable graph-context
    // prefix; a `cachePoint` breakpoint rides its tail for cache-capable models
    // so subsequent turns read the prefix from cache. Providers/models without
    // cache support must be byte-for-byte unaffected.
    // -----------------------------------------------------------------------

    #[test]
    fn cache_capable_models_are_allowlisted() {
        // Anthropic Claude models AWS documents as Converse-cache-capable,
        // including bare and region-prefixed inference-profile ids.
        for id in [
            "anthropic.claude-3-5-haiku-20241022-v1:0",
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
            "us.anthropic.claude-3-7-sonnet-20250219-v1:0",
            "anthropic.claude-3-5-sonnet-20241022-v2:0",
            "anthropic.claude-opus-4-20250514-v1:0",
            "anthropic.claude-opus-4-5-20251101-v1:0",
            "anthropic.claude-sonnet-4-5-20250929-v1:0",
            "anthropic.claude-sonnet-4-6",
            "anthropic.claude-haiku-4-5-20251001-v1:0",
            // Amazon Nova.
            "amazon.nova-micro-v1:0",
            "us.amazon.nova-lite-v1:0",
            "amazon.nova-pro-v1:0",
            // Case-insensitive matching.
            "ANTHROPIC.CLAUDE-3-7-SONNET-20250219-V1:0",
        ] {
            assert!(
                model_supports_cache_point(id),
                "expected cache support for {id}"
            );
        }
    }

    #[test]
    fn non_cache_capable_models_are_not_allowlisted() {
        // Models NOT on the AWS supported list must stay uncached so we never
        // send a `cachePoint` that would trigger a ValidationException. The
        // 3.5 Sonnet v1 (20240620) build and bare/dateless 3.5 Sonnet are
        // deliberately excluded (only the 20241022 v2 build is supported).
        for id in [
            "anthropic.claude-3-5-sonnet-20240620-v1:0",
            "anthropic.claude-3-5-sonnet", // dateless: ambiguous, treat as unsupported
            "anthropic.claude-3-sonnet-20240229-v1:0",
            "anthropic.claude-3-haiku-20240307-v1:0",
            "anthropic.claude-3-opus-20240229-v1:0",
            "meta.llama3-70b-instruct-v1:0",
            "mistral.mistral-large-2402-v1:0",
            "amazon.titan-text-express-v1",
            "",
        ] {
            assert!(
                !model_supports_cache_point(id),
                "expected NO cache support for {id:?}"
            );
        }
    }

    #[test]
    fn cache_capable_model_appends_default_cache_point_after_text() {
        use aws_sdk_bedrockruntime::types::{CachePointType, SystemContentBlock};

        let blocks = build_system_content_blocks(
            "STABLE_PREFIX".to_string(),
            "anthropic.claude-3-7-sonnet-20250219-v1:0",
        );
        // Text block first (the byte-stable prefix), cachePoint breakpoint last.
        assert_eq!(blocks.len(), 2, "expected text + cachePoint");
        match &blocks[0] {
            SystemContentBlock::Text(text) => assert_eq!(text, "STABLE_PREFIX"),
            other => panic!("expected Text first, got {other:?}"),
        }
        match &blocks[1] {
            SystemContentBlock::CachePoint(cache_point) => {
                assert_eq!(cache_point.r#type(), &CachePointType::Default);
                // No explicit TTL → model default (5-minute) window, matching the
                // OpenRouter ephemeral passthrough.
                assert!(cache_point.ttl().is_none(), "expected default TTL (unset)");
            }
            other => panic!("expected CachePoint second, got {other:?}"),
        }
    }

    #[test]
    fn non_cache_capable_model_emits_text_only() {
        use aws_sdk_bedrockruntime::types::SystemContentBlock;

        let blocks = build_system_content_blocks(
            "STABLE_PREFIX".to_string(),
            "anthropic.claude-3-5-sonnet-20240620-v1:0",
        );
        // Byte-for-byte the legacy shape: exactly one Text block, no cachePoint.
        assert_eq!(
            blocks.len(),
            1,
            "unsupported model must not get a cachePoint"
        );
        match &blocks[0] {
            SystemContentBlock::Text(text) => assert_eq!(text, "STABLE_PREFIX"),
            other => panic!("expected Text only, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------------
    // Defense-in-depth content-egress guard (3b9f). The streaming router gate
    // already blocks before constructing the adapter; these tests prove the
    // provider client refuses content egress on its own even if a caller
    // bypasses that router and drives `run()` directly.
    // -----------------------------------------------------------------------

    /// Build an adapter whose history + graph context carry payload-like
    /// markers so we can assert the policy error never echoes them.
    fn adapter_with_sensitive_payload() -> BedrockConverseStreamAdapter {
        BedrockConverseStreamAdapter::new(
            "us-east-1".to_string(),
            "anthropic.claude-3-5-sonnet".to_string(),
            AwsCredentialSource::DefaultChain,
            vec![ChatMessage {
                role: "user".to_string(),
                content: "PATIENT_SAID_PRIVATE_DIAGNOSIS".to_string(),
            }],
            "SECRET_GRAPH_CONTEXT_NODE".to_string(),
            256,
            0.2,
        )
    }

    #[tokio::test]
    async fn blocked_policy_emits_redacted_terminal_error_without_calling_aws() {
        // A blocked privacy mode must refuse the request at the adapter layer.
        // The guard returns BEFORE building the SDK config/client, so this test
        // never needs AWS credentials or network access.
        let adapter = adapter_with_sensitive_payload().with_content_egress_policy(
            crate::asr::ProviderContentEgressPolicy::block("local_only"),
        );

        let (tx, mut rx) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        adapter.run(tx, cancel, test_metadata()).await;

        let frame = rx.recv().await.expect("terminal frame");
        match &frame {
            TokenDelta::Error { message, full_text } => {
                assert!(full_text.is_empty(), "no partial text before any AWS call");
                assert!(
                    message.contains("Privacy policy blocked"),
                    "expected redacted policy error, got: {message}"
                );
                assert!(
                    message.contains("local_only"),
                    "error should name the blocked privacy mode, got: {message}"
                );
                // The error must NOT leak the prompt, graph context, or any
                // secret/payload material.
                assert!(
                    !message.contains("PATIENT_SAID_PRIVATE_DIAGNOSIS"),
                    "policy error leaked prompt content: {message}"
                );
                assert!(
                    !message.contains("SECRET_GRAPH_CONTEXT_NODE"),
                    "policy error leaked graph context: {message}"
                );
            }
            other => panic!("expected Error terminal, got {other:?}"),
        }

        assert!(
            rx.recv().await.is_none(),
            "blocked stream must end after exactly one terminal frame"
        );
    }

    #[tokio::test]
    async fn default_policy_fails_closed_and_blocks_egress() {
        // An adapter built via `new` without threading a policy must default to
        // fail-closed: it refuses egress rather than silently allowing it.
        let adapter = adapter_with_sensitive_payload();

        let (tx, mut rx) = mpsc::channel(64);
        adapter
            .run(tx, CancellationToken::new(), test_metadata())
            .await;

        match rx.recv().await.expect("terminal frame") {
            TokenDelta::Error { message, .. } => {
                assert!(message.contains("Privacy policy blocked"));
                assert!(
                    message.contains("explicit_policy_required"),
                    "default policy should require an explicit allow, got: {message}"
                );
            }
            other => panic!("expected fail-closed Error terminal, got {other:?}"),
        }
    }

    #[test]
    fn allow_policy_passes_both_prompt_and_json_guards() {
        // The same guard the adapter runs internally: an explicit allow must
        // clear both the prompt and json checks (a ByokCloud-derived policy).
        let policy = crate::asr::ProviderContentEgressPolicy::allow();
        assert!(policy.check_prompt(POLICY_PROVIDER).is_ok());
        assert!(policy.check_json(POLICY_PROVIDER).is_ok());

        // And `with_content_egress_policy` actually overrides the fail-closed
        // default so a routed request can proceed.
        let adapter = adapter_with_sensitive_payload().with_content_egress_policy(policy);
        assert!(
            adapter
                .content_egress_policy
                .check_prompt(POLICY_PROVIDER)
                .is_ok(),
            "builder must override the fail-closed default with the allow policy"
        );
    }
}
