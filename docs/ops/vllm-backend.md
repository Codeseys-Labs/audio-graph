# vLLM Backend Runbook

AudioGraph uses the existing OpenAI-compatible LLM provider to talk to vLLM.
Keep vLLM as a separate long-running backend process and point AudioGraph at
its `/v1` endpoint.

## Recommended Local Server

AudioGraph does not embed vLLM. Run vLLM as a separate OpenAI-compatible
server on a Linux/CUDA host, WSL2, or a remote GPU box, then point AudioGraph's
generic LLM provider at the server's `/v1` route.

The streaming speech-to-speech reference currently defaults to
`Qwen/Qwen2.5-1.5B-Instruct`, which is a good low-latency starting point for
agent turns and entity/relation extraction. A practical local command is:

```bash
vllm serve Qwen/Qwen2.5-1.5B-Instruct \
  --host 127.0.0.1 \
  --port 8000 \
  --dtype auto \
  --served-model-name Qwen/Qwen2.5-1.5B-Instruct \
  --max-model-len 8192 \
  --generation-config vllm \
  --enable-prefix-caching \
  --gpu-memory-utilization 0.85
```

For an 8B-class reasoning model, use the same shape with a model your GPU can
hold, for example:

```bash
vllm serve meta-llama/Llama-3.1-8B-Instruct \
  --host 127.0.0.1 \
  --port 8000 \
  --dtype auto \
  --served-model-name meta-llama/Llama-3.1-8B-Instruct \
  --max-model-len 8192 \
  --generation-config vllm \
  --enable-prefix-caching \
  --gpu-memory-utilization 0.85
```

Other reasonable 7B/8B starting points include `mistralai/Mistral-7B-Instruct-v0.3`
for a smaller general-purpose model or any organization-approved 8B instruct
checkpoint that fits your GPU memory budget. Keep `--served-model-name` equal
to the model string you intend to enter in AudioGraph Settings unless you need
a shorter local alias.

Current vLLM defaults use a hybrid of CUDA graphs and eager execution for
performance. Do **not** pass `--enforce-eager` for the normal low-latency
AudioGraph path; that flag forces eager-mode PyTorch and disables CUDA graphs.
Use `--enforce-eager` only when debugging compatibility issues or memory
fragmentation.

If you want an API key on the local server, add `--api-key token-abc123` and
put `token-abc123` in AudioGraph's LLM API key field.

In AudioGraph settings:

- LLM provider: `OpenAI-compatible API`
- Endpoint URL: `http://localhost:8000/v1`
- Model: `Qwen/Qwen2.5-1.5B-Instruct`
- API key: blank unless the vLLM server was started with `--api-key`

The settings UI includes a `vLLM local preset` button that fills the endpoint
and model fields.

For a remote GPU server, bind vLLM to an external interface and protect it with
network policy plus `--api-key`:

```bash
vllm serve meta-llama/Llama-3.1-8B-Instruct \
  --host 0.0.0.0 \
  --port 8000 \
  --api-key "$VLLM_API_KEY" \
  --dtype auto \
  --served-model-name meta-llama/Llama-3.1-8B-Instruct \
  --max-model-len 8192 \
  --generation-config vllm \
  --enable-prefix-caching \
  --gpu-memory-utilization 0.85
```

Then set AudioGraph's endpoint to `http://<server>:8000/v1` or the HTTPS
reverse-proxy URL. Do not expose an unauthenticated vLLM server to a shared
network.

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
- The AudioGraph model field must match the vLLM served model name. If you pass
  `--served-model-name`, use that exact string in Settings.
- Reuse stable system prompts and graph-context prefixes so vLLM prefix caching
  can help repeated entity extraction and agent requests.
- Use structured outputs for entity/relation extraction when the endpoint is
  the local vLLM preset. The backend falls back to JSON mode if the server
  rejects the structured-output request.
- StreamingInput-style prompt overlap from the S2S project remains a future
  Python/vLLM-side optimization. AudioGraph should first keep a supervised Rust
  pipeline and call a warm OpenAI-compatible vLLM server.
- On Windows, prefer WSL2 with NVIDIA GPU passthrough or a remote Linux GPU
  server. Treat native Windows vLLM launchers as unsupported experiments until
  the vLLM project documents them as supported.

## References

- AudioGraph provider matrix: [`../adr/0003-speech-to-speech-agent-provider-matrix.md`](../adr/0003-speech-to-speech-agent-provider-matrix.md)
- Streaming/prefill tradeoff: [`../adr/0012-turn-gated-incremental-prefill-llama-cpp.md`](../adr/0012-turn-gated-incremental-prefill-llama-cpp.md)
- vLLM Rust frontend research: [`../research/vllm-rust-frontend.md`](../research/vllm-rust-frontend.md)
