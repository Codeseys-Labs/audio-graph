//! Projection history verification belongs to the capture that requested it.
use super::*;
use crate::persistence::{FileMemoryRepository, VerifiedCaptureCoverage};
use crate::projections::ProjectionCoverageStatus::BehindTranscript;

pub(crate) fn spawn_capture_verification(
    state: &crate::state::AppState,
    app: &AppHandle,
    settings: &crate::settings::AppSettings,
    check: Arc<()>,
) {
    let provider = settings.llm_provider.clone();
    let dispatch = ProjectionDispatchContext {
        speaker_timeline: state.speaker_timeline.clone(),
        transcript_ledger: state.transcript_ledger.clone(),
        projection_schedulers: state.projection_schedulers.clone(),
        projection_runtime: state.projection_runtime_handle(),
        projection_job_workers: state.projection_job_workers.clone(),
        projection_lane_stopping: state.projection_lane_stopping.clone(),
        event_sink: Arc::new(TauriProjectionRuntimeEventSink {
            app_handle: app.clone(),
        }),
        patch_generator: Arc::new(ExecutorProjectionPatchGenerator {
            llm_executor: state.llm_executor.clone(),
            llm_provider: provider.clone(),
        }),
        llm_provider: provider,
        llm_allow_cloud_fallbacks: settings
            .privacy_mode
            .allows_session_cloud_content_transfer(),
        data_movement_sink: Arc::new(FileProjectionDataMovementSink),
    };
    #[cfg(all(test, not(debug_assertions)))]
    let dispatch = test_probe::with_timing_generator(dispatch);
    launch(state, check, dispatch);
}

pub(super) fn launch(
    state: &crate::state::AppState,
    check: Arc<()>,
    dispatch: ProjectionDispatchContext,
) {
    let session = state.current_session_id();
    let lifecycle = state.session_lifecycle.clone();
    let view = state.materialized_projection_state.clone();
    let publication = state.transcript_publication.clone();
    #[cfg(test)]
    let probe = test_probe::take();
    tauri::async_runtime::spawn_blocking(move || {
        // Start owns this mutex until ASR has started. Release it before
        // reading any history or acquiring any filesystem coordination guard.
        {
            let _lifecycle = lifecycle.blocking_lock();
            if !dispatch
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .owns_coverage_check(&session, &check)
            {
                return;
            }
        }
        #[cfg(test)]
        let _probe = test_probe::enter(probe);
        #[cfg(test)]
        test_probe::point(test_probe::Phase::BeforeRead);
        let still_active = || {
            dispatch
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .owns_coverage_check(&session, &check)
        };
        let verified = FileMemoryRepository::user_data().verify_capture_coverage(&session, || {
            still_active()
                .then_some(())
                .ok_or_else(|| "capture verification cancelled".into())
        });
        if !still_active() {
            return;
        }
        #[cfg(test)]
        test_probe::point(test_probe::Phase::BeforeInstall);

        // Serialize acceptance and dispatch against Stop, and against the
        // session swap. A new visit to the same session gets a different token.
        let _lifecycle = lifecycle.blocking_lock();
        let _epoch = publication.read().unwrap_or_else(|p| p.into_inner());
        let decisions = {
            let ledger = dispatch
                .transcript_ledger
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            let mut lanes = dispatch
                .projection_schedulers
                .lock()
                .unwrap_or_else(|p| p.into_inner());
            if !lanes.owns_coverage_check(&session, &check) || ledger.session_id != session {
                return;
            }
            let ok = verified.is_ok();
            let (coverage, behind, resume_basis) = match verified {
                Ok(VerifiedCaptureCoverage {
                    coverage,
                    transcript_count,
                    behind,
                    resume_basis,
                }) => {
                    let same_prefix = transcript_count == ledger.accepted_event_count;
                    // A partial can extend the live prefix without creating an
                    // eligible scheduler observation. Keep the verified repair
                    // for that prefix. A newer observation, if any, supersedes
                    // its basis and normal job admission rechecks it.
                    let basis = if transcript_count <= ledger.accepted_event_count {
                        resume_basis
                    } else {
                        None
                    };
                    (
                        coverage,
                        if same_prefix { behind } else { [true, true] },
                        basis,
                    )
                }
                Err(_) => {
                    log::warn!("projection_capture.coverage_unverified");
                    (Default::default(), [true, true], None)
                }
            };
            let (heads, gaps) = coverage;
            {
                let mut view = view.lock().unwrap_or_else(|p| p.into_inner());
                view.notes.transcript_coverage = behind[0].then_some(BehindTranscript);
                view.graph.transcript_coverage = behind[1].then_some(BehindTranscript);
            }
            lanes.finish_coverage_check(
                heads,
                gaps.into_lanes(),
                ok,
                resume_basis,
                current_unix_millis(),
            )
        };
        dispatch_projection_observation(dispatch, decisions);
    });
}

#[cfg(test)]
pub(crate) mod test_probe {
    use std::{
        cell::RefCell,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
            mpsc,
        },
        time::Duration,
    };
    const HANG: Duration = Duration::from_secs(60);
    #[derive(Debug, Copy, Clone, PartialEq, Eq)]
    pub(crate) enum Phase {
        BeforeRead,
        BeforeTranscript,
        BeforeInstall,
        Done,
    }
    pub(super) struct Probe {
        arrived: mpsc::Sender<Phase>,
        resume: mpsc::Receiver<()>,
        reads: Arc<AtomicUsize>,
        stop_before_transcript: bool,
        logs: Arc<std::sync::Mutex<Vec<crate::test_log_capture::CapturedLog>>>,
    }
    thread_local! {
        static PROBE: RefCell<Option<Probe>> = const { RefCell::new(None) };
        static READS: RefCell<Option<Arc<AtomicUsize>>> = const { RefCell::new(None) };
    }
    pub(crate) struct Control {
        arrived: mpsc::Receiver<Phase>,
        resume: mpsc::Sender<()>,
        reads: Arc<AtomicUsize>,
        logs: Arc<std::sync::Mutex<Vec<crate::test_log_capture::CapturedLog>>>,
    }
    impl Control {
        pub(crate) fn arm() -> Self {
            Self::arm_for(false)
        }
        pub(crate) fn arm_for(stop_before_transcript: bool) -> Self {
            let (arrived, events) = mpsc::channel();
            let (next, resume) = mpsc::channel();
            let reads = Arc::new(AtomicUsize::new(0));
            let logs = Arc::new(std::sync::Mutex::new(Vec::new()));
            READS.replace(Some(reads.clone()));
            PROBE.replace(Some(Probe {
                arrived,
                resume,
                reads: reads.clone(),
                stop_before_transcript,
                logs: logs.clone(),
            }));
            Self {
                arrived: events,
                resume: next,
                reads,
                logs,
            }
        }
        pub(crate) fn wait(&self, phase: Phase) {
            assert_eq!(self.arrived.recv_timeout(HANG).unwrap(), phase);
        }
        pub(crate) fn resume(&self) {
            self.resume.send(()).unwrap();
        }
        pub(crate) fn reads(&self) -> usize {
            self.reads.load(Ordering::SeqCst)
        }
        pub(crate) fn logs(&self) -> Vec<crate::test_log_capture::CapturedLog> {
            self.logs.lock().unwrap().clone()
        }
    }
    impl Drop for Control {
        fn drop(&mut self) {
            READS.take();
            PROBE.take();
        }
    }
    pub(super) fn take() -> Option<Probe> {
        PROBE.take()
    }
    pub(super) struct Worker(Option<crate::test_log_capture::ThreadLogCapture>);
    pub(super) fn enter(probe: Option<Probe>) -> Worker {
        READS.replace(probe.as_ref().map(|p| p.reads.clone()));
        let logs = probe
            .is_some()
            .then(crate::test_log_capture::ThreadLogCapture::start);
        PROBE.replace(probe);
        Worker(logs)
    }
    impl Drop for Worker {
        fn drop(&mut self) {
            if let Some(p) = PROBE.take() {
                if let Some(logs) = self.0.take() {
                    *p.logs.lock().unwrap() = logs.finish();
                }
                let _ = p.arrived.send(Phase::Done);
            }
            READS.take();
        }
    }
    pub(super) fn point(phase: Phase) {
        PROBE.with_borrow(|p| {
            if let Some(p) = p {
                p.arrived.send(phase).unwrap();
                p.resume.recv_timeout(HANG).unwrap();
            }
        });
    }
    pub(crate) fn record_read() {
        READS.with_borrow(|r| {
            if let Some(n) = r {
                n.fetch_add(1, Ordering::SeqCst);
            }
        });
    }
    pub(crate) fn before_transcript() {
        if PROBE.with_borrow(|p| p.as_ref().is_some_and(|p| p.stop_before_transcript)) {
            point(Phase::BeforeTranscript);
        }
    }

    #[cfg(not(debug_assertions))]
    thread_local! {
        static TIMING: RefCell<Option<mpsc::Sender<std::time::Instant>>> = const { RefCell::new(None) };
    }

    // Replace only the model invocation. Start, verification, policy dispatch and
    // entry into the live projection job worker all run on their normal path.
    #[cfg(not(debug_assertions))]
    pub(crate) fn measure_first_job() -> mpsc::Receiver<std::time::Instant> {
        let (sender, receiver) = mpsc::channel();
        TIMING.replace(Some(sender));
        receiver
    }

    #[cfg(not(debug_assertions))]
    pub(super) fn with_timing_generator(
        mut dispatch: super::ProjectionDispatchContext,
    ) -> super::ProjectionDispatchContext {
        struct FirstJob(mpsc::Sender<std::time::Instant>);
        impl super::ProjectionPatchGenerator for FirstJob {
            fn generate_projection_patch(
                &self,
                _: super::ProjectionJob,
                _: super::TranscriptLedger,
                _: crate::projection_llm::ProjectionSnapshots,
                _: u64,
                _: u64,
            ) -> super::ProjectionPatchAttempt {
                let _ = self.0.send(std::time::Instant::now());
                super::ProjectionPatchAttempt {
                    outcome: Err("measurement complete".into()),
                    attempted_route: None,
                }
            }
        }
        if let Some(sender) = TIMING.take() {
            dispatch.patch_generator = Arc::new(FirstJob(sender));
        }
        dispatch
    }
}
