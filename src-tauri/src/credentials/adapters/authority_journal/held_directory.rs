//! Owner-only directories held open for the authority journal and the native
//! namespace lock.
//!
//! A directory is opened without following a final link and then verified and
//! hardened on that same handle. Children are opened relative to it (Unix
//! `openat`; on Windows `NtCreateFile` with the held handle as its root), and
//! each one is verified and hardened on the handle that is then used for I/O.
//! All of this happens before a single journal byte is written. A directory
//! this module creates is made durable in its parent before it is used.
//! Every failure is a closed `CredentialStoreFailure`; no OS text or path
//! leaves this module.

use crate::credentials::domain::CredentialStoreFailure;
use std::io::Read;

#[cfg(unix)]
pub(in crate::credentials::adapters) use unix_held::{
    HeldDirectory, StagedFile, take_ownership_if_refused,
};
#[cfg(all(windows, test))]
pub(in crate::credentials::adapters) use windows_held::open_directory_handle;
#[cfg(windows)]
pub(in crate::credentials::adapters) use windows_held::{
    HeldDirectory, StagedFile, take_ownership_if_refused, token_profile_directory,
};

/// Why a staged replacement did not report success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::credentials::adapters) enum PublishFailure {
    /// The target provably still holds what it held before: the staged bytes
    /// never became visible under the target name.
    NotPublished,
    /// The rename may have taken effect, or took effect and could not be
    /// flushed to stable storage.
    Unknown,
}

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

/// Splits `path` into its parent directory, the working directory for a bare
/// name, and its final component.
fn split_parent(
    path: &std::path::Path,
) -> Result<(&std::path::Path, &std::ffi::OsStr), CredentialStoreFailure> {
    let name = path
        .file_name()
        .ok_or(CredentialStoreFailure::Unavailable)?;
    let parent = match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => std::path::Path::new("."),
    };
    Ok((parent, name))
}

/// Whether `candidate` has exactly the shape [`staged_name`] produces for
/// `name`: `.{name}.` and a `.tmp` suffix around a simple-form version 4
/// UUID (32 lowercase hex digits, version digit `4`, variant digit one of
/// `8`, `9`, `a`, `b`).
///
/// This is a reserved name inside the journal root, not proof of origin: a
/// file someone else deliberately gives such a name is deleted as a stray
/// staged file. A directory under such a name fails the sweep closed; any
/// other entry, a link included, is removed itself, never what it points to.
fn is_staged_name(candidate: &str, name: &str) -> bool {
    candidate
        .strip_prefix('.')
        .and_then(|rest| rest.strip_prefix(name))
        .and_then(|rest| rest.strip_prefix('.'))
        .and_then(|rest| rest.strip_suffix(".tmp"))
        .is_some_and(|token| {
            let token = token.as_bytes();
            token.len() == 32
                && token
                    .iter()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(byte))
                && token[12] == b'4'
                && matches!(token[16], b'8' | b'9' | b'a' | b'b')
        })
}

#[cfg(unix)]
mod unix_held {
    use super::{PublishFailure, is_staged_name, read_at_most, split_parent, staged_name};
    use crate::credentials::domain::CredentialStoreFailure;
    use rustix::fd::{AsFd, OwnedFd};
    use rustix::fs::{
        AtFlags, CWD, Dir, FileType, Mode, OFlags, RawMode, fchmod, fstat, fsync, mkdirat, openat,
        renameat, statat, unlinkat,
    };
    use rustix::io::Errno;
    use std::ffi::OsStr;
    use std::fs::File;
    use std::io::Write;
    use std::path::Path;

    const DIRECTORY_MODE: RawMode = 0o700;
    const FILE_MODE: RawMode = 0o600;
    /// Ancestors created on the way to a held root get the mode
    /// `create_dir_all` would give them; only the held root is owner-only.
    const ANCESTOR_MODE: RawMode = 0o777;
    const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::CLOEXEC);

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

    /// Creates the directory `name` inside `parent` and, if this call created
    /// it, syncs `parent`, so the new entry is durable before anything is
    /// placed in it.
    fn create_child_directory(
        parent: &OwnedFd,
        name: &OsStr,
        bits: RawMode,
    ) -> Result<(), CredentialStoreFailure> {
        match mkdirat(parent, name, mode(bits)) {
            Ok(()) => fsync(parent).map_err(|_| CredentialStoreFailure::Unavailable),
            Err(Errno::EXIST) => Ok(()),
            Err(errno) => Err(open_failure(errno)),
        }
    }

    /// Opens the directory `path`, first creating it and every missing
    /// ancestor. Each directory this creates is made relative to its open
    /// parent, which is synced, and then opened relative to that parent
    /// without following a link.
    fn open_or_create_ancestor(path: &Path) -> Result<OwnedFd, CredentialStoreFailure> {
        match openat(CWD, path, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => return Ok(fd),
            Err(Errno::NOENT) => {}
            Err(errno) => return Err(open_failure(errno)),
        }
        let (parent, name) = split_parent(path)?;
        let parent = open_or_create_ancestor(parent)?;
        create_child_directory(&parent, name, ANCESTOR_MODE)?;
        openat(
            &parent,
            name,
            DIRECTORY_FLAGS | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(open_failure)
    }

    impl HeldDirectory {
        pub(in crate::credentials::adapters) fn open_existing(
            path: &Path,
        ) -> Result<Option<Self>, CredentialStoreFailure> {
            match openat(CWD, path, DIRECTORY_FLAGS | OFlags::NOFOLLOW, Mode::empty()) {
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
            let (parent, name) = split_parent(path)?;
            let parent = open_or_create_ancestor(parent)?;
            // The umask can only remove bits, so the directory is never
            // broader than owner-only before it is hardened on its handle.
            create_child_directory(&parent, name, DIRECTORY_MODE)?;
            let fd = openat(
                &parent,
                name,
                DIRECTORY_FLAGS | OFlags::NOFOLLOW,
                Mode::empty(),
            )
            .map_err(open_failure)?;
            harden(&fd, FileType::Directory, DIRECTORY_MODE)?;
            Ok(Self { fd })
        }

        /// A second descriptor for the held directory, for inspecting the
        /// same object rather than whatever its path names by then.
        #[cfg(target_os = "macos")]
        pub(in crate::credentials::adapters) fn duplicate_descriptor(
            &self,
        ) -> std::io::Result<OwnedFd> {
            self.fd.try_clone()
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

        /// Whether `name` in the held directory still designates `file`. A
        /// lock taken on a file the name no longer designates excludes nobody
        /// who opens the name now.
        pub(in crate::credentials::adapters) fn names_file(
            &self,
            name: &str,
            file: &File,
        ) -> Result<bool, CredentialStoreFailure> {
            let held = fstat(file).map_err(|_| CredentialStoreFailure::Unavailable)?;
            match statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(named) => Ok(named.st_dev == held.st_dev && named.st_ino == held.st_ino),
                Err(Errno::NOENT) => Ok(false),
                Err(_) => Err(CredentialStoreFailure::Unavailable),
            }
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
        /// Eligibility is by name alone (see [`is_staged_name`]). Anything
        /// that cannot be removed fails closed as `Unavailable` rather than
        /// being ignored.
        pub(in crate::credentials::adapters) fn sweep_staged(
            &self,
            name: &str,
        ) -> Result<(), CredentialStoreFailure> {
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
        /// durable. POSIX `rename` leaves the target unaffected on every
        /// failure except `EIO`, so only that error and a failed directory
        /// sync leave the outcome open.
        pub(in crate::credentials::adapters) fn publish(mut self) -> Result<(), PublishFailure> {
            let directory = self.directory.fd.as_fd();
            match renameat(directory, self.temp.as_str(), directory, self.name.as_str()) {
                Ok(()) => {}
                Err(Errno::IO) => return Err(PublishFailure::Unknown),
                Err(_) => return Err(PublishFailure::NotPublished),
            }
            self.published = true;
            fsync(directory).map_err(|_| PublishFailure::Unknown)
        }

        pub(in crate::credentials::adapters) fn commit(mut self) -> Result<(), PublishFailure> {
            self.sync().map_err(|_| PublishFailure::NotPublished)?;
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

    /// An unprivileged process cannot give itself ownership of a file on
    /// Unix, so a directory or child another user owns stays refused. This
    /// only re-applies the owner-only modes to `path` and the `children`
    /// that exist, and reports that no owner changed.
    pub(in crate::credentials::adapters) fn take_ownership_if_refused(
        path: &Path,
        children: &[&str],
    ) -> Result<bool, CredentialStoreFailure> {
        let Some(held) = HeldDirectory::open_existing(path)? else {
            return Ok(false);
        };
        for child in children {
            held.open_child(child, OFlags::RDONLY)?;
        }
        Ok(false)
    }
}

#[cfg(windows)]
mod windows_held {
    use super::{PublishFailure, is_staged_name, read_at_most, split_parent, staged_name};
    use crate::credentials::domain::CredentialStoreFailure;
    use ::windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use ::windows::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF, FILE_OPEN_REPARSE_POINT,
        FILE_RENAME_INFORMATION, FILE_RENAME_POSIX_SEMANTICS, FILE_RENAME_REPLACE_IF_EXISTS,
        FILE_SYNCHRONOUS_IO_NONALERT, FileRenameInformationEx, NtCreateFile, NtSetInformationFile,
    };
    use ::windows::Win32::Foundation::{
        CloseHandle, HANDLE, NO_ERROR, NTSTATUS, OBJ_CASE_INSENSITIVE, STATUS_ACCESS_DENIED,
        STATUS_FILE_IS_A_DIRECTORY, STATUS_INVALID_INFO_CLASS, STATUS_INVALID_PARAMETER,
        STATUS_NOT_SUPPORTED, STATUS_OBJECT_NAME_COLLISION, STATUS_OBJECT_NAME_NOT_FOUND,
        STATUS_SHARING_VIOLATION, UNICODE_STRING,
    };
    use ::windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
    use ::windows::Win32::Security::{
        GetTokenInformation, OWNER_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use ::windows::Win32::Storage::FileSystem::{
        DELETE, FILE_ACCESS_RIGHTS, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_GENERIC_WRITE, FILE_ID_INFO, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
        FileDispositionInfo, FileIdInfo, FlushFileBuffers, GetFileInformationByHandleEx,
        GetFinalPathNameByHandleW, READ_CONTROL, SYNCHRONIZE, SetFileInformationByHandle,
        VOLUME_NAME_GUID, WRITE_DAC, WRITE_OWNER,
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
    /// handle to, replacing an existing entry even while other handles have
    /// it open (POSIX semantics, Windows 10 1709 and later): those handles
    /// keep reading the replaced file, and only an opener that refused
    /// `FILE_SHARE_DELETE` still blocks the rename. Both objects are named by
    /// handle: the kernel resolves `name` relative to `root` and nothing
    /// else. The Win32 `SetFileInformationByHandle(FileRenameInfo)` wrapper is
    /// not used because it resolves a relative target against the process
    /// working directory and refuses one paired with a root handle (measured
    /// on Windows 11 for #521).
    fn rename_within(file: &File, root: &File, name: &str) -> Result<(), PublishFailure> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_bytes = units
            .len()
            .checked_mul(2)
            .ok_or(PublishFailure::NotPublished)?;
        let length = rename_information_length(name_bytes).ok_or(PublishFailure::NotPublished)?;
        let name_length = u32::try_from(name_bytes).map_err(|_| PublishFailure::NotPublished)?;
        let info_length = u32::try_from(length).map_err(|_| PublishFailure::NotPublished)?;
        // `u64` storage gives the 8-byte alignment the structure needs.
        let mut buffer = vec![0_u64; length.div_ceil(size_of::<u64>())];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        // SAFETY: `buffer` is aligned for the structure and holds at least
        // `length` bytes, which covers the header plus `units` copied at the
        // `FileName` offset; nothing is written past that.
        unsafe {
            (*info).Anonymous.Flags = FILE_RENAME_REPLACE_IF_EXISTS | FILE_RENAME_POSIX_SEMANTICS;
            (*info).RootDirectory = HANDLE(root.as_raw_handle());
            (*info).FileNameLength = name_length;
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
                info_length,
                FileRenameInformationEx,
            )
        };
        if status.is_ok() {
            Ok(())
        } else if rename_refused_before_effect(status) {
            Err(PublishFailure::NotPublished)
        } else {
            Err(PublishFailure::Unknown)
        }
    }

    /// Statuses with which the kernel refuses a rename request (a sharing or
    /// access conflict, a name collision, or a request it does not support)
    /// before it changes any name, so the target still holds what it held.
    /// Every other failure leaves the outcome open.
    fn rename_refused_before_effect(status: NTSTATUS) -> bool {
        matches!(
            status,
            STATUS_SHARING_VIOLATION
                | STATUS_ACCESS_DENIED
                | STATUS_OBJECT_NAME_COLLISION
                | STATUS_INVALID_PARAMETER
                | STATUS_INVALID_INFO_CLASS
                | STATUS_NOT_SUPPORTED
        )
    }

    /// Flushes a directory's metadata to stable storage through `directory`,
    /// which must carry `FILE_ADD_FILE` or `FILE_ADD_SUBDIRECTORY`, the
    /// directory meanings of the write rights a flush requires (measured on
    /// Windows 11 for #521: without one the flush is denied). NTFS documents
    /// a directory flush as persisting the directory structure ([MS-FSA]
    /// 2.1.5.7); see `StagedFile::publish` for what that does not prove.
    fn flush_directory(directory: &File) -> std::io::Result<()> {
        // SAFETY: the handle stays open for the call.
        unsafe { FlushFileBuffers(HANDLE(directory.as_raw_handle())) }
            .map_err(|_| std::io::Error::from(ErrorKind::Other))
    }

    /// Flushes the directory at `path` once an entry was created in it.
    fn flush_directory_at(path: &Path) -> std::io::Result<()> {
        let directory = OpenOptions::new()
            .access_mode(FILE_ADD_SUBDIRECTORY.0 | SYNCHRONIZE.0)
            .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)?;
        flush_directory(&directory)
    }

    /// Creates `path` and every missing ancestor with inherited security, as
    /// `create_dir_all` would, flushing the parent of each directory this
    /// creates so the new entry is durable before anything is placed in it.
    fn create_missing_directories(path: &Path) -> Result<(), CredentialStoreFailure> {
        match std::fs::metadata(path) {
            Ok(metadata) if metadata.is_dir() => return Ok(()),
            Ok(_) => return Err(CredentialStoreFailure::Unavailable),
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(CredentialStoreFailure::Unavailable),
        }
        let (parent, _) = split_parent(path)?;
        create_missing_directories(parent)?;
        match std::fs::create_dir(path) {
            Ok(()) => flush_directory_at(parent).map_err(|_| CredentialStoreFailure::Unavailable),
            Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
            Err(_) => Err(CredentialStoreFailure::Unavailable),
        }
    }

    /// The volume serial number and 128-bit file ID of an open file.
    fn file_identity(file: &File) -> Option<(u64, [u8; 16])> {
        let mut info = FILE_ID_INFO::default();
        // SAFETY: `FileIdInfo` writes exactly one `FILE_ID_INFO` into `info`,
        // which outlives the call; the handle stays open throughout.
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileIdInfo,
                std::ptr::addr_of_mut!(info).cast(),
                size_of::<FILE_ID_INFO>() as u32,
            )
        }
        .ok()?;
        Some((info.VolumeSerialNumber, info.FileId.Identifier))
    }

    /// Makes the process user the owner of `object`, whose handle carries
    /// `WRITE_OWNER`. Assigning oneself needs no privilege, only that right.
    fn set_owner_to_process_user(object: &File) -> bool {
        let mut token = HANDLE::default();
        // SAFETY: on success `token` receives a new handle, closed below.
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) }.is_err() {
            return false;
        }
        let mut needed = 0_u32;
        // SAFETY: a null buffer only asks for the size; `token` is open.
        let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut needed) };
        // `u64` storage aligns the `TOKEN_USER` header and the SID after it.
        let mut buffer = vec![0_u64; (needed as usize).div_ceil(size_of::<u64>())];
        let read = (needed as usize) >= size_of::<TOKEN_USER>()
            // SAFETY: `buffer` holds at least `needed` writable bytes and
            // `token` is open.
            && unsafe {
                GetTokenInformation(
                    token,
                    TokenUser,
                    Some(buffer.as_mut_ptr().cast()),
                    needed,
                    &mut needed,
                )
            }
            .is_ok();
        // SAFETY: `token` is open and owned here; it is closed exactly once.
        let _ = unsafe { CloseHandle(token) };
        if !read {
            return false;
        }
        // SAFETY: on success the buffer starts with a `TOKEN_USER` whose SID
        // points into the same buffer, which outlives the call below.
        let sid = unsafe { (*buffer.as_ptr().cast::<TOKEN_USER>()).User.Sid };
        // SAFETY: the handle stays open and carries `WRITE_OWNER`; `sid` is a
        // valid SID for the duration of the call.
        let status = unsafe {
            SetSecurityInfo(
                HANDLE(object.as_raw_handle()),
                SE_FILE_OBJECT,
                OWNER_SECURITY_INFORMATION,
                Some(sid),
                None,
                None,
                None,
            )
        };
        status == NO_ERROR
    }

    /// Takes ownership of `object` when its owner is not trusted, and
    /// returns whether it did.
    fn take_ownership(object: &File) -> Result<bool, CredentialStoreFailure> {
        if matches!(crate::fs_util::owner_is_trusted(object), Ok(true)) {
            return Ok(false);
        }
        if !set_owner_to_process_user(object) {
            return Err(CredentialStoreFailure::PermissionHardeningFailed);
        }
        match crate::fs_util::owner_is_trusted(object) {
            Ok(true) => Ok(true),
            Ok(false) | Err(_) => Err(CredentialStoreFailure::PermissionHardeningFailed),
        }
    }

    /// Explicit repair of a root that hardening refused because another
    /// principal owns it, or owns one of its `children`. Makes the process
    /// user the owner of the directory at `path` and of each existing child
    /// whose owner is not trusted (see `fs_util::owner_is_trusted`), then
    /// hardens each one. Assigning oneself as owner needs `WRITE_OWNER` on
    /// the object, which a DACL granting the user full control carries;
    /// without it the refusal stands as `PermissionHardeningFailed`. Returns
    /// whether any owner changed.
    pub(in crate::credentials::adapters) fn take_ownership_if_refused(
        path: &Path,
        children: &[&str],
    ) -> Result<bool, CredentialStoreFailure> {
        let directory = match open_directory_with(path, WRITE_OWNER.0) {
            Ok(directory) => directory,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(open_failure(error.kind())),
        };
        require_kind(&directory, true)?;
        let mut changed = take_ownership(&directory)?;
        harden_directory(&directory)?;
        for child in children {
            let file = match open_relative(
                &directory,
                child,
                WRITE_OWNER.0 | WRITE_DAC.0,
                ChildOpen::OpenExisting,
                CHILD_SHARE,
            ) {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => continue,
                Err(error) => return Err(open_failure(error.kind())),
            };
            require_kind(&file, false)?;
            changed |= take_ownership(&file)?;
            harden(&file)?;
        }
        Ok(changed)
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
        share: FILE_SHARE_MODE,
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
                share,
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

    /// What the lock file shares with every other opener: not delete, so
    /// its name cannot be removed or given to another file while any process
    /// holds it open.
    const LOCK_SHARE: FILE_SHARE_MODE = FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);

    fn open_failure(kind: ErrorKind) -> CredentialStoreFailure {
        match kind {
            // Opening a directory as a file is refused with access denied.
            ErrorKind::PermissionDenied => CredentialStoreFailure::PermissionHardeningFailed,
            _ => CredentialStoreFailure::Unavailable,
        }
    }

    /// Opens `path` as a held directory: without following a final reparse
    /// point, without `FILE_SHARE_DELETE`, with `WRITE_DAC` for hardening,
    /// and with `FILE_ADD_FILE`, which lets the same handle flush the
    /// directory after a publication.
    pub(in crate::credentials::adapters) fn open_directory_handle(
        path: &Path,
    ) -> std::io::Result<File> {
        open_directory_with(path, 0)
    }

    fn open_directory_with(path: &Path, extra_access: u32) -> std::io::Result<File> {
        OpenOptions::new()
            .access_mode(
                FILE_LIST_DIRECTORY.0
                    | FILE_TRAVERSE.0
                    | FILE_ADD_FILE.0
                    | FILE_READ_ATTRIBUTES.0
                    | READ_CONTROL.0
                    | WRITE_DAC.0
                    | SYNCHRONIZE.0
                    | extra_access,
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
            let (parent, _) = split_parent(path)?;
            create_missing_directories(parent)?;
            // Created with its protected owner-only DACL, so no other principal
            // can open it between creation and hardening.
            match crate::fs_util::create_owner_only_directory(path) {
                Ok(()) => {
                    flush_directory_at(parent).map_err(|_| CredentialStoreFailure::Unavailable)?
                }
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(open_failure(error.kind())),
            }
            Self::open_existing(path)?.ok_or(CredentialStoreFailure::Unavailable)
        }

        /// A second handle to the held directory, for inspecting the same
        /// object rather than whatever its path names by then.
        pub(in crate::credentials::adapters) fn duplicate_handle(&self) -> std::io::Result<File> {
            self.handle.try_clone()
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
            share: FILE_SHARE_MODE,
        ) -> Result<Option<File>, CredentialStoreFailure> {
            let file = match open_relative(&self.handle, name, access, open, share) {
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
                LOCK_SHARE,
            )?
            .ok_or(CredentialStoreFailure::Unavailable)
        }

        /// Whether `name` in the held directory still designates `file`. A
        /// lock taken on a file the name no longer designates excludes nobody
        /// who opens the name now.
        pub(in crate::credentials::adapters) fn names_file(
            &self,
            name: &str,
            file: &File,
        ) -> Result<bool, CredentialStoreFailure> {
            let named =
                match open_relative(&self.handle, name, 0, ChildOpen::OpenExisting, CHILD_SHARE) {
                    Ok(named) => named,
                    Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
                    Err(_) => return Err(CredentialStoreFailure::Unavailable),
                };
            match (file_identity(file), file_identity(&named)) {
                (Some(held), Some(named)) => Ok(held == named),
                _ => Err(CredentialStoreFailure::Unavailable),
            }
        }

        pub(in crate::credentials::adapters) fn read_capped(
            &self,
            name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            match self.open_child(
                name,
                FILE_GENERIC_READ.0,
                ChildOpen::OpenExisting,
                CHILD_SHARE,
            )? {
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
        /// Eligibility is by name alone (see [`is_staged_name`]). Anything
        /// that cannot be deleted fails closed as `Unavailable` rather than
        /// being ignored.
        pub(in crate::credentials::adapters) fn sweep_staged(
            &self,
            name: &str,
        ) -> Result<(), CredentialStoreFailure> {
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
                    CHILD_SHARE,
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
                    // The rename, the flush and the discard all go through
                    // this handle.
                    FILE_GENERIC_WRITE.0 | DELETE.0,
                    ChildOpen::CreateNew,
                    CHILD_SHARE,
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

        /// Renames the flushed staged file over `name` in the held root, then
        /// flushes the renamed file and the held root before closing it. The
        /// rename names both objects by handle, so an ancestor that points
        /// elsewhere by now cannot redirect it.
        ///
        /// The two flushes are the strongest barrier a standard user has: a
        /// file flush persists its data and metadata, and an NTFS directory
        /// flush persists the directory structure ([MS-FSA] 2.1.5.7). No
        /// Windows contract states that they make a same-volume rename
        /// survive power loss (Microsoft Q&A 6057877, November 2026), so that
        /// survival stays unproven until packaged evidence exists (#210). A
        /// failed flush leaves the outcome `Unknown`.
        pub(in crate::credentials::adapters) fn publish(mut self) -> Result<(), PublishFailure> {
            let file = self.file.as_ref().ok_or(PublishFailure::NotPublished)?;
            rename_within(file, &self.directory.handle, &self.name)?;
            self.published = true;
            let flushed = file
                .sync_all()
                .and_then(|()| flush_directory(&self.directory.handle));
            drop(self.file.take());
            flushed.map_err(|_| PublishFailure::Unknown)
        }

        pub(in crate::credentials::adapters) fn commit(mut self) -> Result<(), PublishFailure> {
            self.sync().map_err(|_| PublishFailure::NotPublished)?;
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
