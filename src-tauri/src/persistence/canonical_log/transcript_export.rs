//! Portable recovery evidence accompanies the transcript, outside its replayable events.
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

use super::transcript_commit::{TranscriptGap, decode_gap, transcript_recovery_artifacts};
use crate::persistence::canonical_durability::{CanonicalCoordinationError, CanonicalSharedGuard};

#[derive(Serialize)]
pub(crate) struct TranscriptRecoveryExport {
    gaps: Vec<TranscriptGap>,
    evidence: Vec<Evidence>,
}

impl fmt::Debug for TranscriptRecoveryExport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TranscriptRecoveryExport")
    }
}

#[derive(Serialize)]
struct Evidence {
    #[serde(flatten)]
    kind: EvidenceKind,
    data_base64: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum EvidenceKind {
    Attempt,
    Gap { attempt_id: String },
    Quarantine { attempt_id: String },
}

impl EvidenceKind {
    fn from_path(path: &Path) -> Option<Self> {
        // The shared inventory accepts only this Session's exact filenames and
        // canonical UUIDs. The portable bundle carries no filesystem address.
        let extension = path.extension()?.to_str()?;
        if extension == "attempt" {
            return Some(Self::Attempt);
        }
        let attempt_id = Path::new(path.file_stem()?)
            .extension()?
            .to_str()?
            .to_owned();
        match extension {
            "gap" => Some(Self::Gap { attempt_id }),
            "quarantine" => Some(Self::Quarantine { attempt_id }),
            _ => None,
        }
    }
}

fn read_evidence(path: &Path, remaining: &mut u64) -> io::Result<Vec<u8>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        use rustix::fs::OFlags;
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags((OFlags::NOFOLLOW | OFlags::NONBLOCK).bits() as i32);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || crate::sessions::is_reparse_point(&metadata) {
        return Err(io::ErrorKind::InvalidData.into());
    }
    if metadata.len() > *remaining {
        return Err(io::ErrorKind::FileTooLarge.into());
    }
    let mut bytes = Vec::new();
    file.take(remaining.saturating_add(1))
        .read_to_end(&mut bytes)?;
    *remaining = remaining
        .checked_sub(bytes.len() as u64)
        .ok_or(io::ErrorKind::FileTooLarge)?;
    Ok(bytes)
}

fn inventory(path: &Path) -> Result<Vec<std::path::PathBuf>, String> {
    let mut paths = transcript_recovery_artifacts(path).map_err(evidence_io)?;
    paths.retain(|path| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension, "attempt" | "quarantine" | "gap"))
    });
    Ok(paths)
}

fn evidence_io(error: io::Error) -> String {
    format!(
        "Transcript recovery evidence unavailable: {:?}",
        error.kind()
    )
}

/// Read the same transcript generation as its attempt/gap/quarantine evidence.
/// Historical sessions need no lock creation. A first writer that introduces
/// evidence during that observation makes the read retryable.
pub(crate) fn read_for_export<T>(
    path: &Path,
    ceiling: u64,
    read_transcript: impl FnOnce() -> Result<T, String>,
) -> Result<(T, Option<TranscriptRecoveryExport>), String> {
    let observed = inventory(path)?;
    if observed.is_empty() {
        let transcript = read_transcript()?;
        if !inventory(path)?.is_empty() {
            return Err("Transcript recovery evidence changed during export".into());
        }
        return Ok((transcript, None));
    }
    let parent = path
        .parent()
        .ok_or_else(|| evidence_io(io::ErrorKind::InvalidInput.into()))?;
    let guard = match CanonicalSharedGuard::open_existing(parent) {
        Ok(guard) => Some(guard),
        Err(CanonicalCoordinationError::Missing) => None, // A restored, offline session.
        Err(error) => return Err(format!("Transcript recovery coordination: {error:?}")),
    };
    let paths = inventory(path)?;
    let transcript = read_transcript()?;
    let mut evidence = Vec::new();
    let mut gaps = Vec::new();
    let source_bytes = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata.len(),
        Err(error) if error.kind() == io::ErrorKind::NotFound => 0,
        Err(error) => return Err(evidence_io(error)),
    };
    let mut remaining = ceiling
        .checked_sub(source_bytes)
        .ok_or_else(|| evidence_io(io::ErrorKind::FileTooLarge.into()))?;
    for artifact in &paths {
        let Some(kind) = EvidenceKind::from_path(artifact) else {
            continue;
        };
        let bytes = read_evidence(artifact, &mut remaining).map_err(evidence_io)?;
        if matches!(kind, EvidenceKind::Gap { .. }) {
            gaps.push(
                decode_gap(&bytes)
                    .map_err(|error| format!("Transcript recovery gap unavailable: {error:?}"))?,
            );
        }
        evidence.push(Evidence {
            kind,
            data_base64: STANDARD.encode(bytes),
        });
    }
    if guard.is_none()
        && (inventory(path)? != paths
            || !matches!(
                CanonicalSharedGuard::open_existing(parent),
                Err(CanonicalCoordinationError::Missing)
            ))
    {
        return Err("Transcript recovery evidence changed during export".into());
    }
    gaps.sort_by_key(|gap| gap.after.as_ref().map(|head| head.sequence));
    Ok((
        transcript,
        (!evidence.is_empty()).then_some(TranscriptRecoveryExport { gaps, evidence }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_export_bounds_the_source_and_evidence_together() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        let source = root.join("orchard.events.jsonl");
        fs::write(&source, b"wood").unwrap();
        fs::write(source.with_added_extension("attempt"), b"oak").unwrap();
        assert!(read_for_export(&source, 7, || Ok(())).is_ok());
        assert_eq!(
            read_for_export(&source, 6, || Ok(())).unwrap_err(),
            "Transcript recovery evidence unavailable: FileTooLarge"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn recovery_export_refuses_linked_evidence() {
        let root = std::env::temp_dir().join(uuid::Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        let source = root.join("orchard.events.jsonl");
        fs::write(&source, b"").unwrap();
        let other = root.join("other");
        fs::write(&other, b"pear").unwrap();
        let link = source.with_added_extension(format!("{}.quarantine", uuid::Uuid::new_v4()));
        std::os::unix::fs::symlink(&other, &link).unwrap();
        let reason = read_for_export(&source, 16, || Ok(())).unwrap_err();
        assert!(reason.starts_with("Transcript recovery evidence unavailable: "));
        assert!(!reason.contains("pear") && !reason.contains("orchard"));
        fs::remove_dir_all(root).unwrap();
    }
}
