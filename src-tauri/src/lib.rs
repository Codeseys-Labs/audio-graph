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
mod source_separation_fixtures;

use state::AppState;
use tauri::Manager;

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
    // respawn.
    {
        let handle = persistence::spawn_graph_autosave(
            app_state.session_id.clone(),
            app_state.knowledge_graph.clone(),
            app_state.transcript_buffer.clone(),
            app_state.rotation_in_progress.clone(),
        );
        if let Ok(mut guard) = app_state.graph_autosave_thread.lock() {
            *guard = handle;
        }
    }

    // Capture the session_id handle for the shutdown finalizer. At Exit,
    // we read the CURRENT session (may differ from `initial_session_id` if
    // the user rotated via `new_session_cmd`).
    let session_id_handle = app_state.session_id.clone();

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
            commands::test_gemini_api_key,
            commands::test_aws_credentials,
            commands::test_openrouter_connection_cmd,
            commands::list_deepgram_models_cmd,
            commands::list_soniox_models_cmd,
            commands::list_cerebras_models_cmd,
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
            // Mark the session as complete on clean shutdown. Best-effort: if
            // the process is killed we rely on register_session()'s
            // "crashed" detection on the next launch.
            if let tauri::RunEvent::Exit = event {
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
        });
}
