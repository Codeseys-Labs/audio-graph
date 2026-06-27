use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use crossbeam_channel::Receiver;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::assemblyai::{self, AssemblyAIEvent};
use super::deepgram::{self, DeepgramEvent};
use super::openai_realtime::{self, OpenAiRealtimeEvent};

#[derive(Debug, Deserialize)]
struct EventFixture {
    schema_version: u32,
    id: String,
    provider: EventFixtureProvider,
    messages: Vec<EventFixtureMessage>,
    expected_events: Vec<Value>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum EventFixtureProvider {
    Assemblyai,
    Deepgram,
    OpenaiRealtime,
}

#[derive(Debug, Deserialize)]
struct EventFixtureMessage {
    raw: String,
    #[serde(default)]
    expected_session_ready: Option<bool>,
}

#[test]
fn deepgram_server_event_fixture_replays_ordered_events() {
    run_fixture("deepgram/server_events.json");
}

#[test]
fn assemblyai_server_event_fixture_replays_ordered_events() {
    run_fixture("assemblyai/server_events.json");
}

#[test]
fn openai_realtime_server_event_fixture_replays_ordered_events() {
    run_fixture("openai_realtime/server_events.json");
}

fn run_fixture(relative_path: &str) {
    let fixture = load_fixture(relative_path);
    assert_eq!(
        fixture.schema_version, 1,
        "{relative_path}: unsupported schema version for {}",
        fixture.id
    );

    let actual_events = match fixture.provider {
        EventFixtureProvider::Assemblyai => replay_assemblyai(&fixture, relative_path),
        EventFixtureProvider::Deepgram => replay_deepgram(&fixture, relative_path),
        EventFixtureProvider::OpenaiRealtime => replay_openai_realtime(&fixture, relative_path),
    };

    assert_eq!(
        actual_events, fixture.expected_events,
        "{relative_path}: serialized event stream"
    );
}

fn load_fixture(relative_path: &str) -> EventFixture {
    let path = fixture_path(relative_path);
    let body = fs::read_to_string(&path).unwrap_or_else(|error| {
        panic!(
            "failed to read ASR event fixture {}: {error}",
            path.display()
        )
    });
    serde_json::from_str(&body).unwrap_or_else(|error| {
        panic!(
            "failed to parse ASR event fixture {}: {error}",
            path.display()
        )
    })
}

fn fixture_path(relative_path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("asr")
        .join(relative_path)
}

fn replay_assemblyai(fixture: &EventFixture, relative_path: &str) -> Vec<Value> {
    let (tx, rx) = crossbeam_channel::unbounded::<AssemblyAIEvent>();
    let mut events = Vec::new();
    for (index, message) in fixture.messages.iter().enumerate() {
        assert_no_session_ready_expectation(relative_path, index, message);
        assemblyai::handle_server_message(&message.raw, &tx);
        events.extend(drain_serialized_events(&rx, relative_path));
    }
    events
}

fn replay_deepgram(fixture: &EventFixture, relative_path: &str) -> Vec<Value> {
    let (tx, rx) = crossbeam_channel::unbounded::<DeepgramEvent>();
    let mut events = Vec::new();
    for (index, message) in fixture.messages.iter().enumerate() {
        assert_no_session_ready_expectation(relative_path, index, message);
        deepgram::handle_server_message(&message.raw, &tx);
        events.extend(drain_serialized_events(&rx, relative_path));
    }
    events
}

fn replay_openai_realtime(fixture: &EventFixture, relative_path: &str) -> Vec<Value> {
    let (tx, rx) = crossbeam_channel::unbounded::<OpenAiRealtimeEvent>();
    let mut accumulator = HashMap::new();
    let mut events = Vec::new();
    for (index, message) in fixture.messages.iter().enumerate() {
        let session_ready =
            openai_realtime::handle_server_message(&message.raw, &tx, &mut accumulator);
        if let Some(expected) = message.expected_session_ready {
            assert_eq!(
                session_ready, expected,
                "{relative_path}: message {index} OpenAI session readiness"
            );
        }
        events.extend(drain_serialized_events(&rx, relative_path));
    }
    events
}

fn assert_no_session_ready_expectation(
    relative_path: &str,
    index: usize,
    message: &EventFixtureMessage,
) {
    assert!(
        message.expected_session_ready.is_none(),
        "{relative_path}: message {index} session readiness is only valid for OpenAI Realtime fixtures"
    );
}

fn drain_serialized_events<T>(rx: &Receiver<T>, relative_path: &str) -> Vec<Value>
where
    T: Serialize,
{
    rx.try_iter()
        .map(|event| {
            serde_json::to_value(event).unwrap_or_else(|error| {
                panic!("{relative_path}: failed to serialize event: {error}")
            })
        })
        .collect()
}
