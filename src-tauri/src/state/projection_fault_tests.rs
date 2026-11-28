#[test]
fn recovered_projection_matches_live_pre_placement_request_and_its_exact_retry() {
    let f = ReceiptFixture::new();
    let mut patch = runtime_note_patch(1, f.basis.clone(), "leaf", "green");
    patch.provenance.provider = "openrouter".into();
    patch.provenance.route_id = None;
    patch.provenance.model_placement = None;
    patch.provenance.model = "garden/fern".into();
    f.runtime
        .projection_event_writer
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .fail_next_projection_sync();
    let result = f.apply_patch(patch.clone()).unwrap();
    assert!(result.materialized_snapshot_saved && !result.already_accepted);
    assert!(f.apply_patch(patch).unwrap().already_accepted);
    let replay = f
        .repo
        .replay_projection_state(ReceiptFixture::SESSION)
        .unwrap();
    let installed = f.runtime.materialized_projection_state.lock().unwrap();
    assert_eq!(installed.notes, replay.state.notes);
    assert_eq!(installed.notes.last_sequence, 1);
}

#[test]
fn projection_disk_full_reports_storage_full_without_advancing_the_live_or_saved_lane() {
    let f = ReceiptFixture::new();
    f.apply(1).unwrap();
    let snapshot = f
        .repo
        .load_materialized_notes(ReceiptFixture::SESSION)
        .unwrap()
        .unwrap();
    f.runtime
        .projection_event_writer
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .fail_next_projection_write_disk_full();
    assert!(matches!(
        f.apply(2),
        Err(ProjectionRuntimeApplyError::ProjectionStorage {
            reason: ProjectionSaveError::StorageFull
        })
    ));
    assert_eq!(
        f.runtime
            .materialized_projection_state
            .lock()
            .unwrap()
            .notes,
        snapshot
    );
    assert_eq!(
        f.repo
            .load_materialized_notes(ReceiptFixture::SESSION)
            .unwrap(),
        Some(snapshot)
    );
    assert_eq!(
        f.repo
            .load_projection_patches(ReceiptFixture::SESSION)
            .unwrap()
            .len(),
        1
    );
}

pub(crate) fn snapshot_replace_child(dir: &std::path::Path, checkpoint: fn() -> !) -> ! {
    let f = ReceiptFixture::new_in(dir.into());
    assert!(f.apply(1).unwrap().materialized_snapshot_saved);
    let repo = f.repo.clone();
    let response = f
        .runtime
        .queue_projection_apply(
            ReceiptFixture::SESSION,
            &f.basis,
            runtime_note_patch(2, f.basis.clone(), "leaf", "gold"),
            ReceiptFixture::SESSION,
            Vec::new(),
            projection_apply::Savers::new(
                move |id, notes| {
                    crate::persistence::SNAPSHOT_BEFORE_RENAME.set(Some(checkpoint));
                    repo.save_materialized_notes(id, notes)
                },
                |_, _| unreachable!("notes snapshot"),
            ),
        )
        .unwrap();
    panic!(
        "snapshot writer returned before the checkpoint: {:?}",
        response.recv()
    );
}

#[test]
fn killing_the_projection_snapshot_writer_recovers_the_committed_patch_over_the_previous_snapshot()
{
    let dir = unique_tempdir("snapshot-cut");
    crate::persistence::canonical_log::kill_at_writer_checkpoint(&dir, "snapshot-replace", || {});
    // The parent opens new storage after the OS has killed and reaped the child.
    let repo = FileMemoryRepository::with_data_root(&dir);
    let id = ReceiptFixture::SESSION;
    let original = repo.load_materialized_notes(id).unwrap().unwrap();
    assert_eq!(original.last_sequence, 1);
    assert_eq!(original.projection_log_head.as_ref().unwrap().sequence, 1);
    let staged: MaterializedNotes =
        crate::persistence::load_json(&dir.join("notes").join(format!("{id}.json.tmp"))).unwrap();
    let verified = repo.replay_projection_state(id).unwrap();
    assert_eq!(verified.validation.invalid_patch_count, 0);
    assert_eq!(verified.state.notes.last_sequence, 2);
    assert_eq!(
        verified
            .state
            .notes
            .projection_log_head
            .as_ref()
            .unwrap()
            .sequence,
        2
    );
    assert_eq!(verified.state.notes, staged);
    assert_eq!(repo.load_projection_patches(id).unwrap().len(), 2);
    assert_eq!(repo.load_materialized_notes(id).unwrap(), Some(original));
    std::fs::remove_dir_all(dir).unwrap();
}
