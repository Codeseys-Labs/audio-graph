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
use std::fs::File;
use std::io::Read;

#[cfg(unix)]
pub(in crate::credentials::adapters) use unix_held::{HeldDirectory, StagedFile};
#[cfg(windows)]
pub(in crate::credentials::adapters) use windows_held::{
    HeldDirectory, StagedFile, token_profile_directory,
};

/// Reads at most `limit` bytes, so an oversized journal is classified by its
/// decoder without first being allocated in full.
fn read_at_most(file: File, limit: usize) -> Result<Vec<u8>, CredentialStoreFailure> {
    let mut bytes = Vec::new();
    file.take(u64::try_from(limit).map_err(|_| CredentialStoreFailure::Internal)?)
        .read_to_end(&mut bytes)
        .map_err(|_| CredentialStoreFailure::Unavailable)?;
    Ok(bytes)
}

fn staged_name(name: &str) -> String {
    format!(".{name}.{}.tmp", uuid::Uuid::new_v4().simple())
}

#[cfg(unix)]
mod unix_held {
    use super::{read_at_most, staged_name};
    use crate::credentials::domain::CredentialStoreFailure;
    use rustix::fd::{AsFd, OwnedFd};
    use rustix::fs::{
        AtFlags, CWD, FileType, Mode, OFlags, RawMode, fchmod, fstat, fsync, mkdirat, openat,
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

        pub(in crate::credentials::adapters) fn create_staged(
            &self,
            name: &str,
        ) -> Result<StagedFile<'_>, CredentialStoreFailure> {
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

        /// Flushes the staged bytes, renames them over `name` relative to the
        /// held directory, then flushes that directory so the rename itself is
        /// durable.
        pub(in crate::credentials::adapters) fn commit(mut self) -> std::io::Result<()> {
            self.file.sync_all()?;
            let directory = self.directory.fd.as_fd();
            renameat(directory, self.temp.as_str(), directory, self.name.as_str())?;
            self.published = true;
            fsync(directory)?;
            Ok(())
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
    use super::{read_at_most, staged_name};
    use crate::credentials::domain::CredentialStoreFailure;
    use ::windows::Win32::Foundation::{CloseHandle, HANDLE};
    use ::windows::Win32::Security::TOKEN_QUERY;
    use ::windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE, GetFinalPathNameByHandleW,
        MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW, READ_CONTROL, SYNCHRONIZE,
        VOLUME_NAME_GUID, WRITE_DAC,
    };
    use ::windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use ::windows::Win32::UI::Shell::GetUserProfileDirectoryW;
    use ::windows::core::{PCWSTR, PWSTR};
    use std::ffi::OsString;
    use std::fs::{File, OpenOptions};
    use std::io::{ErrorKind, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::AsRawHandle;
    use std::path::{Path, PathBuf};

    const MAX_FINAL_PATH_UNITS: usize = 32_768;

    /// A directory handle opened without `FILE_SHARE_DELETE`, so the held
    /// directory cannot be renamed or deleted out from under its operations.
    pub(in crate::credentials::adapters) struct HeldDirectory {
        _handle: File,
        path: PathBuf,
        final_path: PathBuf,
    }

    pub(in crate::credentials::adapters) struct StagedFile<'directory> {
        directory: &'directory HeldDirectory,
        file: Option<File>,
        temp: String,
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

    fn open_failure(kind: ErrorKind) -> CredentialStoreFailure {
        match kind {
            // Opening a directory as a file is refused with access denied.
            ErrorKind::PermissionDenied => CredentialStoreFailure::PermissionHardeningFailed,
            _ => CredentialStoreFailure::Unavailable,
        }
    }

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    impl HeldDirectory {
        pub(in crate::credentials::adapters) fn open_existing(
            path: &Path,
        ) -> Result<Option<Self>, CredentialStoreFailure> {
            let handle = match OpenOptions::new()
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
            {
                Ok(handle) => handle,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(open_failure(error.kind())),
            };
            require_kind(&handle, true)?;
            harden_directory(&handle)?;
            let final_path =
                final_path(&handle).ok_or(CredentialStoreFailure::PermissionHardeningFailed)?;
            Ok(Some(Self {
                _handle: handle,
                path: path.to_owned(),
                final_path,
            }))
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
            match std::fs::create_dir(path) {
                Ok(()) => {}
                Err(error) if error.kind() == ErrorKind::AlreadyExists => {}
                Err(error) => return Err(open_failure(error.kind())),
            }
            Self::open_existing(path)?.ok_or(CredentialStoreFailure::Unavailable)
        }

        /// Opens a child without following a final reparse point, refuses it
        /// unless its final path names a direct child of the held directory,
        /// and hardens it through the handle before returning it.
        fn open_child(
            &self,
            name: &str,
            options: &mut OpenOptions,
        ) -> Result<Option<File>, CredentialStoreFailure> {
            let file = match options
                .share_mode(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0)
                .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(self.path.join(name))
            {
                Ok(file) => file,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(open_failure(error.kind())),
            };
            require_kind(&file, false)?;
            let child =
                final_path(&file).ok_or(CredentialStoreFailure::PermissionHardeningFailed)?;
            if child.parent() != Some(self.final_path.as_path()) {
                return Err(CredentialStoreFailure::PermissionHardeningFailed);
            }
            harden(&file)?;
            Ok(Some(file))
        }

        pub(in crate::credentials::adapters) fn open_lock_file(
            &self,
            name: &str,
        ) -> Result<File, CredentialStoreFailure> {
            self.open_child(name, OpenOptions::new().read(true).write(true).create(true))?
                .ok_or(CredentialStoreFailure::Unavailable)
        }

        pub(in crate::credentials::adapters) fn read_capped(
            &self,
            name: &str,
            limit: usize,
        ) -> Result<Option<Vec<u8>>, CredentialStoreFailure> {
            match self.open_child(name, OpenOptions::new().read(true))? {
                Some(file) => read_at_most(file, limit).map(Some),
                None => Ok(None),
            }
        }

        pub(in crate::credentials::adapters) fn create_staged(
            &self,
            name: &str,
        ) -> Result<StagedFile<'_>, CredentialStoreFailure> {
            let temp = staged_name(name);
            let file = match self.open_child(&temp, OpenOptions::new().write(true).create_new(true))
            {
                Ok(Some(file)) => file,
                Ok(None) => return Err(CredentialStoreFailure::Unavailable),
                Err(failure) => {
                    // The uniquely named file may exist without its hardening.
                    let _ = std::fs::remove_file(self.path.join(&temp));
                    return Err(failure);
                }
            };
            Ok(StagedFile {
                directory: self,
                file: Some(file),
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
            match self.file.as_mut() {
                Some(file) => file.write_all(bytes),
                None => Err(std::io::Error::from(ErrorKind::BrokenPipe)),
            }
        }

        /// Flushes the staged bytes and replaces `name` with them. The move
        /// names the held directory, which cannot be renamed while open; if an
        /// ancestor swap redirected it anyway, the readback through the held
        /// directory would not see the new bytes. `MOVEFILE_WRITE_THROUGH`
        /// only flushes a move that degrades to copy-and-delete; same-volume
        /// rename durability stays unproven (#521 item 7).
        pub(in crate::credentials::adapters) fn commit(mut self) -> std::io::Result<()> {
            let file = self
                .file
                .take()
                .ok_or_else(|| std::io::Error::from(ErrorKind::BrokenPipe))?;
            file.sync_all()?;
            drop(file);
            let source = wide(&self.directory.path.join(&self.temp));
            let destination = wide(&self.directory.path.join(&self.name));
            // SAFETY: both paths are NUL-terminated and outlive the call.
            unsafe {
                MoveFileExW(
                    PCWSTR(source.as_ptr()),
                    PCWSTR(destination.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
            .map_err(|_| std::io::Error::from(ErrorKind::Other))?;
            self.published = true;
            Ok(())
        }
    }

    impl Drop for StagedFile<'_> {
        fn drop(&mut self) {
            if !self.published {
                drop(self.file.take());
                let _ = std::fs::remove_file(self.directory.path.join(&self.temp));
            }
        }
    }
}
