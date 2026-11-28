//! Portable recovery evidence accompanies the transcript, outside its replayable events.
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Read};
use std::path::Path;

use base64::{Engine as _, engine::general_purpose::STANDARD};
use serde::Serialize;

use super::transcript_commit::{
    GapStream, TranscriptGap, decode_gap, transcript_recovery_artifacts,
};
use crate::persistence::canonical_durability::{
    CanonicalCoordinationError, transcript_coordination_path, wait_for_transcript_lock,
};

#[derive(Serialize)]
pub(crate) struct TranscriptRecoveryExport {
    /// Content-free storage provenance for the active descriptor only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unqualified_attempt_id: Option<String>,
    gaps: Vec<TranscriptGap>,
    evidence: Vec<Evidence>,
    #[serde(skip)]
    pub(crate) gap_positions: Vec<(String, TranscriptGap)>,
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
    AttemptOrigin,
    Gap { attempt_id: String },
    Quarantine { attempt_id: String },
}

impl EvidenceKind {
    fn from_path(path: &Path) -> Option<Self> {
        // The shared inventory accepts only this Session's exact filenames and
        // canonical UUIDs. The portable bundle carries no filesystem address.
        let extension = path.extension()?.to_str()?;
        if extension == "attempt-origin" {
            return Some(Self::AttemptOrigin);
        }
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
    let has_attempt = paths
        .iter()
        .any(|p| p.extension().is_some_and(|ext| ext == "attempt"));
    paths.retain(|path| {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| {
                matches!(extension, "attempt" | "quarantine" | "gap")
                    || (has_attempt && extension == "attempt-origin")
            })
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
    capture(
        path,
        ceiling,
        read_transcript,
        true,
        GapStream::TranscriptRevisions,
    )
}

/// Holding a shared lock binds the evidence inventory to the captured projection
/// bytes. A history view needs only gap records. Portable exports retain raw
/// quarantine bytes as well. Parsing and encoding happen after releasing it.
pub(crate) fn capture_projection<T>(
    path: &Path,
    ceiling: u64,
    capture_source: impl FnOnce() -> Result<T, String>,
    portable: bool,
) -> Result<(T, Option<TranscriptRecoveryExport>), String> {
    capture(
        path,
        ceiling,
        capture_source,
        portable,
        GapStream::ProjectionPatches,
    )
}

fn capture<T>(
    path: &Path,
    ceiling: u64,
    read_transcript: impl FnOnce() -> Result<T, String>,
    portable: bool,
    stream: GapStream,
) -> Result<(T, Option<TranscriptRecoveryExport>), String> {
    let lock = transcript_coordination_path(path);
    let acquire = || wait_for_transcript_lock(&lock, true, false);
    let guard = match acquire() {
        Ok(file) => Some(file),
        Err(CanonicalCoordinationError::Missing) => None,
        Err(error) => return Err(format!("Transcript recovery coordination: {error:?}")),
    };
    let paths = inventory(path)?;
    let transcript = read_transcript()?;
    if paths.is_empty() {
        if guard.is_none()
            && (!inventory(path)?.is_empty()
                || !matches!(acquire(), Err(CanonicalCoordinationError::Missing)))
        {
            return Err("Transcript recovery evidence changed during export".into());
        }
        return Ok((transcript, None));
    }
    let mut captured_evidence = Vec::new();
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
        if !portable && !matches!(kind, EvidenceKind::Gap { .. }) {
            continue;
        }
        let bytes = read_evidence(artifact, &mut remaining).map_err(evidence_io)?;
        captured_evidence.push((kind, bytes));
    }
    if guard.is_none()
        && (inventory(path)? != paths
            || !matches!(acquire(), Err(CanonicalCoordinationError::Missing)))
    {
        return Err("Transcript recovery evidence changed during export".into());
    }
    drop(guard);
    let mut evidence = Vec::new();
    let mut gaps = Vec::new();
    let mut gap_positions = Vec::new();
    #[derive(serde::Deserialize)]
    struct DescriptorId<'a> {
        attempt: &'a str,
    }
    let origin = captured_evidence
        .iter()
        .find(|(kind, _)| matches!(kind, EvidenceKind::AttemptOrigin));
    let unqualified_attempt_id = origin.and_then(|(_, bytes)| {
        let id = std::str::from_utf8(bytes).ok()?;
        let uuid = uuid::Uuid::parse_str(id).ok()?;
        if uuid.to_string() != id {
            return None;
        }
        captured_evidence.iter().find_map(|(kind, bytes)| {
            if !matches!(kind, EvidenceKind::Attempt) {
                return None;
            }
            let descriptor: DescriptorId = serde_json::from_slice(bytes).ok()?;
            (descriptor.attempt == id).then(|| id.to_owned())
        })
    });
    for (kind, bytes) in captured_evidence {
        if matches!(kind, EvidenceKind::AttemptOrigin) {
            continue;
        }
        if let EvidenceKind::Gap { attempt_id } = &kind {
            let gap = decode_gap(&bytes)
                .map_err(|error| format!("Recovery gap unavailable: {error:?}"))?;
            if gap.stream != stream {
                return Err("Recovery gap stream mismatch".into());
            }
            gap_positions.push((attempt_id.clone(), gap.clone()));
            gaps.push(gap);
        }
        if portable {
            evidence.push(Evidence {
                kind,
                data_base64: STANDARD.encode(bytes),
            });
        }
    }
    gaps.sort_by_key(|gap| gap.after.as_ref().map(|head| head.sequence));
    Ok((
        transcript,
        Some(TranscriptRecoveryExport {
            unqualified_attempt_id,
            gaps,
            evidence,
            gap_positions,
        }),
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
