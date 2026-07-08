import { describe, expect, it } from "vitest";
import { ALLOWED_CREDENTIAL_KEYS } from "../types";
import { GENERATED_PROVIDER_REGISTRY } from "./providerRegistry";

describe("GENERATED_PROVIDER_REGISTRY", () => {
  it("keeps provider ids unique", () => {
    const ids = GENERATED_PROVIDER_REGISTRY.map((provider) => provider.id);

    expect(new Set(ids).size).toBe(ids.length);
  });

  it("carries the MVP-scoped ui_selectable axis through the generator", () => {
    // MVP scoping (audio-graph-ad56): the generated registry must surface the
    // dedicated `ui_selectable` axis, and only these ids may be true. This is
    // the TS-side twin of the Rust `ui_selectable_set_matches_mvp_scoping_decision`
    // test — it guards against a generator/serde regression dropping the field
    // or a re-scoping that forgets to regenerate.
    const selectable = GENERATED_PROVIDER_REGISTRY.filter(
      (provider) => provider.ui_selectable,
    ).map((provider) => provider.id);

    expect(new Set(selectable)).toEqual(
      new Set([
        "asr.deepgram",
        "llm.local_llama",
        "llm.api",
        "llm.cerebras",
        "llm.sambanova",
        "llm.openrouter",
        "llm.aws_bedrock",
        "llm.mistralrs",
        "tts.none",
        "tts.deepgram_aura",
      ]),
    );

    // Deferred-but-implemented providers keep a truthful status; only their UI
    // selection is withheld.
    for (const id of [
      "asr.local_whisper",
      "asr.api",
      "asr.aws_transcribe",
      "asr.assemblyai",
      "asr.sherpa_onnx",
      "asr.openai_realtime",
    ]) {
      const provider = GENERATED_PROVIDER_REGISTRY.find((p) => p.id === id);
      expect(provider?.status).toBe("implemented");
      expect(provider?.ui_selectable).toBe(false);
    }

    // A non-implemented provider is never selectable.
    for (const provider of GENERATED_PROVIDER_REGISTRY) {
      if (provider.status !== "implemented") {
        expect(provider.ui_selectable).toBe(false);
      }
    }
  });

  it("includes the planned streaming STT candidates", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    for (const id of [
      "asr.soniox",
      "asr.speechmatics",
      "asr.elevenlabs_scribe",
      "asr.revai",
    ]) {
      const provider = providersById.get(id);

      expect(provider?.stage).toBe("asr");
      expect(provider?.status).toBe("planned");
      expect(provider?.supports_streaming).toBe(true);
      expect(provider?.supports_partial_revisions).toBe(true);
      expect(provider?.event_semantics).toBe(
        id === "asr.revai"
          ? "transcript_partial_final"
          : "transcript_partial_final_turns",
      );
      expect(provider?.audio_input?.pipeline_format).toMatchObject({
        sample_rate_hz: 16_000,
        channels: 1,
        frame_format: "f32",
      });
      expect(provider?.audio_input?.provider_format).toMatchObject({
        sample_rate_hz: 16_000,
        channels: 1,
        frame_format: "pcm_s16_le",
      });
      expect(provider?.audio_input?.transport_encoding).toBe(
        "web_socket_binary",
      );
      expect(provider?.lifecycle).toMatchObject({
        auth: "saved_api_key",
        session: "long_lived_web_socket",
        keepalive: "provider_specific",
        close: "provider_specific",
      });
      expect(provider?.privacy).toMatchObject({
        data_leaves_device: true,
        data_boundary: "vendor_cloud",
        data_classes_sent: ["audio", "provider_configuration"],
        retention_policy: "unknown",
        training_policy: "unknown",
        deletion_policy: "unknown",
        enterprise_no_training_config: "unknown",
        sensitive_error_policy: "audio_graph_redacted",
      });
      expect(provider?.privacy.data_classes_returned).toContain(
        "transcript_text",
      );
      expect(provider?.privacy.health_check_data_classes).toEqual(
        id === "asr.soniox"
          ? ["credential_auth", "provider_configuration"]
          : [],
      );
    }

    expect(providersById.get("asr.gladia")).toMatchObject({
      stage: "asr",
      status: "planned",
      transport: "rest_init_web_socket",
      default_model: "solaria-1",
      supports_streaming: true,
      supports_partial_revisions: true,
      supports_diarization: false,
      event_semantics: "transcript_partial_final_turns",
      audio_input: {
        transport_encoding: "web_socket_binary",
        pipeline_format: {
          sample_rate_hz: 16_000,
          channels: 1,
          frame_format: "f32",
        },
        provider_format: {
          sample_rate_hz: 16_000,
          channels: 1,
          frame_format: "pcm_s16_le",
        },
      },
      lifecycle: {
        auth: "saved_api_key",
        session: "long_lived_web_socket",
        keepalive: "client_audio_stream",
        close: "provider_close_message_then_close_frame",
      },
    });
  });

  it("includes non-selectable roadmap watch STT candidates with source and auth metadata", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    expect(providersById.get("asr.xai_grok_stt")).toMatchObject({
      stage: "asr",
      status: "watch",
      transport: "web_socket",
      credential_keys: [],
      supports_diarization: true,
      lifecycle: {
        auth: "saved_api_key",
        session: "long_lived_web_socket",
      },
      audio_input: {
        supports_multichannel: false,
        attribution: {
          mode: "speaker",
          max_channels: 1,
          requires_source_native_channels: false,
          channel_label_semantics: "none",
          capability_source_url:
            "https://docs.x.ai/developers/rest-api-reference/inference/voice#speech-to-text---streaming",
          capability_source_date: "2026-06-26",
        },
      },
      roadmap: {
        source_url: "https://artificialanalysis.ai/speech-to-text/streaming",
        source_date: "2026-06-25",
        auth_schema: "required_not_wired",
      },
    });
    expect(providersById.get("asr.xai_grok_stt")?.status).not.toBe(
      "implemented",
    );
    expect(
      providersById.get("asr.xai_grok_stt")?.roadmap?.not_selectable_reason,
    ).toMatch(/credential schema and runtime adapter are not wired/i);

    expect(providersById.get("asr.nvidia_nemotron_asr")).toMatchObject({
      stage: "asr",
      status: "enterprise_watch",
      transport: "grpc_bidi",
      credential_keys: [],
      lifecycle: {
        auth: "saved_api_key",
        session: "grpc_bidirectional_stream",
      },
      roadmap: {
        source_url: "https://artificialanalysis.ai/speech-to-text/streaming",
        source_date: "2026-06-25",
        auth_schema: "required_not_wired",
      },
      enterprise: {
        endpoint_modes: ["custom_endpoint", "private_endpoint"],
        packaging: ["protobuf_grpc_client", "sidecar_process"],
      },
    });
    expect(providersById.get("asr.nvidia_nemotron_asr")?.status).not.toBe(
      "implemented",
    );

    expect(providersById.get("asr.cartesia_ink2")).toMatchObject({
      stage: "asr",
      status: "watch",
      transport: "web_socket",
      credential_keys: [],
      event_semantics: "transcript_partial_final_turns",
      roadmap: {
        source_url: "https://artificialanalysis.ai/speech-to-text/streaming",
        source_date: "2026-06-25",
        auth_schema: "required_not_wired",
      },
    });
    expect(providersById.get("asr.cartesia_ink2")?.status).not.toBe(
      "implemented",
    );
    expect(
      providersById.get("asr.cartesia_ink2")?.health_check_command,
    ).toBeUndefined();
    expect(
      providersById.get("asr.cartesia_ink2")?.model_catalog_command,
    ).toBeUndefined();

    expect(providersById.get("asr.alibaba_qwen3_asr_flash")).toMatchObject({
      stage: "asr",
      status: "enterprise_watch",
      transport: "web_socket",
      credential_keys: [],
      audio_input: {
        transport_encoding: "web_socket_json_base64",
      },
      privacy: {
        data_boundary: "user_configured_region",
      },
      roadmap: {
        source_url: "https://artificialanalysis.ai/speech-to-text/streaming",
        source_date: "2026-06-25",
        auth_schema: "required_not_wired",
      },
      enterprise: {
        endpoint_modes: ["default_region", "custom_endpoint"],
        packaging: ["system_certificates"],
      },
    });
    expect(
      providersById.get("asr.alibaba_qwen3_asr_flash")?.health_check_command,
    ).toBeUndefined();
    expect(providersById.get("asr.alibaba_qwen3_asr_flash")?.status).not.toBe(
      "implemented",
    );
  });

  it("declares event semantics for every ASR provider", () => {
    const missingAsrEventSemantics = GENERATED_PROVIDER_REGISTRY.filter(
      (provider) => provider.stage === "asr" && !provider.event_semantics,
    ).map((provider) => provider.id);

    expect(missingAsrEventSemantics).toEqual([]);
    expect(
      GENERATED_PROVIDER_REGISTRY.find((provider) => provider.id === "asr.api")
        ?.event_semantics,
    ).toBe("transcript_final_only");
    expect(
      GENERATED_PROVIDER_REGISTRY.find(
        (provider) => provider.id === "asr.deepgram",
      )?.event_semantics,
    ).toBe("transcript_partial_final_turns");
  });

  it("declares remote model commands for provider-owned catalogs", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    expect(providersById.get("asr.deepgram")).toMatchObject({
      model_catalog: "remote_command",
      model_catalog_command: "list_deepgram_models_cmd",
      health_check_command: "test_deepgram_connection",
    });
    expect(providersById.get("asr.soniox")).toMatchObject({
      status: "planned",
      model_catalog: "remote_command",
      model_catalog_command: "list_soniox_models_cmd",
      health_check_command: "test_soniox_connection",
      default_model: "stt-rt-v5",
    });
    expect(providersById.get("llm.openrouter")).toMatchObject({
      model_catalog: "remote_command",
      model_catalog_command: "list_openrouter_models_cmd",
    });
    expect(providersById.get("llm.cerebras")).toMatchObject({
      model_catalog: "remote_command",
      model_catalog_command: "list_cerebras_models_cmd",
      health_check_command: "test_cerebras_connection_cmd",
      default_model: "gpt-oss-120b",
    });
  });

  it("declares audio input contracts for audio-consuming providers", () => {
    const missingAudioInputs = GENERATED_PROVIDER_REGISTRY.filter(
      (provider) =>
        (provider.stage === "asr" || provider.stage === "realtime_agent") &&
        !provider.audio_input,
    ).map((provider) => provider.id);

    expect(missingAudioInputs).toEqual([]);

    for (const provider of GENERATED_PROVIDER_REGISTRY.filter(
      (item) => item.stage === "asr" || item.stage === "realtime_agent",
    )) {
      expect(provider.audio_input?.pipeline_format).toEqual({
        sample_rate_hz: 16_000,
        channels: 1,
        frame_format: "f32",
      });
      expect(provider.audio_input?.provider_format.channels).toBe(1);
      expect(provider.audio_input?.supports_multichannel).toBe(false);
      expect(provider.audio_input?.attribution.max_channels).toBe(1);
      expect(
        provider.audio_input?.attribution.requires_source_native_channels,
      ).toBe(false);
      expect(provider.audio_input?.attribution.channel_label_semantics).toBe(
        "none",
      );
      expect(provider.audio_input?.attribution.accepted_layouts).toEqual([
        "mono",
      ]);
      expect(provider.audio_input?.attribution.mode).not.toContain("channel");
    }
  });

  it("exposes provider speaker attribution without claiming source-native channels", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    expect(
      providersById.get("asr.deepgram")?.audio_input?.attribution,
    ).toMatchObject({
      mode: "speaker",
      max_channels: 1,
      requires_source_native_channels: false,
      channel_label_semantics: "none",
    });
    expect(
      providersById.get("asr.gladia")?.audio_input?.attribution,
    ).toMatchObject({
      mode: "none",
      max_channels: 1,
      requires_source_native_channels: false,
    });
    expect(
      providersById.get("asr.xai_grok_stt")?.audio_input?.attribution,
    ).toMatchObject({
      mode: "speaker",
      max_channels: 1,
      requires_source_native_channels: false,
      channel_label_semantics: "none",
    });
  });

  it("declares cargo feature requirements for local runtime providers", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    expect(providersById.get("asr.local_whisper")?.required_features).toEqual([
      "local-ml",
      "asr-whisper",
    ]);
    expect(providersById.get("asr.sherpa_onnx")?.required_features).toEqual([
      "sherpa-streaming",
    ]);
    expect(providersById.get("llm.local_llama")?.required_features).toEqual([
      "local-ml",
      "llm-llama",
    ]);
    expect(providersById.get("llm.mistralrs")?.required_features).toEqual([
      "local-ml",
      "llm-mistralrs",
    ]);
    expect(
      providersById.get("diarization.sortformer")?.required_features,
    ).toEqual(["diarization"]);
    expect(
      providersById.get("diarization.clustering")?.required_features,
    ).toEqual(["diarization-clustering"]);

    for (const provider of GENERATED_PROVIDER_REGISTRY.filter(
      (item) => item.transport === "local",
    )) {
      if (provider.id === "tts.none") continue;
      expect(provider.required_features.length).toBeGreaterThan(0);
    }
  });

  it("captures provider-specific wire audio formats", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    expect(
      providersById.get("asr.api")?.audio_input?.provider_format,
    ).toMatchObject({
      sample_rate_hz: 16_000,
      frame_format: "wav_pcm_s16_le",
    });
    expect(providersById.get("asr.api")?.audio_input?.transport_encoding).toBe(
      "multipart_wav",
    );

    expect(
      providersById.get("asr.deepgram")?.audio_input?.transport_encoding,
    ).toBe("web_socket_binary");

    expect(
      providersById.get("asr.assemblyai")?.audio_input?.transport_encoding,
    ).toBe("web_socket_binary");

    expect(
      providersById.get("asr.openai_realtime")?.audio_input?.provider_format,
    ).toMatchObject({
      sample_rate_hz: 24_000,
      frame_format: "pcm_s16_le",
    });
    expect(
      providersById.get("asr.openai_realtime")?.audio_input?.adapter_resamples,
    ).toBe(true);
  });

  it("declares settings groups consistently", () => {
    for (const provider of GENERATED_PROVIDER_REGISTRY) {
      expect(provider.settings_groups).toContain("basic");
      expect(new Set(provider.settings_groups).size).toBe(
        provider.settings_groups.length,
      );

      if (
        provider.credential_keys.length > 0 ||
        provider.health_check_command
      ) {
        expect(provider.settings_groups).toContain("health");
      }

      if (
        provider.model_catalog !== "none" &&
        provider.model_catalog !== "user_supplied"
      ) {
        expect(provider.settings_groups).toContain("model_catalog");
      }
    }
  });

  it("keeps complex provider controls in advanced settings groups", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    for (const id of [
      "asr.aws_transcribe",
      "asr.deepgram",
      "asr.assemblyai",
      "asr.soniox",
      "asr.gladia",
      "asr.speechmatics",
      "asr.elevenlabs_scribe",
      "asr.revai",
      "asr.xai_grok_stt",
      "asr.nvidia_nemotron_asr",
      "asr.inworld_stt1",
      "asr.smallest_pulse",
      "asr.gradium_stt",
      "asr.mistral_voxtral_realtime",
      "asr.alibaba_qwen3_asr_flash",
      "asr.cartesia_ink2",
      "llm.cerebras",
      "llm.openrouter",
      "llm.aws_bedrock",
      "tts.deepgram_aura",
      "realtime_agent.gemini_live",
      "realtime_agent.openai_realtime",
    ]) {
      expect(providersById.get(id)?.settings_groups).toContain("advanced");
    }
  });

  it("exports fixed provider model catalogs when the backend owns the list", () => {
    const aura = GENERATED_PROVIDER_REGISTRY.find(
      (provider) => provider.id === "tts.deepgram_aura",
    );

    expect(aura?.model_catalog).toBe("fixed");
    expect(aura?.default_model).toBe("aura-asteria-en");
    // The Aura catalog spans Aura-1 + Aura-2 + non-English voices; assert
    // on presence of key entries rather than a magic count so catalog
    // growth doesn't rot this test.
    expect(aura?.fixed_model_catalog?.length ?? 0).toBeGreaterThan(0);
    expect(aura?.fixed_model_catalog?.[0]).toEqual({
      id: "aura-asteria-en",
      display_name: "Asteria (en, female)",
      is_default: true,
    });
    expect(aura?.fixed_model_catalog?.map((item) => item.id)).toContain(
      "aura-zeus-en",
    );
    expect(aura?.fixed_model_catalog?.map((item) => item.id)).toContain(
      "aura-2-thalia-en",
    );
  });

  it("declares local diarization runtime model dependencies", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    expect(providersById.get("diarization.sortformer")).toMatchObject({
      stage: "diarization",
      status: "planned",
      transport: "local",
      model_catalog: "local_files",
      default_model: "diar_streaming_sortformer_4spk-v2.onnx",
      lifecycle: {
        auth: "none",
        session: "local_streaming_runtime",
        keepalive: "none",
        close: "drop_runtime",
      },
      privacy: {
        data_leaves_device: false,
        data_boundary: "local_only",
      },
      local_models: [
        {
          model_id: "diar_streaming_sortformer_4spk-v2.onnx",
          kind: "file",
          required_files: ["diar_streaming_sortformer_4spk-v2.onnx"],
        },
      ],
    });

    expect(providersById.get("diarization.clustering")).toMatchObject({
      stage: "diarization",
      status: "planned",
      transport: "local",
      model_catalog: "local_files",
      lifecycle: {
        auth: "none",
        session: "local_streaming_runtime",
        keepalive: "none",
        close: "drop_runtime",
      },
      privacy: {
        data_leaves_device: false,
        data_boundary: "local_only",
      },
      local_models: [
        {
          model_id: "sherpa-onnx-pyannote-segmentation-3-0",
          kind: "directory",
          required_files: ["model.onnx", "model.int8.onnx"],
        },
        {
          model_id: "nemo_en_titanet_small.onnx",
          kind: "file",
          required_files: ["nemo_en_titanet_small.onnx"],
        },
      ],
    });
  });

  it("declares lifecycle and privacy metadata consistently", () => {
    for (const provider of GENERATED_PROVIDER_REGISTRY) {
      const hasFlexibleEnterpriseAuth =
        provider.enterprise !== undefined &&
        provider.credential_keys.length === 0;
      const hasRoadmapAuthWithoutWiredSchema =
        provider.roadmap?.auth_schema === "required_not_wired";

      if (
        provider.credential_keys.length > 0 ||
        hasFlexibleEnterpriseAuth ||
        hasRoadmapAuthWithoutWiredSchema
      ) {
        expect(provider.lifecycle.auth).not.toBe("none");
      } else {
        expect(provider.lifecycle.auth).toBe("none");
      }

      if (provider.transport === "local") {
        expect(provider.privacy).toMatchObject({
          data_leaves_device: false,
          data_boundary: "local_only",
          data_classes_sent: [],
          cloud_transfer_acknowledgement_required: false,
          retention_policy: "not_applicable",
          training_policy: "not_applicable",
          deletion_policy: "not_applicable",
          sensitive_error_policy: "local_only",
        });
      } else {
        expect(provider.privacy.data_leaves_device).toBe(true);
        expect(provider.privacy.data_boundary).not.toBe("local_only");
        expect(provider.privacy.data_classes_sent.length).toBeGreaterThan(0);
        expect(provider.privacy.cloud_transfer_acknowledgement_required).toBe(
          true,
        );
        // Honesty rule: a provider only asserts a non-"unknown" retention/
        // training/deletion claim when it carries a sourced official policy
        // URL, and that URL must be an https link paired with a source date.
        // Providers with no sourced policy stay fully "unknown".
        const policies = [
          provider.privacy.retention_policy,
          provider.privacy.training_policy,
          provider.privacy.deletion_policy,
        ];
        if (provider.privacy.policy_url === undefined) {
          for (const policy of policies) {
            expect(policy).toBe("unknown");
          }
          expect(provider.privacy.policy_url_source_date).toBeUndefined();
          expect(provider.privacy.subprocessors_url).toBeUndefined();
        } else {
          expect(provider.privacy.policy_url).toMatch(/^https:\/\//);
          expect(typeof provider.privacy.policy_url_source_date).toBe("string");
          // At least one policy field is sourced (not "unknown").
          expect(
            policies.some((policy) => policy === "provider_docs_linked"),
          ).toBe(true);
          if (provider.privacy.subprocessors_url !== undefined) {
            expect(provider.privacy.subprocessors_url).toMatch(/^https:\/\//);
          }
        }
        // enterprise_no_training_config has no sourced value yet for any
        // provider, so it must never imply enterprise-only support.
        expect(provider.privacy.enterprise_no_training_config).not.toBe(
          "enterprise_only",
        );
        expect(provider.privacy.sensitive_error_policy).toBe(
          "audio_graph_redacted",
        );
      }

      const hasProviderProbe =
        provider.health_check_command !== undefined ||
        provider.model_catalog_command !== undefined;
      if (provider.transport !== "local") {
        expect(provider.privacy.health_check_data_classes).toEqual(
          hasProviderProbe ? ["credential_auth", "provider_configuration"] : [],
        );
      }

      if (
        provider.transport === "web_socket" ||
        provider.transport === "rest_init_web_socket"
      ) {
        expect(provider.lifecycle.session).toBe("long_lived_web_socket");
        expect(provider.lifecycle.close).not.toMatch(
          /^(noop|request_completes|drop_runtime)$/,
        );
      }

      if (provider.transport === "grpc_bidi") {
        expect(provider.lifecycle.session).toBe("grpc_bidirectional_stream");
        expect(provider.audio_input?.transport_encoding).toBe("grpc_streaming");
      }

      if (provider.transport === "sdk_native") {
        expect(provider.lifecycle.session).toBe("native_sdk_conversation");
        expect(provider.audio_input?.transport_encoding).toBe("sdk_native");
      }
    }
  });

  it("captures implemented provider lifecycle behavior", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    expect(providersById.get("asr.deepgram")?.lifecycle).toMatchObject({
      keepalive: "client_control_message",
      close: "end_stream_then_close_frame",
    });
    expect(providersById.get("asr.assemblyai")?.lifecycle.close).toBe(
      "terminate_message_then_close_frame",
    );
    expect(providersById.get("asr.aws_transcribe")?.lifecycle).toMatchObject({
      auth: "aws_credential_chain",
      session: "aws_streaming_sdk",
      close: "aws_end_stream",
    });
    expect(
      providersById.get("realtime_agent.gemini_live")?.lifecycle.auth,
    ).toBe("google_api_key_or_service_account");
    expect(
      providersById.get("realtime_agent.gemini_live")?.privacy,
    ).toMatchObject({
      data_leaves_device: true,
      data_boundary: "provider_account_boundary",
      data_classes_sent: expect.arrayContaining(["audio", "graph_context"]),
      data_classes_returned: expect.arrayContaining([
        "generated_audio",
        "generated_text",
      ]),
      retention_policy: "unknown",
      training_policy: "unknown",
      deletion_policy: "unknown",
    });
    expect(providersById.get("tts.deepgram_aura")?.lifecycle).toMatchObject({
      keepalive: "client_control_message",
      close: "provider_close_message_then_close_frame",
    });
    expect(providersById.get("llm.api")?.privacy.data_boundary).toBe(
      "user_configured_endpoint",
    );
    expect(providersById.get("llm.cerebras")?.privacy.data_boundary).toBe(
      "vendor_cloud",
    );
  });

  it("only references accepted credential keys", () => {
    const allowedKeys = new Set(ALLOWED_CREDENTIAL_KEYS);

    const unknownCredentialKeys = GENERATED_PROVIDER_REGISTRY.flatMap(
      (provider) =>
        provider.credential_keys
          .filter((key) => !allowedKeys.has(key))
          .map((key) => `${provider.id}:${key}`),
    );

    expect(unknownCredentialKeys).toEqual([]);
  });

  it("exposes sourced data-boundary policy links (or honest unknowns)", () => {
    const providersById = new Map(
      GENERATED_PROVIDER_REGISTRY.map((provider) => [provider.id, provider]),
    );

    // Sourced providers: Settings can show a dated official policy link.
    for (const id of [
      "asr.openai_realtime",
      "realtime_agent.openai_realtime",
      "asr.deepgram",
      "tts.deepgram_aura",
      "asr.aws_transcribe",
      "llm.aws_bedrock",
      "asr.assemblyai",
    ]) {
      const privacy = providersById.get(id)?.privacy;
      expect(privacy, id).toBeDefined();
      expect(privacy?.policy_url, id).toMatch(/^https:\/\//);
      expect(typeof privacy?.policy_url_source_date, id).toBe("string");
    }

    // Deepgram trains on a sample of customer audio by default (Model
    // Improvement Program) and publishes a subprocessors list.
    const deepgram = providersById.get("asr.deepgram")?.privacy;
    expect(deepgram?.training_policy).toBe("provider_docs_linked");
    expect(deepgram?.subprocessors_url).toMatch(/^https:\/\//);

    // AssemblyAI sources retention + deletion but NOT training (its policy is
    // silent on training), so training stays an honest "unknown".
    const assemblyai = providersById.get("asr.assemblyai")?.privacy;
    expect(assemblyai?.retention_policy).toBe("provider_docs_linked");
    expect(assemblyai?.deletion_policy).toBe("provider_docs_linked");
    expect(assemblyai?.training_policy).toBe("unknown");

    // Soniox has no verified official policy: fully unknown, no links.
    const soniox = providersById.get("asr.soniox")?.privacy;
    expect(soniox?.policy_url).toBeUndefined();
    expect(soniox?.policy_url_source_date).toBeUndefined();
    expect(soniox?.subprocessors_url).toBeUndefined();
    expect(soniox?.retention_policy).toBe("unknown");
    expect(soniox?.training_policy).toBe("unknown");
    expect(soniox?.deletion_policy).toBe("unknown");

    // The policy links never expose secrets (no credential material in URLs).
    for (const provider of GENERATED_PROVIDER_REGISTRY) {
      for (const url of [
        provider.privacy.policy_url,
        provider.privacy.subprocessors_url,
      ]) {
        if (url !== undefined) {
          expect(url).not.toMatch(/api[_-]?key|secret|token|password/i);
        }
      }
    }
  });
});
