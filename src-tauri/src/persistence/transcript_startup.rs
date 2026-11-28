//! Resolve historical sessions before the first window can request one.
use super::canonical_log::{PreparedTranscriptStorage, TranscriptReceipt};
use crate::projections::TranscriptEvent;
use std::{fs, io, path::Path};

/// Registered after the single-instance guard. Tauri initializes plugins
/// during build, before the event loop creates the configured windows.
pub(crate) fn plugin<R: tauri::Runtime>(live_session: String) -> tauri::plugin::TauriPlugin<R> {
    tauri::plugin::Builder::new("historical-session-recovery")
        .setup(move |_, _| {
            recover_at_startup(&live_session);
            Ok(())
        })
        .build()
}

fn recover_at_startup(live_session: &str) {
    let Ok(root) = crate::user_data::resolve_data_root() else {
        log::warn!("transcript recovery scan unavailable");
        return;
    };
    if let Err(error) = recover_in_directory(
        &root.join("transcripts"),
        live_session,
        crate::speech::publish_recovered_transcript,
    ) {
        log::warn!("transcript recovery scan failed: {:?}", error.kind());
    }
}

pub(super) fn recover_in_directory(
    directory: &Path,
    live_session: &str,
    mut publish: impl FnMut(&str, &TranscriptEvent, TranscriptReceipt),
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    for entry in entries {
        let name = match entry {
            Ok(entry) => entry.file_name(),
            Err(error) => {
                log::warn!("transcript recovery entry unavailable: {:?}", error.kind());
                continue;
            }
        };
        let Some(session) = name
            .to_str()
            .and_then(|n| n.strip_suffix(".events.jsonl.attempt"))
        else {
            continue;
        };
        if session == live_session || crate::sessions::validate_session_id(session).is_err() {
            continue;
        }
        let path = directory.join(format!("{session}.events.jsonl"));
        let result = PreparedTranscriptStorage::open(&path, session).and_then(|storage| {
            storage.resolve_pending()?;
            for kept in storage.take_recovered() {
                publish(session, &kept.event, kept.receipt);
            }
            Ok(())
        });
        if let Err(error) = result {
            log::warn!("transcript recovery pending: {error:?}");
        }
    }
    Ok(())
}
