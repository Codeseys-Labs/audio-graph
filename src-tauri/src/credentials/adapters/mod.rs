mod authority_journal;
mod keyring_entry;
// Capability-blind raw cores: the only modules allowed to hold platform
// `unsafe`. Their policy adapters live under `native_interaction`.
mod macos_keychain_ffi;
mod native_interaction;
pub(crate) mod native_keyring;
mod windows_credential_manager_ffi;

// Re-sealed for the credential-v2 native assembly (#158): lexical platform
// children, the platform factory seam, and redacted recovery unwinds.
#[cfg(test)]
const NATIVE_INTERACTION_REVIEWED_SHA256: [u8; 32] = [
    0x5c, 0xca, 0xc2, 0x8a, 0xda, 0x90, 0x6b, 0x70, 0xb3, 0x55, 0x84, 0x2f, 0x33, 0xae, 0x86, 0xf3,
    0x8c, 0x1f, 0x6a, 0x0c, 0xda, 0x71, 0xdb, 0x2d, 0xbf, 0x99, 0xce, 0xd1, 0x7b, 0xac, 0x45, 0x39,
];
