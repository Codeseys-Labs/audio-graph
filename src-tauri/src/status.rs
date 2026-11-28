//! The one state authority for what the window displays (#141 decision of
//! 2026-11-28).
//!
//! Every change to the [`PipelineStatus`] goes through
//! [`StatusAuthority::publish`]: take the status lock, apply the change,
//! increment the revision, write the truth flags worker threads read, clone,
//! release the lock, then emit the clone. The latest snapshot stays behind
//! the lock, so a delivery that failed is repaired by a read.
//!
//! Lock order: the capture manager, then the status, then the truth flags. A
//! publish closure takes no lock, and no caller holds a truth flag's guard
//! while it publishes.

use crate::events::{self, CapturePhase, PipelineStatus};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, PoisonError, RwLock};
use tauri::Emitter;

/// The flags threads read to decide whether to keep running. Each mirrors a
/// field of the snapshot and is written only by [`StatusAuthority::publish`].
#[derive(Clone)]
pub struct TruthFlags {
    pub is_capturing: Arc<RwLock<bool>>,
    pub is_transcribing: Arc<AtomicBool>,
    pub is_gemini_active: Arc<RwLock<bool>>,
    pub is_converse_active: Arc<RwLock<bool>>,
    pub is_openai_realtime_active: Arc<RwLock<bool>>,
    pub session_id: Arc<RwLock<String>>,
}

impl TruthFlags {
    fn mirror(&self, status: &PipelineStatus) {
        let write = |flag: &RwLock<bool>, value: bool| {
            *flag.write().unwrap_or_else(PoisonError::into_inner) = value;
        };
        write(
            &self.is_capturing,
            status.capture_state().phase == CapturePhase::Running,
        );
        self.is_transcribing
            .store(status.transcription().is_some(), Ordering::SeqCst);
        let realtime = status.realtime();
        write(&self.is_gemini_active, realtime.gemini);
        write(&self.is_converse_active, realtime.converse);
        write(&self.is_openai_realtime_active, realtime.openai_realtime);
        let mut session_id = self
            .session_id
            .write()
            .unwrap_or_else(PoisonError::into_inner);
        if *session_id != status.session().id {
            session_id.clone_from(&status.session().id);
        }
    }
}

/// The owner of the live [`PipelineStatus`]. Cheap to clone; every clone
/// shares the one snapshot.
#[derive(Clone)]
pub struct StatusAuthority {
    inner: Arc<Inner>,
}

struct Inner {
    status: RwLock<PipelineStatus>,
    flags: TruthFlags,
    emitter: RwLock<Option<tauri::AppHandle>>,
    #[cfg(test)]
    recorder: std::sync::Mutex<Option<Vec<PipelineStatus>>>,
}

impl StatusAuthority {
    pub fn new(flags: TruthFlags) -> Self {
        let session_id = flags
            .session_id
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        Self {
            inner: Arc::new(Inner {
                status: RwLock::new(PipelineStatus::new(session_id)),
                flags,
                emitter: RwLock::new(None),
                #[cfg(test)]
                recorder: std::sync::Mutex::new(None),
            }),
        }
    }

    /// Where published snapshots are emitted from now on.
    pub fn set_emitter(&self, app: tauri::AppHandle) {
        *self
            .inner
            .emitter
            .write()
            .unwrap_or_else(PoisonError::into_inner) = Some(app);
    }

    /// The latest snapshot.
    pub fn snapshot(&self) -> PipelineStatus {
        self.read(PipelineStatus::clone)
    }

    /// Read the latest snapshot in place.
    pub fn read<R>(&self, read: impl FnOnce(&PipelineStatus) -> R) -> R {
        read(
            &self
                .inner
                .status
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        )
    }

    /// Apply `change` as one revision and emit it.
    pub fn publish(&self, change: impl FnOnce(&mut PipelineStatus)) -> PipelineStatus {
        self.publish_if(|status| {
            change(status);
            true
        })
        .expect("an unconditional change publishes")
    }

    /// Apply `change`; if it reports a change, publish it as one revision and
    /// emit it. Returns the published snapshot.
    pub fn publish_if(
        &self,
        change: impl FnOnce(&mut PipelineStatus) -> bool,
    ) -> Option<PipelineStatus> {
        let snapshot = {
            let mut status = self
                .inner
                .status
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            if !change(&mut status) {
                return None;
            }
            status.bump_revision();
            self.inner.flags.mirror(&status);
            status.clone()
        };
        self.emit(&snapshot);
        Some(snapshot)
    }

    fn emit(&self, snapshot: &PipelineStatus) {
        #[cfg(test)]
        if let Some(recorded) = self
            .inner
            .recorder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_mut()
        {
            recorded.push(snapshot.clone());
        }
        let emitter = self
            .inner
            .emitter
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        if let Some(app) = emitter
            && app.emit(events::PIPELINE_STATUS_EVENT, snapshot).is_err()
        {
            log::warn!("A status snapshot could not be emitted; a status read repairs it");
        }
    }

    /// Test-only: an authority whose flags nothing else reads.
    #[cfg(test)]
    pub(crate) fn detached() -> Self {
        Self::new(TruthFlags {
            is_capturing: Arc::default(),
            is_transcribing: Arc::default(),
            is_gemini_active: Arc::default(),
            is_converse_active: Arc::default(),
            is_openai_realtime_active: Arc::default(),
            session_id: Arc::default(),
        })
    }

    /// Test-only: record every snapshot published from now on, in emission
    /// order.
    #[cfg(test)]
    pub(crate) fn record(&self) {
        *self
            .inner
            .recorder
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Vec::new());
    }

    /// Test-only: the snapshots published since [`Self::record`].
    #[cfg(test)]
    pub(crate) fn recorded(&self) -> Vec<PipelineStatus> {
        self.inner
            .recorder
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .unwrap_or_default()
    }
}
