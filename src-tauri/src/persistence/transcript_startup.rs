//! Resolve previous sessions before their first historical load, once the
//! runtime is available to receive confirmations.
use super::canonical_log::{
    PreparedProjectionStorage, PreparedTranscriptStorage, TranscriptCommitError, TranscriptReceipt,
};
use crate::projections::TranscriptEvent;
use std::{fs, io, path::Path};

pub(crate) fn recover_at_startup() {
    let Ok(root) = crate::user_data::resolve_data_root() else {
        log::warn!("transcript recovery scan unavailable");
        return;
    };
    if let Err(error) = recover_in_directory(
        &root.join("transcripts"),
        crate::speech::publish_recovered_transcript,
    ) {
        log::warn!("transcript recovery scan failed: {:?}", error.kind());
        return;
    }
    if let Err(error) = recover_projection_directory(&root) {
        log::warn!("projection recovery scan failed: {:?}", error.kind());
    }
}

pub(super) fn recover_in_directory(
    directory: &Path,
    mut publish: impl FnMut(&str, &TranscriptEvent, TranscriptReceipt),
) -> io::Result<()> {
    scan_attempts(directory, "transcript", |path, session| {
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
pub(super) fn recover_projection_directory(root: &Path) -> io::Result<()> {
    let transcripts = root.join("transcripts");
    scan_attempts(&root.join("projections"), "projection", |path, session| {
        let preceding = transcripts.join(format!("{session}.events.jsonl.attempt"));
        match preceding.try_exists() {
            Ok(false) => PreparedProjectionStorage::open(path, session)?.resolve_pending(),
            Ok(true) => Err(TranscriptCommitError::ConflictingAttempt),
            Err(_) => Err(TranscriptCommitError::InvalidHistory),
        }
    })
}

fn scan_attempts(
    directory: &Path,
    stream: &'static str,
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
        if crate::sessions::validate_session_id(session).is_err()
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
