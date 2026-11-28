//! Leave for an executor job to send from the client it was queued with.
//!
//! A job copies its HTTP client when it enters the executor and sends from no
//! other. A settings save takes effect for executor work when it publishes a
//! client into the app's slot: the save writes the settings first and the slot
//! after, and it is the slot a job is compared with.
//!
//! Before every transport attempt the job's copy is compared with the slot,
//! under the slot's mutex, which is the one a save holds while it publishes.
//! An attempt is in flight from the moment that comparison passes. A save
//! published after it does not stop that attempt; it stops the next one, a
//! retry after backoff and a format downgrade included.

use std::sync::{Arc, Mutex};

use audio_graph_ipc_contract::runtime_diagnostic::RuntimeErrorCode as Code;

/// An app slot holding the current client of one kind.
pub(crate) type ClientSlot<C> = Arc<Mutex<Option<C>>>;

/// Why a job may not send from its client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SendRefused {
    /// The slot's lock is poisoned: a panic unwound through it, leaving the
    /// client in a state nobody inspected.
    Poisoned,
    /// The slot holds another client, or none.
    Replaced,
}

impl SendRefused {
    /// The closed code for this refusal, with a replaced client logged in
    /// closed words. Neither configuration is formatted, nor what differed.
    pub(crate) fn report(self) -> Code {
        match self {
            Self::Poisoned => Code::Internal,
            Self::Replaced => {
                log::warn!("LLM job refused: its client was replaced after it was queued");
                Code::PolicyBlocked
            }
        }
    }
}

/// Whether `slot` still holds a client that `same` accepts.
pub(crate) fn leave_to_send<C>(
    slot: &Mutex<Option<C>>,
    same: impl FnOnce(&C) -> bool,
) -> Result<(), SendRefused> {
    match slot.lock() {
        Ok(current) if current.as_ref().is_some_and(same) => Ok(()),
        Ok(_) => Err(SendRefused::Replaced),
        Err(_) => Err(SendRefused::Poisoned),
    }
}
