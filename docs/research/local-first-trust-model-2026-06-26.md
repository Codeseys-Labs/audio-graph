# Local-First Trust Model

Date: 2026-06-26
Seed: audio-graph-eeec
Status: architecture recommendation

## Recommendation

AudioGraph should make "where did my audio and text go?" a first-class product
contract, not a privacy-policy footnote.

The current architecture already points in the right direction: capture,
processed PCM, provider sockets, credentials, transcript revisions, projection
patches, notes, graph state, live-assist cards, and future org promotion are
backend-owned. The missing architecture layer is a session-scoped data movement
ledger that records every durable artifact write and every external provider
transfer with a redacted, user-visible summary.

For alpha, AudioGraph does not need to claim SOC 2, GDPR compliance, HIPAA, or
enterprise certification. It does need testable readiness:

- local-first defaults and explicit cloud-provider opt-in;
- saved credentials that never return plaintext to React during readiness/model
  discovery;
- a per-session data route panel showing which data classes stayed local and
  which were sent to which providers;
- retention, export, delete, and recovery semantics for every session artifact;
- redacted audit events for provider calls, export, delete, credential changes,
  and future org promotion.

This is architecture guidance, not legal advice or a certification statement.

## External Anchors

These sources should shape implementation language and acceptance criteria:

- European Commission GDPR guidance emphasizes lawfulness, fairness,
  transparency, purpose limitation, data minimization, accuracy, storage
  limitation, integrity, and confidentiality. The same guidance describes data
  protection by design and by default as product-design obligations, not only
  policy work.
- EDPB controller/processor guidance and its 2024 processor opinion reinforce
  that applications relying on providers need a current inventory of processors
  and sub-processors where applicable. In AudioGraph terms, provider descriptors
  should carry enough non-secret transfer metadata for the user and future
  admin/export surfaces to know which processors touched a session.
- EDPB DPIA guidance describes DPIA as a written assessment for high-risk
  processing. AudioGraph should maintain DPIA-ready data-flow records for
  meeting audio, transcript, speaker, notes, graph, and provider transfers,
  without claiming a DPIA has been legally completed for every deployment.
- NIST Privacy Framework maps well to product requirements: identify data
  processing, govern policy, control user choices, communicate what happens, and
  protect data. AudioGraph's product equivalent is a data route inventory plus
  local/cloud/provider controls.
- NIST AI RMF and the Generative AI Profile point to Govern, Map, Measure, and
  Manage risk functions. AudioGraph should apply that to transcript-to-LLM
  projection, live-assist cards, prompt provenance, hallucination/retcon
  metrics, and provider routing.
- AICPA SOC 2 Trust Services Criteria cover security, availability, processing
  integrity, confidentiality, and privacy. AudioGraph can use those categories
  as a readiness checklist, but should not make SOC 2 claims until an actual
  examination exists.
- FTC AI/privacy guidance is directly relevant to provider and product claims:
  do not promise local/private behavior and then silently use cloud providers,
  training, broader retention, or third-party sharing through hidden defaults or
  retroactive copy changes.
- OWASP LLM guidance highlights prompt injection, sensitive information
  disclosure, and system prompt leakage. AudioGraph's LLM prompts, graph
  context, provider errors, and live-assist actions must be treated as sensitive
  application context, not harmless logs.

## Product Stance

### Local Private By Default

Local transcript, notes, graph, speaker timeline, live-assist cards, usage, and
promotion drafts are private local session memory unless the user selects a
cloud provider or explicitly promotes/exports data.

Local-first does not mean "no cloud ever." It means cloud transfer is visible,
provider-scoped, credential-gated, and reversible at the product level.

### Backend-Enforced Boundaries

React should render controls, readiness, source labels, and data route summaries.
Rust should enforce:

- provider sockets and HTTP requests;
- credential reads and saved-key health/model discovery;
- processed PCM routing;
- transcript/projection writes;
- audit event writes;
- local-only/cloud-disabled gates;
- export/delete/session artifact enumeration.

### No Hidden Provider Training Claims

Provider descriptors should avoid unsupported claims such as "not used for
training" unless a provider-specific source and configuration prove it. The
safer descriptor language is:

- data class sent: audio, transcript text, graph context, prompts, generated
  outputs;
- transfer mode: local, user BYOK cloud, enterprise cloud, org sync;
- user action that caused the transfer;
- provider retention/training policy: unknown, user-configured, documented link,
  or enterprise contractual.

### Explicit Org Promotion

Org knowledge is a selected, redacted, approved copy of a local object version.
It is not automatic sync of a meeting. Promotion needs source object version,
redaction snapshot, ACL, retention, sync state, and revocation records.

## Data Classification Matrix

| Data class | Default boundary | Durable storage | Cloud transfer conditions | Retention/delete/export requirement | User-visible route |
|---|---|---|---|---|---|
| Raw capture audio | Local process memory | Do not persist by default | Never unless a provider requires raw/encoded audio for active ASR/S2S | Drop after bounded processing window unless explicit recording feature exists | Capture source, sample rate, provider transfer yes/no |
| Processed PCM chunks | Local Rust backend | Do not persist by default | Sent only to selected realtime ASR/S2S/TTS provider | Drop after bounded queues drain; queue overflow is observable | Consumer list and backpressure/drop status |
| ASR provider audio stream | Selected provider | Provider-side unknown unless documented | Only when user selects cloud ASR/S2S | Ledger records provider, model, endpoint class, start/end, redacted error | "Audio left device" with provider id |
| Transcript span revisions | Local session memory | JSON/JSONL or repository record | Sent to LLM/live-assist providers only when selected and required | Exportable, erasable, replayable; hard delete removes event logs | Transcript persisted locally; LLM transfer yes/no |
| Speaker timeline | Local session memory | Future revisioned speaker records | Provider speaker labels may arrive from ASR; local diarization stays local | Exportable with session, redaction-capable before sharing/promotion | Speaker source: local/provider/unknown |
| Materialized notes | Local session memory | Notes JSON or repository record | Sent to recall/live-assist/org sync only by explicit mode/action | Revision/diff history must be recoverable; delete with session | Notes generated by provider/local model |
| Projection patches | Local session memory | Projection JSONL or repository record | Not directly sent unless used as LLM context | Required for retcon replay; delete with session | Last projection run provider/model/prompt |
| Temporal graph facts | Local session memory | Graph JSON/repository records | Sent to recall/live-assist providers only as selected context | Exportable, redaction-capable, versioned by basis | Graph facts with provenance and provider/model |
| Embeddings/vector indexes | Local memory by default | Future repository/index files | Cloud embeddings only behind explicit provider setting | Rebuildable from source; delete/rebuild with session/workspace | Embedding provider and source objects |
| Live-assist cards | Local session memory | Live assist audit/current records | Generated through selected LLM/realtime provider | Approved/dismissed history retained with session; delete with session | Card cites transcript spans/graph context |
| Prompt/context payloads | Backend transient; ledger summary only | Do not persist full prompts by default | Sent to selected LLM/realtime provider | Store prompt id, model, data classes, hashes/sizes; not raw secrets | Prompt template id and context classes |
| Provider responses | Local generated artifacts | Notes/cards/graph patches, redacted error snippets | Returned by selected provider | Store normalized outputs; redact provider errors/logs | Provider/model/output artifact linkage |
| Provider logs/errors | Local redacted diagnostics | Redacted logs only | N/A | No secrets/raw transcript excerpts unless explicit debug export | Error class, provider, retry/correlation id |
| Credentials | Credential backend | OS keychain target; YAML fallback/import/dev | Used only backend-side for provider calls | Presence/source export only; delete per key | Saved/missing/source/error, never plaintext |
| Config | Local non-secret settings | `config.yaml` | Not transferred by default | Exportable as non-secret support artifact | Provider settings without keys |
| Usage/latency metadata | Local session usage files | Usage JSON/repository record | Optional diagnostics only after explicit consent | Delete with session; export redacted | Token/audio seconds/cost estimate by provider |
| Org promoted knowledge | Local plus selected org target | Promotion events, redaction snapshots, org item records | Only after explicit approve/sync action | Retention/ACL/revocation required | Org-visible preview and sync state |

## Session Data Movement Ledger

Add a backend-owned append-only audit stream for data movement and artifact
events. This is separate from transcript/projection event logs: it answers trust
questions, not graph replay questions.

Suggested event shape:

```json
{
  "event_id": "uuid",
  "schema_version": 1,
  "session_id": "uuid",
  "created_at_ms": 0,
  "actor": "system|user|provider|sync",
  "event_type": "capture_started",
  "data_classes": ["processed_pcm"],
  "source": {
    "kind": "rsac",
    "source_id": "stable-source-id",
    "source_label_redacted": "System audio"
  },
  "destination": {
    "boundary": "local|provider|org|export",
    "provider_id": null,
    "endpoint_class": null
  },
  "artifact_refs": [
    {
      "kind": "transcript_events",
      "storage": "file",
      "path_hash": "sha256"
    }
  ],
  "basis": {
    "transcript_sequence": 12,
    "projection_sequence": 4
  },
  "model": {
    "provider_id": "llm.openrouter",
    "model_id": "anthropic/claude-sonnet-4"
  },
  "counts": {
    "audio_ms": 500,
    "text_chars": 1200,
    "tokens_in": 300,
    "tokens_out": 80
  },
  "policy": {
    "privacy_mode": "local_only|byok_cloud|org_sync",
    "user_visible": true,
    "retention_class": "session_artifact"
  },
  "result": {
    "status": "started|succeeded|failed|cancelled|blocked",
    "error_code": null,
    "error_message_redacted": null
  }
}
```

Required event types:

- `capture_started`, `capture_stopped`, `audio_consumer_started`,
  `audio_consumer_backpressure`, `audio_consumer_dropped`;
- `provider_call_started`, `provider_call_succeeded`,
  `provider_call_failed`, `provider_call_cancelled`;
- `artifact_written`, `artifact_loaded`, `artifact_exported`,
  `artifact_soft_deleted`, `artifact_hard_deleted`,
  `artifact_delete_failed`;
- `credential_saved`, `credential_deleted`, `credential_source_changed`,
  `provider_readiness_checked`;
- `projection_job_queued`, `projection_job_started`,
  `projection_patch_accepted`, `projection_patch_rejected`;
- `promotion_draft_created`, `promotion_redaction_reviewed`,
  `promotion_sync_started`, `promotion_sync_succeeded`,
  `promotion_revoked`.

Implementation detail: redact payload text by default. The event can store
hashes, byte counts, token counts, model ids, provider ids, source ids, and
artifact descriptors. It should not store raw audio, raw transcript excerpts,
API keys, bearer tokens, service-account JSON, or full prompt bodies.

## Runtime Policy Modes

Add a backend-owned privacy/provider policy resolved at session start:

| Mode | Meaning | Enforcement |
|---|---|---|
| `local_only` | No external provider transfers for audio, transcript, notes, graph, prompts, embeddings, or live-assist. | Reject cloud ASR/LLM/TTS/S2S/provider readiness calls that would send session content. Allow local model readiness and credential presence. |
| `byok_cloud` | User-selected cloud providers may receive only the data classes required for the active pipeline. | Provider descriptors and active settings determine transfer classes; ledger records every transfer. |
| `cloud_disabled_readiness_only` | Saved cloud credentials can be checked for health/model catalog, but session content cannot leave device. | Allow no-content probes; block session audio/text/provider LLM calls. |
| `org_promotion` | Explicit redacted object versions may sync to an org target. | Requires redaction snapshot, ACL, retention, source basis, and approval event. |

This policy belongs in Rust. React can ask to change it, but provider clients
must refuse disallowed transfers without relying on UI state.

## Provider Registry Requirements

Extend provider descriptors over time with:

- `data_classes_sent`: audio, transcript_text, notes, graph_context,
  embeddings, live_audio_output, prompts, tool_calls;
- `data_classes_returned`: transcript_spans, speaker_labels, notes,
  graph_patches, live_audio, tool_results;
- `health_check_data_classes`: none, credential_only, account_metadata,
  model_catalog;
- `retention_policy_url` and `training_policy_url` when documented;
- `requires_explicit_cloud_transfer_ack`;
- `supports_enterprise_no_training_config`;
- `supports_region_or_data_residency`;
- `supports_data_deletion_request`;
- `sensitive_error_policy`: redact_all, code_only, provider_message_redacted;
- `processor_identity`: provider legal/service name if known.

Do not block provider additions on every field being perfect. Start with
`unknown` values and display them honestly.

## UX Requirements

### Settings

Settings should keep saved-key behavior:

- saved key presence and source are visible without plaintext readback;
- model discovery and health checks use saved keys automatically;
- cloud providers show which data classes they may receive;
- local-only/cloud-disabled mode is visible near provider selection, not buried
  in advanced settings;
- provider readiness distinguishes "credential missing", "credential saved but
  unchecked", "no-content health passed", and "provider content transfer
  disabled by policy".

### Session Data Route Panel

Each active and historical session should have a "Data route" view:

- capture source and OS backend;
- active privacy mode;
- ASR/LLM/TTS/S2S provider ids and models;
- whether audio left the device;
- whether transcript/notes/graph context left the device;
- artifact list and storage boundary;
- delete/export status;
- redacted provider errors and last health status;
- org promotion/export history when present.

For alpha, this can be a compact inspector. It does not need legal prose; it
needs trustworthy state.

### Exports

Exports should offer at least two modes:

- `raw_session_export`: local transcript, notes, graph, revisions, and route
  ledger for the user's own archive.
- `redacted_share_export`: user-reviewed export that can omit transcript text,
  speaker identities, provider ids, or source paths.

Clipboard exports of only the visible transcript/graph are useful, but they are
not sufficient for compliance/readiness because they omit artifact and route
metadata.

## Engineering Gaps

Current strengths:

- secrets and non-secret settings are mostly split;
- settings/readiness use saved credential presence without normal plaintext
  readback;
- session artifacts are enumerable and hard-delete primitives exist;
- soft delete plus 30-day trash retention exists;
- transcript/projection event logs support replay and retcon;
- org promotion design already treats org memory as explicit redacted copies;
- provider registry/codegen is emerging as the right place for capabilities.

Current gaps:

- no dedicated data movement ledger;
- no user-facing session data route panel;
- export commands return current in-memory transcript/graph snapshots, not a
  complete route/artifact bundle;
- hard delete is best-effort and logs failures but does not return a user-facing
  deletion report for all artifact classes;
- provider descriptors do not yet expose data classes sent/returned or
  retention/training/deletion policy metadata;
- local-only/cloud-disabled policy is not a backend-enforced session gate across
  every provider call;
- provider error/log redaction is not centralized around data classes;
- embeddings/vector indexes are not yet in the artifact taxonomy;
- SOC2/GDPR/DPIA readiness exists only as scattered architecture intent, not a
  living checklist.

## Proposed Seed Backlog

These Seeds should be created or updated from this architecture session:

1. Data movement ledger and audit event schema.
   - Priority: P2, becomes P1 before any org sync/sharing release.
   - Acceptance: backend writes redacted movement events for provider calls,
     artifact writes/export/delete, credential changes, readiness checks, and
     promotion state; tests prove no secrets/raw payloads are persisted.

2. Session data route UI.
   - Priority: P2.
   - Acceptance: active and loaded sessions show local/cloud/provider route,
     whether audio/text left device, provider/model ids, artifact descriptors,
     export/delete state, and redacted errors.

3. Backend-enforced local-only/cloud-disabled policy.
   - Priority: P1/P2 depending on alpha cutline.
   - Acceptance: provider clients cannot send session audio/text/graph context
     when policy blocks it; no-content readiness probes remain allowed; tests
     cover ASR, LLM, TTS, S2S, embeddings, and live-assist routes.

4. Complete session export/delete reports.
   - Priority: P2.
   - Acceptance: export bundles transcript/projection/materialized notes/graph,
     live-assist, usage, route ledger, and metadata; delete returns per-artifact
     success/failure and includes repository records, future embeddings, and
     promotion artifacts.

5. Provider data-class and retention metadata.
   - Priority: P2.
   - Acceptance: registry supports sent/returned data classes, no-content
     health-check classification, provider policy URLs/status, and unknown
     values; Settings displays unknowns honestly.

6. Redacted provider diagnostics contract.
   - Priority: P2.
   - Acceptance: provider logs/errors pass through a shared redaction layer;
     tests cover API keys, bearer tokens, raw transcript text, source paths,
     prompt bodies, provider request ids, and safe error codes.

7. SOC2/GDPR/DPIA readiness checklist.
   - Priority: P3 until enterprise sales, but useful now.
   - Acceptance: docs/checklist maps product controls to security,
     availability, processing integrity, confidentiality, privacy,
     controller/processor inventory, data subject/export/delete support, and
     DPIA-ready records without claiming certification.

## Sources

- European Commission, data protection explained:
  https://commission.europa.eu/law/law-topic/data-protection/data-protection-explained_en
- European Commission, GDPR processing principles:
  https://commission.europa.eu/law/law-topic/data-protection/rules-business-and-organisations/principles-gdpr/overview-principles/what-data-can-we-process-and-under-which-conditions_en
- European Commission, data protection by design and by default:
  https://commission.europa.eu/law/law-topic/data-protection/rules-business-and-organisations/obligations/what-does-data-protection-design-and-default-mean_en
- European Commission, international data transfers:
  https://commission.europa.eu/law/law-topic/data-protection/international-dimension-data-protection/rules-international-data-transfers_en
- EDPB, controller/processor concepts:
  https://www.edpb.europa.eu/our-work-tools/our-documents/guidelines/guidelines-072020-concepts-controller-and-processor-gdpr_en
- EDPB, processor/sub-processor obligations news summary:
  https://www.edpb.europa.eu/news/news/2024/edpb-adopts-opinion-processors-guidelines-legitimate-interest-statement-draft_en
- EDPB, DPIA guidance:
  https://www.edpb.europa.eu/sme-data-protection-guide/be-compliant_en
- EDPB, data subject rights:
  https://www.edpb.europa.eu/sme-data-protection-guide/respect-individuals-rights_en
- EDPB, information to communicate to individuals:
  https://www.edpb.europa.eu/sme-data-protection-guide/faq-frequently-asked-questions/answer/what-information-should-i_en
- NIST Privacy Framework:
  https://www.nist.gov/privacy-framework
- NIST Privacy Framework FAQ:
  https://www.nist.gov/privacy-framework/frequently-asked-questions
- NIST AI RMF:
  https://www.nist.gov/itl/ai-risk-management-framework
- NIST AI RMF Generative AI Profile:
  https://www.nist.gov/publications/artificial-intelligence-risk-management-framework-generative-artificial-intelligence
- AICPA SOC suite:
  https://www.aicpa-cima.com/resources/landing/system-and-organization-controls-soc-suite-of-services
- AICPA Trust Services Criteria:
  https://www.aicpa-cima.com/resources/download/2017-trust-services-criteria-with-revised-points-of-focus-2022
- AICPA SOC 2 description criteria:
  https://www.aicpa-cima.com/resources/download/get-description-criteria-for-your-organizations-soc-2-r-report
- FTC, AI companies privacy and confidentiality commitments:
  https://www.ftc.gov/policy/advocacy-research/tech-at-ftc/2024/01/ai-companies-uphold-your-privacy-confidentiality-commitments
- FTC, retroactive AI data-practice changes:
  https://www.ftc.gov/policy/advocacy-research/tech-at-ftc/2024/02/ai-other-companies-quietly-changing-your-terms-service-could-be-unfair-or-deceptive
- FTC AI topic page:
  https://www.ftc.gov/industry/technology/artificial-intelligence
- OWASP Top 10 for LLM Applications:
  https://owasp.org/www-project-top-10-for-large-language-model-applications/
- OWASP LLM prompt injection:
  https://genai.owasp.org/llmrisk/llm01-prompt-injection/
- OWASP LLM sensitive information disclosure:
  https://genai.owasp.org/llmrisk/llm02-insecure-output-handling/
- OWASP LLM system prompt leakage:
  https://genai.owasp.org/llmrisk/llm07-insecure-plugin-design/
