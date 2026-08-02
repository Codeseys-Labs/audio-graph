//! Direct-Win32 implementation of the stage-aware credential file primitive.
//!
//! This module remains dark until the credential adapter assembly workstream.

use super::{
    CommitState, CreateTempFault, NativeReplaceReturn, PlatformFault, ReadbackState,
    ReplaceEnvelope, ReplaceFailure, ReplaceFailureCode, ReplacePlatform, ReplaceReceipt,
    ReplaceStage, missing_candidate_path_is_clean, replace_with,
};
use crate::credentials::filesystem_policy::windows::{WindowsChildPath, WindowsQualifiedParent};
use core::ffi::c_void;
use std::mem::{size_of, size_of_val};

use windows::Win32::Foundation::{
    CloseHandle, ERROR_ALREADY_EXISTS, ERROR_FILE_EXISTS, ERROR_FILE_NOT_FOUND,
    ERROR_PATH_NOT_FOUND, ERROR_SUCCESS, GENERIC_READ, GENERIC_WRITE, GetHandleInformation, HANDLE,
    HANDLE_FLAG_INHERIT, HLOCAL, LocalFree,
};
use windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_FILE_OBJECT,
};
use windows::Win32::Security::{
    ACCESS_ALLOWED_ACE, ACE_HEADER, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
    DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetLengthSid,
    GetSecurityDescriptorControl, GetSecurityDescriptorDacl, GetTokenInformation, IsValidSid,
    OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, SECURITY_ATTRIBUTES,
    TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows::Win32::Storage::FileSystem::{
    CREATE_NEW, CreateFileW, DELETE, DeleteFileW, FILE_ALL_ACCESS, FILE_ATTRIBUTE_NORMAL,
    FILE_ID_INFO, FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    FileIdInfo, FlushFileBuffers, GetFileInformationByHandleEx, GetFileSizeEx,
    MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, OPEN_EXISTING, READ_CONTROL,
    ReadFile, SYNCHRONIZE, WRITE_DAC, WriteFile,
};
use windows::Win32::System::SystemServices::{
    ACCESS_ALLOWED_ACE_TYPE, SECURITY_DESCRIPTOR_REVISION,
};
use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
use windows::core::{BOOL, Error as WindowsError, HRESULT, PCWSTR, PWSTR};
use zeroize::Zeroizing;

pub(crate) enum WindowsReplaceTarget {
    AuthorityJournal,
    FileV2,
    #[cfg(test)]
    DummySmoke(String),
}

impl WindowsReplaceTarget {
    fn leaf(&self) -> &str {
        match self {
            Self::AuthorityJournal => "state.json",
            Self::FileV2 => "credentials.json",
            #[cfg(test)]
            Self::DummySmoke(leaf) => leaf,
        }
    }
}

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: this guard owns the successful Win32 handle and closes it
            // exactly once when the candidate or readback object leaves scope.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

struct OwnedLocal(HLOCAL);

impl Drop for OwnedLocal {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: SDDL conversion and GetSecurityInfo allocate with
            // LocalAlloc; this guard owns that allocation and frees it once.
            unsafe {
                let _ = LocalFree(Some(self.0));
            }
        }
    }
}

struct CurrentUserSecurity {
    token_user: Vec<usize>,
    descriptor: OwnedLocal,
}

impl CurrentUserSecurity {
    fn new() -> Result<Self, PlatformFault> {
        let mut token = HANDLE::default();
        // SAFETY: GetCurrentProcess returns the process pseudo-handle and
        // `token` is valid writable storage for the returned owned handle.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }
            .map_err(|_| PlatformFault::Permission)?;
        let token = OwnedHandle(token);

        let mut required = 0u32;
        // SAFETY: the first call intentionally supplies no buffer to obtain the
        // exact required size. `required` remains valid through the call.
        let _ = unsafe { GetTokenInformation(token.0, TokenUser, None, 0, &mut required) };
        if required < size_of::<TOKEN_USER>() as u32 {
            return Err(PlatformFault::Permission);
        }
        let word_count = (required as usize).div_ceil(size_of::<usize>());
        let mut token_user = vec![0usize; word_count];
        // SAFETY: the usize allocation is suitably aligned for TOKEN_USER and
        // has at least `required` writable bytes.
        unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(token_user.as_mut_ptr().cast::<c_void>()),
                required,
                &mut required,
            )
        }
        .map_err(|_| PlatformFault::Permission)?;
        let sid = token_user_sid(&token_user)?;

        let mut sid_text = PWSTR(std::ptr::null_mut());
        // SAFETY: `sid` points into the live token_user allocation; Windows
        // allocates the returned NUL-terminated SID string with LocalAlloc.
        unsafe { ConvertSidToStringSidW(sid, &mut sid_text) }
            .map_err(|_| PlatformFault::Permission)?;
        if sid_text.is_null() {
            return Err(PlatformFault::Permission);
        }
        let sid_text_guard = OwnedLocal(HLOCAL(sid_text.0.cast::<c_void>()));
        let sid_text = wide_string(sid_text.0, 256).ok_or(PlatformFault::Permission)?;
        let sddl = format!("O:{sid_text}D:P(A;;FA;;;{sid_text})");
        let sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();

        let mut descriptor = PSECURITY_DESCRIPTOR::default();
        // SAFETY: `sddl` is NUL-terminated and live for the synchronous call;
        // Windows allocates the returned self-relative descriptor.
        unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                PCWSTR(sddl.as_ptr()),
                SDDL_REVISION_1,
                &mut descriptor,
                None,
            )
        }
        .map_err(|_| PlatformFault::Permission)?;
        drop(sid_text_guard);
        if descriptor.is_invalid() {
            return Err(PlatformFault::Permission);
        }

        Ok(Self {
            token_user,
            descriptor: OwnedLocal(HLOCAL(descriptor.0)),
        })
    }

    fn sid(&self) -> Result<PSID, PlatformFault> {
        token_user_sid(&self.token_user)
    }

    fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.descriptor.0.0,
            bInheritHandle: BOOL::from(false),
        }
    }
}

fn token_user_sid(token_user: &[usize]) -> Result<PSID, PlatformFault> {
    if size_of_val(token_user) < size_of::<TOKEN_USER>() {
        return Err(PlatformFault::Permission);
    }
    // SAFETY: callers allocate the buffer aligned as usize and populate at
    // least TOKEN_USER bytes through GetTokenInformation(TokenUser).
    let sid = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
    // SAFETY: the SID pointer is part of the live TokenUser result buffer.
    if sid.is_invalid() || !unsafe { IsValidSid(sid) }.as_bool() {
        return Err(PlatformFault::Permission);
    }
    Ok(sid)
}

fn wide_string(pointer: *const u16, bound: usize) -> Option<String> {
    if pointer.is_null() {
        return None;
    }
    let mut length = 0usize;
    // SAFETY: ConvertSidToStringSidW returned a NUL-terminated allocation. The
    // bounded scan refuses a malformed result instead of walking indefinitely.
    unsafe {
        while length < bound && *pointer.add(length) != 0 {
            length += 1;
        }
        if length == bound {
            return None;
        }
        String::from_utf16(std::slice::from_raw_parts(pointer, length)).ok()
    }
}

fn verify_owner_only_security(
    handle: HANDLE,
    expected_sid: PSID,
    require_empty: bool,
) -> Result<(), PlatformFault> {
    if require_empty {
        let mut length = -1i64;
        // SAFETY: `length` is writable and the caller keeps `handle` open for
        // the synchronous query.
        unsafe { GetFileSizeEx(handle, &mut length) }.map_err(|_| PlatformFault::Permission)?;
        if length != 0 {
            return Err(PlatformFault::Permission);
        }
    }

    let mut owner = PSID::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: all output pointers reference valid writable storage; the handle
    // remains open and Windows allocates `descriptor` on success.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            Some(&mut dacl),
            None,
            Some(&mut descriptor),
        )
    };
    if status != ERROR_SUCCESS || descriptor.is_invalid() {
        return Err(PlatformFault::Permission);
    }
    let _descriptor_guard = OwnedLocal(HLOCAL(descriptor.0));

    // SAFETY: both SIDs remain live for this comparison.
    if owner.is_invalid() || unsafe { EqualSid(owner, expected_sid) }.is_err() {
        return Err(PlatformFault::Permission);
    }

    let mut dacl_present = BOOL::from(false);
    let mut dacl_defaulted = BOOL::from(false);
    let mut descriptor_dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: descriptor is live under its LocalFree guard and every output is
    // valid writable storage.
    unsafe {
        GetSecurityDescriptorDacl(
            descriptor,
            &mut dacl_present,
            &mut descriptor_dacl,
            &mut dacl_defaulted,
        )
    }
    .map_err(|_| PlatformFault::Permission)?;
    if !dacl_present.as_bool()
        || dacl_defaulted.as_bool()
        || dacl.is_null()
        || descriptor_dacl != dacl
    {
        return Err(PlatformFault::Permission);
    }

    let mut control = 0u16;
    let mut revision = 0u32;
    // SAFETY: descriptor and both output pointers remain valid.
    unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) }
        .map_err(|_| PlatformFault::Permission)?;
    if revision != SECURITY_DESCRIPTOR_REVISION || control & SE_DACL_PROTECTED.0 == 0 {
        return Err(PlatformFault::Permission);
    }

    let mut size = ACL_SIZE_INFORMATION::default();
    // SAFETY: dacl is owned by the live descriptor; `size` is exact writable
    // storage for the requested information class.
    unsafe {
        GetAclInformation(
            dacl,
            std::ptr::addr_of_mut!(size).cast::<c_void>(),
            size_of_val(&size) as u32,
            AclSizeInformation,
        )
    }
    .map_err(|_| PlatformFault::Permission)?;
    if size.AceCount != 1 {
        return Err(PlatformFault::Permission);
    }

    let mut ace_pointer: *mut c_void = std::ptr::null_mut();
    // SAFETY: the ACL reports one ACE and the output receives its borrowed
    // pointer, which remains valid under the descriptor guard.
    unsafe { GetAce(dacl, 0, &mut ace_pointer) }.map_err(|_| PlatformFault::Permission)?;
    if ace_pointer.is_null() {
        return Err(PlatformFault::Permission);
    }
    // SAFETY: GetAce returned a pointer to at least ACE_HEADER bytes.
    let header = unsafe { &*ace_pointer.cast::<ACE_HEADER>() };
    if u32::from(header.AceType) != ACCESS_ALLOWED_ACE_TYPE
        || header.AceFlags != 0
        || usize::from(header.AceSize) < size_of::<ACCESS_ALLOWED_ACE>()
    {
        return Err(PlatformFault::Permission);
    }
    // SAFETY: the header size/type checks establish ACCESS_ALLOWED_ACE layout.
    let ace = unsafe { &*ace_pointer.cast::<ACCESS_ALLOWED_ACE>() };
    if ace.Mask != FILE_ALL_ACCESS.0 {
        return Err(PlatformFault::Permission);
    }
    let sid_offset = std::mem::offset_of!(ACCESS_ALLOWED_ACE, SidStart);
    let sid_capacity = usize::from(header.AceSize)
        .checked_sub(sid_offset)
        .ok_or(PlatformFault::Permission)?;
    const SID_HEADER_BYTES: usize = 8;
    if sid_capacity < SID_HEADER_BYTES {
        return Err(PlatformFault::Permission);
    }
    let sid_pointer = std::ptr::addr_of!(ace.SidStart).cast::<u8>();
    // SAFETY: the fixed SID header fits inside the ACE by the check above.
    let sub_authority_count = unsafe { *sid_pointer.add(1) } as usize;
    let sid_length = SID_HEADER_BYTES
        .checked_add(
            sub_authority_count
                .checked_mul(size_of::<u32>())
                .ok_or(PlatformFault::Permission)?,
        )
        .ok_or(PlatformFault::Permission)?;
    if sid_length > sid_capacity {
        return Err(PlatformFault::Permission);
    }
    let ace_sid = PSID(sid_pointer.cast_mut().cast::<c_void>());
    // SAFETY: the complete variable-length SID has been bounded inside AceSize.
    if !unsafe { IsValidSid(ace_sid) }.as_bool()
        || unsafe { GetLengthSid(ace_sid) } as usize != sid_length
        || unsafe { EqualSid(ace_sid, expected_sid) }.is_err()
    {
        return Err(PlatformFault::Permission);
    }
    Ok(())
}

fn is_win32(error: &WindowsError, code: u32) -> bool {
    error.code() == HRESULT::from_win32(code)
}

fn is_not_found(error: &WindowsError) -> bool {
    is_win32(error, ERROR_FILE_NOT_FOUND.0) || is_win32(error, ERROR_PATH_NOT_FOUND.0)
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct NativeFileIdentity {
    volume_serial: u64,
    file_id: [u8; 16],
}

fn file_identity(handle: HANDLE) -> Result<NativeFileIdentity, PlatformFault> {
    let mut info = FILE_ID_INFO::default();
    // SAFETY: `info` is exact writable storage for FileIdInfo and the handle
    // remains open throughout the synchronous query.
    unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileIdInfo,
            std::ptr::addr_of_mut!(info).cast::<c_void>(),
            size_of_val(&info) as u32,
        )
    }
    .map_err(|_| PlatformFault::Failed)?;
    Ok(NativeFileIdentity {
        volume_serial: info.VolumeSerialNumber,
        file_id: info.FileId.Identifier,
    })
}

fn open_for_read(path: &WindowsChildPath) -> Result<Option<OwnedHandle>, PlatformFault> {
    let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
    let opened = path.with_pcwstr(|path| {
        // SAFETY: the opaque child path is NUL-terminated and lives through
        // the call. A successful returned handle is immediately guarded.
        unsafe {
            CreateFileW(
                path,
                GENERIC_READ.0 | READ_CONTROL.0,
                share,
                None,
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                None,
            )
        }
    });
    match opened {
        Ok(handle) => Ok(Some(OwnedHandle(handle))),
        Err(error) if is_not_found(&error) => Ok(None),
        Err(_) => Err(PlatformFault::Failed),
    }
}

fn read_complete(handle: HANDLE) -> Result<Zeroizing<Vec<u8>>, PlatformFault> {
    let mut length = -1i64;
    // SAFETY: `length` is writable and `handle` remains open for the query.
    unsafe { GetFileSizeEx(handle, &mut length) }.map_err(|_| PlatformFault::Failed)?;
    if length < 0 || length as usize > super::MAX_ENVELOPE_BYTES {
        return Err(PlatformFault::Failed);
    }
    let mut bytes = Zeroizing::new(vec![0u8; length as usize]);
    let mut offset = 0usize;
    while offset < bytes.len() {
        let mut read = 0u32;
        // SAFETY: the remaining slice is valid writable storage, and no
        // OVERLAPPED pointer is supplied for this synchronous handle.
        unsafe { ReadFile(handle, Some(&mut bytes[offset..]), Some(&mut read), None) }
            .map_err(|_| PlatformFault::Failed)?;
        if read == 0 || read as usize > bytes.len() - offset {
            return Err(PlatformFault::Failed);
        }
        offset += read as usize;
    }
    Ok(bytes)
}

struct WindowsCandidate {
    handle: OwnedHandle,
    path: WindowsChildPath,
}

impl std::fmt::Debug for WindowsCandidate {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("WindowsCandidate([REDACTED])")
    }
}

struct WindowsCandidateIdentity {
    file: NativeFileIdentity,
}

struct NativeWindowsReplaceApi<'a> {
    parent: &'a WindowsQualifiedParent<'a>,
    final_path: WindowsChildPath,
    security: CurrentUserSecurity,
}

impl<'a> NativeWindowsReplaceApi<'a> {
    fn new(
        parent: &'a WindowsQualifiedParent<'a>,
        target: &WindowsReplaceTarget,
    ) -> Result<Self, PlatformFault> {
        let final_path = parent
            .child_path(target.leaf())
            .map_err(|_| PlatformFault::UnsupportedTarget)?;
        let security = CurrentUserSecurity::new()?;
        Ok(Self {
            parent,
            final_path,
            security,
        })
    }

    fn validate_qualified_parent(&self) -> Result<(), PlatformFault> {
        if !self
            .parent
            .identity_is_unchanged()
            .map_err(|_| PlatformFault::UnsupportedTarget)?
        {
            return Err(PlatformFault::UnsupportedTarget);
        }
        let sid = self.security.sid()?;
        self.parent
            .with_scoped_handle(|handle| verify_owner_only_security(handle, sid, false))
    }
}

impl ReplacePlatform for NativeWindowsReplaceApi<'_> {
    type Candidate = WindowsCandidate;
    type CandidateIdentity = WindowsCandidateIdentity;

    fn validate_parent(&mut self) -> Result<(), PlatformFault> {
        self.validate_qualified_parent()
    }

    fn create_temp(&mut self, _attempt: u8) -> Result<Self::Candidate, CreateTempFault> {
        let leaf = format!(
            ".audiograph-credential-{}.tmp",
            uuid::Uuid::new_v4().simple()
        );
        let path = self
            .parent
            .child_path(&leaf)
            .map_err(|_| CreateTempFault::Failed)?;
        let attributes = self.security.attributes();
        let desired_access = GENERIC_READ.0
            | GENERIC_WRITE.0
            | DELETE.0
            | READ_CONTROL.0
            | WRITE_DAC.0
            | SYNCHRONIZE.0;
        let share = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_DELETE.0);
        let created = path.with_pcwstr(|path| {
            // SAFETY: the opaque child path is NUL-terminated; the explicit
            // descriptor and SECURITY_ATTRIBUTES remain live for the call.
            unsafe {
                CreateFileW(
                    path,
                    desired_access,
                    share,
                    Some(&attributes),
                    CREATE_NEW,
                    FILE_ATTRIBUTE_NORMAL,
                    None,
                )
            }
        });
        match created {
            Ok(handle) => Ok(WindowsCandidate {
                handle: OwnedHandle(handle),
                path,
            }),
            Err(error)
                if is_win32(&error, ERROR_FILE_EXISTS.0)
                    || is_win32(&error, ERROR_ALREADY_EXISTS.0) =>
            {
                Err(CreateTempFault::Collision)
            }
            Err(_) => Err(CreateTempFault::Failed),
        }
    }

    fn verify_temp_security(&mut self, candidate: &Self::Candidate) -> Result<(), PlatformFault> {
        verify_owner_only_security(candidate.handle.0, self.security.sid()?, true)?;
        let mut handle_flags = 0u32;
        // SAFETY: the candidate handle remains live and `handle_flags` is valid
        // writable output storage.
        unsafe { GetHandleInformation(candidate.handle.0, &mut handle_flags) }
            .map_err(|_| PlatformFault::Permission)?;
        if handle_flags & HANDLE_FLAG_INHERIT.0 != 0 {
            return Err(PlatformFault::Permission);
        }
        Ok(())
    }

    fn write_complete(
        &mut self,
        candidate: &Self::Candidate,
        bytes: &[u8],
    ) -> Result<(), PlatformFault> {
        let mut offset = 0usize;
        while offset < bytes.len() {
            let mut written = 0u32;
            // SAFETY: the remaining byte slice is live for this synchronous
            // write; no OVERLAPPED pointer is supplied.
            unsafe {
                WriteFile(
                    candidate.handle.0,
                    Some(&bytes[offset..]),
                    Some(&mut written),
                    None,
                )
            }
            .map_err(|_| PlatformFault::Failed)?;
            if written == 0 || written as usize > bytes.len() - offset {
                return Err(PlatformFault::Failed);
            }
            offset += written as usize;
        }
        Ok(())
    }

    fn flush(&mut self, candidate: &Self::Candidate) -> Result<(), PlatformFault> {
        // SAFETY: the candidate handle remains open through the synchronous
        // file-buffer flush.
        unsafe { FlushFileBuffers(candidate.handle.0) }.map_err(|_| PlatformFault::Failed)
    }

    fn capture_candidate_identity(
        &mut self,
        candidate: &Self::Candidate,
    ) -> Result<Self::CandidateIdentity, PlatformFault> {
        let file = file_identity(candidate.handle.0)?;
        if !self.parent.volume_matches(file.volume_serial) {
            return Err(PlatformFault::UnsupportedTarget);
        }
        Ok(WindowsCandidateIdentity { file })
    }

    fn invoke_replace(
        &mut self,
        candidate: &Self::Candidate,
        _identity: &Self::CandidateIdentity,
        state: CommitState,
    ) -> NativeReplaceReturn {
        if state != CommitState::Unknown {
            return NativeReplaceReturn::False;
        }
        if self.validate_qualified_parent().is_err() {
            return NativeReplaceReturn::False;
        }
        let moved = candidate.path.with_pcwstr(|candidate_path| {
            self.final_path.with_pcwstr(|final_path| {
                // SAFETY: both opaque paths are NUL-terminated and live for the
                // call. MOVEFILE_COPY_ALLOWED is deliberately absent.
                unsafe {
                    MoveFileExW(
                        candidate_path,
                        final_path,
                        MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                    )
                }
            })
        });
        if moved.is_ok() {
            NativeReplaceReturn::True
        } else {
            NativeReplaceReturn::False
        }
    }

    fn readback(
        &mut self,
        candidate: &Self::Candidate,
        identity: &Self::CandidateIdentity,
        envelope: &ReplaceEnvelope<'_>,
    ) -> Result<ReadbackState, PlatformFault> {
        let final_handle = open_for_read(&self.final_path)?;
        let temp_handle = open_for_read(&candidate.path)?;
        let final_bytes = final_handle
            .as_ref()
            .map(|handle| read_complete(handle.0))
            .transpose()?;
        let temp_bytes = temp_handle
            .as_ref()
            .map(|handle| read_complete(handle.0))
            .transpose()?;

        let final_is_candidate = final_bytes
            .as_ref()
            .is_some_and(|bytes| envelope.candidate_is_exact(bytes.as_slice()));
        let final_is_prior_bytes =
            envelope.prior_is_exact(final_bytes.as_ref().map(|bytes| bytes.as_slice()));
        let final_prior_metadata_is_exact = match (final_handle.as_ref(), envelope.prior) {
            (None, None) => true,
            (Some(handle), Some(_)) => {
                let identity = file_identity(handle.0)?;
                self.parent.volume_matches(identity.volume_serial)
                    && verify_owner_only_security(handle.0, self.security.sid()?, false).is_ok()
            }
            _ => false,
        };
        let temp_is_complete_candidate = match temp_handle.as_ref() {
            Some(handle) => {
                temp_bytes
                    .as_ref()
                    .is_some_and(|bytes| envelope.candidate_is_exact(bytes.as_slice()))
                    && file_identity(handle.0)? == identity.file
                    && self.parent.volume_matches(identity.file.volume_serial)
                    && verify_owner_only_security(handle.0, self.security.sid()?, false).is_ok()
            }
            None => false,
        };

        if final_is_candidate && temp_handle.is_none() {
            Ok(ReadbackState::ExactCandidate)
        } else if final_is_prior_bytes
            && final_prior_metadata_is_exact
            && temp_is_complete_candidate
            && self.validate_qualified_parent().is_ok()
        {
            Ok(ReadbackState::ExactPriorWithCompleteCandidate)
        } else {
            Ok(ReadbackState::MixedOrUnknown)
        }
    }

    fn verify_final_metadata(
        &mut self,
        identity: &Self::CandidateIdentity,
    ) -> Result<(), PlatformFault> {
        let final_handle = open_for_read(&self.final_path)?.ok_or(PlatformFault::Failed)?;
        if file_identity(final_handle.0)? != identity.file {
            return Err(PlatformFault::Failed);
        }
        verify_owner_only_security(final_handle.0, self.security.sid()?, false)?;
        self.validate_qualified_parent()
    }

    fn cleanup(
        &mut self,
        candidate: &Self::Candidate,
        state: CommitState,
    ) -> Result<(), PlatformFault> {
        let candidate_identity = file_identity(candidate.handle.0)?;
        let Some(path_handle) = open_for_read(&candidate.path)? else {
            return if missing_candidate_path_is_clean(state) {
                Ok(())
            } else {
                Err(PlatformFault::Failed)
            };
        };
        if file_identity(path_handle.0)? != candidate_identity {
            return Err(PlatformFault::Failed);
        }
        verify_owner_only_security(path_handle.0, self.security.sid()?, false)?;
        drop(path_handle);
        candidate
            .path
            .with_pcwstr(|path| {
                // SAFETY: the verified wrapper-owned candidate path is
                // NUL-terminated and live for this synchronous delete request.
                unsafe { DeleteFileW(path) }
            })
            .map_err(|_| PlatformFault::Failed)
    }
}

pub(crate) fn replace_qualified(
    parent: &WindowsQualifiedParent<'_>,
    target: &WindowsReplaceTarget,
    envelope: &ReplaceEnvelope<'_>,
) -> Result<ReplaceReceipt, ReplaceFailure> {
    let mut platform =
        NativeWindowsReplaceApi::new(parent, target).map_err(|fault| ReplaceFailure {
            stage: ReplaceStage::ValidateParent,
            code: match fault {
                PlatformFault::UnsupportedTarget => ReplaceFailureCode::UnsupportedTarget,
                PlatformFault::Permission => ReplaceFailureCode::PermissionHardeningFailure,
                PlatformFault::Failed => ReplaceFailureCode::InternalBackendFailure,
            },
            commit_state: CommitState::DefinitelyNotCommitted,
            cleanup_pending: false,
        })?;
    replace_with(&mut platform, envelope)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::adapters::file_replace::EnvelopeIdentity;
    use crate::credentials::filesystem_policy::PlatformRelease;
    use crate::credentials::filesystem_policy::windows::WindowsFilesystemDetector;
    use std::collections::BTreeSet;

    fn parse_dummy_identity(bytes: &[u8]) -> Option<EnvelopeIdentity> {
        let header = bytes.get(..36)?;
        Some(EnvelopeIdentity::new(
            u32::from_le_bytes(header.get(..4)?.try_into().ok()?),
            header.get(4..20)?.try_into().ok()?,
            header.get(20..36)?.try_into().ok()?,
        ))
    }

    fn dummy_generation(
        schema: u32,
        operation_byte: u8,
        revision_byte: u8,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut bytes = Vec::from(schema.to_le_bytes());
        bytes.extend_from_slice(&[operation_byte; 16]);
        bytes.extend_from_slice(&[revision_byte; 16]);
        bytes.extend_from_slice(payload);
        bytes
    }

    fn wrapper_temp_names(directory: &std::path::Path) -> BTreeSet<std::ffi::OsString> {
        std::fs::read_dir(directory)
            .expect("enumerate the explicit dummy-only directory")
            .map(|entry| entry.expect("read dummy-only directory entry").file_name())
            .filter(|name| {
                let name = name.to_string_lossy();
                name.starts_with(".audiograph-credential-") && name.ends_with(".tmp")
            })
            .collect()
    }

    #[test]
    fn replacement_flags_are_the_exact_write_through_same_volume_contract() {
        assert_eq!(
            (MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH).0,
            9,
            "the selected flags contain no copy or delayed-reboot fallback"
        );
        assert_eq!(WindowsReplaceTarget::AuthorityJournal.leaf(), "state.json");
        assert_eq!(WindowsReplaceTarget::FileV2.leaf(), "credentials.json");
    }

    #[test]
    fn candidate_debug_never_contains_private_path_or_native_identity() {
        let candidate = WindowsCandidate {
            handle: OwnedHandle(HANDLE::default()),
            path: WindowsChildPath::redacted_test_fixture(
                "private-path-sid-file-volume-native-canary",
            ),
        };
        assert_eq!(format!("{candidate:?}"), "WindowsCandidate([REDACTED])");
    }

    #[test]
    #[ignore = "requires native Windows, a protected dummy-only directory, and explicit env gates"]
    fn native_dummy_smoke_never_uses_a_real_credential_path() {
        assert_eq!(
            std::env::var("AUDIO_GRAPH_WINDOWS_FILE_REPLACE_DUMMY_ONLY").as_deref(),
            Ok("I_UNDERSTAND_DUMMY_ONLY"),
            "set the explicit dummy-only acknowledgement"
        );
        let directory = std::env::var_os("AUDIO_GRAPH_WINDOWS_FILE_REPLACE_SMOKE_DIR")
            .map(std::path::PathBuf::from)
            .expect("set a throwaway protected directory outside real AudioGraph state");
        let detector = WindowsFilesystemDetector::new(PlatformRelease::new(10, 0, 0));
        let held = detector
            .open_target(&directory)
            .expect("open the dummy-only parent");
        let parent = detector
            .qualify_parent(&held)
            .expect("qualify the dummy-only local NTFS parent");
        let target = WindowsReplaceTarget::DummySmoke(format!(
            ".audiograph-credential-dummy-{}.json",
            uuid::Uuid::new_v4().simple()
        ));
        let residue_before = wrapper_temp_names(&directory);
        let first = dummy_generation(1, 0x11, 0x22, b"AUDIOGRAPH-DUMMY-FILE-REPLACE-V1");
        let first_envelope = ReplaceEnvelope::new(
            None,
            &first,
            1,
            [0x11; 16],
            [0x22; 16],
            parse_dummy_identity,
        )
        .expect("bounded first dummy envelope");

        let first_receipt = replace_qualified(&parent, &target, &first_envelope)
            .expect("verified absent-destination dummy commit");
        assert_eq!(first_receipt.attempts, 1);
        assert!(!first_receipt.cleanup_pending);

        let final_path = parent
            .child_path(target.leaf())
            .expect("same closed dummy leaf");
        let security = CurrentUserSecurity::new().expect("current dummy user");
        let first_handle = open_for_read(&final_path)
            .expect("open first dummy readback")
            .expect("first dummy final exists");
        assert_eq!(read_complete(first_handle.0).unwrap().as_slice(), first);
        verify_owner_only_security(first_handle.0, security.sid().unwrap(), false)
            .expect("first dummy final owner-only metadata");
        drop(first_handle);

        let second = dummy_generation(1, 0x33, 0x44, b"AUDIOGRAPH-DUMMY-FILE-REPLACE-V2");
        let second_envelope = ReplaceEnvelope::new(
            Some(&first),
            &second,
            1,
            [0x33; 16],
            [0x44; 16],
            parse_dummy_identity,
        )
        .expect("bounded second dummy envelope");
        let second_receipt = replace_qualified(&parent, &target, &second_envelope)
            .expect("verified existing-destination dummy commit");
        assert_eq!(second_receipt.attempts, 1);
        assert!(!second_receipt.cleanup_pending);

        let second_handle = open_for_read(&final_path)
            .expect("open second dummy readback")
            .expect("second dummy final exists");
        assert_eq!(read_complete(second_handle.0).unwrap().as_slice(), second);
        verify_owner_only_security(second_handle.0, security.sid().unwrap(), false)
            .expect("second dummy final owner-only metadata");
        drop(second_handle);
        assert_eq!(
            wrapper_temp_names(&directory),
            residue_before,
            "the two replacements leave no new wrapper temp residue"
        );

        final_path
            .with_pcwstr(|path| {
                // SAFETY: only the unique verified dummy file created by this
                // ignored test is deleted; the path is NUL-terminated.
                unsafe { DeleteFileW(path) }
            })
            .expect("remove the unique dummy final");
        assert!(
            open_for_read(&final_path)
                .expect("verify unique dummy cleanup")
                .is_none()
        );
    }
}
