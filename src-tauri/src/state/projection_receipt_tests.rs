struct ReceiptFixture {
    dir: std::path::PathBuf,
    repo: Arc<FileMemoryRepository>,
    runtime: ProjectionRuntimeHandle,
    basis: ProjectionBasis,
}

impl ReceiptFixture {
    const SESSION: &'static str = "orchard";
    fn new() -> Self {
        Self::new_in(unique_tempdir("projection-receipt"))
    }

    fn new_in(dir: std::path::PathBuf) -> Self {
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
fn seed_projection_history(f: &ReceiptFixture, depth: usize) {
    use crate::persistence::canonical_log::PreparedProjectionStorage;
    let session = ReceiptFixture::SESSION;
    let writer = f
        .runtime
        .projection_event_writer
        .lock()
        .unwrap()
        .take()
        .unwrap();
    assert!(writer.shutdown_with_timeout(Duration::from_secs(30)));
    if depth == 0 {
        return;
    }
    let path = f.repo.projection_commit_path(session).unwrap();
    let storage = PreparedProjectionStorage::open(&path, session).unwrap();
    let source_count = f
        .runtime
        .transcript_ledger
        .lock()
        .unwrap()
        .accepted_event_count;
    let mut state = f.runtime.materialized_projection_state.lock().unwrap();
    for index in 0..depth {
        let sequence = (index / 2 + 1) as u64;
        let patch = if index % 2 == 0 {
            runtime_note_patch(sequence, f.basis.clone(), "leaf", "green")
        } else {
            runtime_graph_patch(sequence, f.basis.clone())
        };
        let receipt = storage.commit_after(&patch, Some(source_count)).unwrap();
        assert_eq!(receipt.head.sequence, index as u64 + 1);
        state.apply_replayed_patch(&patch, None).unwrap();
        state.bind_projection_head(&patch.kind, receipt.head);
    }
    assert_eq!(state.notes.notes.len(), 1);
    assert_eq!(state.graph.nodes.len(), 1);
    f.repo
        .save_materialized_notes(session, &state.notes)
        .unwrap();
    f.repo
        .save_materialized_graph(session, &state.graph)
        .unwrap();
}

// A fresh process isolates allocation and residency from fixture construction
// and the simultaneous reader. Its RSS includes allocator-retained scratch
// space and page residency; the deltas are approximate, not live-heap sizes.
#[cfg(not(debug_assertions))]
fn measure_projection_cache_memory(root: std::path::PathBuf, source_depth: u64) {
    use crate::persistence::canonical_log::{CanonicalRecord, PreparedProjectionStorage};
    use crate::projections::TranscriptEvent;
    let session = ReceiptFixture::SESSION;
    let repo = FileMemoryRepository::with_data_root(root);
    let storage =
        PreparedProjectionStorage::open(&repo.projection_commit_path(session).unwrap(), session)
            .unwrap();
    let mut system = sysinfo::System::new();
    let pid = sysinfo::get_current_pid().unwrap();
    let mut rss = || {
        system.refresh_processes_specifics(
            sysinfo::ProcessesToUpdate::Some(&[pid]),
            false,
            sysinfo::ProcessRefreshKind::nothing().with_memory(),
        );
        system.process(pid).unwrap().memory()
    };
    let before = rss();
    storage.ready().unwrap();
    let after_stream = rss();
    let patch = runtime_note_patch(
        1,
        ProjectionBasis::from_transcript_events(&[]),
        "leaf",
        "green",
    );
    let proof = storage.live_basis(&patch, source_depth).unwrap();
    assert_eq!(proof.heads()["transcript_revisions"].sequence, source_depth);
    let after_sources = rss();
    println!(
        "projection_cache_memory before_bytes={before} stream_ready_bytes={after_stream} source_ready_bytes={after_sources} record_inline_bytes={} event_inline_bytes={}",
        std::mem::size_of::<CanonicalRecord<TranscriptEvent>>(),
        std::mem::size_of::<TranscriptEvent>()
    );
}

#[cfg(not(debug_assertions))]
fn observe_projection_cache_memory(
    f: &ReceiptFixture,
    source_depth: usize,
    projection_depth: usize,
) {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "state::rotation_tests::release_projection_receipt_latency",
            "--exact",
            "--ignored",
            "--nocapture",
        ])
        .env("AUDIOGRAPH_PROJECTION_MEMORY_FIXTURE", &f.dir)
        .env(
            "AUDIOGRAPH_PROJECTION_MEMORY_SOURCE_DEPTH",
            source_depth.to_string(),
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "cache measurement child failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let (_, measurement) = stdout.split_once("projection_cache_memory ").unwrap();
    println!(
        "projection_cache_memory source_depth={source_depth} projection_depth={projection_depth} {}",
        measurement.lines().next().unwrap()
    );
}

#[cfg(not(debug_assertions))]
fn profile_projection_apply(
    f: &ReceiptFixture,
    patch: ProjectionPatch,
) -> crate::persistence::projection_profile::Sample {
    use crate::persistence::projection_profile;
    let (send, result) = std::sync::mpsc::channel();
    let observe = move |save: &mut dyn FnMut() -> Result<(), String>| {
        let start = std::time::Instant::now();
        let saved = save();
        let mut sample = projection_profile::sample();
        sample.snapshot = start.elapsed();
        send.send(sample).unwrap();
        saved
    };
    let observe_graph = observe.clone();
    let notes = f.repo.clone();
    let graph = f.repo.clone();
    let receipt = f
        .runtime
        .apply_runtime_projection_patch_with_savers(
            ReceiptFixture::SESSION,
            &f.basis,
            patch,
            move |id, lane| observe(&mut || notes.save_materialized_notes(id, lane)),
            move |id, lane| observe_graph(&mut || graph.save_materialized_graph(id, lane)),
        )
        .unwrap();
    assert!(receipt.materialized_snapshot_saved);
    result.try_recv().unwrap()
}

#[cfg(not(debug_assertions))]
#[test]
#[ignore = "explicit release measurement of projection apply and cache save"]
fn release_projection_receipt_latency() {
    if let Some(root) = std::env::var_os("AUDIOGRAPH_PROJECTION_MEMORY_FIXTURE") {
        let depth = std::env::var("AUDIOGRAPH_PROJECTION_MEMORY_SOURCE_DEPTH")
            .unwrap()
            .parse()
            .unwrap();
        measure_projection_cache_memory(root.into(), depth);
        return;
    }
    let profile = std::env::var("AUDIOGRAPH_PROJECTION_LOCK_PROFILE").as_deref() == Ok("1");
    let vary_history =
        profile || std::env::var("AUDIOGRAPH_PROJECTION_HISTORY_MATRIX").as_deref() == Ok("1");
    let cells: &[(usize, usize)] = if profile {
        &[(10_000, 10_000)]
    } else if vary_history {
        &[(10_000, 0), (10_000, 1_000), (10_000, 10_000)]
    } else if std::env::var("AUDIOGRAPH_PROJECTION_LATENCY_10K_ONLY").as_deref() == Ok("1") {
        &[(10_000, 0)]
    } else {
        &[(1_000, 0), (10_000, 0), (50_000, 0)]
    };
    for &(depth, projection_depth) in cells {
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
        let mut index = crate::projections::TranscriptLiveIndex::default();
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
                index.apply(&mut ledger, event).unwrap();
            }
            done += size;
        }
        f.basis = ledger.current_basis();
        drop(ledger);
        if vary_history {
            seed_projection_history(&f, projection_depth);
            observe_projection_cache_memory(&f, depth, projection_depth);
            let open = std::time::Instant::now();
            let writer =
                ProjectionEventWriter::repository(ReceiptFixture::SESSION, f.repo.clone()).unwrap();
            *f.runtime.projection_event_writer.lock().unwrap() = Some(writer);
            println!(
                "projection_cold_open source_depth={depth} projection_depth={projection_depth} time_us={}",
                open.elapsed().as_micros()
            );
        }
        let preparation = prepared.elapsed().as_millis();
        let stop = Arc::new(AtomicBool::new(false));
        let stop_reader = stop.clone();
        let reader_repo = f.repo.clone();
        let (ready, started) = std::sync::mpsc::channel();
        let reads: Arc<[AtomicU64; 4]> = Arc::new(std::array::from_fn(|_| AtomicU64::new(0)));
        let counters = reads.clone();
        let snapshot = || -> [u64; 4] { std::array::from_fn(|i| reads[i].load(Ordering::Relaxed)) };
        let reader = std::thread::spawn(move || {
            let mut first = true;
            loop {
                match reader_repo.load_transcript_events(ReceiptFixture::SESSION) {
                    Ok(s) => {
                        assert!(!s.is_empty());
                        counters[0].fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        counters[1].fetch_add(1, Ordering::Relaxed);
                    }
                }
                match reader_repo.load_projection_patches(ReceiptFixture::SESSION) {
                    Ok(_) => {
                        counters[2].fetch_add(1, Ordering::Relaxed);
                    }
                    Err(_) => {
                        counters[3].fetch_add(1, Ordering::Relaxed);
                    }
                }
                if first {
                    first = false;
                    ready.send(()).unwrap();
                }
                if stop_reader.load(Ordering::Acquire) {
                    return;
                }
            }
        });
        started.recv_timeout(Duration::from_secs(120)).unwrap();
        let mut first_sequence = (projection_depth / 2) as u64;
        if vary_history {
            first_sequence += 1;
            let cold = std::time::Instant::now();
            assert!(f.apply(first_sequence).unwrap().materialized_snapshot_saved);
            println!(
                "projection_cold_apply source_depth={depth} projection_depth={projection_depth} kind=Notes time_us={}",
                cold.elapsed().as_micros()
            );
            // Both cold-empty and seeded cells now maintain one note and one node.
            f.apply_patch(runtime_graph_patch(first_sequence, f.basis.clone()))
                .unwrap();
        }
        let mut reader = Some(reader);
        for with_reader in [true, false] {
            if !with_reader {
                if !profile {
                    break;
                }
                stop.store(true, Ordering::Release);
                reader.take().unwrap().join().unwrap();
            }
            for kind in [ProjectionKind::Notes, ProjectionKind::Graph] {
                let mut sequence = first_sequence;
                for changing in [false, true] {
                    let mut samples = Vec::new();
                    let mut starts = snapshot();
                    for i in 0..52 {
                        if changing {
                            let mut row =
                                projection_transcript_event(&format!("later-{done}"), 1, "reed");
                            row.start_time = done as f64;
                            row.end_time = row.start_time + 1.;
                            storage.commit_after(&row, Some(done as u64)).unwrap();
                            let mut ledger = f.runtime.transcript_ledger.lock().unwrap();
                            index.apply(&mut ledger, row).unwrap();
                            f.basis = ledger.current_basis();
                            done += 1;
                        }
                        sequence += 1;
                        let patch = match kind {
                            ProjectionKind::Notes => {
                                runtime_note_patch(sequence, f.basis.clone(), "leaf", "green")
                            }
                            ProjectionKind::Graph => runtime_graph_patch(sequence, f.basis.clone()),
                        };
                        if i == 2 {
                            starts = snapshot();
                        }
                        let start = std::time::Instant::now();
                        let counters = profile_projection_apply(&f, patch);
                        let elapsed = start.elapsed().as_micros();
                        if i >= 2 {
                            samples.push((elapsed, counters));
                        }
                    }
                    let ends = snapshot();
                    let [
                        transcripts,
                        transcript_refused,
                        projections,
                        projection_refused,
                    ] = std::array::from_fn(|i| ends[i] - starts[i]);
                    samples.sort_unstable_by_key(|(elapsed, _)| *elapsed);
                    let (p95, at_p95) = &samples[47];
                    let retries: usize = samples.iter().map(|(_, c)| c.retries).sum();
                    let contended = samples.iter().filter(|(_, c)| c.retries > 0).count();
                    println!(
                        "projection_lock_profile kind={kind:?} moving={changing} reader={with_reader} contended={contended} retries={retries} p95_us={p95} basis_us={} commit_us={} snapshot_us={} shared_wait_us={} exclusive_wait_us={} p95_retries={} intent_publish_us={} frame_write_sync_us={} intent_retire_us={}",
                        at_p95.basis.as_micros(),
                        at_p95.commit.as_micros(),
                        at_p95.snapshot.as_micros(),
                        at_p95.shared_wait.as_micros(),
                        at_p95.exclusive_wait.as_micros(),
                        at_p95.retries,
                        at_p95.intent_publish.as_micros(),
                        at_p95.frame_write_sync.as_micros(),
                        at_p95.intent_retire.as_micros(),
                    );
                    println!(
                        "projection_latency kind={kind:?} starting_revisions={depth} projection_depth={projection_depth} changing_basis={changing} prep_ms={preparation} samples=50 p50_us={} p95_us={} transcript_completed={transcripts} transcript_refused={transcript_refused} projection_completed={projections} projection_refused={projection_refused}",
                        samples[24].0, p95
                    );
                }
            }
            first_sequence += 104; // Two runs of 52 patches on each lane.
        }
        stop.store(true, Ordering::Release);
        if let Some(reader) = reader {
            reader.join().unwrap();
        }
        let [completed, refused, projection_completed, projection_refused] = snapshot();
        println!(
            "projection_reader starting_revisions={depth} projection_depth={projection_depth} transcript_completed={completed} transcript_refused={refused} projection_completed={projection_completed} projection_refused={projection_refused}"
        );
    }
}
