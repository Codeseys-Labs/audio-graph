//! Owner-only directories held open for the authority journal and the native
//! namespace lock.
//!
//! A directory is opened without following a final link and then verified and
//! hardened on that same handle. Children are opened relative to it (Unix
//! `openat`; on Windows by name, then refused unless the opened object's final
//! path is a direct child of the held directory), and each one is verified and
//! hardened on the handle that is then used for I/O. All of this happens
//! before a single journal byte is written.
//! Every failure is a closed `CredentialStoreFailure`; no OS text or path
//! leaves this module.

use crate::credentials::domain::CredentialStoreFailure;
use std::io::Read;

#[cfg(unix)]
pub(in crate::credentials::adapters) use unix_held::{HeldDirectory, StagedFile};
#[cfg(all(windows, test))]
pub(in crate::credentials::adapters) use windows_held::open_directory_handle;
#[cfg(windows)]
pub(in crate::credentials::adapters) use windows_held::{
    HeldDirectory, StagedFile, token_profile_directory,
};

/// Reads at most `limit` bytes: the reader is never asked for more, so an
/// oversized journal reaches its decoder without first being read in full.
fn read_at_most(reader: impl Read, limit: usize) -> Result<Vec<u8>, CredentialStoreFailure> {
    let mut bytes = Vec::new();
    reader
        .take(u64::try_from(limit).map_err(|_| CredentialStoreFailure::Internal)?)
        .read_to_end(&mut bytes)
        .map_err(|_| CredentialStoreFailure::Unavailable)?;
    Ok(bytes)
}

fn staged_name(name: &str) -> String {
    format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple())
}

/// Whether `candidate` is a name [`staged_name`] produces for `name`.
fn is_staged_name(candidate: &str, name: &str) -> bool {
    candidate
        .strip_prefix('.')
        .and_then(|rest| rest.strip_prefix(name))
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|token| {
            token.len() == 32
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })
}

#[cfg(unix)]
mod unix_held {
    use super::{is_staged_name, read_at_most, staged_name};
    use crate::credentials::domain::CredentialStoreFailure;
    use rustix::fd::{AsFd, OwnedFd};
    use rustix::fs::{
        AtFlags, CWD, Dir, FileType, Mode, OFlags, RawMode, fchmod, fstat, fsync, mkdirat, openat,
        renameat, unlinkat,
    };
    use rustix::io::Errno;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;

    const DIRECTORY_MODE: RawMode = 0o700;
    const FILE_MODE: RawMode = 0o600;

    pub(in crate::credentials::adapters) struct HeldDirectory {
        fd: OwnedFd,
    }

    pub(in crate::credentials::adapters) struct StagedFile<'directory> {
        directory: &'directory HeldDirectory,
        file: File,
        temp: String,
        name: String,
        published: bool,
    }

    fn mode(bits: RawMode) -> Mode {
        Mode::from_raw_mode(bits)
    }

    /// Verifies the object behind `fd` is of the expected type and owned by
    /// the effective user, sets the owner-only mode on that descriptor, then
    /// re-reads it from the same descriptor. On macOS it also replaces any
    /// extended ACL with an empty one and proves no entry remains, so an
    /// inherited allow entry cannot widen access beyond the mode bits.
    fn harden(
        fd: &OwnedFd,
        expected: FileType,
        bits: RawMode,
    ) -> Result<(), CredentialStoreFailure> {
        let owner = rustix::process::geteuid().as_raw();
        let stat = fstat(fd).map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        if FileType::from_raw_mode(stat.st_mode) != expected || stat.st_uid != owner {
            return Err(CredentialStoreFailure::PermissionHardeningFailed);
        }
        fchmod(fd, mode(bits)).map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        #[cfg(target_os = "macos")]
        crate::credentials::adapters::macos_acl_ffi::clear_extended_acl(fd.as_fd())
            .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        let stat = fstat(fd).map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        if Mode::from_raw_mode(stat.st_mode) != mode(bits) || stat.st_uid != owner {
            return Err(CredentialStoreFailure::PermissionHardeningFailed);
        }
        Ok(())
    }

    fn open_failure(errno: Errno) -> CredentialStoreFailure {
        match errno {
            Errno::LOOP | Errno::NOTDIR | Errno::ISDIR => {
                CredentialStoreFailure::PermissionHardeningFailed
            }
            _ => CredentialStoreFailure::Unavailable,
        }
    }

    impl HeldDirectory {
        pub(in crate::credentials::adapters) fn open_existing(
            path: &Path,
        ) -> Result<Option<Self>, CredentialStoreFailure> {
            let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
            match openat(CWD, path, flags, Mode::empty()) {
                Ok(fd) => {
                    harden(&fd, FileType::Directory, DIRECTORY_MODE)?;
                    Ok(Some(Self { fd }))
                }
                Err(Errno::NOENT) => Ok(None),
                Err(errno) => Err(open_failure(errno)),
            }
        }

        pub(in crate::credentials::adapters) fn open_or_create(
            path: &Path,
        ) -> Result<Self, CredentialStoreFailure> {
            if let Some(held) = Self::open_existing(path)? {
                return Ok(held);
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| CredentialStoreFailure::Unavailable)?;
            }
            // The umask can only remove bits, so the directory is never
            // broader than owner-only before it is hardened on its handle.
            match mkdirat(CWD, path, mode(DIRECTORY_MODE)) {
                Ok(()) | Err(Errno::EXIST) => {}
                Err(errno) => return Err(open_failure(errno)),
            }
            Self::open_existing(path)?.ok_or(CredentialStoreFailure::Unavailable)
        }

        fn open_child(
            &self,
            name: &str,
            flags: OFlags,
        ) -> Result<Option<File>, CredentialStoreFailure> {
            // NONBLOCK keeps a FIFO planted at the name from stalling the open;
            // the type check below then refuses it.
            let flags = flags | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK;
            match openat(&self.fd, name, flags, mode(FILE_MODE)) {
                Ok(fd) => {
                    harden(&fd, FileType::RegularFile, FILE_MODE)?;
                    Ok(Some(File::from(fd)))
                }
                Err(Errno::NOENT) if !flags.contains(OFlags::CREATE) => Ok(None),
                Err(errno) => Err(open_failure(errno)),
            }
        }

        pub(in crate::credentials::adapters) fn open_lock_file(
            &self,
            name: &str,
        ) -> Result<File, CredentialStoreFailure> {
            self.open_child(name, OFlags::RDWR | OFlags::CREATE)?
                .ok_or(CredentialStoreFailure::Unavailable)
        }

        pub(in crate::credentials::adapters) fn read_capped(
            &self,
            name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            match self.open_child(name, OFlags::RDONLY)? {
                Some(file) => read_at_most(file, limit).map(Some),
                None => Ok(None),
            }
        }

        /// Unlinks every staged file a failed discard left for `name`,
        /// relative to the held descriptor. Callers hold the authority
        /// mutation lock, so no other writer's staged file is in flight.
        /// Anything that cannot be removed fails closed as `Unavailable`
        /// rather than being ignored.
        fn sweep_staged(&self, name: &str) -> Result<(), CredentialStoreFailure> {
            let entries =
                Dir::read_from(&self.fd).map_err(|_| CredentialStoreFailure::Unavailable)?;
            let mut strays = Vec::new();
            for entry in entries {
                let entry = entry.map_err(|_| CredentialStoreFailure::Unavailable)?;
                if let Ok(candidate) = entry.file_name().to_str()
                    && is_staged_name(candidate, name)
                {
                    strays.push(candidate.to_owned());
                }
            }
            for stray in strays {
                unlinkat(&self.fd, stray.as_str(), AtFlags::empty())
                    .map_err(|_| CredentialStoreFailure::Unavailable)?;
            }
            Ok(())
        }

        pub(in crate::credentials::adapters) fn create_staged(
            &self,
            name: &str,
        ) -> Result<StagedFile<'_>, CredentialStoreFailure> {
            self.sweep_staged(name)?;
            let temp = staged_name(name);
            let file = self
                .open_child(&temp, OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL)?
                .ok_or(CredentialStoreFailure::Unavailable)?;
            Ok(StagedFile {
                directory: self,
                file,
                temp,
                name: name.to_owned(),
                published: false,
            })
        }
    }

    impl StagedFile<'_> {
        pub(in crate::credentials::adapters) fn write_all(
            &mut self,
            bytes: &[u8],
        ) -> std::io::Result<()> {
            self.file.write_all(bytes)
        }

        /// Flushes the staged bytes to the file.
        pub(in crate::credentials::adapters) fn sync(&mut self) -> std::io::Result<()> {
            self.file.sync_all()
        }

        /// Renames the flushed bytes over `name` relative to the held
        /// directory, then flushes that directory so the rename itself is
        /// durable.
        pub(in crate::credentials::adapters) fn publish(mut self) -> std::io::Result<()> {
            let directory = self.directory.fd.as_fd();
            renameat(directory, self.temp.as_str(), directory, self.name.as_str())?;
            self.published = true;
            fsync(directory)?;
            Ok(())
        }

        pub(in crate::credentials::adapters) fn commit(mut self) -> std::io::Result<()> {
            self.sync()?;
            self.publish()
        }
    }

    impl Drop for StagedFile<'_> {
        fn drop(&mut self) {
            if !self.published {
                let _ = unlinkat(&self.directory.fd, self.temp.as_str(), AtFlags::empty());
            }
        }
    }
}

#[cfg(windows)]
mod windows_held {
    use super::{is_staged_name, read_at_most, staged_name};
    use crate::credentials::domain::CredentialStoreFailure;
    use ::windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use ::windows::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
        FILE_RENAME_INFORMATION, FILE_SYNCHRONOUS_IO_NONALERT, FileRenameInformation, NtCreateFile,
        NtSetInformationFile,
    };
    use ::windows::Win32::Foundation::{
        CloseHandle, HANDLE, OBJ_CASE_INSENSITIVE, STATUS_ACCESS_DENIED,
        STATUS_FILE_IS_A_DIRECTORY, STATUS_OBJECT_NAME_COLLISION, STATUS_OBJECT_NAME_NOT_FOUND,
        UNICODE_STRING,
    };
    use ::windows::Win32::Security::TOKEN_QUERY;
    use ::windows::Win32::Storage::FileSystem::{
        DELETE, FILE_ACCESS_RIGHTS, FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_NORMAL,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_TRAVERSE, FileDispositionInfo, GetFinalPathNameByHandleW,
        READ_CONTROL, SYNCHRONIZE, SetFileInformationByHandle, VOLUME_NAME_GUID, WRITE_DAC,
    };
    use ::windows::Win32::System::IO::IO_STATUS_BLOCK;
    use ::windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use ::windows::Win32::UI::Shell::GetUserProfileDirectoryW;
    use ::windows::core::PWSTR;
    use std::ffi::OsString;
    use std::fs::{File, OpenOptions};
    use std::io::{ErrorKind, Write};
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Path, PathBuf};

    const MAX_FINAL_PATH_UNITS: usize = 32_768;

    /// A directory handle opened without `FILE_SHARE_DELETE`, so the held
    /// directory cannot be renamed or deleted out from under its operations.
    /// Children are opened and renamed relative to this handle.
    pub(in crate::credentials::adapters) struct HeldDirectory {
        handle: File,
        /// The volume-GUID path the handle resolved to at open, with every
        /// junction already resolved. It keeps naming the held directory: the
        /// directory cannot be renamed while held, and NTFS refuses to rename
        /// an ancestor of a held directory.
        final_path: PathBuf,
    }

    pub(in crate::credentials::adapters) struct StagedFile<'directory> {
        directory: &'directory HeldDirectory,
        file: Option<File>,
        name: String,
        published: bool,
    }

    fn final_path(file: &File) -> Option<PathBuf> {
        let mut buffer = vec![0_u16; MAX_FINAL_PATH_UNITS];
        // SAFETY: the handle stays open for the call and the buffer is
        // writable for its full length.
        let length = unsafe {
            GetFinalPathNameByHandleW(HANDLE(file.as_raw_handle()), &mut buffer, VOLUME_NAME_GUID)
        } as usize;
        (length != 0 && length < buffer.len())
            .then(|| PathBuf::from(OsString::from_wide(&buffer[..length])))
    }

    /// The profile directory registered for the process token's user.
    /// `SHGetKnownFolderPath` expands `%USERPROFILE%` from the process
    /// environment; this does not, so every process of one user names the
    /// same directory.
    pub(in crate::credentials::adapters) fn token_profile_directory() -> Option<PathBuf> {
        let mut token = HANDLE::default();
        // SAFETY: on success `token` receives a new handle, closed below.
        unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.ok()?;
        let mut buffer = vec![0_u16; MAX_FINAL_PATH_UNITS];
        let mut length = u32::try_from(buffer.len()).ok()?;
        // SAFETY: the buffer is writable for `length` units and the token
        // stays open for the call.
        let read = unsafe {
            GetUserProfileDirectoryW(token, Some(PWSTR(buffer.as_mut_ptr())), &mut length)
        };
        // SAFETY: `token` is open and owned here; it is closed exactly once.
        let _ = unsafe { CloseHandle(token) };
        read.ok()?;
        // On success `length` counts the terminating NUL.
        let units = buffer.get(..usize::try_from(length).ok()?.checked_sub(1)?)?;
        (!units.is_empty() && !units.contains(&0))
            .then(|| PathBuf::from(OsString::from_wide(units)))
    }

    /// Refuses a reparse point and an object of the wrong kind, using the
    /// attributes of the open handle rather than a second path lookup.
    fn require_kind(file: &File, directory: bool) -> Result<(), CredentialStoreFailure> {
        let attributes = file
            .metadata()
            .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?
            .file_attributes();
        let is_directory = attributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
        if attributes & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 || is_directory != directory {
            return Err(CredentialStoreFailure::PermissionHardeningFailed);
        }
        Ok(())
    }

    /// Applies the protected owner-only DACL to an open file through its
    /// handle, so a path swapped after the open cannot redirect it.
    fn harden(file: &File) -> Result<(), CredentialStoreFailure> {
        crate::fs_util::try_set_owner_only_file(file)
            .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)
    }

    /// `ReOpenFile` refuses directory handles, so a directory is opened with
    /// `WRITE_DAC` and hardened through that handle. Its one owner ACE is
    /// inherited, so existing children keep owner access once every other
    /// principal's inherited ACE is gone.
    fn harden_directory(directory: &File) -> Result<(), CredentialStoreFailure> {
        crate::fs_util::try_set_owner_only_directory_handle(directory)
            .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)
    }

    /// Refuses an object whose owner could undo its hardening. Any owner keeps
    /// implicit `WRITE_DAC`, so a protected owner-only DACL means nothing on an
    /// object someone else owns. See `fs_util::owner_is_trusted` for which
    /// owners are accepted.
    fn require_trusted_owner(file: &File) -> Result<(), CredentialStoreFailure> {
        match crate::fs_util::owner_is_trusted(file) {
            Ok(true) => Ok(()),
            Ok(false) | Err(_) => Err(CredentialStoreFailure::PermissionHardeningFailed),
        }
    }

    /// Marks the open file for deletion when its last handle closes. The
    /// handle names the object, whatever its path resolves to by then.
    fn dispose(file: &File) -> std::io::Result<()> {
        let info = FILE_DISPOSITION_INFO { DeleteFile: true };
        // SAFETY: `info` is the exact structure `FileDispositionInfo` takes and
        // outlives the call; the handle stays open throughout.
        unsafe {
            SetFileInformationByHandle(
                HANDLE(file.as_raw_handle()),
                FileDispositionInfo,
                std::ptr::addr_of!(info).cast(),
                size_of_val(&info) as u32,
            )
        }
        .map_err(|_| std::io::Error::from(ErrorKind::Other))
    }

    /// The `FILE_RENAME_INFORMATION` length for a target name of `name_bytes`
    /// bytes. The WDK documents the minimum as
    /// `sizeof(FILE_RENAME_INFORMATION) + FileNameLength`; the structure's own
    /// one-unit `FileName` array and trailing padding are not credited.
    fn rename_information_length(name_bytes: usize) -> Option<usize> {
        size_of::<FILE_RENAME_INFORMATION>().checked_add(name_bytes)
    }

    /// Renames the open `file` to `name` inside the directory `root` is a
    /// handle to, replacing an existing entry. Both objects are named by
    /// handle: the kernel resolves `name` relative to `root` and nothing
    /// else. The Win32 `SetFileInformationByHandle(FileRenameInfo)` wrapper is
    /// not used because it resolves a relative target against the process
    /// working directory and refuses one paired with a root handle (measured
    /// on Windows 11 for #521).
    fn rename_within(file: &File, root: &File, name: &str) -> std::io::Result<()> {
        let invalid = || std::io::Error::from(ErrorKind::InvalidInput);
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_bytes = units.len().checked_mul(2).ok_or_else(invalid)?;
        let length = rename_information_length(name_bytes).ok_or_else(invalid)?;
        // `u64` storage gives the 8-byte alignment the structure needs.
        let mut buffer = vec![0_u64; length.div_ceil(size_of::<u64>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: `buffer` is aligned for the structure and holds at least
        // `length` bytes, which covers the header plus `units` copied at the
        // `FileName` offset; nothing is written past that.
        unsafe {
            (*info).Anonymous.ReplaceIfExists = true;
            (*info).RootDirectory = HANDLE(root.as_raw_handle());
            (*info).FileNameLength = u32::try_from(name_bytes).map_err(|_| invalid())?;
            std::ptr::copy_nonoverlapping(
                units.as_ptr(),
                std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
                units.len(),
            );
        }
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: both handles stay open for the call, `info` points to a
        // complete `FILE_RENAME_INFORMATION` of the stated length, and
        // `status_block` is writable.
        let status = unsafe {
            NtSetInformationFile(
                HANDLE(file.as_raw_handle()),
                &mut status_block,
                info.cast_const().cast(),
                u32::try_from(length).map_err(|_| invalid())?,
                FileRenameInformation,
            )
        };
        if status.is_ok() {
            Ok(())
        } else {
            Err(std::io::Error::from(ErrorKind::Other))
        }
    }

    /// How a child of the held directory is opened.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ChildOpen {
        OpenExisting,
        OpenOrCreate,
        /// Creates the child and fails if it exists, so the object is known to
        /// be ours to delete if it is then refused.
        CreateNew,
    }

    /// Opens `name` inside the directory `root` is a handle to, resolved by
    /// the kernel relative to that handle alone (the Windows counterpart of
    /// `openat`), so no ancestor path is consulted. A final reparse point is
    /// opened as itself, and a directory is refused.
    fn open_relative(
        root: &File,
        name: &str,
        access: u32,
        open: ChildOpen,
    ) -> std::io::Result<File> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let length = units
            .len()
            .checked_mul(2)
            .and_then(|bytes| u16::try_from(bytes).ok())
            .ok_or_else(|| std::io::Error::from(ErrorKind::InvalidInput))?;
        let object_name = UNICODE_STRING {
            Length: length,
            MaximumLength: length,
            Buffer: PWSTR(units.as_ptr().cast_mut()),
        };
        let attributes = OBJECT_ATTRIBUTES {
            Length: size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: HANDLE(root.as_raw_handle()),
            ObjectName: &object_name,
            Attributes: OBJ_CASE_INSENSITIVE,
            SecurityDescriptor: std::ptr::null(),
            SecurityQualityOfService: std::ptr::null(),
        };
        let disposition = match open {
            ChildOpen::OpenExisting => FILE_OPEN,
            ChildOpen::OpenOrCreate => FILE_OPEN_IF,
            ChildOpen::CreateNew => FILE_CREATE,
        };
        let mut handle = HANDLE::default();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: every pointer refers to a local that outlives the call, and
        // `root` stays open throughout. The kernel only reads `units` through
        // `object_name`.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                FILE_ACCESS_RIGHTS(
                    access | FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0 | SYNCHRONIZE.0,
                ),
                &attributes,
                &mut status_block,
                None,
                FILE_ATTRIBUTE_NORMAL,
                CHILD_SHARE,
                disposition,
                FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
                None,
                0,
            )
        };
        if status.is_ok() {
            // SAFETY: on success `handle` is a new handle owned by nothing else.
            return Ok(unsafe { File::from_raw_handle(handle.0) });
        }
        Err(std::io::Error::from(match status {
            STATUS_OBJECT_NAME_NOT_FOUND => ErrorKind::NotFound,
            STATUS_OBJECT_NAME_COLLISION => ErrorKind::AlreadyExists,
            STATUS_ACCESS_DENIED | STATUS_FILE_IS_A_DIRECTORY => ErrorKind::PermissionDenied,
            _ => ErrorKind::Other,
        }))
    }

    const CHILD_SHARE: FILE_SHARE_MODE =
        FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);

    fn open_failure(kind: ErrorKind) -> CredentialStoreFailure {
        match kind {
            // Opening a directory as a file is refused with access denied.
            ErrorKind::PermissionDenied => CredentialStoreFailure::PermissionHardeningFailed,
            _ => CredentialStoreFailure::Unavailable,
        }
    }

    /// Opens `path` as a held directory: without following a final reparse
    /// point, without `FILE_SHARE_DELETE`, and with `WRITE_DAC` for hardening.
    pub(in crate::credentials::adapters) fn open_directory_handle(
        path: &Path,
    ) -> std::io::Result<File> {
        OpenOptions::new()
            .access_mode(
                FILE_LIST_DIRECTORY.0
                    | FILE_TRAVERSE.0
                    | FILE_READ_ATTRIBUTES.0
                    | READ_CONTROL.0
                    | WRITE_DAC.0
                    | SYNCHRONIZE.0,
            )
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
    }

    impl HeldDirectory {
        pub(in crate::credentials::adapters) fn open_existing(
            path: &Path,
        ) -> Result<Option<Self>, CredentialStoreFailure> {
            let handle = match open_directory_handle(path) {
                Ok(handle) => handle,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(open_failure(error.kind())),
            };
            require_kind(&handle, true)?;
            require_trusted_owner(&handle)?;
            harden_directory(&handle)?;
            let final_path =
                final_path(&handle).ok_or(CredentialStoreFailure::PermissionHardeningFailed)?;
            Ok(Some(Self { handle, final_path }))
        }

        pub(in crate::credentials::adapters) fn open_or_create(
            path: &Path,
        ) -> Result<Self, CredentialStoreFailure> {
            if let Some(held) = Self::open_existing(path)? {
                return Ok(held);
            }
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|_| CredentialStoreFailure::Unavailable)?;
            }
            // Created with its protected owner-only DACL, so no other principal
            // can open it between creation and hardening.
            match crate::fs_util::create_owner_only_directory(path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(open_failure(error.kind())),
            }
            Self::open_existing(path)?.ok_or(CredentialStoreFailure::Unavailable)
        }

        /// Opens `name` relative to the held directory handle, refuses it
        /// unless it is a regular file that a trusted principal owns, and
        /// hardens it through the handle before returning it. A child this
        /// open created is deleted through its handle if it is refused.
        fn open_child(
            &self,
            name: &str,
            access: u32,
            open: ChildOpen,
        ) -> Result<Option<File>, CredentialStoreFailure> {
            let file = match open_relative(&self.handle, name, access, open) {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(open_failure(error.kind())),
            };
            match self.verify_and_harden_child(&file) {
                Ok(()) => Ok(Some(file)),
                Err(failure) => {
                    if open == ChildOpen::CreateNew {
                        let _ = dispose(&file);
                    }
                    Err(failure)
                }
            }
        }

        fn verify_and_harden_child(&self, file: &File) -> Result<(), CredentialStoreFailure> {
            require_kind(file, false)?;
            // Redundant with the handle-relative open; kept so a resolution
            // outside the held directory can never pass silently.
            let child =
                final_path(file).ok_or(CredentialStoreFailure::PermissionHardeningFailed)?;
            if child.parent() != Some(self.final_path.as_path()) {
                return Err(CredentialStoreFailure::PermissionHardeningFailed);
            }
            require_trusted_owner(file)?;
            harden(file)
        }

        pub(in crate::credentials::adapters) fn open_lock_file(
            &self,
            name: &str,
        ) -> Result<File, CredentialStoreFailure> {
            self.open_child(
                name,
                FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                ChildOpen::OpenOrCreate,
            )?
            .ok_or(CredentialStoreFailure::Unavailable)
        }

        pub(in crate::credentials::adapters) fn read_capped(
            &self,
            name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            match self.open_child(name, FILE_GENERIC_READ.0, ChildOpen::OpenExisting)? {
                Some(file) => read_at_most(file, limit).map(Some),
                None => Ok(None),
            }
        }

        /// Deletes every staged file a failed discard left for `name`, each
        /// through a handle opened relative to the held directory. Callers
        /// hold the authority mutation lock, so no other writer's staged file
        /// is in flight. Names come from listing the held directory's final
        /// path, never the caller's path, whose ancestors can be swapped; a
        /// name that no longer resolves in the held directory is skipped.
        /// Anything that cannot be deleted fails closed as `Unavailable`
        /// rather than being ignored.
        fn sweep_staged(&self, name: &str) -> Result<(), CredentialStoreFailure> {
            let entries = std::fs::read_dir(&self.final_path)
                .map_err(|_| CredentialStoreFailure::Unavailable)?;
            for entry in entries {
                let entry = entry.map_err(|_| CredentialStoreFailure::Unavailable)?;
                let Some(candidate) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !is_staged_name(&candidate, name) {
                    continue;
                }
                let stray = match open_relative(
                    &self.handle,
                    &candidate,
                    DELETE.0,
                    ChildOpen::OpenExisting,
                ) {
                    Ok(stray) => stray,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(_) => return Err(CredentialStoreFailure::Unavailable),
                };
                dispose(&stray).map_err(|_| CredentialStoreFailure::Unavailable)?;
            }
            Ok(())
        }

        pub(in crate::credentials::adapters) fn create_staged(
            &self,
            name: &str,
        ) -> Result<StagedFile<'_>, CredentialStoreFailure> {
            self.sweep_staged(name)?;
            let file = self
                .open_child(
                    &staged_name(name),
                    // The rename and the discard both go through this handle.
                    FILE_GENERIC_WRITE.0 | DELETE.0,
                    ChildOpen::CreateNew,
                )?
                .ok_or(CredentialStoreFailure::Unavailable)?;
            Ok(StagedFile {
                directory: self,
                file: Some(file),
                name: name.to_owned(),
                published: false,
            })
        }
    }

    impl StagedFile<'_> {
        pub(in crate::credentials::adapters) fn write_all(
            &mut self,
            bytes: &[u8],
        ) -> std::io::Result<()> {
            match self.file.as_mut() {
                Some(file) => file.write_all(bytes),
                None => Err(std::io::Error::from(ErrorKind::BrokenPipe)),
            }
        }

        /// Flushes the staged bytes to the file.
        pub(in crate::credentials::adapters) fn sync(&mut self) -> std::io::Result<()> {
            match self.file.as_ref() {
                Some(file) => file.sync_all(),
                None => Err(std::io::Error::from(ErrorKind::BrokenPipe)),
            }
        }

        /// Renames the flushed staged file over `name` in the held root and
        /// closes it. The rename names both objects by handle, so an ancestor
        /// that points elsewhere by now cannot redirect it. No directory
        /// barrier follows: same-volume rename survival across power loss
        /// stays unproven (#521 item 7).
        pub(in crate::credentials::adapters) fn publish(mut self) -> std::io::Result<()> {
            let file = self
                .file
                .as_ref()
                .ok_or_else(|| std::io::Error::from(ErrorKind::BrokenPipe))?;
            rename_within(file, &self.directory.handle, &self.name)?;
            self.published = true;
            drop(self.file.take());
            Ok(())
        }

        pub(in crate::credentials::adapters) fn commit(mut self) -> std::io::Result<()> {
            self.sync()?;
            self.publish()
        }
    }

    /// An unpublished staged file is deleted through its own handle, never by
    /// a name that might now resolve elsewhere.
    impl Drop for StagedFile<'_> {
        fn drop(&mut self) {
            if !self.published
                && let Some(file) = self.file.take()
            {
                let _ = dispose(&file);
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::{FILE_RENAME_INFORMATION, rename_information_length};

        /// The length passed to the kernel is the documented minimum,
        /// `sizeof(FILE_RENAME_INFORMATION) + FileNameLength`, not the
        /// `FileName` offset plus the name, which is short by the structure's
        /// one-unit array and padding.
        #[test]
        fn rename_information_length_is_the_documented_minimum() {
            let name_bytes = "authority-journal.json".encode_utf16().count() * 2;
            let length = rename_information_length(name_bytes).expect("in range");

            assert_eq!(length, size_of::<FILE_RENAME_INFORMATION>() + name_bytes);
            assert!(length > std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName) + name_bytes);
            #[cfg(target_pointer_width = "64")]
            assert_eq!(size_of::<FILE_RENAME_INFORMATION>(), 24);
            assert_eq!(rename_information_length(usize::MAX), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::read_at_most;

    /// An endless source that counts every byte handed out.
    struct CountingReader {
        handed_out: usize,
    }

    impl std::io::Read for CountingReader {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            buffer.fill(b' ');
            self.handed_out += buffer.len();
            Ok(buffer.len())
        }
    }

    /// A reader that would hand out unbounded bytes is asked for exactly the
    /// limit, so nothing past it is ever read, let alone allocated.
    #[test]
    fn reading_stops_at_the_limit_instead_of_truncating_a_full_read() {
        let limit = 256 * 1024 + 1;
        let mut reader = CountingReader { handed_out: 0 };

        let bytes = read_at_most(&mut reader, limit).expect("bounded read");

        assert_eq!(bytes.len(), limit);
        assert_eq!(reader.handed_out, limit);
    }
}
