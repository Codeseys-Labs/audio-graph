//! Resolve previous sessions before their first historical load, once the
//! runtime is available to receive confirmations.
use super::canonical_log::{PreparedTranscriptStorage, TranscriptReceipt};
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
    }
}

pub(super) fn recover_in_directory(
    directory: &Path,
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
        if crate::sessions::validate_session_id(session).is_err() {
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
