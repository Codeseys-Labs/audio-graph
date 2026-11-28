struct ReceiptFixture {
    dir: std::path::PathBuf,
    repo: Arc<FileMemoryRepository>,
    runtime: ProjectionRuntimeHandle,
    basis: ProjectionBasis,
}

impl ReceiptFixture {
    const SESSION: &'static str = "orchard";
    fn new() -> Self {
        let dir = unique_tempdir("projection-receipt");
        let repo = Arc::new(FileMemoryRepository::with_data_root(&dir));
        let runtime = ProjectionRuntimeHandle::in_memory_for_tests(Self::SESSION);
        let event = projection_transcript_event("stem", 1, "branch");
        repo.append_transcript_event(Self::SESSION, &event).unwrap();
        let basis = {
            let mut ledger = runtime.transcript_ledger.lock().unwrap();
            ledger.apply_event(event).unwrap();
            ledger.current_basis()
        };
        *runtime.projection_event_writer.lock().unwrap() =
            ProjectionEventWriter::repository(Self::SESSION, repo.clone());
        Self {
            dir,
            repo,
            runtime,
            basis,
        }
    }
    fn apply(&self, seq: u64) -> Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError> {
        self.apply_patch(runtime_note_patch(seq, self.basis.clone(), "leaf", "green"))
    }
    fn apply_patch(
        &self,
        patch: ProjectionPatch,
    ) -> Result<ProjectionRuntimeApplyResult, ProjectionRuntimeApplyError> {
        let notes = self.repo.clone();
        let graph = notes.clone();
        self.runtime.apply_runtime_projection_patch_with_savers(
            Self::SESSION,
            &self.basis,
            patch,
            move |id, m| notes.save_materialized_notes(id, m),
            move |id, m| graph.save_materialized_graph(id, m),
        )
    }
}
impl Drop for ReceiptFixture {
    fn drop(&mut self) {
        if let Some(writer) = self.runtime.projection_event_writer.lock().unwrap().take() {
            writer.shutdown_with_timeout(Duration::from_secs(3));
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn uncertain_projection_receipt_is_materialized_without_a_second_patch_and_exact_retry_is_idempotent()
 {
    let f = ReceiptFixture::new();
    f.runtime
        .projection_event_writer
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .fail_next_projection_sync();
    let receipt = f.apply(1).expect("the actor settles its uncertain frame");
    assert!(!receipt.already_accepted && receipt.materialized_snapshot_saved);
    let retry = f
        .apply(1)
        .expect("redelivery may omit generated apply telemetry");
    assert!(retry.already_accepted);
    let rows = f
        .repo
        .load_projection_patches(ReceiptFixture::SESSION)
        .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        f.runtime
            .materialized_projection_state
            .lock()
            .unwrap()
            .notes
            .last_sequence,
        1
    );
    assert_eq!(
        f.repo
            .load_materialized_notes(ReceiptFixture::SESSION)
            .unwrap()
            .unwrap()
            .last_sequence,
        1
    );
}

#[test]
fn projection_snapshot_io_holds_no_capture_or_rotation_mutex_and_sees_its_receipt() {
    let f = ReceiptFixture::new();
    let (entered, inside) = std::sync::mpsc::channel();
    let (release, done) = std::sync::mpsc::channel();
    let repo = f.repo.clone();
    let result = f
        .runtime
        .queue_projection_apply(
            ReceiptFixture::SESSION,
            &f.basis,
            runtime_note_patch(1, f.basis.clone(), "leaf", "green"),
            ReceiptFixture::SESSION,
            Vec::new(),
            projection_apply::Savers::new(
                move |session, notes| {
                    let rows = repo.load_projection_patches(session)?;
                    assert_eq!(rows.len(), 1);
                    assert_eq!(notes.last_sequence, 1);
                    let projected = repo.load_projection_patch_stream(session)?;
                    let strict = match projected {
                        crate::persistence::canonical_reader::StrictCanonicalRead::Present(
                            snapshot,
                        ) => snapshot,
                        _ => panic!("canonical projection must exist before snapshot save"),
                    };
                    let row = strict.records.last().unwrap();
                    let head = notes.projection_log_head.as_ref().unwrap();
                    assert_eq!(
                        (head.sequence, &head.record_hash),
                        (row.sequence, &row.record_hash)
                    );
                    entered.send(()).unwrap();
                    done.recv_timeout(Duration::from_secs(5)).unwrap();
                    Err("cache lag".into())
                },
                |_, _| unreachable!(),
            ),
        )
        .unwrap();
    inside.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(f.runtime.transcript_publication.try_write().is_ok());
    assert!(f.runtime.transcript_ledger.try_lock().is_ok());
    assert!(f.runtime.projection_event_writer.try_lock().is_ok());
    assert_eq!(
        f.runtime
            .materialized_projection_state
            .try_lock()
            .unwrap()
            .notes
            .last_sequence,
        1
    );
    release.send(()).unwrap();
    let applied = result
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    assert!(!applied.materialized_snapshot_saved);
    assert_eq!(
        f.apply(2).unwrap().outcome,
        MaterializedProjectionApplyOutcome::Notes {
            last_sequence: 2,
            note_count: 1
        }
    );
}

#[test]
fn a_queued_projection_rechecks_cited_revisions_before_any_append() {
    let f = ReceiptFixture::new();
    let (entered, hold) = std::sync::mpsc::channel();
    let (release, done) = std::sync::mpsc::channel();
    let first = f
        .runtime
        .queue_projection_apply(
            ReceiptFixture::SESSION,
            &f.basis,
            runtime_note_patch(1, f.basis.clone(), "leaf", "green"),
            ReceiptFixture::SESSION,
            vec![],
            projection_apply::Savers::new(
                move |_, _| {
                    entered.send(()).unwrap();
                    done.recv_timeout(Duration::from_secs(10)).unwrap();
                    Ok(())
                },
                |_, _| unreachable!(),
            ),
        )
        .unwrap();
    hold.recv_timeout(Duration::from_secs(10)).unwrap();
    let queued = f
        .runtime
        .queue_projection_apply(
            ReceiptFixture::SESSION,
            &f.basis,
            runtime_note_patch(2, f.basis.clone(), "leaf", "green"),
            ReceiptFixture::SESSION,
            vec![],
            projection_apply::Savers::new(|_, _| unreachable!(), |_, _| unreachable!()),
        )
        .unwrap();
    let revision = projection_transcript_event("stem", 2, "wood");
    f.repo
        .append_transcript_event(ReceiptFixture::SESSION, &revision)
        .unwrap();
    f.runtime
        .transcript_ledger
        .lock()
        .unwrap()
        .apply_event(revision)
        .unwrap();
    release.send(()).unwrap();
    first
        .recv_timeout(Duration::from_secs(10))
        .unwrap()
        .unwrap();
    assert!(matches!(
        queued.recv_timeout(Duration::from_secs(10)).unwrap(),
        Err(ProjectionRuntimeApplyError::Apply {
            error: ProjectionApplyError::StaleBasis { .. }
        })
    ));
    assert_eq!(
        f.repo
            .load_projection_patches(ReceiptFixture::SESSION)
            .unwrap()
            .len(),
        1
    );
}

#[cfg(not(debug_assertions))]
#[test]
#[ignore = "explicit release measurement of projection apply and cache save"]
fn release_projection_receipt_latency() {
    for depth in [1_usize, 1_000] {
        let prepared = std::time::Instant::now();
        let mut f = ReceiptFixture::new();
        let path = f
            .dir
            .join("transcripts")
            .join(format!("{}.events.jsonl", ReceiptFixture::SESSION));
        let storage = crate::persistence::canonical_log::PreparedTranscriptStorage::open(
            &path,
            ReceiptFixture::SESSION,
        )
        .unwrap();
        storage.ready().unwrap();
        let mut done = 1;
        // Four ASR revisions per span, including partials before final. The
        // large setup is excluded from every materialization timing.
        let mut ledger = f.runtime.transcript_ledger.lock().unwrap();
        while done < depth {
            let size = (depth - done).min(crate::persistence::MAX_TRANSCRIPT_GROUP);
            let events: Vec<_> = (done..done + size)
                .map(|i| {
                    let revision = (i - 1) % 4 + 1;
                    let mut row = projection_transcript_event(
                        &format!("span-{}", (i - 1) / 4),
                        revision as u64,
                        "stem",
                    );
                    row.start_time = ((i - 1) / 4) as f64;
                    row.end_time = row.start_time + 1.0;
                    row.received_at_ms = 1_700_000_000_000 + i as u64;
                    if revision < 4 {
                        row.is_final = false;
                        row.stability = crate::projections::TranscriptEventStability::Partial;
                    }
                    row
                })
                .collect();
            if storage
                .commit_group_after(&events, done as u64)
                .unwrap()
                .is_none()
            {
                for (n, row) in events.iter().enumerate() {
                    storage.commit_after(row, Some((done + n) as u64)).unwrap();
                }
            }
            for event in events {
                ledger.apply_event(event).unwrap();
            }
            done += size;
        }
        f.basis = ledger.current_basis();
        let observed = ledger.clone();
        drop(ledger);
        let bytes = std::fs::metadata(&path).unwrap().len();
        let preparation = prepared.elapsed().as_millis();
        for kind in [ProjectionKind::Notes, ProjectionKind::Graph] {
            let mut source = MaterializedProjectionState::new(ReceiptFixture::SESSION);
            let baseline_dir = unique_tempdir("projection-cache-baseline");
            let baseline_repo = FileMemoryRepository::with_data_root(&baseline_dir);
            let mut baseline = Vec::new();
            let mut activation = Vec::new();
            for seq in 1..=52 {
                let patch = match kind {
                    ProjectionKind::Notes => {
                        runtime_note_patch(seq, f.basis.clone(), "leaf", "green")
                    }
                    ProjectionKind::Graph => runtime_graph_patch(seq, f.basis.clone()),
                };
                // Old foreground work: materialize and sync its derived cache.
                let start = std::time::Instant::now();
                source
                    .apply_validated_patch_reporting_currency(&observed, &patch)
                    .unwrap();
                match kind {
                    ProjectionKind::Notes => baseline_repo
                        .save_materialized_notes(ReceiptFixture::SESSION, &source.notes),
                    ProjectionKind::Graph => baseline_repo
                        .save_materialized_graph(ReceiptFixture::SESSION, &source.graph),
                }
                .unwrap();
                let previous = start.elapsed().as_micros();
                let start = std::time::Instant::now();
                let receipt = f.apply_patch(patch).unwrap();
                let current = start.elapsed().as_micros();
                assert!(receipt.materialized_snapshot_saved);
                if seq > 2 {
                    baseline.push(previous);
                    activation.push(current);
                }
            }
            baseline.sort_unstable();
            activation.sort_unstable();
            println!(
                "projection_kind={kind:?} transcript_revisions={depth} transcript_bytes={bytes} prep_ms={preparation} warmups=2 samples=50 baseline_p50_us={} baseline_p95_us={} receipt_and_snapshot_p50_us={} receipt_and_snapshot_p95_us={}",
                baseline[24], baseline[47], activation[24], activation[47]
            );
            let _ = std::fs::remove_dir_all(baseline_dir);
        }
    }
}
