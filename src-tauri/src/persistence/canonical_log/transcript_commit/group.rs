//! One bounded write and file barrier for already queued, new transcript
//! revisions. Readers stay at the old verified head until intent retirement.
use super::*;
use crate::persistence::MAX_TRANSCRIPT_GROUP as MAX_MEMBERS;

const MAX_GROUP_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
thread_local! { pub(super) static GROUP_WRITES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Member {
    length: usize,
    hash: String,
}

struct Plan {
    bytes: Vec<u8>,
    members: Vec<Member>,
    frames: Vec<PendingAppend>,
}

impl PreparedTranscriptStorage {
    /// None leaves all requests available for individual admission. An error
    /// after publication belongs to the whole group; no member was acknowledged.
    pub(crate) fn commit_group_after(
        &self,
        events: &[TranscriptEvent],
        previous: u64,
    ) -> Result<Option<Vec<TranscriptReceipt>>, TranscriptCommitError> {
        if !(2..=MAX_MEMBERS).contains(&events.len())
            || self
                .cache
                .borrow()
                .as_ref()
                .is_none_or(|prefix| prefix.stamp.is_none())
        {
            return Ok(None);
        }
        with_writable_v1(&self.root, &self.store, || {
            Commit::open(self)?.append_group(events, previous, &self.cache)
        })
    }
}

impl Commit<'_> {
    fn plan_group(
        &self,
        events: &[TranscriptEvent],
        prefix: &VerifiedPrefix,
    ) -> Result<Option<Plan>, TranscriptCommitError> {
        let mut head = prefix.stream.head.clone();
        let mut latest = HashMap::new();
        let mut byte_len = prefix.stream.byte_len;
        let mut ends_with_newline = prefix.stream.ends_with_newline;
        let mut plan = Plan {
            bytes: Vec::new(),
            frames: Vec::new(),
            members: Vec::new(),
        };
        for event in events {
            if !event.start_time.is_finite()
                || !event.end_time.is_finite()
                || !event.confidence.is_finite()
                || latest
                    .get(event.span_id.as_str())
                    .or_else(|| prefix.revisions.get(&event.span_id))
                    .is_some_and(|&revision| revision >= event.revision_number)
            {
                return Ok(None);
            }
            let basis = FrameBasis {
                session_id: self.session,
                stream_id: STREAM,
                domain_schema_version: 1,
                head: head.as_ref(),
                events: &prefix.stream.events,
                byte_len,
                ends_with_newline,
                poisoned: None,
            };
            let metadata = CanonicalEventMetadata::new(revision_id(event)?);
            let PlannedAppend::New(mut frame) = basis.plan(&metadata, event) else {
                return Ok(None);
            };
            let length = frame.frame.len();
            if length > MAX_GROUP_BYTES.saturating_sub(plan.bytes.len()) {
                return Ok(None);
            }
            latest.insert(event.span_id.as_str(), event.revision_number);
            head = Some(CanonicalStreamHead {
                sequence: frame.sequence,
                event_id: frame.event_id.clone(),
                record_hash: frame.record_hash.clone(),
            });
            byte_len = byte_len
                .checked_add(length as u64)
                .ok_or(TranscriptCommitError::InvalidHistory)?;
            ends_with_newline = true;
            plan.members.push(Member {
                length,
                hash: digest(&frame.frame),
            });
            plan.bytes.extend(std::mem::take(&mut frame.frame));
            plan.frames.push(frame);
        }
        Ok(Some(plan))
    }

    fn append_group(
        &self,
        events: &[TranscriptEvent],
        previous: u64,
        verified: &std::cell::RefCell<Option<VerifiedPrefix>>,
    ) -> Result<Option<Vec<TranscriptReceipt>>, TranscriptCommitError> {
        let intent_path = self.path.with_added_extension("attempt");
        if let Some(bytes) = checked_bytes_bounded(&intent_path, 4096)? {
            let intent: Intent = serde_json::from_slice(&bytes)
                .map_err(|_| TranscriptCommitError::InvalidHistory)?;
            self.sync_existing_intent(&intent_path)?;
            return self.retry_group(&intent, events, previous, verified);
        }
        let mut cached = verified.borrow_mut();
        let Some(prefix) = cached.as_mut() else {
            return Ok(None);
        };
        if prefix.stream.head.as_ref().map_or(0, |h| h.sequence) != previous
            || prefix.stamp.is_none()
        {
            return Ok(None);
        }
        let mut file = match self.guard.open_recovery_source(self.path) {
            Ok(file) => file,
            Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                kind: io::ErrorKind::NotFound,
                ..
            }) => return Ok(None),
            Err(error) => return Err(TranscriptCommitError::Rejected(error)),
        };
        if SourceStamp::read(&file).ok() != prefix.stamp {
            return Ok(None);
        }
        let Some(plan) = self.plan_group(events, prefix)? else {
            return Ok(None);
        };
        let mut lease = self.acquire_lease()?;
        if lease.pending.is_some() {
            return Ok(None);
        }
        let intent = Intent {
            attempt: Uuid::new_v4().to_string(),
            owner: process_owner().to_owned(),
            after: head(&prefix.stream),
            base_len: prefix.stream.byte_len,
            source_was_absent: false,
            frame_len: plan.bytes.len() as u64,
            frame_hash: digest(&plan.bytes),
            members: plan.members,
        };
        let descriptor =
            serde_json::to_vec(&intent).map_err(|_| TranscriptCommitError::InvalidHistory)?;
        let group_members = member_identities(events)?;
        lease.pending = Some(RetainedFrame {
            bytes: plan.bytes,
            payload: String::new(),
            revision_id: String::new(),
            group_members,
            synced: false,
        });
        self.publish(&intent_path, &descriptor)?;
        #[cfg(test)]
        kill_at(Cut::KillIntent);
        let receipts = self.write_group(&intent, &mut file, &mut lease)?;
        for (frame, event) in plan.frames.into_iter().zip(events) {
            prefix.stream.head = Some(CanonicalStreamHead {
                sequence: frame.sequence,
                event_id: frame.event_id.clone(),
                record_hash: frame.record_hash.clone(),
            });
            prefix.stream.events.insert(
                frame.event_id,
                CachedEvent {
                    sequence: frame.sequence,
                    record_hash: frame.record_hash,
                    payload_hash: frame.payload_hash,
                    commitment_hash: frame.commitment_hash,
                },
            );
            prefix
                .revisions
                .insert(event.span_id.clone(), event.revision_number);
        }
        prefix.stream.byte_len += intent.frame_len;
        prefix.stream.ends_with_newline = true;
        drop(file);
        prefix.stamp = File::open(self.path)
            .ok()
            .and_then(|f| SourceStamp::read(&f).ok());
        Ok(Some(receipts))
    }

    fn retry_group(
        &self,
        intent: &Intent,
        events: &[TranscriptEvent],
        previous: u64,
        verified: &std::cell::RefCell<Option<VerifiedPrefix>>,
    ) -> Result<Option<Vec<TranscriptReceipt>>, TranscriptCommitError> {
        if intent.members.len() != events.len()
            || intent.owner != process_owner()
            || intent.after.as_ref().map_or(0, |h| h.sequence) != previous
        {
            return Ok(None);
        }
        let members = member_identities(events)?;
        let mut lease = self.acquire_lease()?;
        if lease
            .pending
            .as_ref()
            .is_none_or(|r| r.group_members != members)
        {
            return Ok(None);
        }
        let result = self.retry_retained(intent, &mut lease)?;
        if result.is_some() {
            *verified.borrow_mut() = None;
        }
        Ok(result)
    }

    fn retry_retained(
        &self,
        intent: &Intent,
        lease: &mut Lease,
    ) -> Result<Option<Vec<TranscriptReceipt>>, TranscriptCommitError> {
        let retained = lease
            .pending
            .as_ref()
            .ok_or(TranscriptCommitError::InvalidHistory)?;
        let (mut file, _, _) = self.read_group_source(intent)?;
        let evidence = self
            .path
            .with_added_extension(format!("{}.quarantine", intent.attempt));
        let gap = self
            .path
            .with_added_extension(format!("{}.gap", intent.attempt));
        for path in [evidence, gap] {
            if path
                .try_exists()
                .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
            {
                return Ok(None);
            }
        }
        if digest(&retained.bytes) != intent.frame_hash
            || retained.bytes.len() as u64 != intent.frame_len
            || matched_members(&retained.bytes, &intent.members)?.1 != intent.members.len()
        {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        // The base is authenticated in read_group_source. Only the planned
        // suffix can be overwritten, even if its dirty bytes were lost.
        self.write_group(intent, &mut file, lease).map(Some)
    }

    pub(super) fn resolve_group_for_request(
        &self,
        intent: &Intent,
        lease: &mut Lease,
        event: Option<&TranscriptEvent>,
        digest: Option<&str>,
    ) -> Result<(), TranscriptCommitError> {
        if let Some(retained) = &lease.pending {
            let id = revision_id(event.ok_or(TranscriptCommitError::ConflictingAttempt)?)?;
            if let Some((_, original)) = retained
                .group_members
                .iter()
                .find(|(member, _)| *member == id)
            {
                if Some(original.as_str()) != digest {
                    return Err(TranscriptCommitError::ConflictingAttempt);
                }
                // A single member can be redelivered without its peers. The
                // live owner still holds the entire exact group for its retry.
                if self.retry_retained(intent, lease)?.is_some() {
                    return Ok(());
                }
            }
        }
        self.recover_group(intent)
    }

    fn write_group(
        &self,
        intent: &Intent,
        file: &mut File,
        lease: &mut Lease,
    ) -> Result<Vec<TranscriptReceipt>, TranscriptCommitError> {
        let retained = lease
            .pending
            .as_mut()
            .ok_or(TranscriptCommitError::InvalidHistory)?;
        self.guard
            .revalidate_recovery_source(self.path, file)
            .map_err(TranscriptCommitError::Rejected)?;
        file.seek(SeekFrom::Start(intent.base_len))
            .map_err(|e| io_failure(CanonicalAppendPhase::Write, e))?;
        #[cfg(test)]
        {
            GROUP_WRITES.set(GROUP_WRITES.get() + 1);
            if matches!(CUT.get(), Some(Cut::KillGroupTorn | Cut::KillGroupMember)) {
                let cut = intent.members[0].length
                    + if CUT.get() == Some(Cut::KillGroupTorn) {
                        intent.members[1].length / 2
                    } else {
                        0
                    };
                file.write_all(&retained.bytes[..cut]).unwrap();
                wait_for_kill();
            }
        }
        write_frame(file, &retained.bytes)?;
        retained.synced = true;
        self.finish_intent(&self.path.with_added_extension("attempt"))?;
        lease.pending = None;
        let barrier = match self.guard {
            TranscriptGuard::Qualified(..) => CanonicalDurabilityBarrier::FileDataAndMetadata,
            TranscriptGuard::Unqualified(_) => CanonicalDurabilityBarrier::SyncedUnqualified,
        };
        let previous = intent.after.as_ref().map_or(0, |h| h.sequence);
        Ok((1..=intent.members.len())
            .map(|i| TranscriptReceipt {
                sequence: previous + i as u64,
                already_accepted: false,
                barrier,
            })
            .collect())
    }

    fn read_group_source(
        &self,
        intent: &Intent,
    ) -> Result<(File, Vec<u8>, usize), TranscriptCommitError> {
        if !(2..=MAX_MEMBERS).contains(&intent.members.len())
            || intent.source_was_absent
            || !valid_attempt(&intent.attempt)
            || !valid_attempt(&intent.owner)
            || !valid_hash(&intent.frame_hash)
            || intent.frame_len > MAX_GROUP_BYTES as u64
            || intent
                .members
                .iter()
                .any(|m| m.length == 0 || !valid_hash(&m.hash))
            || intent
                .members
                .iter()
                .try_fold(0_u64, |n, m| n.checked_add(m.length as u64))
                != Some(intent.frame_len)
        {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        let mut file = self
            .guard
            .open_recovery_source(self.path)
            .map_err(TranscriptCommitError::Rejected)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)
            .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?;
        let index =
            usize::try_from(intent.base_len).map_err(|_| TranscriptCommitError::InvalidHistory)?;
        let (base, suffix) = bytes
            .split_at_checked(index)
            .ok_or(TranscriptCommitError::InvalidHistory)?;
        if suffix.len() as u64 > intent.frame_len || head(&self.cache(base)?.0) != intent.after {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        Ok((file, bytes, index))
    }

    pub(super) fn recover_group(&self, intent: &Intent) -> Result<(), TranscriptCommitError> {
        let (mut file, bytes, base_len) = self.read_group_source(intent)?;
        let (verified, count) = matched_members(&bytes[base_len..], &intent.members)?;
        let end = base_len + verified;
        let after = head(&self.cache(&bytes[..end])?.0);
        let previous = intent.after.as_ref().map_or(0, |h| h.sequence);
        if previous.checked_add(count as u64) != Some(after.as_ref().map_or(0, |h| h.sequence))
            || (count == intent.members.len() && digest(&bytes[base_len..end]) != intent.frame_hash)
        {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        if count == intent.members.len() {
            // An earlier recovery that published rollback evidence may only
            // finish that disposition. Never release all members alongside
            // an existing gap or quarantine for this same attempt.
            for extension in ["gap", "quarantine"] {
                let path = self
                    .path
                    .with_added_extension(format!("{}.{extension}", intent.attempt));
                if path
                    .try_exists()
                    .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
                {
                    return Err(TranscriptCommitError::InvalidHistory);
                }
            }
        }
        self.guard
            .revalidate_recovery_source(self.path, &file)
            .map_err(TranscriptCommitError::Rejected)?;
        if count != 0 {
            file.seek(SeekFrom::Start(intent.base_len))
                .map_err(|e| io_failure(CanonicalAppendPhase::Write, e))?;
            write_frame(&mut file, &bytes[base_len..end])?;
        }
        if count < intent.members.len() {
            let remainder = Intent {
                base_len: end as u64,
                after,
                ..intent.clone()
            };
            self.recover_gap(&remainder, &bytes[end..], Some(&file))?;
        }
        self.finish_intent(&self.path.with_added_extension("attempt"))
    }
}

fn matched_members(
    bytes: &[u8],
    members: &[Member],
) -> Result<(usize, usize), TranscriptCommitError> {
    let mut end = 0;
    let mut count = 0;
    for member in members {
        let Some(frame) = bytes[end..].get(..member.length) else {
            break;
        };
        if digest(frame) != member.hash {
            break;
        }
        let line = frame.strip_prefix(b"\n").unwrap_or(frame);
        if !line
            .strip_suffix(b"\n")
            .is_some_and(|row| row.starts_with(FRAME_PREFIX_V1) && !row.contains(&b'\n'))
        {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        end += member.length;
        count += 1;
    }
    Ok((end, count))
}

fn member_identities(
    events: &[TranscriptEvent],
) -> Result<Vec<(String, String)>, TranscriptCommitError> {
    events
        .iter()
        .map(|event| {
            let bytes =
                serde_json::to_vec(event).map_err(|_| TranscriptCommitError::InvalidHistory)?;
            Ok((revision_id(event)?, digest(&bytes)))
        })
        .collect()
}
