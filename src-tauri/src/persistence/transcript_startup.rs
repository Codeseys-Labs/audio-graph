//! Resolve historical sessions before the first window can request one.
use super::canonical_log::{
    PreparedProjectionStorage, PreparedTranscriptStorage, TranscriptCommitError, TranscriptReceipt,
};
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
        return;
    }
    if let Err(error) = recover_projection_directory(&root, live_session) {
        log::warn!("projection recovery scan failed: {:?}", error.kind());
    }
}

pub(super) fn recover_in_directory(
    directory: &Path,
    live_session: &str,
    mut publish: impl FnMut(&str, &TranscriptEvent, TranscriptReceipt),
) -> io::Result<()> {
    scan_attempts(directory, "transcript", live_session, |path, session| {
        let storage = PreparedTranscriptStorage::open(path, session)?;
        storage.resolve_pending()?;
        for kept in storage.take_recovered() {
            publish(session, &kept.event, kept.receipt);
        }
        Ok(())
    })
}

/// Historical projection recovery admits existing frames against the recovered
/// transcript. Review replays those frames locally without scheduling a job.
pub(super) fn recover_projection_directory(root: &Path, live_session: &str) -> io::Result<()> {
    let transcripts = root.join("transcripts");
    scan_attempts(
        &root.join("projections"),
        "projection",
        live_session,
        |path, session| {
            let preceding = transcripts.join(format!("{session}.events.jsonl.attempt"));
            match preceding.try_exists() {
                Ok(false) => PreparedProjectionStorage::open(path, session)?.resolve_pending(),
                Ok(true) => Err(TranscriptCommitError::ConflictingAttempt),
                Err(_) => Err(TranscriptCommitError::InvalidHistory),
            }
        },
    )
}

fn scan_attempts(
    directory: &Path,
    stream: &'static str,
    live_session: &str,
    mut resolve: impl FnMut(&Path, &str) -> Result<(), TranscriptCommitError>,
) -> io::Result<()> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let mut resolved = std::collections::HashSet::new();
    for entry in entries {
        let name = match entry {
            Ok(entry) => entry.file_name(),
            Err(error) => {
                log::warn!("{stream} recovery entry unavailable: {:?}", error.kind());
                continue;
            }
        };
        let Some(session) = name.to_str().and_then(|n| {
            n.strip_suffix(".events.jsonl.attempt")
                .or_else(|| n.strip_suffix(".events.jsonl.attempt-origin"))
        }) else {
            continue;
        };
        if session == live_session
            || crate::sessions::validate_session_id(session).is_err()
            || !resolved.insert(session.to_owned())
        {
            continue;
        }
        let path = directory.join(format!("{session}.events.jsonl"));
        let result = resolve(&path, session);
        if let Err(error) = result {
            log::warn!("{stream} recovery pending: {error:?}");
        }
    }
    Ok(())
}
