//! Cross-platform helpers for restrictive file permissions.

use std::path::Path;

/// Set a file to owner-only read/write (0o600 on Unix, owner-only ACL on Windows).
/// Best-effort — logs a warning on failure.
pub fn set_owner_only(path: &Path) {
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(path, fs::Permissions::from_mode(0o600)) {
            log::warn!("Failed to set 0o600 on {}: {}", path.display(), e);
        }
    }
    #[cfg(windows)]
    {
        // Real ACL hardening (critique H4): remove INHERITED ACEs and grant
        // ONLY the current user Full control, so the file isn't readable by
        // other non-admin users even if the parent dir's ACLs are looser.
        // `icacls` ships with Windows; best-effort with a logged warning.
        let user = std::env::var("USERNAME").unwrap_or_default();
        if user.trim().is_empty() {
            log::warn!(
                "USERNAME not set; cannot harden ACL on {} (relying on parent dir)",
                path.display()
            );
            return;
        }
        // CREATE_NO_WINDOW (0x08000000): icacls is a console app, so without this
        // flag every credential/settings save flashes a console window on the
        // user's screen (reported in the field). This is a silent background
        // hardening step, so suppress the window. (User-facing "open folder"
        // spawns in commands.rs deliberately do NOT set this — they should show.)
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        match std::process::Command::new("icacls")
            .arg(path)
            .arg("/inheritance:r")
            .arg("/grant:r")
            .arg(format!("{user}:F"))
            .creation_flags(CREATE_NO_WINDOW)
            .output()
        {
            Ok(out) if out.status.success() => {}
            Ok(out) => log::warn!(
                "icacls ACL hardening on {} returned non-zero: {}",
                path.display(),
                String::from_utf8_lossy(&out.stderr).trim()
            ),
            Err(e) => log::warn!("Failed to run icacls on {}: {}", path.display(), e),
        }
    }
}
