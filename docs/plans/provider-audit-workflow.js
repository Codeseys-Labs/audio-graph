export const meta = {
  name: 'provider-api-audit',
  description: 'Audit each cloud provider implementation against its official API docs; adversarially verify every discrepancy; synthesize a ranked fix plan',
  phases: [
    { title: 'Audit', detail: 'one agent per provider: read impl + fetch official docs + compare' },
    { title: 'Verify', detail: 'adversarially confirm each suspect/broken finding against the docs' },
    { title: 'Synthesize', detail: 'rank findings + write fix plan' },
  ],
}

// Providers with real cloud API surface. Baked as constants (args do not thread).
// files = impl to read; docs = official references; established = known evidence.
const HR = '/home/codeseys/.local/share/uv/tools/hyperresearch/bin/hyperresearch'
const PROVIDERS = [
  {
    key: 'deepgram', kind: 'asr', priority: true,
    files: ['src-tauri/src/asr/deepgram.rs', 'src-tauri/src/commands.rs (list_deepgram_models_cmd only)'],
    docs: [
      'https://developers.deepgram.com/reference/speech-to-text/listen-streaming',
      'https://developers.deepgram.com/reference/speech-to-text/listen-flux',
      'https://developers.deepgram.com/reference/manage/models/list',
      'https://developers.deepgram.com/docs/model',
    ],
    established: 'CONFIRMED EVIDENCE: the user config saves asr_provider.model = "general", so deepgram_listen_url() (deepgram.rs ~593) builds wss://api.deepgram.com/v1/listen?...&model=general. "general" is a LEGACY model tier; current streaming expects nova-3 / nova-3-general. Determine: (a) is model=general still accepted by v1/listen streaming or is it invalid/deprecated and the true cause of a 401/400 or empty transcript; (b) is there ANY mapping layer from a friendly name to a model id, or does config.model pass through raw; (c) is the flux branch (v2/listen, eot_threshold/eager_eot) correct per the listen-flux ref; (d) does list_deepgram_models_cmd hit the correct models endpoint and correctly filter to STREAMING stt models (not batch/tts) and mark the right default. This is the PRIORITY provider.',
  },
  {
    key: 'soniox', kind: 'asr', files: ['src-tauri/src/asr/soniox.rs'],
    docs: ['https://soniox.com/docs/stt/api-reference/websocket-api', 'https://soniox.com/docs/stt/models'],
    established: 'Registry default_model = stt-rt-v5. Verify the WS endpoint, auth header, config/handshake JSON shape, and that list_soniox_models_cmd targets the right models endpoint.',
  },
  {
    key: 'assemblyai', kind: 'asr', files: ['src-tauri/src/asr/assemblyai.rs'],
    docs: ['https://www.assemblyai.com/docs/api-reference/streaming-api/streaming', 'https://www.assemblyai.com/docs/speech-to-text/universal-streaming'],
    established: 'Registry default_model = universal-3-5-pro (Fixed catalog). Verify the streaming endpoint (v3 universal-streaming vs legacy v2 realtime), auth (Authorization vs token query param), and whether universal-3-5-pro is a real current model id.',
  },
  {
    key: 'aws_transcribe', kind: 'asr', files: ['src-tauri/src/asr/aws_transcribe.rs'],
    docs: ['https://docs.aws.amazon.com/transcribe/latest/dg/streaming.html', 'https://docs.aws.amazon.com/transcribe/latest/APIReference/API_streaming_StartStreamTranscriptionWebSocket.html'],
    established: 'Registry default_model = transcribe-streaming. Verify SigV4 signing of the WS URL, region handling, and required query params (language-code, media-encoding, sample-rate).',
  },
  {
    key: 'gladia', kind: 'asr', files: ['src-tauri/src/asr/gladia.rs'],
    docs: ['https://docs.gladia.io/api-reference/v2/live/init', 'https://docs.gladia.io/chapters/live-stt/getting-started'],
    established: 'Registry default_model = solaria-1. Verify the 2-step init (POST /v2/live to get a session URL, then WS) vs direct WS, auth header (x-gladia-key), and config shape.',
  },
  {
    key: 'speechmatics', kind: 'asr', files: ['src-tauri/src/asr/speechmatics.rs'],
    docs: ['https://docs.speechmatics.com/rt-api-ref', 'https://docs.speechmatics.com/introduction/supported-languages'],
    established: 'Registry default_model = enhanced. Verify the RT WS endpoint, JWT/temp-token vs api-key auth, StartRecognition message shape, and operating_point (enhanced/standard) vs model.',
  },
  {
    key: 'revai', kind: 'asr', files: ['src-tauri/src/asr/revai.rs'],
    docs: ['https://docs.rev.ai/api/streaming/', 'https://docs.rev.ai/api/streaming/requests/'],
    established: 'Registry default_model = machine_v2. Verify the streaming WS endpoint, access_token query param auth, content-type/codec params, and whether machine_v2 maps to a real transcriber value.',
  },
  {
    key: 'openai_realtime', kind: 'asr', files: ['src-tauri/src/asr/openai_realtime.rs'],
    docs: ['https://platform.openai.com/docs/guides/realtime-transcription', 'https://platform.openai.com/docs/api-reference/realtime'],
    established: 'JUST FIXED on master: realtime_url() now returns wss://api.openai.com/v1/realtime?intent=transcription (no model= param); model goes via session.update transcription.model. VERIFY the whole handshake is coherent post-fix: session.update payload shape, the transcription session config, input_audio_format, and that the fixed-catalog models (gpt-realtime-whisper default, gpt-4o-transcribe, gpt-4o-mini-transcribe) are all valid transcription models for this endpoint.',
  },
  {
    key: 'deepgram_aura', kind: 'tts', files: ['src-tauri/src/tts/deepgram_aura.rs'],
    docs: ['https://developers.deepgram.com/reference/text-to-speech-api/speak-streaming', 'https://developers.deepgram.com/docs/tts-models'],
    established: 'TTS provider. Registry has aura-*-en voices. Verify the speak endpoint (REST vs WS streaming), auth, and that the aura voice ids are current (aura vs aura-2 naming).',
  },
  {
    key: 'llm_openai_compat', kind: 'llm', files: ['src-tauri/src/llm/api_client.rs', 'src-tauri/src/commands.rs (list_openai_compatible_llm_models_cmd only)'],
    docs: ['https://platform.openai.com/docs/api-reference/chat/create', 'https://platform.openai.com/docs/api-reference/models/list'],
    established: 'The generic OpenAI-compatible client (also reused for asr.api after PR #30). Verify chat/completions request shape, streaming SSE parse, base-url handling (trailing /v1), and that list_openai_compatible_llm_models_cmd hits GET {base}/models.',
  },
  {
    key: 'bedrock', kind: 'llm', files: ['src-tauri/src/llm/bedrock.rs'],
    docs: ['https://docs.aws.amazon.com/bedrock/latest/APIReference/API_runtime_ConverseStream.html', 'https://docs.aws.amazon.com/bedrock/latest/userguide/models-supported.html'],
    established: 'Registry default llm model shows openai/gpt-oss-120b in config for some provider. Verify Bedrock ConverseStream vs InvokeModelWithResponseStream, SigV4, region, and model-id format (inference-profile ARNs vs bare ids).',
  },
  {
    key: 'openrouter', kind: 'llm', files: ['src-tauri/src/llm/openrouter.rs'],
    docs: ['https://openrouter.ai/docs/api-reference/chat-completion', 'https://openrouter.ai/docs/api-reference/list-available-models'],
    established: 'Verify base url (openrouter.ai/api/v1), required headers (HTTP-Referer / X-Title), chat completions shape, and that list_openrouter_models_cmd hits GET /api/v1/models.',
  },
]

const FINDING_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['provider', 'kind', 'verdict', 'severity', 'headline', 'notePath', 'discrepancies'],
  properties: {
    provider: { type: 'string' },
    kind: { type: 'string' },
    verdict: { type: 'string', enum: ['correct', 'suspect', 'broken'] },
    severity: { type: 'string', enum: ['none', 'low', 'medium', 'high', 'critical'] },
    headline: { type: 'string', description: 'one sentence: the single most important finding' },
    discrepancies: {
      type: 'array', maxItems: 12,
      items: {
        type: 'object', additionalProperties: false,
        required: ['what', 'impl_says', 'docs_say', 'severity', 'file_line'],
        properties: {
          what: { type: 'string' },
          impl_says: { type: 'string' },
          docs_say: { type: 'string' },
          severity: { type: 'string', enum: ['low', 'medium', 'high', 'critical'] },
          file_line: { type: 'string' },
        },
      },
    },
    notePath: { type: 'string', description: 'path to the detailed markdown note on disk' },
  },
}

const VERIFY_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['provider', 'confirmed', 'verifyVerdict', 'note'],
  properties: {
    provider: { type: 'string' },
    confirmed: { type: 'boolean', description: 'true if the discrepancy holds up against the docs; false if it was a false alarm' },
    verifyVerdict: { type: 'string', enum: ['confirmed', 'downgraded', 'refuted'] },
    note: { type: 'string', description: 'one-paragraph verdict with the doc citation that settles it' },
  },
}

function auditPrompt(p) {
  const filesList = p.files.join(', ')
  const docsList = p.docs.map((d) => '  - ' + d).join('\n')
  return [
    'You are auditing ONE provider implementation in the Tauri app at /mnt/e/CS/github/audio-graph against its OFFICIAL API docs. READ-ONLY: do not edit any code. Be precise and cite file:line.',
    '',
    'PROVIDER: ' + p.key + ' (' + p.kind + ')',
    'IMPL FILES to read fully: ' + filesList,
    'OFFICIAL DOCS to fetch and compare against:',
    docsList,
    '',
    'ESTABLISHED EVIDENCE (do not re-derive; investigate these specifically): ' + p.established,
    '',
    'HOW TO FETCH DOCS: prefer the project fetch CLI: ' + HR + ' fetch "<url>"  (it handles JS-rendered pages and PDFs). You may also load WebFetch or exa/tavily via ToolSearch if the CLI fails. Fetch every doc URL above and read the relevant sections.',
    '',
    'AUDIT CHECKLIST (map the impl against the docs):',
    '  1. ENDPOINT: exact host + path + protocol (WS vs REST vs 2-step). Does the impl hit the current endpoint or a deprecated one?',
    '  2. AUTH: header name / query-param / signing scheme. Correct per docs?',
    '  3. MODEL/PARAMS: how is the model id chosen and passed? Is the default (and any friendly-name like "general"/"enhanced") a REAL current model id the API accepts? Any missing/wrong required query params or handshake fields?',
    '  4. MODEL-LIST command (if this provider has a list_*_models_cmd): does it hit the right endpoint and filter/label correctly?',
    '  5. RESPONSE parse: does the impl parse the current response/event shape?',
    '',
    'DELIVERABLE:',
    '  - Write a DETAILED markdown note to /tmp/provider-audit/' + p.key + '.md with: each checklist item, impl file:line, the doc-backed correct behavior, and every discrepancy with severity + a concrete failure scenario + the doc URL that proves it.',
    '  - Then return the thin StructuredOutput: provider, kind, verdict (correct=matches docs / suspect=likely wrong, needs a second look / broken=definitely wrong), overall severity, a one-sentence headline, the notePath, and the discrepancies array (keep each field short; the depth lives in the note).',
    '  - If you cannot fetch a doc, say so in the note and mark affected items as "unverified" rather than guessing.',
  ].join('\n')
}

phase('Audit')
// pipeline: each provider audited, then any non-correct finding adversarially verified,
// independently (no barrier between audit and verify).
const audited = await pipeline(
  PROVIDERS,
  (p) => agent(auditPrompt(p), { label: 'audit:' + p.key, phase: 'Audit', schema: FINDING_SCHEMA, model: 'opus' }),
  async (finding, p) => {
    if (!finding) return null
    if (finding.verdict === 'correct') return { finding, verify: null }
    // adversarial verify: a fresh skeptic tries to REFUTE the top discrepancies against the docs
    const top = (finding.discrepancies || []).slice(0, 5)
      .map((d, i) => (i + 1) + '. ' + d.what + ' | impl: ' + d.impl_says + ' | claimed docs: ' + d.docs_say + ' | ' + d.file_line)
      .join('\n')
    const vp = [
      'You are ADVERSARIALLY verifying an audit finding for provider ' + p.key + ' in /mnt/e/CS/github/audio-graph. READ-ONLY.',
      'The auditor claims verdict=' + finding.verdict + '. Your job: try to REFUTE each claimed discrepancy by checking the ACTUAL current official docs and the ACTUAL impl code. Default to skepticism — if the docs do not clearly support the discrepancy, mark it refuted/downgraded.',
      '',
      'IMPL FILES: ' + p.files.join(', '),
      'OFFICIAL DOCS: ' + p.docs.join(' , '),
      'FETCH via: ' + HR + ' fetch "<url>"',
      'The detailed audit note is at /tmp/provider-audit/' + p.key + '.md — read it.',
      '',
      'CLAIMED DISCREPANCIES:',
      top,
      '',
      'Re-read the impl at the cited lines and the current docs. Return the thin StructuredOutput: confirmed (true only if at least one HIGH/CRITICAL discrepancy genuinely holds), verifyVerdict (confirmed/downgraded/refuted), and a one-paragraph note citing the exact doc text that settles it. Append your verdict to the bottom of the note file is NOT required (you are Read-only on docs; do not edit the note).',
    ].join('\n')
    const verify = await agent(vp, { label: 'verify:' + p.key, phase: 'Verify', schema: VERIFY_SCHEMA, model: 'opus' })
    return { finding, verify }
  }
)

phase('Synthesize')
const rows = audited.filter(Boolean)
// Build a compact table for the synthesizer; substance is in the per-provider notes on disk.
const summary = rows.map((r) => {
  const f = r.finding
  const v = r.verify
  return {
    provider: f.provider, kind: f.kind, verdict: f.verdict, severity: f.severity,
    headline: f.headline, notePath: f.notePath,
    verifyVerdict: v ? v.verifyVerdict : 'n/a', verified: v ? v.confirmed : null,
    topDiscrepancies: (f.discrepancies || []).slice(0, 6),
  }
})
log('Audited ' + rows.length + ' providers; ' + rows.filter((r) => r.finding.verdict !== 'correct').length + ' non-correct.')

const synthPrompt = [
  'You are the synthesizer for a provider-API audit of the Tauri app at /mnt/e/CS/github/audio-graph. Read-only on code; you WRITE one report file.',
  'Per-provider audit notes are on disk at /tmp/provider-audit/<provider>.md — READ the notes for every provider whose verdict is suspect or broken, and spot-check any "correct" you doubt.',
  '',
  'Here is the machine summary of all providers (JSON):',
  JSON.stringify(summary, null, 2),
  '',
  'PRODUCE a decision-grade report and WRITE it to docs/plans/2026-07-02-provider-api-audit.md with:',
  '  1. Executive summary: which providers are BROKEN, which SUSPECT, which CORRECT — ranked by user impact (Deepgram is the priority since the user hit it).',
  '  2. For each broken/suspect provider: the confirmed discrepancy (only those the verify stage CONFIRMED — call out any the verify stage REFUTED as false alarms and exclude them from the fix list), impl file:line, the doc-backed correct behavior, and a concrete minimal fix.',
  '  3. A DEEPGRAM section specifically addressing: is model=general the cause; the correct model id / mapping; the flux v2 path; and the list-models command.',
  '  4. A prioritized fix plan (P0/P1/P2) with effort (S/M/L) per item, and note which fixes are independent (parallelizable) vs ordered.',
  '  5. Cross-provider patterns (e.g. friendly-name-vs-model-id mismatch appearing in multiple providers).',
  '',
  'Then return the thin StructuredOutput.',
].join('\n')

const SYNTH_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['reportPath', 'broken', 'suspect', 'correct', 'p0_fixes', 'deepgram_verdict'],
  properties: {
    reportPath: { type: 'string' },
    broken: { type: 'array', items: { type: 'string' } },
    suspect: { type: 'array', items: { type: 'string' } },
    correct: { type: 'array', items: { type: 'string' } },
    p0_fixes: { type: 'array', maxItems: 12, items: { type: 'string' } },
    deepgram_verdict: { type: 'string', description: 'one paragraph: is model=general the cause, and the correct fix' },
  },
}

const report = await agent(synthPrompt, { label: 'synthesize', phase: 'Synthesize', schema: SYNTH_SCHEMA, model: 'opus' })
return { audited: summary, report }
