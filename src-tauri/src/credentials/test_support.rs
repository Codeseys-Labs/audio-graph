//! Shared deterministic credential-service test support.

use super::service::CredentialTokenSource;
use audio_graph_ipc_contract::credential_contract::{CredentialOperationId, CredentialRevision};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Default)]
pub(crate) struct DeterministicTokenSource {
    next: AtomicU64,
}

impl DeterministicTokenSource {
    fn next_token(&self) -> String {
        let value = self.next.fetch_add(1, Ordering::SeqCst) + 1;
        format!("00000000-0000-0000-0000-{value:012x}")
    }
}

impl CredentialTokenSource for DeterministicTokenSource {
    fn next_operation_id(&self) -> CredentialOperationId {
        CredentialOperationId::parse(self.next_token()).expect("deterministic canonical operation")
    }

    fn next_revision(&self) -> CredentialRevision {
        CredentialRevision::parse(self.next_token()).expect("deterministic canonical revision")
    }
}
