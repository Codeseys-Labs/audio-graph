# vLLM Backend Runbook

AudioGraph uses the existing OpenAI-compatible LLM provider to talk to vLLM.
Keep vLLM as a separate long-running backend process and point AudioGraph at
its `/v1` endpoint.

## Recommended Local Server

The streaming speech-to-speech project currently defaults to
`Qwen/Qwen2.5-1.5B-Instruct`, which is a good low-latency starting point for
agent turns and entity/relation extraction.

```bash
vllm serve Qwen/Qwen2.5-1.5B-Instruct \
  --dtype auto \
  --generation-config vllm \
  --enable-prefix-caching \
  --gpu-memory-utilization 0.85
```

If you want an API key on the local server, add `--api-key token-abc123` and
put `token-abc123` in AudioGraph's LLM API key field.

In AudioGraph settings:

- LLM provider: `OpenAI-compatible API`
- Endpoint URL: `http://localhost:8000/v1`
- Model: `Qwen/Qwen2.5-1.5B-Instruct`
- API key: blank unless the vLLM server was started with `--api-key`

The settings UI includes a `vLLM local preset` button that fills the endpoint
and model fields.

## Warmup

Warm vLLM before a live session so CUDA graph capture, model load, and prompt
template setup do not land on the first transcript segment:

```bash
curl http://localhost:8000/v1/chat/completions \
  -H "Content-Type: application/json" \
  -d '{
    "model": "Qwen/Qwen2.5-1.5B-Instruct",
    "messages": [{"role": "user", "content": "Reply with ok."}],
    "max_tokens": 4,
    "temperature": 0
  }'
```

If the server uses `--api-key`, add:

```bash
-H "Authorization: Bearer token-abc123"
```

## Pipeline Notes

- Keep `rsac` capture, ASR, diarization, transcript joining, and graph mutation
  in the Rust/Tauri backend.
- Use vLLM as the LLM executor behind the OpenAI-compatible API client.
- Reuse stable system prompts and graph-context prefixes so vLLM prefix caching
  can help repeated entity extraction and agent requests.
- Use structured outputs for entity/relation extraction when the endpoint is
  the local vLLM preset. The backend falls back to JSON mode if the server
  rejects the structured-output request.
- StreamingInput-style prompt overlap from the S2S project remains a future
  Python/vLLM-side optimization. AudioGraph should first keep a supervised Rust
  pipeline and call a warm OpenAI-compatible vLLM server.

