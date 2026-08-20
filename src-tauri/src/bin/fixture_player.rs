//! Thin binary wrapper. All real logic lives in
//! `audio_graph::audio::fixture_player` so it stays unit-testable via
//! `cargo test -p audio-graph --lib` without opening a real audio device.
//! See that module's docs for CLI usage.

fn main() {
    let argv: Vec<String> = std::env::args().collect();
    std::process::exit(audio_graph::audio::fixture_player::run(argv));
}
