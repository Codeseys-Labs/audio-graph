//! One bounded write and file barrier for already queued, new transcript
//! revisions. Readers stay at the old verified head until intent retirement.
use super::*;
use crate::persistence::MAX_TRANSCRIPT_GROUP as MAX_MEMBERS;

pub(super) const MAX_GROUP_BYTES: usize = 8 * 1024 * 1024;

#[cfg(test)]
thread_local! { pub(super) static GROUP_WRITES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Member {
    pub(super) length: usize,
    pub(super) hash: String,
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
        let mut lease = self.acquire_lease()?;
        let prior_receipts = self.recovered.borrow().len();
        if self.resolve_lease(&mut lease)? {
            verified.take();
            let recovered = self.recovered.borrow();
            let rows = &recovered[prior_receipts..];
            // A direct storage caller retrying the very same group needs a
            // return value. The live worker drains recovery before admission,
            // so it publishes these rows only via Recovery::resolve.
            if rows.len() == events.len()
                && rows
                    .first()
                    .is_some_and(|row| Some(row.receipt.sequence) == previous.checked_add(1))
                && rows
                    .iter()
                    .zip(events)
                    .all(|(row, event)| row.event == *event)
            {
                return Ok(Some(
                    rows.iter()
                        .map(|row| TranscriptReceipt {
                            already_accepted: true,
                            ..row.receipt
                        })
                        .collect(),
                ));
            }
            drop(recovered);
            drop(lease);
            self.run(
                None,
                None,
                &[],
                CommitExpectation::default(),
                process_owner(),
                verified,
            )?;
            lease = self.acquire_lease()?;
        }
        let intent_path = self.path.with_added_extension("attempt");
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
        let stamp = SourceStamp::read(&file).ok();
        if !prefix
            .matches(&mut file, stamp.as_ref(), self.session, STREAM)
            .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
        {
            return Ok(None);
        }
        let Some(plan) = self.plan_group(events, prefix)? else {
            return Ok(None);
        };
        let witness = tier::ReceiptTierWitness::from_frame(&plan.frames[0]);
        let intent = Intent {
            projection_lanes: Vec::new(),
            attempt: Uuid::new_v4().to_string(),
            owner: process_owner().to_owned(),
            after: head(&prefix.stream),
            base_len: prefix.stream.byte_len,
            source_was_absent: false,
            frame_len: plan.bytes.len() as u64,
            frame_hash: digest(&plan.bytes),
            members: plan.members.clone(),
        };
        self.record_attempt_origin(&intent, &prefix.stream);
        let descriptor =
            serde_json::to_vec(&intent).map_err(|_| TranscriptCommitError::InvalidHistory)?;
        self.publish_intent(&mut lease, &intent_path, &descriptor)?;
        #[cfg(test)]
        kill_at(Cut::KillIntent);
        let receipts = self.write_group(&intent, &plan, &mut file)?;
        self.record_barrier(&witness, &prefix.stream, None);
        if let Err(error) = self.finish_intent(&self.path.with_added_extension("attempt")) {
            lease.pending.as_mut().unwrap().retiring = Some(
                events
                    .iter()
                    .cloned()
                    .zip(receipts.iter().copied())
                    .zip(&plan.frames)
                    .map(|((event, receipt), frame)| RecoveredEvent {
                        event,
                        receipt,
                        record_hash: frame.record_hash.clone(),
                    })
                    .collect(),
            );
            return Err(error);
        }
        lease.pending = None;
        prefix.head_frame = HeadFrame::from_verified(&plan.bytes, intent.base_len);
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
        domain::remember_head(
            self.path,
            prefix.stamp.as_ref(),
            prefix.head_frame.as_ref().map(|frame| frame.offset),
            prefix.stream.head.as_ref(),
        );
        Ok(Some(receipts))
    }

    fn write_group(
        &self,
        intent: &Intent,
        plan: &Plan,
        file: &mut File,
    ) -> Result<Vec<TranscriptReceipt>, TranscriptCommitError> {
        let bytes = &plan.bytes;
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
                file.write_all(&bytes[..cut]).unwrap();
                wait_for_kill();
            }
        }
        write_new_frame(file, bytes)?;
        let barrier = match self.guard {
            TranscriptGuard::Qualified(..) => CanonicalDurabilityBarrier::FileDataAndMetadata,
            TranscriptGuard::Unqualified(_) => CanonicalDurabilityBarrier::SyncedUnqualified,
        };
        let previous = intent.after.as_ref().map_or(0, |h| h.sequence);
        let receipts: Vec<_> = (1..=intent.members.len())
            .map(|i| TranscriptReceipt {
                sequence: previous + i as u64,
                already_accepted: false,
                barrier,
            })
            .collect();
        Ok(receipts)
    }
}

pub(super) fn matched_members(
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
