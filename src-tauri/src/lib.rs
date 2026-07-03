//! AudioGraph — Real-time audio capture → transcription → knowledge graph
//!
//! This is the Tauri backend for the AudioGraph application.
//! Module structure:
//!   state       — AppState definition (Arc<Mutex<...>>)
//!   commands    — Tauri IPC command handlers
//!   events      — Event name constants and payload types
//!   audio       — Audio capture manager + processing pipeline
//!   asr         — Automatic speech recognition (whisper-rs)
//!   diarization — Speaker diarization (pyannote-rs)
//!   graph       — Temporal knowledge graph (petgraph)
//!   models      — Model management and downloading
//!   persistence — File-based persistence (transcripts + knowledge graph)
//!   sessions    — Session metadata index (~/.audiograph/sessions.json)

// Rust 2024 edition (B21): keep the drop-order / if-let-scope compatibility lints
// VISIBLE so any *new* tail-expression temporary with a significant Drop
// (MutexGuard / RwLockGuard / last Sender / pinned future) is surfaced for review.
// `warn` not `deny`: the 24 existing flagged sites were audited and proven benign
// (their guards protect independent state; the 2024 earlier-release is harmless —
// the full test suite passes under 2024 on Windows + Linux, macOS via CI), so we
// don't force churn-rewrites; we just don't want a *silent* new hazard.
#![warn(tail_expr_drop_order, if_let_rescope)]

// ADR-0017: the parakeet Sortformer diarization (`diarization`) and the
// sherpa-onnx clustering diarization (`diarization-clustering`) each link their
// own ONNX Runtime and cannot co-link in one binary. Fail fast at compile time
// rather than emit confusing duplicate-symbol linker errors.
#[cfg(all(feature = "diarization", feature = "diarization-clustering"))]
compile_error!(
    "features `diarization` (parakeet Sortformer) and `diarization-clustering` \
     (sherpa-onnx) both link ONNX Runtime and are mutually exclusive — enable only one."
);

pub mod aec_vad;
pub mod analytics;
pub mod asr;
pub mod audio;
pub mod aws_util;
pub mod commands;
pub mod config;
pub mod converse;
pub mod crash_handler;
pub mod credentials;
pub mod diarization;
pub mod error;
pub mod events;
pub mod fs_util;
pub mod gemini;
pub mod graph;
pub mod llm;
pub mod logging;
pub mod models;
pub mod ontology;
pub mod openai_realtime;
pub mod persistence;
pub mod playback;
pub mod projection_eval;
pub mod projection_llm;
pub mod projection_scheduler;
pub mod projections;
pub mod promotion;
pub mod provider_registry;
pub mod sessions;
pub mod settings;
pub mod speak_aloud;
pub mod speech;
pub mod state;
pub mod tts;
pub mod user_data;

#[cfg(test)]
mod aec_vad_fixtures;
#[cfg(test)]
mod source_separation_fixtures;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use state::AppState;
use tauri::Manager;

/// Budget for joining the graph-autosave daemon during graceful shutdown. The
/// loop polls its stop flag every ~500ms, so this only has to cover one poll
/// tick plus one in-flight save; kept short so a wedged fs write can't hang the
/// quit — an un-flushed tail is acceptable, a hung quit is not.
const AUTOSAVE_JOIN_TIMEOUT: Duration = Duration::from_secs(3);

/// Per-writer flush budget at exit. Mirrors the rotation shutdown budget in
/// [`crate::state`]; on timeout the writer thread is left detached (it exits on
/// its own when the disk unsticks) rather than blocking the quit.
const WRITER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);

/// Budget for the bounded Sentry flush on `RunEvent::ExitRequested` (the
/// window-close path that fires before the terminal `Exit`). Short so it can't
/// stall the quit if the network is wedged — an unsent tail is acceptable, a
/// hung close is not; the `Exit` handler still gets its own (longer) flush.
const SENTRY_EXIT_REQUEST_FLUSH_TIMEOUT: Duration = Duration::from_millis(1000);

/// Cheap `Arc` clones of the shared [`AppState`] fields the graceful-shutdown
/// teardown needs. Captured before `app_state` is moved into `.manage(...)` and
/// owned by the `move` `RunEvent` closure so teardown can run without a live
/// `State<'_, AppState>` handle at Exit.
struct ShutdownHandles {
    session_id: Arc<RwLock<String>>,
    autosave_stop: Arc<AtomicBool>,
    graph_autosave_thread: Arc<Mutex<Option<std::thread::JoinHandle<()>>>>,
    knowledge_graph: Arc<Mutex<crate::graph::temporal::TemporalKnowledgeGraph>>,
    transcript_buffer: Arc<RwLock<std::collections::VecDeque<state::TranscriptSegment>>>,
    transcript_writer: Arc<Mutex<Option<crate::persistence::TranscriptWriter>>>,
    transcript_event_writer: Arc<Mutex<Option<crate::persistence::TranscriptEventWriter>>>,
    projection_event_writer: Arc<Mutex<Option<crate::persistence::ProjectionEventWriter>>>,
    is_capturing: Arc<RwLock<bool>>,
    is_transcribing: Arc<AtomicBool>,
    is_gemini_active: Arc<RwLock<bool>>,
    is_converse_active: Arc<RwLock<bool>>,
    is_openai_realtime_active: Arc<RwLock<bool>>,
}

/// Bounded, best-effort graceful teardown run once from `RunEvent::Exit`.
///
/// Order matters:
///  1. Flip every "mode active" running flag to false so worker/audio threads
///     wind their loops down and stop producing new state.
///  2. Signal the autosave daemon to stop and join it (bounded), so there is no
///     concurrent writer for the session graph file...
///  3. ...then perform ONE final synchronous graph save. A clean File→Quit
///     otherwise loses up to ~30s of derived-graph state to the missed tick.
///  4. Flush + shut down the transcript / event / projection writers (bounded
///     per writer) so a clean quit gets the same durable flush a rotation does.
///  5. Flush the Sentry transport (bounded) — `static` guards don't `Drop` at
///     normal termination, so this is the intentional flush-at-quit hook.
///
/// Every step is individually timeout-bounded; the function NEVER blocks the
/// exit indefinitely, even on a wedged disk or network.
fn graceful_shutdown(h: &ShutdownHandles) {
    log::info!("Graceful shutdown: begin");

    // 1. Wind down worker/audio threads by clearing the mode-active flags. This
    // is cooperative: the capture/transcribe/S2S loops observe these and exit.
    // `is_transcribing` is an AtomicBool (lock-free flag the speech processor
    // polls); the rest are RwLock<bool> read by their respective mode loops.
    h.is_transcribing.store(false, Ordering::SeqCst);
    for (flag, name) in [
        (&h.is_capturing, "is_capturing"),
        (&h.is_gemini_active, "is_gemini_active"),
        (&h.is_converse_active, "is_converse_active"),
        (&h.is_openai_realtime_active, "is_openai_realtime_active"),
    ] {
        match flag.write() {
            Ok(mut g) => *g = false,
            Err(poisoned) => *poisoned.into_inner() = false,
        }
        log::debug!("Graceful shutdown: cleared {name}");
    }

    // 2. Signal the autosave daemon to stop, then join it (bounded) so the
    // final save below is the sole writer for the session graph file.
    h.autosave_stop.store(true, Ordering::SeqCst);
    let autosave_handle = match h.graph_autosave_thread.lock() {
        Ok(mut g) => g.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    };
    if let Some(handle) = autosave_handle {
        // JoinHandle::join has no timeout in std; join on a watchdog thread and
        // wait on a bounded channel so a wedged tick can't hang the quit. On
        // timeout the thread is left detached (it exits on its own next poll).
        let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
        if std::thread::Builder::new()
            .name("autosave-join".to_string())
            .spawn(move || {
                let _ = handle.join();
                let _ = done_tx.send(());
            })
            .is_ok()
        {
            let joined = done_rx.recv_timeout(AUTOSAVE_JOIN_TIMEOUT).is_ok();
            log::info!(
                "Graceful shutdown: autosave thread joined={} (timeout_ms={})",
                joined,
                AUTOSAVE_JOIN_TIMEOUT.as_millis()
            );
        }
    }

    // 3. One final synchronous graph save + stats refresh (best-effort,
    // bounded by the single fs write inside). Safe: the autosave thread is
    // gone, so this is the only writer for the session graph file.
    crate::persistence::autosave_final_save(
        &h.session_id,
        &h.knowledge_graph,
        &h.transcript_buffer,
    );

    // 4. Flush + shut down the persistence writers (bounded per writer). Take
    // the owned writer out of its slot so `shutdown_with_timeout` can consume
    // it; the slot is left `None` (we are exiting, nothing respawns).
    if let Some(writer) = take_writer(&h.transcript_writer) {
        let flushed = writer.shutdown_with_timeout(WRITER_SHUTDOWN_TIMEOUT);
        log::info!("Graceful shutdown: transcript writer flushed={}", flushed);
    }
    if let Some(writer) = take_writer(&h.transcript_event_writer) {
        let flushed = writer.shutdown_with_timeout(WRITER_SHUTDOWN_TIMEOUT);
        log::info!(
            "Graceful shutdown: transcript event writer flushed={}",
            flushed
        );
    }
    if let Some(writer) = take_writer(&h.projection_event_writer) {
        let flushed = writer.shutdown_with_timeout(WRITER_SHUTDOWN_TIMEOUT);
        log::info!(
            "Graceful shutdown: projection event writer flushed={}",
            flushed
        );
    }

    // 5. Flush the Sentry transport (bounded; no-op when analytics is off).
    let sentry_flushed = crate::analytics::flush_on_exit();
    log::info!("Graceful shutdown: sentry flushed={}", sentry_flushed);

    log::info!("Graceful shutdown: complete");
}

/// Take the owned writer out of a `Arc<Mutex<Option<W>>>` slot, recovering from
/// a poisoned lock (we are shutting down; a poisoned writer mutex should not
/// prevent the flush attempt).
fn take_writer<W>(slot: &Arc<Mutex<Option<W>>>) -> Option<W> {
    match slot.lock() {
        Ok(mut g) => g.take(),
        Err(poisoned) => poisoned.into_inner().take(),
    }
}

/// Initialize and run the Tauri application.
pub fn run() {
    // Install the global panic hook before anything else so panics during
    // Tauri startup (builder, state init, plugin load) get captured too.
    crash_handler::install();

    // Install the global tee logger (stderr + optional file sink). Starts
    // with file logging on (Archive mode) so startup is always captured; the
    // setup hook below applies the user's persisted preference.
    crate::logging::init();

    let app_state = AppState::new();
    let initial_session_id = app_state.current_session_id();

    // Register this session in the sessions index (~/.audiograph/sessions.json).
    // Also marks any prior "active" sessions as "crashed" so the UI can
    // distinguish clean shutdowns from crashes.
    if let Err(e) = sessions::register_session(&initial_session_id) {
        log::warn!("Failed to register session in index: {}", e);
    }

    // Surface any persisted token usage from the most-recent prior session
    // so operators can confirm persistence survived the restart. The
    // frontend will wire this to the UI in a later loop; for now it's a
    // log breadcrumb + the `get_session_usage` command is registered below.
    {
        let prior = sessions::load_index();
        if let Some(most_recent) = prior.iter().find(|s| s.id != initial_session_id) {
            let usage = sessions::usage::load_usage(&most_recent.id);
            log::info!(
                "Session restored from prior run {}: {} turns, {} total tokens",
                most_recent.id,
                usage.turns,
                usage.total
            );
        }
    }

    // Spawn graph auto-save background thread (saves every 30s, also refreshes
    // session index stats: segment/speaker/entity counts). The thread reads
    // the current session_id via the shared Arc<RwLock<String>> on each tick
    // so in-process rotation via `new_session_cmd` takes effect without a
    // respawn. It also polls `autosave_stop` so the graceful-shutdown path can
    // stop it and take over the final save (see the RunEvent::Exit handler).
    {
        let handle = persistence::spawn_graph_autosave(
            app_state.session_id.clone(),
            app_state.knowledge_graph.clone(),
            app_state.transcript_buffer.clone(),
            app_state.rotation_in_progress.clone(),
            app_state.autosave_stop.clone(),
        );
        if let Ok(mut guard) = app_state.graph_autosave_thread.lock() {
            *guard = handle;
        }
    }

    // Capture the session_id handle for the shutdown finalizer. At Exit,
    // we read the CURRENT session (may differ from `initial_session_id` if
    // the user rotated via `new_session_cmd`).
    let session_id_handle = app_state.session_id.clone();

    // Capture the handles the graceful-shutdown teardown needs BEFORE
    // `app_state` is moved into `.manage(...)`. The `RunEvent::Exit` closure is
    // `move`, so it owns these clones. All are cheap `Arc` clones of the shared
    // state (see [`crate::state::AppState`]).
    let shutdown = ShutdownHandles {
        session_id: app_state.session_id.clone(),
        autosave_stop: app_state.autosave_stop.clone(),
        graph_autosave_thread: app_state.graph_autosave_thread.clone(),
        knowledge_graph: app_state.knowledge_graph.clone(),
        transcript_buffer: app_state.transcript_buffer.clone(),
        transcript_writer: app_state.transcript_writer.clone(),
        transcript_event_writer: app_state.transcript_event_writer.clone(),
        projection_event_writer: app_state.projection_event_writer.clone(),
        is_capturing: app_state.is_capturing.clone(),
        is_transcribing: app_state.is_transcribing.clone(),
        is_gemini_active: app_state.is_gemini_active.clone(),
        is_converse_active: app_state.is_converse_active.clone(),
        is_openai_realtime_active: app_state.is_openai_realtime_active.clone(),
    };

    tauri::Builder::default()
        // BUG-3: single-instance guard MUST be the first plugin registered
        // (plugins run in registration order). A second launch hands its argv
        // here instead of spawning a process that fails WebView2 creation with
        // 0x800700AA (the running instance holds the user-data-dir lock); we
        // unminimize + focus the existing window so a re-launch behaves like
        // "bring to front" rather than silently dying.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.unminimize();
                let _ = win.show();
                let _ = win.set_focus();
            }
        }))
        .manage(app_state)
        .setup(|app| {
            // Load the persisted log-level preference as soon as we have an
            // AppHandle (env_logger::init() already ran — this only nudges
            // log::max_level()). RUST_LOG still wins at startup since
            // env_logger honoured it before we got here; the setting only
            // overrides the compiled-in default (Info) and is the level
            // every subsequent `set_log_level` command will persist to.
            let handle = app.handle();
            let loaded_settings = crate::settings::load_settings_with_status(handle);
            let load_status = loaded_settings.status;
            let mut settings = loaded_settings.settings;
            if let Some(ref lvl) = settings.log_level {
                crate::logging::apply_log_level(lvl);
            }
            // Apply the persisted file-logging preference (init() defaulted to
            // on/archive so startup was captured). Only reconfigure when the
            // user's choice differs, to avoid re-archiving the fresh log.
            {
                let enabled = settings.file_logging.unwrap_or(true);
                let mode = crate::logging::LogFileMode::from_str_or_default(
                    settings.log_file_mode.as_deref(),
                );
                let already = enabled && mode == crate::logging::LogFileMode::Archive;
                if !already && let Err(e) = crate::logging::configure_file_logging(enabled, mode) {
                    log::warn!("Failed to apply file-logging settings: {e}");
                }
            }
            // Initialize OPT-IN anonymous analytics (Sentry) only when the
            // persisted setting is true. Independent of file-logging and of the
            // local crash handler (installed unconditionally above). The guard
            // is held for the process lifetime inside the analytics module so
            // buffered events flush on exit; runtime toggling is via the
            // set_analytics_enabled command (bind/unbind on the hub).
            {
                let enabled = settings.analytics_enabled.unwrap_or(false);
                crate::analytics::init_if_enabled(enabled);
                // Emit one anonymous startup ping so an opted-in user gets
                // immediate confirmation in Sentry that telemetry is flowing
                // (and a stable "app launched" signal for release-health
                // baselines). No PII — just the event name; the before_send
                // scrubber still applies. No-op when analytics is disabled.
                if enabled {
                    crate::analytics::capture_anonymous_event("app.startup");
                    // The 0.48 transport POSTs on a background thread with no
                    // debounce, and a fast window close on Windows can force-kill
                    // the process before that POST lands (and RunEvent::Exit does
                    // not fire on a taskkill). Trigger a SHORT, BOUNDED flush off
                    // the UI thread so this most-fragile event gets on the wire
                    // before the user can act. Non-blocking: it spawns its own
                    // detached, timeout-bounded thread. No-op when disabled.
                    crate::analytics::flush_after_capture();
                }
            }
            if crate::settings::has_inline_credentials(&settings)
                && crate::settings::allow_automatic_settings_writeback(
                    load_status,
                    "migrating/redacting settings credentials",
                )
                && let Err(e) = crate::settings::save_settings(handle, &settings)
            {
                log::warn!("Failed to migrate/redact settings credentials: {e}");
            }
            // First-launch demo-mode decision: if `demo_mode` has never been
            // set and no cloud credentials are present, wire the app for
            // local-only providers and persist the decision so subsequent
            // launches skip this branch.
            let store = crate::credentials::load_credentials();
            if crate::settings::apply_first_launch_demo_mode(&mut settings, &store)
                && crate::settings::allow_automatic_settings_writeback(
                    load_status,
                    "persisting first-launch demo-mode settings",
                )
                && let Err(e) = crate::settings::save_settings(handle, &settings)
            {
                log::warn!("Failed to persist first-launch demo-mode settings: {e}");
            }
            // Sync the loaded settings into the in-memory cache so other
            // backend modules see them without re-reading the file.
            if let Some(state) = handle.try_state::<AppState>()
                && let Ok(mut cached) = state.app_settings.write()
            {
                *cached = crate::settings::hydrate_runtime_credentials(&settings, &store);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_audio_sources,
            commands::start_capture,
            commands::stop_capture,
            commands::start_transcribe,
            commands::stop_transcribe,
            commands::get_graph_snapshot,
            commands::get_transcript,
            commands::get_pipeline_status,
            commands::send_chat_message,
            commands::synthesize_notes,
            commands::start_streaming_chat,
            commands::cancel_streaming_chat,
            commands::get_chat_history,
            commands::clear_chat_history,
            commands::approve_agent_proposal,
            commands::dismiss_agent_proposal,
            commands::clear_agent_proposals,
            commands::add_question_to_graph,
            commands::merge_graph_entities,
            commands::list_available_models,
            commands::download_model_cmd,
            commands::get_model_status,
            commands::load_llm_model,
            commands::configure_api_endpoint,
            commands::load_settings_cmd,
            commands::save_settings_cmd,
            commands::set_log_level,
            commands::get_log_info,
            commands::set_logging_config,
            commands::purge_logs_cmd,
            commands::open_logs_dir,
            commands::get_analytics_info,
            commands::set_analytics_enabled,
            commands::report_frontend_diagnostic,
            commands::delete_model_cmd,
            commands::list_running_processes,
            commands::start_gemini,
            commands::stop_gemini,
            commands::start_converse,
            commands::stop_converse,
            commands::start_openai_realtime,
            commands::stop_openai_realtime,
            // Persistence commands
            commands::export_transcript,
            commands::save_graph,
            commands::load_graph,
            commands::export_graph,
            commands::get_session_id,
            commands::get_projection_runtime_status_cmd,
            commands::get_projection_replay_report_cmd,
            commands::retry_storage_write,
            // Session management
            commands::list_sessions,
            commands::load_session,
            commands::load_session_transcript,
            commands::export_session_bundle,
            commands::load_session_data_movement_cmd,
            commands::delete_session,
            commands::restore_session,
            commands::delete_session_permanently,
            commands::purge_expired_sessions,
            commands::recover_orphaned_sessions,
            commands::get_session_usage,
            commands::get_current_session_usage,
            commands::get_lifetime_usage,
            commands::seed_lifetime_migration,
            commands::reset_current_session_usage,
            commands::clear_all_usage,
            commands::new_session_cmd,
            // Credential management
            commands::save_credential_cmd,
            commands::load_credential_presence_cmd,
            commands::get_provider_readiness_cmd,
            commands::cancel_provider_readiness_cmd,
            commands::delete_credential_cmd,
            commands::diagnose_credentials,
            commands::list_aws_profiles,
            // Cloud provider connection tests
            commands::test_cloud_asr_connection,
            commands::test_deepgram_connection,
            commands::test_assemblyai_connection,
            commands::test_soniox_connection,
            commands::test_cerebras_connection_cmd,
            commands::test_sambanova_connection_cmd,
            commands::test_openai_compatible_llm_connection_cmd,
            commands::test_gemini_api_key,
            commands::test_aws_credentials,
            commands::test_openrouter_connection_cmd,
            commands::list_deepgram_models_cmd,
            commands::list_soniox_models_cmd,
            commands::list_cerebras_models_cmd,
            commands::list_sambanova_models_cmd,
            commands::list_openai_compatible_llm_models_cmd,
            commands::list_openrouter_models_cmd,
            commands::list_openrouter_providers_cmd,
            commands::list_openrouter_model_endpoints_cmd,
            commands::test_tts_connection_cmd,
            provider_registry::get_provider_registry_cmd,
            commands::list_audio_output_devices_cmd,
            commands::start_audio_playback_cmd,
            commands::stop_audio_playback_cmd,
        ])
        .build(tauri::generate_context!())
        .expect("error while building AudioGraph")
        .run(move |_app_handle, event| {
            match event {
                // `ExitRequested` fires FIRST on the normal window-X close path
                // (before the terminal `Exit`) and, crucially, on paths where
                // `Exit` may never be observed cleanly. Get a bounded, non-close
                // Sentry flush in here so an opted-in user's buffered events
                // (e.g. the startup ping) get on the wire before the OS tears the
                // process down. This is a FLUSH, not a close — it must not break
                // the runtime OFF-toggle path — and it is a no-op when analytics
                // is disabled. Belt-and-suspenders with the `Exit` flush below;
                // we do NOT prevent the exit here.
                tauri::RunEvent::ExitRequested { .. } => {
                    let flushed = crate::analytics::flush(SENTRY_EXIT_REQUEST_FLUSH_TIMEOUT);
                    log::info!("ExitRequested: sentry pre-exit flushed={}", flushed);
                }
                // On clean shutdown: run the bounded graceful teardown (stop the
                // autosave daemon + one final save, flush the transcript/event/
                // projection writers, wind down worker threads, flush Sentry),
                // THEN mark the session complete. Best-effort throughout: if the
                // process is killed instead we rely on register_session()'s
                // "crashed" detection on the next launch. `Exit` is the terminal,
                // non-vetoable event, so the durable teardown hangs off it.
                tauri::RunEvent::Exit => {
                    graceful_shutdown(&shutdown);

                    let current_sid = match session_id_handle.read() {
                        Ok(g) => g.clone(),
                        Err(poisoned) => poisoned.into_inner().clone(),
                    };
                    if let Err(e) = crate::sessions::finalize_session(&current_sid) {
                        log::warn!("Failed to finalize session {}: {}", current_sid, e);
                    } else {
                        log::info!("Session {} finalized on exit", current_sid);
                    }
                }
                _ => {}
            }
        });
}
