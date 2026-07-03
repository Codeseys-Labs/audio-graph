export const meta = {
  name: 'provider-arch-and-bugs',
  description: 'Investigate flux-clobber regression + Deepgram 401 read-path + flux-catalog gap + Tauri FE<->Rust testing, then design a formalized per-provider contract (base+advanced, load-models, test-model-connection, hover model-info) as an ADR + prioritized fix plan',
  phases: [
    { title: 'Investigate', detail: 'parallel lanes: flux-regression, 401-readpath, flux-catalog, tauri-testing, arch-survey, hover-metadata' },
    { title: 'Verify', detail: 'adversarially confirm the two live-bug root causes' },
    { title: 'Design', detail: 'synthesize ADR + contract design + prioritized fix plan' },
  ],
}

const ROOT = '/mnt/e/CS/github/audio-graph'
const OUT = '/tmp/provider-arch'

// ---------- Phase 1: parallel investigation lanes ----------
const LANES = [
  {
    key: 'flux-regression',
    bug: true,
    prompt: [
      'ROOT-CAUSE (read-only, no edits) a CONFIRMED P0 regression in the Tauri app at ' + ROOT + '. systematic-debugging: find WHY precisely, do NOT fix.',
      'CONFIRMED EVIDENCE (from the user Windows log, do not re-derive): the user had asr_provider.model = "flux" selected (turn-based Deepgram). On settings load the NEW migration logged 4x: "Migrating persisted Deepgram model \'flux\' (not a valid streaming model id) to \'nova-3\' on settings load." So the app SILENTLY REWRITES the user\'s flux selection to nova-3 — this is why the user "cannot see/use flux models."',
      'The regression is OURS: PR #34 added migrate_asr_provider_model (src-tauri/src/settings/mod.rs ~1858) + is_valid_deepgram_streaming_model + sanitize_deepgram_model (src-tauri/src/asr/deepgram.rs ~648-681). The validator treats bare "flux" as invalid because it requires the flux- PREFIX plus a suffix (flux-general-en). But the user (and the UI) apparently persist bare "flux".',
      'INVESTIGATE: (a) what EXACT model string does the Deepgram flux option in the frontend settings write to config (grep the frontend for flux; is it "flux" or "flux-general-en" or similar)? (b) what does Deepgram\'s listen-flux API actually accept as the model value on v2/listen — is it "flux" or "flux-general-en"? Fetch https://developers.deepgram.com/reference/speech-to-text/listen-flux via ' + "/home/codeseys/.local/share/uv/tools/hyperresearch/bin/hyperresearch" + ' fetch. (c) Is the bug in the VALIDATOR (too strict — should accept bare "flux") or in the FRONTEND (writes an invalid model string) or BOTH? (d) does the frontend even OFFER flux as an option, and if so where (grep frontend settings components).',
      'Write findings to ' + OUT + '/flux-regression.md with file:line for: the frontend flux option + what it writes, the validator logic, the migration logic, and the exact minimal fix (e.g. add "flux" to valid set / accept bare flux + route to v2, AND/OR fix the frontend to write the API-correct string). Return the thin schema.',
    ].join('\n'),
  },
  {
    key: '401-readpath',
    bug: true,
    prompt: [
      'ROOT-CAUSE (read-only, no edits) the Deepgram 401 in the Tauri app at ' + ROOT + '. systematic-debugging.',
      'CONFIRMED EVIDENCE (user Windows log, latest build 8f11450): credential SAVE works (proven earlier: save_credential_cmd invoke + keychain set-password + persisted). But at capture time: "Speech processor: ASR provider is Deepgram streaming (model=nova-3-general)" then "Deepgram connect: api_key <present> len=40" then "ERROR ... failed to connect: 401 API key rejected". So a 40-char key IS being sent, model is valid, but Deepgram rejects it.',
      'THE KEY QUESTION: does the RUNTIME READ-PATH (what populates DeepgramConfig.api_key at speech/mod.rs:2957 `api_key: api_key.clone()`) read the SAME credential the SAVE-PATH (save_credential_cmd -> set_credential -> keychain) wrote? Trace backwards: at speech/mod.rs, where does the local `api_key` variable that feeds DeepgramConfig come from? Is it get_credential("deepgram_api_key") from the keychain, or from a cached/config/settings value that could be STALE or a DIFFERENT key? Find the exact function call chain from "start capture" to the api_key value.',
      'ALSO: (a) is it possible a DIFFERENT deepgram key is stored than the one the user thinks (e.g. an old key in keychain vs a new one in config.yaml fallback, and the read-path prefers the stale one)? (b) does redacted_secret_presence / any existing log emit a NON-SECRET FINGERPRINT (first4+last4, or a hash) that could distinguish which key is read vs saved? If not, specify the exact diagnostic to add (a sha256-prefix or first2/last2 of the key, NEVER the full key) at both the save path and the read/connect path so we can PROVE same-vs-different key. (c) Could the key simply be invalid/expired on Deepgram\'s side? Note that as a possibility but focus on the read-vs-save-path question since the user believes the key is valid.',
      'Write to ' + OUT + '/401-readpath.md the full call chain (start_capture -> ... -> DeepgramConfig.api_key) with file:line, the save-path for comparison, and a decisive non-secret fingerprint diagnostic to add. Return the thin schema.',
    ].join('\n'),
  },
  {
    key: 'flux-catalog',
    bug: false,
    prompt: [
      'INVESTIGATE (read-only) why Deepgram FLUX models never appear in the model picker in the Tauri app at ' + ROOT + '.',
      'EVIDENCE: parse_deepgram_stt_model_catalog (src-tauri/src/commands.rs), the fixed catalog, and provider-registry (src-tauri/crates/provider-registry/src/lib.rs) contain ZERO flux entries (confirmed via grep). So even with the "Load models" button, flux is never listed. Deepgram flux is a real turn-based streaming model on v2/listen.',
      'INVESTIGATE: (a) does Deepgram\'s /v1/models API (which list_deepgram_models_cmd calls) even RETURN flux models, or are they only documented separately? Fetch https://developers.deepgram.com/reference/manage/models/list and https://developers.deepgram.com/reference/speech-to-text/listen-flux via the hyperresearch fetch CLI. (b) if the API returns them, why does parse_deepgram_stt_model_catalog filter them out (read the parse fn + its filter logic at commands.rs, grep "streaming"/"architecture"/"filter")? (c) if the API does NOT return flux, they need to be added to the FIXED catalog / registry as curated entries — specify the exact model id(s) (e.g. flux-general-en) and where to add them (provider-registry lib.rs deepgram fixed_model_catalog + regenerate).',
      'Write to ' + OUT + '/flux-catalog.md: whether the API returns flux, the parse/filter logic (file:line), and the exact fix to surface flux in the picker (parse change and/or curated catalog entries with correct ids). Return the thin schema.',
    ].join('\n'),
  },
  {
    key: 'tauri-testing',
    bug: false,
    prompt: [
      'RESEARCH + SURVEY (read-only) how to properly TEST frontend<->Rust IPC communication in this Tauri v2 app at ' + ROOT + ', to answer "is the provider bug in the frontend or the backend?"',
      'Fetch and study the official Tauri testing guide: https://v2.tauri.app/develop/tests/ (and its subpages mockIPC / WebDriver) via ' + "/home/codeseys/.local/share/uv/tools/hyperresearch/bin/hyperresearch" + ' fetch. Key techniques to extract: (1) @tauri-apps/api/mocks mockIPC() for unit-testing frontend invoke() calls without a backend, (2) tauri::test mock Runtime / #[cfg(test)] command tests on the Rust side, (3) WebDriver/tauri-driver for true end-to-end.',
      'THEN survey what THIS repo already does: how do the existing vitest tests mock invoke (grep for mockedInvoke, vi.fn, mockIPC in src/ — they clearly mock invoke since a Rust-only change couldn\'t fail a frontend test earlier)? Do any tests exercise the REAL command contract (args in, shape out) so a frontend/backend arg-name mismatch would be caught? Is there an integration/e2e layer at all?',
      'THE POINT: the provider settings flow (select model, load models, test connection, save credential) spans frontend invoke() -> Rust #[tauri::command]. A silent arg-name or shape mismatch (camelCase vs snake_case, wrong param) fails at runtime but passes mocked unit tests. Recommend a concrete testing strategy: (a) a shared contract/type source so FE and Rust agree on command names + arg shapes, (b) where mockIPC unit tests fit, (c) where a real tauri::test or WebDriver e2e test would catch the FE<->BE mismatches we\'ve been hitting.',
      'Write to ' + OUT + '/tauri-testing.md: the extracted Tauri testing techniques (with doc citations), this repo\'s current test approach + its blind spot, and a recommended layered test strategy for the provider IPC surface. Return the thin schema.',
    ].join('\n'),
  },
  {
    key: 'arch-survey',
    bug: false,
    prompt: [
      'SURVEY (read-only) the CURRENT provider abstraction in the Tauri app at ' + ROOT + ' to inform a formalized per-provider contract design. The user wants: each provider has the SAME BASE functionality (select model, load models, test MODEL connection [not generic], view model info via hover) + its OWN ADVANCED settings (openrouter vs cerebras vs bedrock vs deepgram vs openai vs assemblyai vs transcribe differ).',
      'Map the CURRENT state across ASR + LLM + TTS + realtime providers:',
      '1. BACKEND traits/contracts: CloudAsrRequestConfig (src-tauri/src/asr/cloud.rs:37), TtsProvider (src-tauri/src/tts/mod.rs:419), MoonshineStreamingAdapter — what do they cover, what is NOT unified? Is there any per-provider capability descriptor beyond the provider-registry ProviderDescriptor (src-tauri/crates/provider-registry/src/lib.rs)?',
      '2. The provider-registry ProviderDescriptor: what fields exist (model_catalog policy, model_catalog_command, settings_groups, credentials, etc.)? This is the closest thing to a contract — how could it be extended to carry: base capabilities (has model select, has load-models, has test-model-connection, has model-info), advanced-settings schema per provider, and model metadata (mode/endpoint/languages/features/description for the hover box)?',
      '3. TEST-CONNECTION: the test_*_connection commands (commands.rs ~8582 test_deepgram_connection, 8602 soniox, 8622 cerebras, 8658 assemblyai, 8132 test_cloud_asr_connection, 8839 openrouter) — confirm they are GENERIC (probe /v1/models or auth) and do NOT test the SELECTED MODEL specifically. The user explicitly wants test-MODEL-connection. What would a per-model connection test look like per provider (e.g. Deepgram: open the actual listen WS with the selected model; OpenAI: a tiny transcription/chat with the model)?',
      '4. FRONTEND: how are provider settings panels structured today (src/components/*ProviderSettings.tsx, settings/*Panel.tsx, useSettingsController.tsx)? Is there shared base UI + per-provider advanced, or is it ad-hoc per provider? Where would a formalized "ProviderSettingsBase + advanced slot" live?',
      'Write to ' + OUT + '/arch-survey.md a structured map: current contracts, gaps vs the desired base+advanced model, and the seams where a formalized contract (Rust trait(s) + extended descriptor + shared FE base component) should be introduced. Return the thin schema.',
    ].join('\n'),
  },
  {
    key: 'hover-metadata',
    bug: false,
    prompt: [
      'DESIGN INPUT (read-only) for a model-info hover feature in the Tauri app at ' + ROOT + '. The user validated: a custom combobox where hovering each model row shows a popover of that model\'s features; metadata fields = mode/endpoint, languages, features, description; applies to ALL providers with graceful-empty.',
      'Confirm the current seam: ProviderModelCatalogItem is defined in TWO places — src-tauri/src/commands.rs (~209: {id, display_name, is_default}) and src-tauri/crates/provider-registry/src/lib.rs (~464). The list_*_models fetch fns already PARSE rich metadata then DISCARD it (Soniox reads transcription_mode; Deepgram /v1/models returns languages/features/versions). The frontend ModelCatalogPicker (src/components/ModelCatalogPicker.tsx) + ModelCatalogField (added in PR #30) render only id/name.',
      'INVESTIGATE + SPECIFY: (a) for EACH provider with a list_*_models_cmd (deepgram, soniox, llm.api/openai-compat, cerebras, openrouter), what metadata does its models API actually return that we currently discard (read each fetch/parse fn in commands.rs)? (b) what curated metadata would static/fixed providers need (assemblyai, aws_transcribe, gladia, speechmatics, openai_realtime, etc.)? (c) the exact new fields to add to BOTH ProviderModelCatalogItem definitions (mode, endpoint, languages, features, description — all Option/skip_serializing_if_none) + how the merge works (live-API value wins over curated fallback). (d) the frontend: native <select><option> CANNOT host a hover popover (browsers show only a plain title tooltip) — so specify a custom ARIA combobox/listbox component (keyboard nav + hover popover) replacing ModelCatalogPicker, OR a details-panel fallback. Note a11y requirements.',
      'Write to ' + OUT + '/hover-metadata.md: the per-provider metadata availability table, the exact struct field additions + merge rule, and the frontend combobox component spec (a11y, hover popover, graceful-empty). Return the thin schema.',
    ].join('\n'),
  },
]

const FINDING_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['lane', 'headline', 'notePath', 'severity', 'keyFacts'],
  properties: {
    lane: { type: 'string' },
    headline: { type: 'string', description: 'one-sentence bottom line' },
    notePath: { type: 'string' },
    severity: { type: 'string', enum: ['none', 'low', 'medium', 'high', 'critical'] },
    isBug: { type: 'boolean' },
    rootCause: { type: 'string', description: 'for bug lanes: the mechanism, with file:line; else empty' },
    fix: { type: 'string', description: 'the concrete minimal fix or design recommendation' },
    keyFacts: { type: 'array', maxItems: 10, items: { type: 'string' } },
  },
}

phase('Investigate')
const investigated = await parallel(
  LANES.map((l) => () =>
    agent(l.prompt, { label: 'investigate:' + l.key, phase: 'Investigate', schema: FINDING_SCHEMA, model: 'opus' })
      .then((r) => (r ? { ...r, _lane: l } : null))
  )
)
const found = investigated.filter(Boolean)

// ---------- Phase 2: adversarially verify the two live-bug lanes ----------
phase('Verify')
const VERDICT_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['lane', 'verdict', 'note'],
  properties: {
    lane: { type: 'string' },
    verdict: { type: 'string', enum: ['confirmed', 'downgraded', 'refuted'] },
    note: { type: 'string', description: 'one paragraph: what settles it, with the doc/code citation' },
  },
}
const bugLanes = found.filter((f) => f._lane.bug)
const verdicts = await parallel(
  bugLanes.map((f) => () => {
    const vp = [
      'ADVERSARIALLY VERIFY (read-only) a root-cause claim for the app at ' + ROOT + '. Try to REFUTE it; default to skepticism.',
      'LANE: ' + f.lane,
      'CLAIMED root cause: ' + (f.rootCause || f.headline),
      'CLAIMED fix: ' + f.fix,
      'The full note is at ' + f.notePath + ' — read it. Re-read the cited code (and Deepgram docs where relevant, via the hyperresearch fetch CLI) and decide: does the root cause hold, and is the fix correct + sufficient? For the flux-regression lane specifically, verify the EXACT model string the frontend writes vs what the validator accepts vs what Deepgram accepts — a wrong assumption there means a wrong fix. For the 401 lane, verify whether the read-path truly could read a different credential than the save-path, or whether it is simply an invalid key (be honest if the evidence cannot distinguish without the proposed fingerprint diagnostic).',
      'Return the thin schema: verdict (confirmed/downgraded/refuted) + a one-paragraph note citing what settles it.',
    ].join('\n')
    return agent(vp, { label: 'verify:' + f.lane, phase: 'Verify', schema: VERDICT_SCHEMA, model: 'opus' })
      .then((v) => ({ lane: f.lane, verdict: v }))
  })
)

// ---------- Phase 3: synthesize ADR + design + fix plan ----------
phase('Design')
const summary = found.map((f) => ({
  lane: f.lane, headline: f.headline, severity: f.severity, isBug: !!f._lane.bug,
  rootCause: f.rootCause || '', fix: f.fix, notePath: f.notePath,
  verdict: (verdicts.find((v) => v && v.lane === f.lane) || {}).verdict || 'n/a',
}))
log('Investigated ' + found.length + ' lanes; verified ' + verdicts.filter(Boolean).length + ' bug lanes.')

const SYNTH_SCHEMA = {
  type: 'object', additionalProperties: false,
  required: ['adrPath', 'planPath', 'p0_fixes', 'flux_verdict', 'fourofourVerdict', 'contractSummary', 'testingSummary', 'workstreams'],
  properties: {
    adrPath: { type: 'string' },
    planPath: { type: 'string' },
    p0_fixes: { type: 'array', items: { type: 'string' }, maxItems: 10 },
    flux_verdict: { type: 'string', description: 'is the flux clobber the validator or the frontend or both, and the fix' },
    fourofourVerdict: { type: 'string', description: 'the 401 read-path finding + the fingerprint diagnostic to add' },
    contractSummary: { type: 'string', description: 'the formalized provider-contract design in 3-5 sentences' },
    testingSummary: { type: 'string', description: 'the FE<->Rust testing strategy in 2-4 sentences' },
    workstreams: {
      type: 'array', maxItems: 12,
      items: {
        type: 'object', additionalProperties: false,
        required: ['title', 'priority', 'effort', 'independent'],
        properties: {
          title: { type: 'string' },
          priority: { type: 'string', enum: ['P0', 'P1', 'P2'] },
          effort: { type: 'string', enum: ['S', 'M', 'L'] },
          independent: { type: 'boolean', description: 'true if it can land without depending on another workstream' },
        },
      },
    },
  },
}

const synthPrompt = [
  'You are the lead architect synthesizing a design for the Tauri app at ' + ROOT + '. Read-only on code; you WRITE two docs.',
  'Read ALL six lane notes in ' + OUT + '/ (flux-regression.md, 401-readpath.md, flux-catalog.md, tauri-testing.md, arch-survey.md, hover-metadata.md). Spot-check cited files.',
  '',
  'Machine summary of lanes + adversarial verdicts (JSON):',
  JSON.stringify(summary, null, 2),
  '',
  'PRODUCE TWO documents:',
  '1. An ADR at ' + ROOT + '/docs/plans/2026-07-03-provider-contract-adr.md — "Formalized Provider Contract". Cover: the problem (ad-hoc per-provider code causing the flux clobber, generic-not-model test-connection, discarded metadata, FE<->BE mismatches); the DECISION (a) a Rust ProviderCapability/contract layer + extended ProviderDescriptor carrying base-capabilities + advanced-settings schema + model metadata; (b) test-MODEL-connection per provider (not generic); (c) a shared frontend ProviderSettingsBase component with a per-provider advanced slot; (d) the model-metadata hover-combobox (mode/endpoint/languages/features/description, live-over-curated merge, ARIA combobox); (e) a layered FE<->Rust test strategy (shared contract types + mockIPC unit + a real command-contract test) per the Tauri testing guide. Include a base-vs-advanced capability matrix across providers (deepgram/openai_realtime/soniox/assemblyai/aws_transcribe/gladia/speechmatics/ + llm cerebras/openrouter/bedrock/api + tts aura).',
  '2. A prioritized fix+build plan at ' + ROOT + '/docs/plans/2026-07-03-provider-arch-plan.md — P0 = the live bugs (flux clobber regression [confirmed], 401 read-path fingerprint diagnostic, flux catalog gap); P1 = the contract formalization + test-model-connection + hover-metadata; P2 = full rollout across all providers. Each item: file:line targets, effort S/M/L, and independence (parallelizable vs ordered). ONLY include bug fixes whose verify verdict was confirmed (note any refuted/downgraded as such and exclude from P0).',
  '',
  'Then return the thin schema (adrPath, planPath, p0_fixes, flux_verdict, fourofourVerdict, contractSummary, testingSummary, workstreams[]).',
].join('\n')

const design = await agent(synthPrompt, { label: 'synthesize-adr', phase: 'Design', schema: SYNTH_SCHEMA, model: 'opus', effort: 'high' })
return { lanes: summary, verdicts: verdicts.filter(Boolean), design }
