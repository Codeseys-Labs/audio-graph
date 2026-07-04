//! Cross-platform helpers for restrictive file permissions.

use std::path::Path;

/// Set a file to owner-only read/write (0o600 on Unix, owner-only ACL on Windows).
pub fn try_set_owner_only(path: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("Failed to set 0o600 on {}: {}", path.display(), e))?;
    }
    #[cfg(windows)]
    {
        // SID-native ACL hardening (seed 403d, supersedes the icacls/%USERNAME%
        // approach from critique H4). Resolve the CURRENT PROCESS TOKEN's user
        // SID directly via the Win32 security API — never `%USERNAME%` (an
        // unauthenticated, spoofable/absent env var whose name→SID resolution is
        // also ambiguous in domain/renamed-account/localized cases). Then apply a
        // PROTECTED, owner-SID-only DACL granting that SID Full control via
        // SetNamedSecurityInfoW — no subprocess, no PATH/console-flash surface.
        windows_owner_only_acl::set_owner_only_dacl(path)?;
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
    }
    Ok(())
}

/// Set owner-only permissions on paths whose callers can tolerate best-effort hardening.
pub fn set_owner_only(path: &Path) {
    if let Err(e) = try_set_owner_only(path) {
        log::warn!("{e}");
    }
}

/// SID-native, subprocess-free owner-only ACL hardening for Windows (seed 403d).
///
/// All Win32 calls are confined to this `#[cfg(windows)]` module so the Unix
/// (`0o600`) and `not(any(unix, windows))` (no-op) paths in `try_set_owner_only`
/// are completely untouched, and the `windows` crate is a Windows-target-only
/// dependency that never affects Linux/macOS builds.
#[cfg(windows)]
mod windows_owner_only_acl {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Foundation::{
        CloseHandle, HANDLE, HLOCAL, LocalFree, NO_ERROR, WIN32_ERROR,
    };
    use windows::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        SDDL_REVISION_1, SE_FILE_OBJECT, SetNamedSecurityInfoW,
    };
    use windows::Win32::Security::{
        DACL_SECURITY_INFORMATION, GetSecurityDescriptorDacl, GetTokenInformation,
        PROTECTED_DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, TOKEN_QUERY, TOKEN_USER,
        TokenUser,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use windows::core::PCWSTR;

    /// RAII guard for an OS handle: closes it on drop so an early `?` return can
    /// never leak the process token handle.
    struct OwnedHandle(HANDLE);
    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: `self.0` is a valid, still-open handle obtained from
                // OpenProcessToken; we only close it once (on drop).
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    /// RAII guard for a LocalAlloc'd security descriptor returned by
    /// ConvertStringSecurityDescriptorToSecurityDescriptorW: LocalFree on drop
    /// so a leaked SD (and the DACL embedded in it) can never occur.
    struct LocalSecurityDescriptor(PSECURITY_DESCRIPTOR);
    impl Drop for LocalSecurityDescriptor {
        fn drop(&mut self) {
            if !self.0.0.is_null() {
                // SAFETY: `self.0.0` was allocated by the Win32 SDDL converter
                // via LocalAlloc; LocalFree is the matching deallocator and we
                // free it exactly once (on drop).
                unsafe {
                    let _ = LocalFree(Some(HLOCAL(self.0.0)));
                }
            }
        }
    }

    /// Resolve the current process token's user SID as an SDDL string SID
    /// (e.g. "S-1-5-21-..."). Returns a descriptive, secret-free Err on failure.
    /// `pub(super)` so the fs_util test can reuse this RAII-correct helper for
    /// its ground-truth SID rather than re-walking the token itself.
    pub(super) fn current_token_sid_string(path: &Path) -> Result<String, String> {
        // SAFETY: GetCurrentProcess returns a pseudo-handle that need not be
        // closed; OpenProcessToken fills `token` with a real handle we wrap in
        // OwnedHandle so it is always closed.
        let token = unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(|e| {
                format!(
                    "OpenProcessToken failed while hardening ACL on {}: {}",
                    path.display(),
                    e.code().0
                )
            })?;
            OwnedHandle(token)
        };

        // Two-call size-then-fill pattern for GetTokenInformation(TokenUser).
        let mut needed: u32 = 0;
        // SAFETY: first call intentionally passes a null buffer to learn the
        // required size; it is expected to "fail" with ERROR_INSUFFICIENT_BUFFER
        // while writing `needed`.
        unsafe {
            let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut needed);
        }
        if needed == 0 {
            return Err(format!(
                "GetTokenInformation(TokenUser) returned zero size while hardening ACL on {}",
                path.display()
            ));
        }

        let mut buf = vec![0u8; needed as usize];
        // SAFETY: `buf` is at least `needed` bytes, matching the size the first
        // call reported; the API fills it with a TOKEN_USER followed by the SID.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(buf.as_mut_ptr() as *mut core::ffi::c_void),
                needed,
                &mut needed,
            )
            .map_err(|e| {
                format!(
                    "GetTokenInformation(TokenUser) failed while hardening ACL on {}: {}",
                    path.display(),
                    e.code().0
                )
            })?;
        }

        // SAFETY: on success the buffer begins with a TOKEN_USER whose
        // `User.Sid` points into the same buffer; the SID stays valid for the
        // duration of this call (we stringify it before `buf` is dropped).
        let sid: PSID = unsafe { (*(buf.as_ptr() as *const TOKEN_USER)).User.Sid };
        if sid.0.is_null() {
            return Err(format!(
                "token user SID was null while hardening ACL on {}",
                path.display()
            ));
        }

        // Convert the raw SID to its canonical string form for the SDDL DACL.
        let mut sid_wstr = windows::core::PWSTR::null();
        // SAFETY: `sid` is a valid SID inside `buf`; ConvertSidToStringSidW
        // LocalAlloc's the string, which we free below via LocalFree.
        unsafe {
            ConvertSidToStringSidW(sid, &mut sid_wstr).map_err(|e| {
                format!(
                    "ConvertSidToStringSidW failed while hardening ACL on {}: {}",
                    path.display(),
                    e.code().0
                )
            })?;
        }
        // SAFETY: on success `sid_wstr` is a valid, NUL-terminated wide string.
        let sid_string = unsafe { sid_wstr.to_string() }.map_err(|e| {
            format!(
                "SID string was not valid UTF-16 while hardening ACL on {}: {}",
                path.display(),
                e
            )
        });
        // Free the LocalAlloc'd SID string regardless of the UTF-16 conversion
        // result so it never leaks.
        // SAFETY: `sid_wstr` was allocated by ConvertSidToStringSidW via
        // LocalAlloc; LocalFree is the matching deallocator, freed once.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sid_wstr.0 as *mut core::ffi::c_void)));
        }
        sid_string
    }

    /// Build a PROTECTED, owner-SID-only DACL and apply it to `path`.
    ///
    /// Built-in-principal decision (seed acceptance b): the DACL grants Full
    /// control (`FA`) to the current-user SID ONLY — no SYSTEM (`SY`) or
    /// Administrators. This is a faithful, tightest port of the prior
    /// `icacls /grant:r <USERNAME>:F` behavior, which likewise granted only the
    /// current user. Owner-only is the strongest guarantee for a secret file; an
    /// admin can still take ownership if genuinely needed. `PAI` marks the DACL
    /// Protected (no inherited ACEs, mirroring `/inheritance:r`) + auto-inherit
    /// resolved.
    pub fn set_owner_only_dacl(path: &Path) -> Result<(), String> {
        let sid = current_token_sid_string(path)?;
        // D:PAI(A;;FA;;;<SID>) — Protected, Auto-inherited, one Allow ACE of
        // Full Access (FA) for the resolved current-user SID and nothing else.
        let sddl = format!("D:PAI(A;;FA;;;{sid})");

        // Encode SDDL as a NUL-terminated wide string.
        let sddl_wide: Vec<u16> = std::ffi::OsStr::new(&sddl)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let mut sd = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl_wide` is a valid NUL-terminated UTF-16 string; on success
        // `sd` receives a LocalAlloc'd security descriptor we wrap for LocalFree.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl_wide.as_ptr()),
                SDDL_REVISION_1,
                &mut sd,
                None,
            )
            .map_err(|e| {
                format!(
                    "ConvertStringSecurityDescriptorToSecurityDescriptorW failed while hardening ACL on {}: {}",
                    path.display(),
                    e.code().0
                )
            })?;
        }
        let sd_guard = LocalSecurityDescriptor(sd);

        // Extract the DACL pointer out of the parsed security descriptor.
        let mut dacl_present = windows::core::BOOL(0);
        let mut dacl_ptr: *mut windows::Win32::Security::ACL = core::ptr::null_mut();
        let mut dacl_defaulted = windows::core::BOOL(0);
        // SAFETY: `sd_guard.0` is a valid security descriptor from the converter;
        // GetSecurityDescriptorDacl reads its DACL pointer into `dacl_ptr` (which
        // points into the SD buffer and stays valid until `sd_guard` drops).
        unsafe {
            GetSecurityDescriptorDacl(
                sd_guard.0,
                &mut dacl_present,
                &mut dacl_ptr,
                &mut dacl_defaulted,
            )
            .map_err(|e| {
                format!(
                    "GetSecurityDescriptorDacl failed while hardening ACL on {}: {}",
                    path.display(),
                    e.code().0
                )
            })?;
        }
        if !dacl_present.as_bool() || dacl_ptr.is_null() {
            return Err(format!(
                "parsed security descriptor had no DACL while hardening ACL on {}",
                path.display()
            ));
        }

        // Encode the path as a NUL-terminated wide string for the W API.
        let path_wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        // Apply the DACL as PROTECTED (strip inheritance, equivalent to
        // icacls /inheritance:r). SetNamedSecurityInfoW returns WIN32_ERROR, not
        // a Result — a missing/inaccessible path yields a non-zero error here,
        // which is exactly the `Err` the best-effort caller and the
        // `rejects_missing_path` test rely on.
        // SAFETY: `path_wide` is a valid NUL-terminated wide path; `dacl_ptr` is
        // the DACL owned by `sd_guard`, valid for the duration of this call.
        let rc: WIN32_ERROR = unsafe {
            SetNamedSecurityInfoW(
                PCWSTR(path_wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(dacl_ptr as *const _),
                None,
            )
        };
        if rc != NO_ERROR {
            return Err(format!(
                "SetNamedSecurityInfoW returned Win32 error {} while hardening ACL on {}",
                rc.0,
                path.display()
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_set_owner_only_rejects_missing_path() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-owner-only-missing-{}",
            uuid::Uuid::new_v4()
        ));

        let err = try_set_owner_only(&path).expect_err("missing path should fail");
        assert!(err.contains(&path.display().to_string()));
    }

    #[test]
    fn try_set_owner_only_accepts_existing_file() {
        let path = std::env::temp_dir().join(format!(
            "audio-graph-owner-only-existing-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "fixture").expect("write fixture");

        try_set_owner_only(&path).expect("harden existing file");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600);
        }

        let _ = std::fs::remove_file(&path);
    }

    /// SID-native ACL smoke evidence (seed 403d, acceptance d). Windows-gated;
    /// runs on the existing Blacksmith `windows-2025` CI runner via
    /// `cargo test --no-default-features --features cloud`. Writes a plain temp
    /// file (NO secret bytes), hardens it, then reads the DACL back via
    /// GetNamedSecurityInfoW and asserts:
    ///   (a) the current process token SID has Full access,
    ///   (b) the DACL is PROTECTED (inheritance removed),
    ///   (c) exactly one ACE (no unexpected principals — owner-SID-only).
    #[cfg(windows)]
    #[test]
    fn try_set_owner_only_sets_current_sid_dacl() {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::Foundation::{HLOCAL, LocalFree};
        use windows::Win32::Security::Authorization::{
            ConvertSidToStringSidW, GetNamedSecurityInfoW, SE_FILE_OBJECT,
        };
        use windows::Win32::Security::{
            ACCESS_ALLOWED_ACE, ACL_SIZE_INFORMATION, AclSizeInformation,
            DACL_SECURITY_INFORMATION, GetAce, GetAclInformation, GetSecurityDescriptorControl,
            PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED,
        };
        use windows::core::PCWSTR;

        // ACCESS_ALLOWED_ACE_TYPE == 0; hard-coded here to avoid pulling in the
        // Win32_System_SystemServices feature just for one constant.
        const ACCESS_ALLOWED_ACE_TYPE: u8 = 0;

        // Ground-truth principal: reuse the module's RAII-correct token walk so
        // the test never re-implements (and never leaks) the token handle.
        let sid_string = super::windows_owner_only_acl::current_token_sid_string(
            std::path::Path::new("test-sid-probe"),
        )
        .expect("resolve current token SID");

        let path = std::env::temp_dir().join(format!(
            "audio-graph-owner-only-dacl-{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, "fixture").expect("write fixture");

        try_set_owner_only(&path).expect("harden existing file");

        // Read the DACL + security-descriptor control back.
        let path_wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut dacl_ptr: *mut windows::Win32::Security::ACL = core::ptr::null_mut();
        let mut sd = PSECURITY_DESCRIPTOR::default();
        let rc = unsafe {
            GetNamedSecurityInfoW(
                PCWSTR(path_wide.as_ptr()),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                None,
                None,
                Some(&mut dacl_ptr),
                None,
                &mut sd,
            )
        };
        assert_eq!(rc.0, 0, "GetNamedSecurityInfoW should succeed");
        assert!(!dacl_ptr.is_null(), "DACL must be present");

        // (b) DACL is PROTECTED (inheritance removed). Query the SD control bits.
        let mut control: u16 = 0;
        let mut revision = 0u32;
        unsafe {
            GetSecurityDescriptorControl(sd, &mut control, &mut revision)
                .expect("GetSecurityDescriptorControl");
        }
        assert!(
            control & SE_DACL_PROTECTED.0 != 0,
            "DACL must be PROTECTED (SE_DACL_PROTECTED set); inheritance not removed"
        );

        // (c) Exactly one ACE — owner-SID-only, no unexpected principals.
        let mut info = ACL_SIZE_INFORMATION::default();
        unsafe {
            GetAclInformation(
                dacl_ptr,
                &mut info as *mut _ as *mut core::ffi::c_void,
                std::mem::size_of::<ACL_SIZE_INFORMATION>() as u32,
                AclSizeInformation,
            )
            .expect("GetAclInformation");
        }
        assert_eq!(
            info.AceCount, 1,
            "owner-only DACL must contain exactly one ACE; found {}",
            info.AceCount
        );

        // (a) The single ACE's SID matches the current process token SID and is
        // an ACCESS_ALLOWED_ACE. Walk the one ACE and stringify its SID.
        unsafe {
            let mut ace_ptr: *mut core::ffi::c_void = core::ptr::null_mut();
            GetAce(dacl_ptr, 0, &mut ace_ptr).expect("GetAce");
            // ACCESS_ALLOWED_ACE layout: ACE_HEADER, ACCESS_MASK, then the SID
            // starts at the SidStart field.
            let ace = ace_ptr as *const ACCESS_ALLOWED_ACE;
            assert_eq!(
                (*ace).Header.AceType,
                ACCESS_ALLOWED_ACE_TYPE,
                "the single ACE must be ACCESS_ALLOWED"
            );
            let ace_sid = PSID(core::ptr::addr_of!((*ace).SidStart) as *mut core::ffi::c_void);
            let mut ace_sid_wstr = windows::core::PWSTR::null();
            ConvertSidToStringSidW(ace_sid, &mut ace_sid_wstr).expect("ConvertSidToStringSidW ace");
            let ace_sid_string = ace_sid_wstr.to_string().expect("ace sid utf16");
            let _ = LocalFree(Some(HLOCAL(ace_sid_wstr.0 as *mut core::ffi::c_void)));
            assert_eq!(
                ace_sid_string, sid_string,
                "the ACE SID must be the current process token SID (owner-only)"
            );
        }

        // Free the SD allocated by GetNamedSecurityInfoW.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(sd.0)));
        }
        let _ = std::fs::remove_file(&path);
    }
}
