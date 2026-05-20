# Deepgram Aura Streaming TTS — Research for Rust `TtsProvider` Impl

> **Verification status (re-checked 2026-05-19):** A dedicated re-verification
> pass was attempted. All external research tools (`context7`, `exa`, `tavily`,
> `deepwiki`, `WebFetch`, and outbound `curl` via Bash) were denied in that
> session, so **no live confirmation was performed**. Confidence remains at
> the Jan 2026 knowledge-cutoff level. Implementer must run this checklist
> from a network-enabled session before merging the impl PR; treat any
> conflict with a live doc as authoritative and update this file:
>
> 1. <https://developers.deepgram.com/docs/tts-websocket> — confirm endpoint
>    path, header form, and exact spelling of every client `type` field
>    (`Speak`, `Flush`, `Clear`, `Close`, `KeepAlive`) and server `type`
>    field (`Metadata`, `Flushed`, `Cleared`, `Warning`, `Error`).
> 2. <https://developers.deepgram.com/reference/transform-text-to-speech-websocket>
>    — confirm query-param table in §1.1, especially `bit_rate` vs `bitrate`
>    (Deepgram has used both spellings historically).
> 3. <https://developers.deepgram.com/docs/tts-models> + `GET /v1/models?type=tts`
>    — refresh Aura-2 voice catalog in §4.
> 4. <https://deepgram.com/pricing> — refresh per-character price and
>    concurrency cap in §6.
> 5. `gh repo view deepgram/deepgram-rust-sdk` — check whether streaming
>    `speak` has landed since this report; if yes, prefer it over a
>    hand-rolled `tokio-tungstenite` impl.

## Executive summary (60-second briefing)

- **Endpoint:** `wss://api.deepgram.com/v1/speak?model=<voice>&encoding=<fmt>&sample_rate=<hz>` with `Authorization: Token <api_key>` upgrade header — same auth shape as `asr/deepgram.rs`.
- **Frame model:** client sends JSON control frames (`Speak`, `Flush`, `Clear`, `Close`, `KeepAlive`); server replies with **binary audio frames** plus JSON status frames (`Metadata`, `Flushed`, `Cleared`, `Warning`, `Error`).
- **Latency lever:** stream text in ≥ ~50-char clause-shaped chunks, send `Flush` at clause boundaries to force synthesis without closing the session. Don't wait for full sentences if a clause boundary (`,`/`;`/`—`) is hit.
- **Barge-in:** `{"type":"Clear"}` aborts in-flight synthesis server-side; expect a `Cleared` ack and ~100–300 ms of trailing audio still on the wire — caller must drop frames received after `Clear` was sent.
- **Output formats:** `linear16` (8/16/24/32/48 kHz), `mulaw`/`alaw` (8 kHz), `mp3` (32k/48k bitrate), `opus`, `flac`, `aac`. For lowest latency to a CPAL sink, use `linear16` @ 24 kHz mono; for telephony, `mulaw` @ 8 kHz.

## 1. Protocol details

### 1.1 Connection

```
GET wss://api.deepgram.com/v1/speak
    ?model=aura-2-thalia-en
    &encoding=linear16
    &sample_rate=24000
    &container=none
Headers:
    Authorization: Token <DEEPGRAM_API_KEY>
    Sec-WebSocket-Protocol: token, <DEEPGRAM_API_KEY>   # alt subprotocol auth
```

Key query params:

| Param         | Values                                                         | Notes |
|---------------|----------------------------------------------------------------|-------|
| `model`       | `aura-2-thalia-en`, `aura-asteria-en`, …                       | Voice **and** model (see §4). |
| `encoding`    | `linear16`, `mulaw`, `alaw`, `mp3`, `opus`, `flac`, `aac`      | `linear16` = headerless i16 LE PCM. |
| `sample_rate` | 8000, 16000, 24000 (default for Aura-2), 32000, 48000          | Ignored for compressed codecs that fix their own rate. |
| `container`   | `none`, `wav`                                                  | `none` recommended for streaming sinks. |
| `bit_rate`    | 32000 / 48000 (mp3), 32000–256000 (aac, opus)                  | Codec-specific. |

### 1.2 Client → server frames (all text-frame JSON)

```jsonc
{"type":"Speak","text":"Hello, world. "}      // append text to synthesis queue
{"type":"Flush"}                              // synthesize everything queued, emit Flushed
{"type":"Clear"}                              // abort in-flight synthesis, drop queued text
{"type":"Close"}                              // graceful shutdown (server emits final Metadata)
{"type":"KeepAlive"}                          // idle ping; server otherwise drops at ~10s idle
```

`Speak.text` is plain UTF-8 — **no SSML** on Aura-2 streaming (SSML is REST-only). Whitespace inside `text` is significant for prosody; trailing space helps the synthesizer commit a word boundary.

### 1.3 Server → client frames

- **Binary frame** — raw audio bytes in the requested `encoding`. Frame size is **not** chunk-aligned to text chunks; expect 4–32 KB per frame.
- **`{"type":"Metadata", "request_id":"…", "model_name":"aura-2-thalia-en", "model_version":"…"}`** — sent once after the upgrade.
- **`{"type":"Flushed", "sequence_id":N}`** — every committed `Flush` produces one `Flushed` after the **last** audio byte for that flush has been pushed.
- **`{"type":"Cleared", "sequence_id":N}`** — confirms `Clear` was applied.
- **`{"type":"Warning", "warn_code":"…", "description":"…"}`** — non-fatal (e.g. text too long).
- **`{"type":"Error", "err_code":"…", "description":"…"}`** — fatal; server closes after.

### 1.4 Keepalive

Same 10 s idle window as Listen. The constants in `asr/deepgram.rs`
(`KEEPALIVE_INTERVAL_SECS = 4`, `KEEPALIVE_PAYLOAD = r#"{"type":"KeepAlive"}"#`)
port over verbatim.

## 2. Latency strategy

Aura-2 first-byte latency is ~150–250 ms once the server has ≥ ~40 chars of
context. Flushing policy:

1. Buffer LLM tokens into a `String` per session.
2. Send a `Speak` when buffer length ≥ 80 chars **or** (≥ 25 chars and the
   buffer ends in a clause-final char `. ! ? ; : , — \n`).
3. Send a `Flush` at upstream `EndOfTurn` — fastest way to force synthesis
   without closing the WS.
4. Never send 1–2 char `Speak`s — JSON frames have fixed RTT cost; batch
   to ≥ 25 chars.

Deepgram's voice-agent demos (`deepgram-js-sdk/examples/streaming-tts`)
chunk on sentence boundaries with a 40-char minimum and call `flush()`
only at agent-turn end.

## 3. Cancellation / barge-in semantics

- `{"type":"Clear"}` discards (a) any text queued but not yet started and (b) any in-flight synthesis. Server replies `{"type":"Cleared","sequence_id":…}`.
- **Trailing audio:** ~100–300 ms of already-synthesized PCM may arrive on the wire after your `Clear` write returns. The TTS provider impl **must** track a monotonic `clear_epoch: AtomicU64` and tag every emitted audio frame with the epoch at the time of receipt; the player drops frames whose epoch is older than the latest `Clear`.
- The same WS stays open across `Clear` — re-prime with new `Speak` immediately. No reconnect needed.
- For barge-in from VAD: emit `Clear` the instant `SpeechStarted` fires from the ASR side; the speak-aloud loop in `audio-graph-92c7` should drain its CPAL output ring buffer in lockstep.

## 4. Voice + model catalog

Query the live list from `GET https://api.deepgram.com/v1/models?type=tts`
(REST, `Authorization: Token …`). As of late 2025 the Aura-2 family ships
40+ English voices plus growing multilingual coverage; the legacy Aura-1
voices remain GA. Naming: `aura-{generation}-{name}-{lang}`.

| Model id                | Gen    | Lang  | Persona / use case                |
|-------------------------|--------|-------|-----------------------------------|
| `aura-2-thalia-en`      | Aura-2 | en-US | Default, conversational female    |
| `aura-2-andromeda-en`   | Aura-2 | en-US | Warm female, customer support     |
| `aura-2-helena-en`      | Aura-2 | en-US | Energetic female                  |
| `aura-2-apollo-en`      | Aura-2 | en-US | Confident male, narration         |
| `aura-2-arcas-en`       | Aura-2 | en-US | Natural male                      |
| `aura-2-orion-en`       | Aura-2 | en-US | Approachable male                 |
| `aura-asteria-en`       | Aura-1 | en-US | Legacy default female             |
| `aura-luna-en`          | Aura-1 | en-US | Polite female                     |
| `aura-stella-en`        | Aura-1 | en-US | Friendly female                   |
| `aura-zeus-en`          | Aura-1 | en-US | Authoritative male                |

Tradeoffs: Aura-2 has higher MOS but ~30–50 ms more first-byte latency than
Aura-1. For a sub-200 ms speak-aloud loop on a slow link, prefer
`aura-asteria-en`; otherwise default to `aura-2-thalia-en`.

## 5. Rust patterns

There is **no official Deepgram Rust SDK for TTS**. The community
[`deepgram-rust-sdk`](https://github.com/deepgram/deepgram-rust-sdk) crate
covers Listen/Manage but, as of v0.6, has only a REST `speak()` helper —
the streaming WS endpoint is unimplemented. Build directly on
`tokio-tungstenite + serde_json`, mirroring `src-tauri/src/asr/deepgram.rs`.

### 5.1 Connect (matches `asr/deepgram.rs::open_ws`)

```rust
use http::Request;
use tokio_tungstenite::{connect_async, tungstenite::handshake::client::generate_key};

async fn open_tts_ws(cfg: &AuraConfig) -> Result<(WsWriter, WsReader), String> {
    let url = format!(
        "wss://api.deepgram.com/v1/speak?model={}&encoding=linear16&sample_rate=24000&container=none",
        cfg.model
    );
    let req = Request::builder()
        .uri(&url)
        .header("Host", "api.deepgram.com")
        .header("Authorization", format!("Token {}", cfg.api_key))
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", generate_key())
        .body(())
        .map_err(|e| format!("build req: {e}"))?;
    let (ws, _) = connect_async(req).await.map_err(|e| format!("connect: {e}"))?;
    Ok(ws.split())
}
```

### 5.2 Reader loop with Clear-epoch tagging

```rust
loop {
    tokio::select! {
        Some(msg) = reader.next() => match msg {
            Ok(Message::Binary(pcm)) => {
                let epoch = clear_epoch.load(Ordering::SeqCst);
                let _ = event_tx.send(AuraEvent::Audio { pcm, epoch });
            }
            Ok(Message::Text(t)) => {
                tracing::trace!(target = "tts.deepgram", frame = %t);
                match serde_json::from_str::<ServerFrame>(&t) {
                    Ok(ServerFrame::Flushed { sequence_id }) =>
                        { let _ = event_tx.send(AuraEvent::Flushed { sequence_id }); }
                    Ok(ServerFrame::Cleared { sequence_id }) =>
                        { let _ = event_tx.send(AuraEvent::Cleared { sequence_id }); }
                    Ok(ServerFrame::Error { description, .. }) => {
                        tracing::error!(target = "tts.deepgram", "{description}");
                        break;
                    }
                    _ => {}
                }
            }
            Ok(Message::Close(_)) | Err(_) => break,
            _ => {}
        }
    }
}
```

### 5.3 Speak-and-flush from the LLM token stream

```rust
pub fn push_token(&self, tok: &str) -> Result<(), String> {
    self.buf.lock().push_str(tok);
    let mut buf = self.buf.lock();
    let should_flush_speak = buf.len() >= 80
        || (buf.len() >= 25 && buf.ends_with(['.', '!', '?', ',', ';', ':', '\n']));
    if should_flush_speak {
        let text = std::mem::take(&mut *buf);
        let frame = serde_json::json!({ "type": "Speak", "text": text });
        self.text_tx.send(TtsCmd::Json(frame.to_string()))
            .map_err(|_| "tts channel closed".to_string())?;
    }
    Ok(())
}

pub fn end_of_turn(&self) {
    let _ = self.text_tx.send(TtsCmd::Json(r#"{"type":"Flush"}"#.into()));
}

pub fn barge_in(&self) {
    self.clear_epoch.fetch_add(1, Ordering::SeqCst);
    let _ = self.text_tx.send(TtsCmd::Json(r#"{"type":"Clear"}"#.into()));
}
```

## 6. Pricing / quota (verify before launch)

- Aura-2 list price: **$0.030 per 1K characters** (pay-as-you-go); Aura-1 is $0.015 / 1K. Prepaid Growth tiers reduce ~30%.
- **Concurrency:** default 5 concurrent WS sessions per project; raise via Deepgram support.
- **Rate limit:** ~480 requests/min per API key on streaming endpoints.
- **Max session duration:** no documented hard cap, but sessions idle >10 s without `KeepAlive` are dropped. Long-running agents should treat the WS as ephemeral and reconnect on `Close`.

## References

External Deepgram docs (verify URLs — not fetched live this session):

- Aura streaming TTS reference — <https://developers.deepgram.com/docs/tts-websocket>
- Speak streaming API reference — <https://developers.deepgram.com/reference/text-to-speech-api/speak-streaming>
- Aura-2 model overview — <https://developers.deepgram.com/docs/tts-models>
- Voice catalog — <https://developers.deepgram.com/docs/tts-feature-overview>
- Models endpoint — <https://developers.deepgram.com/reference/get-models>
- Pricing — <https://deepgram.com/pricing>
- Voice-agent latency study — <https://deepgram.com/learn/voice-agent-latency>
- Rate limits — <https://developers.deepgram.com/docs/working-with-api-rate-limits>
- Community Rust SDK (REST only) — <https://github.com/deepgram/deepgram-rust-sdk>
- Streaming TTS JS example — <https://github.com/deepgram/deepgram-js-sdk/tree/main/examples>
- `tokio-tungstenite` — <https://docs.rs/tokio-tungstenite>

Internal references:

- `/mnt/e/CS/github/audio-graph/src-tauri/src/asr/deepgram.rs` — Listen-side reference impl (auth, runtime model, KeepAlive cadence, reconnect, channel topology to copy).
- `/mnt/e/CS/github/audio-graph/src-tauri/src/asr/mod.rs` — provider trait shape.
- `/mnt/e/CS/github/audio-graph/src-tauri/src/asr/cloud.rs` — sibling cloud provider for cross-reference.
