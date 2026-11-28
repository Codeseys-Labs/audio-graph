//! Resolve every published descriptor before planning a request. A singleton
//! is one member. Only the source bytes and verified descriptor decide which
//! members survive; the request that woke the writer has no bearing on it.
use super::*;
use group::Member;

impl Intent {
    fn checked_members(&self) -> Result<Vec<Member>, TranscriptCommitError> {
        if !valid_attempt(&self.attempt)
            || !valid_attempt(&self.owner)
            || !valid_hash(&self.frame_hash)
            || self.frame_len == 0
            || (self.source_was_absent && (self.base_len != 0 || self.after.is_some()))
        {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        if self.members.is_empty() {
            return Ok(vec![Member {
                length: usize::try_from(self.frame_len)
                    .map_err(|_| TranscriptCommitError::InvalidHistory)?,
                hash: self.frame_hash.clone(),
            }]);
        }
        if !(2..=crate::persistence::MAX_TRANSCRIPT_GROUP).contains(&self.members.len())
            || self.source_was_absent
            || self.frame_len > group::MAX_GROUP_BYTES as u64
            || self
                .members
                .iter()
                .any(|m| m.length == 0 || !valid_hash(&m.hash))
            || self
                .members
                .iter()
                .try_fold(0_u64, |n, m| n.checked_add(m.length as u64))
                != Some(self.frame_len)
        {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        Ok(self.members.clone())
    }
}

impl<E: CommitPayload> Commit<'_, E> {
    pub(super) fn resolve_lease(
        &self,
        lease: &mut Lease<E>,
    ) -> Result<bool, TranscriptCommitError> {
        let path = self.path.with_added_extension("attempt");
        let Some(bytes) = checked_bytes_bounded(&path, 4096)? else {
            // A process can stop between origin and descriptor publication.
            self.clear_attempt_origin();
            let Some(pending) = &mut lease.pending else {
                return Ok(false);
            };
            if pending.retiring.is_some() {
                // Source sync succeeded and the previous unlink already took
                // effect. Re-establish its namespace barrier before delivery.
                self.unlink(&path)?;
                self.recovered
                    .borrow_mut()
                    .extend(pending.retiring.take().into_iter().flatten());
            }
            lease.pending = None;
            return Ok(true);
        };
        let intent: Intent =
            serde_json::from_slice(&bytes).map_err(|_| TranscriptCommitError::InvalidHistory)?;
        let pending = lease
            .pending
            .get_or_insert_with(|| PendingPublication { retiring: None });
        self.sync_existing_intent(&path)?;
        let recovered = self.resolve_members(&intent)?;
        if let Err(error) = self.finish_intent(&path) {
            pending.retiring = Some(recovered);
            return Err(error);
        }
        self.recovered.borrow_mut().extend(recovered);
        lease.pending = None;
        Ok(true)
    }

    fn resolve_members(
        &self,
        intent: &Intent,
    ) -> Result<Vec<RecoveredEvent<E>>, TranscriptCommitError> {
        let members = intent.checked_members()?;
        if !intent.projection_lanes.is_empty()
            && (E::STREAM != GapStream::ProjectionPatches
                || intent.projection_lanes.len() != members.len())
        {
            return Err(TranscriptCommitError::InvalidHistory);
        }

        let mut file = match self.guard.open_recovery_source(self.path) {
            Ok(file) => Some(file),
            Err(CanonicalDurabilityRejection::IoFailedBeforeMutation {
                kind: io::ErrorKind::NotFound,
                ..
            }) if intent.source_was_absent => None,
            Err(e) => return Err(TranscriptCommitError::Rejected(e)),
        };
        let mut bytes = Vec::new();
        if let Some(file) = &mut file {
            file.read_to_end(&mut bytes)
                .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?;
        }
        let base =
            usize::try_from(intent.base_len).map_err(|_| TranscriptCommitError::InvalidHistory)?;
        let (prefix, suffix) = bytes
            .split_at_checked(base)
            .ok_or(TranscriptCommitError::InvalidHistory)?;
        let preceding = self.cache(prefix)?.0;
        if suffix.len() as u64 > intent.frame_len || head(&preceding) != intent.after {
            return Err(TranscriptCommitError::InvalidHistory);
        }
        let evidence_path = self
            .path
            .with_added_extension(format!("{}.quarantine", intent.attempt));
        let evidence = checked_bytes_bounded(&evidence_path, intent.frame_len)?;
        // The previous protocol could quarantine an entire matching singleton.
        // Its already published disposition survives an upgrade or second crash.
        let eligible = if intent.members.is_empty() && evidence.as_deref() == Some(suffix) {
            &[][..]
        } else {
            // A corrupt next member can end in (or equal) bytes from a kept
            // member. After truncation never strip such an accidental match
            // from the authenticated prefix of a group.
            suffix
        };
        let (mut length, mut count) = group::matched_members(eligible, &members)?;
        if count == members.len() {
            let gap = self
                .path
                .with_added_extension(format!("{}.gap", intent.attempt));
            if digest(suffix) != intent.frame_hash
                || evidence.is_some()
                || gap
                    .try_exists()
                    .map_err(|e| io_failure(CanonicalAppendPhase::RecoveryRead, e))?
            {
                return Err(TranscriptCommitError::InvalidHistory);
            }
        }
        let previous = intent.after.as_ref().map_or(0, |h| h.sequence);
        let mut rows = Vec::with_capacity(count);
        let mut after = intent.after.clone();
        if count > 0 {
            self.cache_and_visit(&bytes[..base + length], |sequence, hash, heads, event| {
                if sequence > previous {
                    rows.push((sequence, hash.to_owned(), heads.clone(), event.clone()));
                }
            })?;
            if rows.len() != count {
                return Err(TranscriptCommitError::InvalidHistory);
            }
            for (i, (sequence, hash, heads, event)) in rows.iter().enumerate() {
                if !intent.projection_lanes.is_empty()
                    && event.projection_kind() != intent.projection_lanes.get(i)
                {
                    return Err(TranscriptCommitError::InvalidHistory);
                }
                if !event.recovery_basis(self.path, self.session, heads, self.source_basis)? {
                    count = i;
                    length = members[..count].iter().map(|member| member.length).sum();
                    break;
                }
                after = Some(VerifiedTranscriptHead {
                    sequence: *sequence,
                    record_hash: hash.clone(),
                });
            }
            rows.truncate(count);
        }
        let mut recovered = Vec::with_capacity(count);
        if let Some((sequence, hash, _, event)) = rows.first() {
            let first = tier::ReceiptTierWitness {
                sequence: *sequence,
                record_hash: hash.clone(),
                event_id: event.event_id()?,
            };
            let file = file.as_mut().ok_or(TranscriptCommitError::InvalidHistory)?;
            self.guard
                .revalidate_recovery_source(self.path, file)
                .map_err(TranscriptCommitError::Rejected)?;
            file.seek(SeekFrom::Start(intent.base_len))
                .map_err(|e| io_failure(CanonicalAppendPhase::Write, e))?;
            // A second fsync alone can succeed after a failed writeback.
            write_frame(file, &suffix[..length])?;
            let barrier = self.sync_namespace()?;
            self.record_barrier(&first, &preceding, Some(intent));
            recovered.extend(rows.into_iter().map(|(sequence, record_hash, _, event)| {
                RecoveredEvent {
                    record_hash,
                    event,
                    receipt: TranscriptReceipt {
                        sequence,
                        already_accepted: false,
                        barrier,
                    },
                }
            }));
        }
        if count < members.len() {
            let mut projection_lanes = Vec::new();
            for lane in intent.projection_lanes.iter().skip(count) {
                if !projection_lanes.contains(lane) {
                    projection_lanes.push(lane.clone());
                }
            }
            let remainder = Intent {
                projection_lanes,
                base_len: intent.base_len + length as u64,
                after,
                ..intent.clone()
            };
            self.recover_gap(&remainder, &suffix[length..], file.as_ref())?;
            if count == 0 && intent.source_was_absent {
                drop(file);
                self.unlink(self.path)?;
            }
        }
        Ok(recovered)
    }
}
