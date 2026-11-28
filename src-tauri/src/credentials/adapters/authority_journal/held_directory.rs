//! Owner-only directories held open for the authority journal and the native
//! namespace lock.
//!
//! A directory is opened without following a final link and then verified and
//! hardened on that same handle. Children are opened relative to it (Unix
//! `openat`; on Windows `NtCreateFile` with the held handle as its root), and
//! each one is verified and hardened on the handle that is then used for I/O.
//! All of this happens before a single journal byte is written. A directory
//! this module creates is made durable in its parent before it is used, and a
//! parent is synced only once its entry is verified to be the directory the
//! caller holds. Every failure is a closed `CredentialStoreFailure`; no OS
//! text or path leaves this module.

use crate::credentials::domain::CredentialStoreFailure;
use std::io::Read;

#[cfg(unix)]
pub(in crate::credentials::adapters) use unix_held::{
    DirectoryLock, HeldDirectory, StagedFile, take_ownership_if_refused,
};
#[cfg(windows)]
pub(in crate::credentials::adapters) use windows_held::{
    DIRECTORY_LOCK_FILE_NAME, DirectoryLock, HeldDirectory, StagedFile, take_ownership_if_refused,
    token_profile_directory,
};
#[cfg(all(windows, test))]
pub(in crate::credentials::adapters) use windows_held::{
    create_root_for_test, open_directory_handle, record_refused_owner_for_test,
};

/// Why a staged replacement did not report success.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::credentials::adapters) enum PublishFailure {
    /// Another opener holds the target without sharing delete, so the
    /// kernel refused the rename before it changed any name. The target
    /// still holds what it held, and the same rename can succeed once that
    /// opener lets go.
    Contended,
    /// The target provably still holds what it held before: the staged bytes
    /// never became visible under the target name.
    NotPublished,
    /// The rename may have taken effect, or took effect and could not be
    /// flushed to stable storage.
    Unknown,
}

/// Faults a test arms on its own thread for the next point that checks it.
#[cfg(test)]
pub(in crate::credentials::adapters) mod faults {
    use std::cell::Cell;

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(in crate::credentials::adapters) enum Fault {
        /// Syncing the staged bytes fails.
        StagedSync,
        /// The rename reports an outcome it cannot classify, without having
        /// taken effect.
        RenameUnknownBeforeEffect,
        /// The rename takes effect, then reports an outcome it cannot
        /// classify.
        RenameUnknownAfterEffect,
        /// Syncing the renamed file fails (Windows flushes it; Unix synced it
        /// before the rename).
        PublishedFileSync,
        /// Syncing the held directory after the rename fails.
        HeldDirectorySync,
        /// Syncing a parent once its entry is verified fails.
        ParentSync,
    }

    thread_local! {
        static ARMED: Cell<Option<Fault>> = const { Cell::new(None) };
    }

    /// Arms `fault` for the next point on this thread that checks it.
    pub(in crate::credentials::adapters) fn arm(fault: Fault) {
        ARMED.with(|armed| armed.set(Some(fault)));
    }

    /// Whether `fault` is armed on this thread; disarms it if so.
    pub(super) fn fires(fault: Fault) -> bool {
        ARMED.with(|armed| {
            let fires = armed.get() == Some(fault);
            if fires {
                armed.set(None);
            }
            fires
        })
    }

    #[cfg(unix)]
    thread_local! {
        static PARENT_SYNCS: Cell<usize> = const { Cell::new(0) };
    }

    #[cfg(unix)]
    pub(super) fn count_parent_sync() {
        PARENT_SYNCS.with(|syncs| syncs.set(syncs.get() + 1));
    }

    /// How many parent syncs this thread has attempted.
    #[cfg(unix)]
    pub(in crate::credentials::adapters) fn parent_syncs() -> usize {
        PARENT_SYNCS.with(Cell::get)
    }

    /// Points between two steps of this module where a test can act, as a
    /// concurrent principal could.
    #[cfg(windows)]
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub(in crate::credentials::adapters) enum Point {
        /// An ownership repair has qualified every object it will change and
        /// changed none.
        RepairQualified,
        /// A missing root's parent is held and the root is not created yet.
        RootParentHeld,
    }

    #[cfg(windows)]
    type Hook = (Point, Box<dyn FnOnce()>);

    #[cfg(windows)]
    thread_local! {
        static HOOK: std::cell::RefCell<Option<Hook>> = const { std::cell::RefCell::new(None) };
    }

    /// Runs `hook` when this thread next reaches `point`.
    #[cfg(windows)]
    pub(in crate::credentials::adapters) fn at(point: Point, hook: impl FnOnce() + 'static) {
        HOOK.with(|armed| *armed.borrow_mut() = Some((point, Box::new(hook))));
    }

    #[cfg(windows)]
    pub(super) fn reached(point: Point) {
        let hook = HOOK.with(|armed| {
            let mut armed = armed.borrow_mut();
            match armed.take() {
                Some((armed_point, hook)) if armed_point == point => Some(hook),
                other => {
                    *armed = other;
                    None
                }
            }
        });
        if let Some(hook) = hook {
            hook();
        }
    }
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
/// regular file with no other name that someone else deliberately gives
/// such a name is deleted as a stray staged file. Any other entry under such
/// a name (a directory, a link, a file with another name) fails the sweep
/// closed and stays as it is.
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
        AtFlags, CWD, Dir, FileType, FlockOperation, Mode, OFlags, RawMode, fchmod, flock, fstat,
        fsync, mkdirat, openat, renameat, statat, unlinkat,
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

    /// An exclusive `flock` on a held directory, released when dropped. It
    /// holds a duplicate of the held descriptor, which shares its lock.
    pub(in crate::credentials::adapters) struct DirectoryLock {
        fd: OwnedFd,
    }

    impl Drop for DirectoryLock {
        fn drop(&mut self) {
            let _ = flock(&self.fd, FlockOperation::Unlock);
        }
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
    /// inherited allow entry cannot widen access beyond the mode bits. A
    /// regular file with another name is refused before its mode changes,
    /// since the change would reach it under that name too.
    fn harden(
        fd: &OwnedFd,
        expected: FileType,
        bits: RawMode,
    ) -> Result<(), CredentialStoreFailure> {
        let owner = rustix::process::geteuid().as_raw();
        let stat = fstat(fd).map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        if FileType::from_raw_mode(stat.st_mode) != expected
            || stat.st_uid != owner
            || (expected == FileType::RegularFile && stat.st_nlink != 1)
        {
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

    /// Syncs `parent` once its entry `name` is verified to be `child`, so
    /// the barrier covers the entry the caller holds and not whatever an
    /// ancestor swap put at the path. A link there is not `child`.
    fn sync_entry(
        parent: &OwnedFd,
        name: &OsStr,
        child: &OwnedFd,
    ) -> Result<(), CredentialStoreFailure> {
        let held = fstat(child).map_err(|_| CredentialStoreFailure::Unavailable)?;
        let named = statat(parent, name, AtFlags::SYMLINK_NOFOLLOW)
            .map_err(|_| CredentialStoreFailure::Unavailable)?;
        if named.st_dev != held.st_dev || named.st_ino != held.st_ino {
            return Err(CredentialStoreFailure::Unavailable);
        }
        sync_verified_parent(parent)
    }

    fn sync_verified_parent(parent: &OwnedFd) -> Result<(), CredentialStoreFailure> {
        #[cfg(test)]
        super::faults::count_parent_sync();
        #[cfg(test)]
        if super::faults::fires(super::faults::Fault::ParentSync) {
            return Err(CredentialStoreFailure::Unavailable);
        }
        fsync(parent).map_err(|_| CredentialStoreFailure::Unavailable)
    }

    /// Syncs the directory that holds the entry of the directory `child`,
    /// reached through `child` itself (`..`) and never by a path, so a link
    /// or a swapped ancestor on the way to `child` cannot choose it: the
    /// kernel resolves `..` to the directory whose entry names `child`.
    /// The one exception is a `..` that is `child` itself or on another
    /// device, a filesystem or mount root, which no entry this module
    /// created names; that is verified on the two handles, and nothing is
    /// synced.
    fn sync_containing_directory(child: &OwnedFd) -> Result<(), CredentialStoreFailure> {
        let parent = openat(child, "..", DIRECTORY_FLAGS, Mode::empty()).map_err(open_failure)?;
        let held = fstat(child).map_err(|_| CredentialStoreFailure::Unavailable)?;
        let up = fstat(&parent).map_err(|_| CredentialStoreFailure::Unavailable)?;
        if up.st_dev != held.st_dev || up.st_ino == held.st_ino {
            return Ok(());
        }
        sync_verified_parent(&parent)
    }

    /// Creates the directory `name` inside `parent` unless it exists, opens
    /// it relative to `parent` without following a link, and syncs `parent`
    /// once its entry is verified to be the opened directory. It syncs an
    /// entry another opener created too: that one may not be synced yet, or
    /// its sync may have failed. The entry is durable before anything is
    /// placed in it.
    fn create_child_directory(
        parent: &OwnedFd,
        name: &OsStr,
        bits: RawMode,
    ) -> Result<OwnedFd, CredentialStoreFailure> {
        match mkdirat(parent, name, mode(bits)) {
            Ok(()) | Err(Errno::EXIST) => {}
            Err(errno) => return Err(open_failure(errno)),
        }
        let child = openat(
            parent,
            name,
            DIRECTORY_FLAGS | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(open_failure)?;
        sync_entry(parent, name, &child)?;
        Ok(child)
    }

    /// Opens the directory `path`, first creating it and every missing
    /// ancestor (see [`create_child_directory`]). The deepest directory found
    /// present has its entry synced in the directory that holds it, reached
    /// through that directory's own handle (see
    /// [`sync_containing_directory`]): an earlier open may have created the
    /// directory and failed to sync it.
    fn open_or_create_ancestor(path: &Path) -> Result<OwnedFd, CredentialStoreFailure> {
        match openat(CWD, path, DIRECTORY_FLAGS, Mode::empty()) {
            Ok(fd) => {
                sync_containing_directory(&fd)?;
                return Ok(fd);
            }
            Err(Errno::NOENT) => {}
            Err(errno) => return Err(open_failure(errno)),
        }
        let (parent, name) = split_parent(path)?;
        let parent = open_or_create_ancestor(parent)?;
        create_child_directory(&parent, name, ANCESTOR_MODE)
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

        /// Opens the held root `path`, creating it and its missing ancestors
        /// first. Whether this open created the root or found it, its entry
        /// is synced in the parent that holds it, once verified to be the
        /// held root, before it is returned, so no opener relies on a sync
        /// another open may never have completed. An existing root's parent
        /// is reached through the root itself (`..`), not through `path`.
        pub(in crate::credentials::adapters) fn open_or_create(
            path: &Path,
        ) -> Result<Self, CredentialStoreFailure> {
            if let Some(held) = Self::open_existing(path)? {
                if let Ok((_, name)) = split_parent(path) {
                    let parent = openat(&held.fd, "..", DIRECTORY_FLAGS, Mode::empty())
                        .map_err(open_failure)?;
                    sync_entry(&parent, name, &held.fd)?;
                }
                return Ok(held);
            }
            let (parent, name) = split_parent(path)?;
            let parent = open_or_create_ancestor(parent)?;
            // The umask can only remove bits, so the directory is never
            // broader than owner-only before it is hardened on its handle.
            let fd = create_child_directory(&parent, name, DIRECTORY_MODE)?;
            harden(&fd, FileType::Directory, DIRECTORY_MODE)?;
            Ok(Self { fd })
        }

        /// Locks the held directory itself, exclusively and without waiting
        /// (`flock` on its descriptor), so every process that reaches this
        /// directory, by whatever path, contends on the same lock. Moving or
        /// renaming the directory moves the lock with it.
        pub(in crate::credentials::adapters) fn lock(
            &self,
        ) -> Result<DirectoryLock, CredentialStoreFailure> {
            let fd = self
                .fd
                .try_clone()
                .map_err(|_| CredentialStoreFailure::Unavailable)?;
            match flock(&fd, FlockOperation::NonBlockingLockExclusive) {
                Ok(()) => Ok(DirectoryLock { fd }),
                Err(Errno::WOULDBLOCK) => Err(CredentialStoreFailure::OperationInProgress),
                Err(_) => Err(CredentialStoreFailure::Unavailable),
            }
        }

        /// Whether `path` still resolves to the held directory. A lock or a
        /// journal reached through a held directory that is no longer at its
        /// path is not the one another process opening that path now uses.
        pub(in crate::credentials::adapters) fn is_at(
            &self,
            path: &Path,
        ) -> Result<bool, CredentialStoreFailure> {
            let held = fstat(&self.fd).map_err(|_| CredentialStoreFailure::Unavailable)?;
            match statat(CWD, path, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(named) => Ok(named.st_dev == held.st_dev && named.st_ino == held.st_ino),
                Err(Errno::NOENT | Errno::NOTDIR) => Ok(false),
                Err(_) => Err(CredentialStoreFailure::Unavailable),
            }
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
        /// Eligibility is by name (see [`is_staged_name`]), and only a regular
        /// file with no other name is unlinked. The entry is checked and then
        /// unlinked by its name, which Unix cannot do through a handle; the
        /// held root is owner-only, so only this user can change what that
        /// name designates in between. Anything that does not qualify, or
        /// cannot be removed, fails closed as `Unavailable` and stays.
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
                match self.single_file_at(&stray)? {
                    None => continue,
                    Some(false) => return Err(CredentialStoreFailure::Unavailable),
                    Some(true) => {}
                }
                unlinkat(&self.fd, stray.as_str(), AtFlags::empty())
                    .map_err(|_| CredentialStoreFailure::Unavailable)?;
            }
            Ok(())
        }

        /// Whether the entry `name` of the held directory, not followed if
        /// it is a link, is a regular file with no other name; `None` if there
        /// is no such entry.
        fn single_file_at(&self, name: &str) -> Result<Option<bool>, CredentialStoreFailure> {
            match statat(&self.fd, name, AtFlags::SYMLINK_NOFOLLOW) {
                Ok(stat) => Ok(Some(
                    FileType::from_raw_mode(stat.st_mode) == FileType::RegularFile
                        && stat.st_nlink == 1,
                )),
                Err(Errno::NOENT) => Ok(None),
                Err(_) => Err(CredentialStoreFailure::Unavailable),
            }
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
            #[cfg(test)]
            if super::faults::fires(super::faults::Fault::StagedSync) {
                return Err(std::io::Error::other("injected staged sync failure"));
            }
            self.file.sync_all()
        }

        /// Renames the flushed bytes over `name` relative to the held
        /// directory, then flushes that directory so the rename itself is
        /// durable. The rename replaces the entry `name` only, never writing
        /// the object it named; it goes ahead only if that entry is absent or
        /// a regular file with no other name, checked just before, and a
        /// directory, a link or a file with another name there refuses the
        /// publication before any effect. The held root is owner-only, so
        /// only this user can change that entry between the check and the
        /// rename. POSIX `rename` leaves the target unaffected on every
        /// failure except `EIO`, so only that error and a failed directory
        /// sync leave the outcome open. After `EIO` the staged name is
        /// unlinked if it still exists; a rename that took effect left none.
        pub(in crate::credentials::adapters) fn publish(mut self) -> Result<(), PublishFailure> {
            let directory = self.directory.fd.as_fd();
            #[cfg(test)]
            if super::faults::fires(super::faults::Fault::RenameUnknownBeforeEffect) {
                return Err(PublishFailure::Unknown);
            }
            if !matches!(
                self.directory.single_file_at(&self.name),
                Ok(None | Some(true))
            ) {
                return Err(PublishFailure::NotPublished);
            }
            match renameat(directory, self.temp.as_str(), directory, self.name.as_str()) {
                Ok(()) => {}
                Err(Errno::IO) => return Err(PublishFailure::Unknown),
                Err(_) => return Err(PublishFailure::NotPublished),
            }
            #[cfg(test)]
            if super::faults::fires(super::faults::Fault::RenameUnknownAfterEffect) {
                return Err(PublishFailure::Unknown);
            }
            self.published = true;
            #[cfg(test)]
            if super::faults::fires(super::faults::Fault::HeldDirectorySync) {
                return Err(PublishFailure::Unknown);
            }
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
    /// Unix, so a refusal for a directory or child another user owns stands,
    /// and an ordinary open already re-applies the owner-only modes to what
    /// this user owns. The repair therefore opens and changes nothing here,
    /// so it cannot reach an object a moved path now names, nor a hard-linked
    /// file under its other name.
    pub(in crate::credentials::adapters) fn take_ownership_if_refused(
        _path: &Path,
        _children: &[&str],
    ) -> Result<bool, CredentialStoreFailure> {
        Ok(false)
    }
}

#[cfg(windows)]
mod windows_held {
    use super::{PublishFailure, is_staged_name, read_at_most, split_parent, staged_name};
    use crate::credentials::domain::CredentialStoreFailure;
    use ::windows::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use ::windows::Wdk::Storage::FileSystem::{
        FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
        FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION, FILE_RENAME_POSIX_SEMANTICS,
        FILE_RENAME_REPLACE_IF_EXISTS, FILE_SYNCHRONOUS_IO_NONALERT, FileRenameInformationEx,
        NTCREATEFILE_CREATE_DISPOSITION, NTCREATEFILE_CREATE_OPTIONS, NtCreateFile,
        NtSetInformationFile,
    };
    use ::windows::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, HANDLE, NO_ERROR, NTSTATUS, OBJ_CASE_INSENSITIVE,
        STATUS_ACCESS_DENIED, STATUS_FILE_IS_A_DIRECTORY, STATUS_INVALID_INFO_CLASS,
        STATUS_INVALID_PARAMETER, STATUS_NOT_SUPPORTED, STATUS_OBJECT_NAME_COLLISION,
        STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND, STATUS_SHARING_VIOLATION,
        UNICODE_STRING,
    };
    use ::windows::Win32::Security::Authorization::{SE_FILE_OBJECT, SetSecurityInfo};
    use ::windows::Win32::Security::{
        GetTokenInformation, OWNER_SECURITY_INFORMATION, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use ::windows::Win32::Storage::FileSystem::{
        DELETE, FILE_ACCESS_RIGHTS, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DISPOSITION_INFO,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_FULL_DIR_INFO,
        FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_ID_INFO, FILE_LIST_DIRECTORY,
        FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_MODE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, FILE_STANDARD_INFO, FILE_TRAVERSE, FileDispositionInfo,
        FileFullDirectoryInfo, FileFullDirectoryRestartInfo, FileIdInfo, FileStandardInfo,
        FlushFileBuffers, GetFileInformationByHandleEx, GetFinalPathNameByHandleW, READ_CONTROL,
        SYNCHRONIZE, SetFileInformationByHandle, VOLUME_NAME_GUID, WRITE_DAC, WRITE_OWNER,
    };
    use ::windows::Win32::System::IO::IO_STATUS_BLOCK;
    use ::windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};
    use ::windows::Win32::UI::Shell::GetUserProfileDirectoryW;
    use ::windows::core::PWSTR;
    use std::ffi::{OsStr, OsString};
    use std::fs::{File, OpenOptions};
    use std::io::{ErrorKind, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, PoisonError};

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
        /// The path this directory was opened by, which keys the ownership
        /// refusals recorded for it and its children.
        path: PathBuf,
    }

    /// The file inside a held directory that stands for a lock on the
    /// directory itself (see [`HeldDirectory::lock`]).
    pub(in crate::credentials::adapters) const DIRECTORY_LOCK_FILE_NAME: &str = "directory.lock";

    /// An exclusive lock on a held directory's lock file, released when
    /// dropped.
    pub(in crate::credentials::adapters) struct DirectoryLock {
        file: File,
    }

    impl Drop for DirectoryLock {
        fn drop(&mut self) {
            let _ = self.file.unlock();
        }
    }

    /// The identity of an open object: its volume serial number and 128-bit
    /// file ID.
    type ObjectIdentity = (u64, [u8; 16]);

    /// The names in the directory `directory` is a handle to, listed through
    /// that handle (`FileFullDirectoryInfo`), so no path is resolved: the
    /// listing is of the held object even if its path now leads elsewhere.
    /// `.` and `..` are left out.
    fn list_entries(directory: &File) -> Result<Vec<OsString>, CredentialStoreFailure> {
        // `u64` storage aligns every `FILE_FULL_DIR_INFO` the kernel packs.
        let mut buffer = vec![0_u64; 8 * 1024];
        let buffer_bytes = buffer.len() * size_of::<u64>();
        let mut names = Vec::new();
        let mut class = FileFullDirectoryRestartInfo;
        loop {
            // SAFETY: `buffer` is writable for `buffer_bytes` bytes and the
            // handle stays open for the call.
            let read = unsafe {
                GetFileInformationByHandleEx(
                    HANDLE(directory.as_raw_handle()),
                    class,
                    buffer.as_mut_ptr().cast(),
                    u32::try_from(buffer_bytes).map_err(|_| CredentialStoreFailure::Internal)?,
                )
            };
            match read {
                Ok(()) => {}
                Err(error) if error.code() == ERROR_NO_MORE_FILES.to_hresult() => break,
                Err(_) => return Err(CredentialStoreFailure::Unavailable),
            }
            class = FileFullDirectoryInfo;
            let base = buffer.as_ptr().cast::<u8>();
            let mut offset = 0_usize;
            loop {
                let header = std::mem::offset_of!(FILE_FULL_DIR_INFO, FileName);
                if offset + header > buffer_bytes {
                    return Err(CredentialStoreFailure::Unavailable);
                }
                // SAFETY: the kernel wrote a chain of `FILE_FULL_DIR_INFO`
                // records, each 8-byte aligned, starting at `offset`; the
                // header lies inside the buffer (checked above).
                let entry = unsafe { &*base.add(offset).cast::<FILE_FULL_DIR_INFO>() };
                let units = entry.FileNameLength as usize / size_of::<u16>();
                if offset + header + units * size_of::<u16>() > buffer_bytes {
                    return Err(CredentialStoreFailure::Unavailable);
                }
                // SAFETY: the name's `units` UTF-16 units follow the header
                // inside the buffer (checked above).
                let name = unsafe {
                    std::slice::from_raw_parts(base.add(offset + header).cast::<u16>(), units)
                };
                let name = OsString::from_wide(name);
                if name != "." && name != ".." {
                    names.push(name);
                }
                if entry.NextEntryOffset == 0 {
                    break;
                }
                offset += entry.NextEntryOffset as usize;
            }
        }
        Ok(names)
    }

    /// An object hardening refused because another principal owns it: the
    /// directory path that reached it, its name in that directory for a
    /// child, and its identity at the refusal.
    struct RefusedOwner {
        directory: PathBuf,
        child: Option<String>,
        identity: ObjectIdentity,
    }

    /// The ownership refusals this process has seen. An explicit ownership
    /// repair changes only these objects, and only while the same path still
    /// reaches the same object. Identities never leave this module.
    static REFUSED_OWNERS: Mutex<Vec<RefusedOwner>> = Mutex::new(Vec::new());

    fn record_refused_owner(directory: &Path, child: Option<&str>, object: &File) {
        let Some(identity) = file_identity(object) else {
            return;
        };
        let mut refused = REFUSED_OWNERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        refused.retain(|entry| entry.directory != directory || entry.child.as_deref() != child);
        refused.push(RefusedOwner {
            directory: directory.to_owned(),
            child: child.map(str::to_owned),
            identity,
        });
    }

    fn refused_owner(directory: &Path, child: Option<&str>) -> Option<ObjectIdentity> {
        REFUSED_OWNERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .find(|entry| entry.directory == directory && entry.child.as_deref() == child)
            .map(|entry| entry.identity)
    }

    fn forget_refused_owners(directory: &Path) {
        REFUSED_OWNERS
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|entry| entry.directory != directory);
    }

    /// Records the object at `directory` (or its `child`) as refused for an
    /// untrusted owner, as hardening would, without needing a foreign owner.
    #[cfg(test)]
    pub(in crate::credentials::adapters) fn record_refused_owner_for_test(
        directory: &Path,
        child: Option<&str>,
    ) {
        let path = child.map_or_else(|| directory.to_owned(), |child| directory.join(child));
        let object = OpenOptions::new()
            .access_mode(FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0)
            .share_mode(CHILD_SHARE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
            .expect("open the object to record as refused");
        record_refused_owner(directory, child, &object);
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
    /// `WRITE_DAC` and hardened through that handle. The DACL is set on the
    /// directory alone: existing children keep their own ACEs, so nothing
    /// this module did not open and qualify itself inherits from it. Each
    /// child is hardened on its own handle when it is opened.
    fn harden_directory(directory: &File) -> Result<(), CredentialStoreFailure> {
        crate::fs_util::try_set_owner_only_directory_handle(directory)
            .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)
    }

    /// Refuses an object whose owner could undo its hardening. Any owner keeps
    /// implicit `WRITE_DAC`, so a protected owner-only DACL means nothing on an
    /// object someone else owns. See `fs_util::owner_is_trusted` for which
    /// owners are accepted. A refusal for an untrusted owner is recorded
    /// under `directory` and `child`, the one object an explicit ownership
    /// repair of that path may then change.
    fn require_trusted_owner(
        directory: &Path,
        child: Option<&str>,
        file: &File,
    ) -> Result<(), CredentialStoreFailure> {
        match crate::fs_util::owner_is_trusted(file) {
            Ok(true) => Ok(()),
            Ok(false) => {
                record_refused_owner(directory, child, file);
                Err(CredentialStoreFailure::PermissionHardeningFailed)
            }
            Err(_) => Err(CredentialStoreFailure::PermissionHardeningFailed),
        }
    }

    /// Refuses a file with another name: hardening it, or taking ownership
    /// of it, through this name would change it under the other name too.
    fn require_single_name(file: &File) -> Result<(), CredentialStoreFailure> {
        let mut info = FILE_STANDARD_INFO::default();
        // SAFETY: `FileStandardInfo` writes exactly one `FILE_STANDARD_INFO`
        // into `info`, which outlives the call; the handle stays open.
        unsafe {
            GetFileInformationByHandleEx(
                HANDLE(file.as_raw_handle()),
                FileStandardInfo,
                std::ptr::addr_of_mut!(info).cast(),
                size_of::<FILE_STANDARD_INFO>() as u32,
            )
        }
        .map_err(|_| CredentialStoreFailure::PermissionHardeningFailed)?;
        if info.NumberOfLinks == 1 {
            Ok(())
        } else {
            Err(CredentialStoreFailure::PermissionHardeningFailed)
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
        #[cfg(test)]
        if super::faults::fires(super::faults::Fault::RenameUnknownBeforeEffect) {
            return Err(PublishFailure::Unknown);
        }
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
            #[cfg(test)]
            if super::faults::fires(super::faults::Fault::RenameUnknownAfterEffect) {
                return Err(PublishFailure::Unknown);
            }
            Ok(())
        } else if status == STATUS_SHARING_VIOLATION {
            Err(PublishFailure::Contended)
        } else if rename_refused_before_effect(status) {
            Err(PublishFailure::NotPublished)
        } else {
            Err(PublishFailure::Unknown)
        }
    }

    /// Statuses other than a sharing violation with which the kernel refuses
    /// a rename request (an access conflict, a name collision, or a request
    /// it does not support) before it changes any name, so the target still
    /// holds what it held. Every other failure leaves the outcome open.
    fn rename_refused_before_effect(status: NTSTATUS) -> bool {
        matches!(
            status,
            STATUS_ACCESS_DENIED
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

    /// What a handle to an ancestor of a held root needs: to list it, create
    /// a directory in it, flush it, and read its identity.
    const ANCESTOR_ACCESS: u32 = FILE_LIST_DIRECTORY.0
        | FILE_ADD_SUBDIRECTORY.0
        | FILE_TRAVERSE.0
        | FILE_READ_ATTRIBUTES.0
        | SYNCHRONIZE.0;

    /// Opens the existing directory `path` as an ancestor, following a final
    /// junction as `create_dir_all` would.
    fn open_ancestor(path: &Path) -> std::io::Result<File> {
        OpenOptions::new()
            .access_mode(ANCESTOR_ACCESS)
            .share_mode(CHILD_SHARE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0)
            .open(path)
    }

    /// Flushes `parent` once its entry `name` is verified to be `child`, so
    /// the flush covers the entry the caller holds and not whatever an
    /// ancestor swap put at the path.
    fn flush_entry(
        parent: &File,
        name: &OsStr,
        child: &File,
    ) -> Result<(), CredentialStoreFailure> {
        let entry =
            open_relative_entry(parent, name).map_err(|_| CredentialStoreFailure::Unavailable)?;
        match (file_identity(&entry), file_identity(child)) {
            (Some(named), Some(held)) if named == held => {}
            _ => return Err(CredentialStoreFailure::Unavailable),
        }
        #[cfg(test)]
        if super::faults::fires(super::faults::Fault::ParentSync) {
            return Err(CredentialStoreFailure::Unavailable);
        }
        flush_directory(parent).map_err(|_| CredentialStoreFailure::Unavailable)
    }

    /// Flushes the parent of the directory `child` is a handle to. The
    /// parent and the entry's name come from the handle's own final path,
    /// with every junction resolved, not from a path the caller gave, and
    /// the entry is verified to be `child` before the flush. An ancestor of
    /// an open directory cannot be renamed, so that parent is the one that
    /// holds `child`. A volume root has no containing entry. A parent that
    /// cannot be opened, or whose entry is not `child`, fails the same way a
    /// failed flush does.
    fn flush_parent_of(child: &File) -> Result<(), CredentialStoreFailure> {
        let held = final_path(child).ok_or(CredentialStoreFailure::Unavailable)?;
        let (Some(parent), Some(name)) = (held.parent(), held.file_name()) else {
            return Ok(());
        };
        let parent = open_ancestor(parent).map_err(|_| CredentialStoreFailure::Unavailable)?;
        flush_entry(&parent, name, child)
    }

    /// Opens the directory `path`, first creating it and every missing
    /// ancestor with inherited security, as `create_dir_all` would. Each
    /// directory is created and opened relative to its open parent, as
    /// itself, so a reparse point someone put at that name in between is
    /// refused rather than followed, and the parent is flushed once its
    /// entry is verified to be that directory, an entry a concurrent opener
    /// created included, before anything is placed in it. The deepest
    /// directory found present has its entry flushed in the parent that
    /// holds it too (see [`flush_parent_of`]): an earlier open may have
    /// created the directory and failed to flush.
    fn open_or_create_ancestor(path: &Path) -> Result<File, CredentialStoreFailure> {
        match open_ancestor(path) {
            Ok(directory) => {
                flush_parent_of(&directory)?;
                return Ok(directory);
            }
            Err(error) if error.kind() == ErrorKind::NotFound => {}
            Err(_) => return Err(CredentialStoreFailure::Unavailable),
        }
        let (parent, name) = split_parent(path)?;
        let parent = open_or_create_ancestor(parent)?;
        let directory = nt_create_relative(
            &parent,
            name,
            ANCESTOR_ACCESS,
            CHILD_SHARE,
            FILE_OPEN_IF,
            FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            std::ptr::null(),
        )
        .map_err(|_| CredentialStoreFailure::Unavailable)?;
        require_kind(&directory, true).map_err(|_| CredentialStoreFailure::Unavailable)?;
        flush_entry(&parent, name, &directory)?;
        Ok(directory)
    }

    /// The volume serial number and 128-bit file ID of an open file.
    fn file_identity(file: &File) -> Option<ObjectIdentity> {
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

    /// Opens the entry `name` of `directory` for an ownership repair and
    /// qualifies it on that handle before anything changes: a regular file,
    /// neither a reparse point nor a directory, with no other name, and,
    /// where its refusal was recorded, the very object refused. An entry
    /// the repair will change (`access` not empty) is held with read access
    /// and no delete sharing, so nobody can rename or delete it, or give its
    /// name to another one, until it is changed; a handle with only
    /// attribute or security access takes no part in sharing, so its share
    /// mode would bind nobody. An entry only qualified, a staged file, asks
    /// for no read access the repair does not need.
    fn qualify_repair_entry(
        directory: &File,
        name: &str,
        access: u32,
        refused: Option<ObjectIdentity>,
    ) -> Result<Option<File>, CredentialStoreFailure> {
        let pinning = if access == 0 { 0 } else { FILE_READ_DATA.0 };
        let file = match open_relative(
            directory,
            name,
            access | pinning,
            ChildOpen::OpenExisting,
            NO_DELETE_SHARE,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(open_failure(error.kind())),
        };
        require_kind(&file, false)?;
        require_single_name(&file)?;
        if refused.is_some_and(|refused| file_identity(&file) != Some(refused)) {
            return Err(CredentialStoreFailure::PermissionHardeningFailed);
        }
        Ok(Some(file))
    }

    /// Explicit repair of a root that hardening refused because another
    /// principal owns it, or owns one of its `children`. It changes only an
    /// object whose refusal this process recorded for this path, and only
    /// while the path still reaches that same object; anything else it
    /// reaches is refused with `PermissionHardeningFailed`, and so is every
    /// change, because every object is qualified on its own handle before
    /// the first one changes. When the directory itself was refused, every
    /// entry it holds, listed through its handle, must be one of `children`
    /// or a staged file of one, and must be a regular file with no other
    /// name; the directory is then made the process user's and hardened on
    /// its own (no existing child inherits from it), and so is each child.
    /// Otherwise only the recorded children change. Assigning oneself as
    /// owner needs `WRITE_OWNER` on the object, which a DACL granting the
    /// user full control carries; without it the refusal stands. Returns
    /// whether any owner changed. With no recorded refusal for `path` it
    /// opens nothing.
    pub(in crate::credentials::adapters) fn take_ownership_if_refused(
        path: &Path,
        children: &[&str],
    ) -> Result<bool, CredentialStoreFailure> {
        let refused_directory = refused_owner(path, None);
        if refused_directory.is_none()
            && children
                .iter()
                .all(|child| refused_owner(path, Some(child)).is_none())
        {
            return Ok(false);
        }
        let directory = match open_directory_with(path, WRITE_OWNER.0) {
            Ok(directory) => directory,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(open_failure(error.kind())),
        };
        require_kind(&directory, true)?;
        let mut changing = Vec::new();
        if let Some(refused) = refused_directory {
            if file_identity(&directory) != Some(refused) {
                return Err(CredentialStoreFailure::PermissionHardeningFailed);
            }
            for name in list_entries(&directory)? {
                let name = name
                    .to_str()
                    .ok_or(CredentialStoreFailure::PermissionHardeningFailed)?;
                let child = children.contains(&name);
                if !child && !children.iter().any(|child| is_staged_name(name, child)) {
                    return Err(CredentialStoreFailure::PermissionHardeningFailed);
                }
                let access = if child {
                    WRITE_OWNER.0 | WRITE_DAC.0
                } else {
                    0
                };
                let qualified = qualify_repair_entry(
                    &directory,
                    name,
                    access,
                    refused_owner(path, Some(name)),
                )?;
                if child && let Some(file) = qualified {
                    changing.push(file);
                }
            }
        } else {
            for child in children {
                let Some(refused) = refused_owner(path, Some(child)) else {
                    continue;
                };
                if let Some(file) = qualify_repair_entry(
                    &directory,
                    child,
                    WRITE_OWNER.0 | WRITE_DAC.0,
                    Some(refused),
                )? {
                    changing.push(file);
                }
            }
        }
        #[cfg(test)]
        super::faults::reached(super::faults::Point::RepairQualified);
        let mut changed = false;
        if refused_directory.is_some() {
            changed |= take_ownership(&directory)?;
            harden_directory(&directory)?;
        }
        for file in &changing {
            changed |= take_ownership(file)?;
            harden(file)?;
        }
        forget_refused_owners(path);
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

    /// `NtCreateFile` of `name` inside the directory `root` is a handle to,
    /// resolved by the kernel relative to that handle alone (the Windows
    /// counterpart of `openat`), so no ancestor path is consulted. An object
    /// this creates gets `security_descriptor`, or inherits its security
    /// from `root` when that is null.
    fn nt_create_relative(
        root: &File,
        name: &OsStr,
        access: u32,
        share: FILE_SHARE_MODE,
        disposition: NTCREATEFILE_CREATE_DISPOSITION,
        options: NTCREATEFILE_CREATE_OPTIONS,
        security_descriptor: *const std::ffi::c_void,
    ) -> std::io::Result<File> {
        let units: Vec<u16> = name.encode_wide().collect();
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
            SecurityDescriptor: security_descriptor.cast(),
            SecurityQualityOfService: std::ptr::null(),
        };
        let mut handle = HANDLE::default();
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: every pointer refers to a local, or to a descriptor the
        // caller keeps alive, that outlives the call, and `root` stays open
        // throughout. The kernel only reads `units` through `object_name`.
        let status = unsafe {
            NtCreateFile(
                &mut handle,
                FILE_ACCESS_RIGHTS(access),
                &attributes,
                &mut status_block,
                None,
                FILE_ATTRIBUTE_NORMAL,
                share,
                disposition,
                options,
                None,
                0,
            )
        };
        if status.is_ok() {
            // SAFETY: on success `handle` is a new handle owned by nothing else.
            return Ok(unsafe { File::from_raw_handle(handle.0) });
        }
        Err(std::io::Error::from(match status {
            STATUS_OBJECT_NAME_NOT_FOUND | STATUS_OBJECT_PATH_NOT_FOUND => ErrorKind::NotFound,
            STATUS_OBJECT_NAME_COLLISION => ErrorKind::AlreadyExists,
            STATUS_ACCESS_DENIED | STATUS_FILE_IS_A_DIRECTORY => ErrorKind::PermissionDenied,
            STATUS_SHARING_VIOLATION => ErrorKind::ResourceBusy,
            _ => ErrorKind::Other,
        }))
    }

    /// Opens the file `name` inside the directory `root` is a handle to (see
    /// [`nt_create_relative`]). A final reparse point is opened as itself,
    /// and a directory is refused.
    fn open_relative(
        root: &File,
        name: &str,
        access: u32,
        open: ChildOpen,
        share: FILE_SHARE_MODE,
    ) -> std::io::Result<File> {
        let disposition = match open {
            ChildOpen::OpenExisting => FILE_OPEN,
            ChildOpen::OpenOrCreate => FILE_OPEN_IF,
            ChildOpen::CreateNew => FILE_CREATE,
        };
        nt_create_relative(
            root,
            OsStr::new(name),
            access | FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0 | SYNCHRONIZE.0,
            share,
            disposition,
            FILE_NON_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            std::ptr::null(),
        )
    }

    /// Creates the root `name` inside the directory `parent` is a handle
    /// to, or opens the one a concurrent opener created there, relative to
    /// `parent` alone, so nothing a path names by then can be what is
    /// qualified and hardened. A root this creates carries the protected
    /// owner-only DACL from the moment it exists. A final reparse point is
    /// opened as itself, which [`HeldDirectory::hold`] then refuses.
    fn create_root_relative(parent: &File, name: &OsStr) -> std::io::Result<File> {
        crate::fs_util::with_owner_only_directory_descriptor(|descriptor| {
            nt_create_relative(
                parent,
                name,
                HELD_DIRECTORY_ACCESS,
                NO_DELETE_SHARE,
                FILE_OPEN_IF,
                FILE_DIRECTORY_FILE | FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
                descriptor,
            )
        })
    }

    /// Whether a publication may replace the entry `name` of `root`: it is
    /// absent, or, opened as itself, a regular file with no other name. The
    /// rename replaces that entry only and never writes the object it
    /// names; this refuses, before any effect, a directory (which the
    /// kernel refuses to replace too), a reparse point, and a file with
    /// another name, whose other name would keep the old journal. The held
    /// root is owner-only, so only this user can change the entry between
    /// this check and the rename.
    fn target_may_be_replaced(root: &File, name: &str) -> bool {
        match open_relative_entry(root, OsStr::new(name)) {
            Ok(target) => {
                require_kind(&target, false).is_ok() && require_single_name(&target).is_ok()
            }
            Err(error) => error.kind() == ErrorKind::NotFound,
        }
    }

    /// Opens the entry `name` inside the directory `root` is a handle to as
    /// itself, whatever its kind, a reparse point included, only to read its
    /// attributes and identity.
    fn open_relative_entry(root: &File, name: &OsStr) -> std::io::Result<File> {
        nt_create_relative(
            root,
            name,
            FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0,
            CHILD_SHARE,
            FILE_OPEN,
            FILE_SYNCHRONOUS_IO_NONALERT | FILE_OPEN_REPARSE_POINT,
            std::ptr::null(),
        )
    }

    const CHILD_SHARE: FILE_SHARE_MODE =
        FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);

    /// What an object this module holds in order to act on it shares with
    /// every other opener: not delete, so nobody can rename or delete it, or
    /// give its name to another object, between its qualification and the
    /// act. Held directories, lock files for as long as they are held, the
    /// objects an ownership repair changes, and the strays a sweep deletes
    /// are all opened this way, each with read, write or delete access: a
    /// handle without one takes no part in sharing, whatever it shares.
    const NO_DELETE_SHARE: FILE_SHARE_MODE =
        FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0);

    /// What a held directory handle carries: listing, traversal and
    /// `FILE_ADD_FILE`, which lets the same handle flush the directory after
    /// a publication, the attributes and security it is qualified by, and
    /// `WRITE_DAC` for hardening.
    const HELD_DIRECTORY_ACCESS: u32 = FILE_LIST_DIRECTORY.0
        | FILE_TRAVERSE.0
        | FILE_ADD_FILE.0
        | FILE_READ_ATTRIBUTES.0
        | READ_CONTROL.0
        | WRITE_DAC.0
        | SYNCHRONIZE.0;

    fn open_failure(kind: ErrorKind) -> CredentialStoreFailure {
        match kind {
            // Opening a directory as a file is refused with access denied.
            ErrorKind::PermissionDenied => CredentialStoreFailure::PermissionHardeningFailed,
            // Another program holds the object open without sharing what
            // this open asks for, a read included: contention that ends when
            // it lets go, with nothing changed.
            ErrorKind::ResourceBusy => CredentialStoreFailure::OperationInProgress,
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
            .access_mode(HELD_DIRECTORY_ACCESS | extra_access)
            .share_mode(NO_DELETE_SHARE.0)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
            .open(path)
    }

    /// Creates the root `name` inside the directory `parent_path` names, as
    /// [`HeldDirectory::open_or_create`] does, without qualifying or
    /// hardening it, so a test can inspect what creation alone leaves.
    #[cfg(test)]
    pub(in crate::credentials::adapters) fn create_root_for_test(
        parent_path: &Path,
        name: &str,
    ) -> std::io::Result<()> {
        create_root_relative(&open_ancestor(parent_path)?, OsStr::new(name)).map(drop)
    }

    impl HeldDirectory {
        pub(in crate::credentials::adapters) fn open_existing(
            path: &Path,
        ) -> Result<Option<Self>, CredentialStoreFailure> {
            match open_directory_handle(path) {
                Ok(handle) => Self::hold(path, handle).map(Some),
                Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
                Err(error) => Err(open_failure(error.kind())),
            }
        }

        /// Qualifies the directory `handle`, opened for `path`, and hardens
        /// it on that same handle: a directory, not a reparse point, that a
        /// trusted principal owns.
        fn hold(path: &Path, handle: File) -> Result<Self, CredentialStoreFailure> {
            require_kind(&handle, true)?;
            require_trusted_owner(path, None, &handle)?;
            harden_directory(&handle)?;
            let final_path =
                final_path(&handle).ok_or(CredentialStoreFailure::PermissionHardeningFailed)?;
            Ok(Self {
                handle,
                final_path,
                path: path.to_owned(),
            })
        }

        /// Opens the held root `path`, creating it and its missing ancestors
        /// first. Whether this open created the root or found it, its parent
        /// is flushed, once its entry is verified to be the held root, before
        /// it is returned, so no opener relies on a flush another open may
        /// never have completed. A missing root is created and opened
        /// relative to the parent the walk holds (see
        /// [`create_root_relative`]), and qualified and hardened on that
        /// handle, before anything else touches it.
        pub(in crate::credentials::adapters) fn open_or_create(
            path: &Path,
        ) -> Result<Self, CredentialStoreFailure> {
            if let Some(held) = Self::open_existing(path)? {
                flush_parent_of(&held.handle)?;
                return Ok(held);
            }
            let (parent_path, name) = split_parent(path)?;
            let parent = open_or_create_ancestor(parent_path)?;
            #[cfg(test)]
            super::faults::reached(super::faults::Point::RootParentHeld);
            let handle =
                create_root_relative(&parent, name).map_err(|error| open_failure(error.kind()))?;
            let held = Self::hold(path, handle)?;
            flush_entry(&parent, name, &held.handle)?;
            Ok(held)
        }

        /// Windows cannot lock a directory handle, so this locks the file
        /// [`DIRECTORY_LOCK_FILE_NAME`] inside the held directory,
        /// exclusively and without waiting. The held directory cannot be
        /// renamed while held and the lock file shares no delete, and the
        /// lock counts only while that name still designates the locked file,
        /// so every process that reaches this directory, by whatever path,
        /// contends on the same file.
        pub(in crate::credentials::adapters) fn lock(
            &self,
        ) -> Result<DirectoryLock, CredentialStoreFailure> {
            let file = self.open_lock_file(DIRECTORY_LOCK_FILE_NAME)?;
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => {
                    return Err(CredentialStoreFailure::OperationInProgress);
                }
                Err(std::fs::TryLockError::Error(_)) => {
                    return Err(CredentialStoreFailure::Unavailable);
                }
            }
            let lock = DirectoryLock { file };
            if self.names_file(DIRECTORY_LOCK_FILE_NAME, &lock.file)? {
                Ok(lock)
            } else {
                Err(CredentialStoreFailure::Unavailable)
            }
        }

        /// Whether `path` still resolves to the held directory. A lock or a
        /// journal reached through a held directory that is no longer at its
        /// path is not the one another process opening that path now uses.
        pub(in crate::credentials::adapters) fn is_at(
            &self,
            path: &Path,
        ) -> Result<bool, CredentialStoreFailure> {
            let named = match OpenOptions::new()
                .access_mode(FILE_READ_ATTRIBUTES.0 | SYNCHRONIZE.0)
                .share_mode(CHILD_SHARE.0)
                .custom_flags(FILE_FLAG_BACKUP_SEMANTICS.0 | FILE_FLAG_OPEN_REPARSE_POINT.0)
                .open(path)
            {
                Ok(named) => named,
                Err(error) if error.kind() == ErrorKind::NotFound => return Ok(false),
                Err(_) => return Err(CredentialStoreFailure::Unavailable),
            };
            match (file_identity(&self.handle), file_identity(&named)) {
                (Some(held), Some(named)) => Ok(held == named),
                _ => Err(CredentialStoreFailure::Unavailable),
            }
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
            match self.verify_and_harden_child(name, &file) {
                Ok(()) => Ok(Some(file)),
                Err(failure) => {
                    if open == ChildOpen::CreateNew {
                        let _ = dispose(&file);
                    }
                    Err(failure)
                }
            }
        }

        fn verify_and_harden_child(
            &self,
            name: &str,
            file: &File,
        ) -> Result<(), CredentialStoreFailure> {
            require_kind(file, false)?;
            // Redundant with the handle-relative open; kept so a resolution
            // outside the held directory can never pass silently.
            let child =
                final_path(file).ok_or(CredentialStoreFailure::PermissionHardeningFailed)?;
            if child.parent() != Some(self.final_path.as_path()) {
                return Err(CredentialStoreFailure::PermissionHardeningFailed);
            }
            require_single_name(file)?;
            require_trusted_owner(&self.path, Some(name), file)?;
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
                NO_DELETE_SHARE,
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
        /// is in flight. Names come from listing the held directory through
        /// its handle, never through a path, whose ancestors can be swapped;
        /// a name that no longer resolves in the held directory is skipped.
        /// Eligibility is by name (see [`is_staged_name`]), and a stray is
        /// deleted only once its handle shows a regular file with no other
        /// name, not a reparse point. That handle shares no delete, so the
        /// object qualified is the one deleted. Anything that does not
        /// qualify, or cannot be deleted, fails closed as `Unavailable` and
        /// stays.
        pub(in crate::credentials::adapters) fn sweep_staged(
            &self,
            name: &str,
        ) -> Result<(), CredentialStoreFailure> {
            for candidate in list_entries(&self.handle)? {
                let Some(candidate) = candidate.to_str() else {
                    continue;
                };
                if !is_staged_name(candidate, name) {
                    continue;
                }
                let stray = match open_relative(
                    &self.handle,
                    candidate,
                    DELETE.0,
                    ChildOpen::OpenExisting,
                    NO_DELETE_SHARE,
                ) {
                    Ok(stray) => stray,
                    Err(error) if error.kind() == ErrorKind::NotFound => continue,
                    Err(_) => return Err(CredentialStoreFailure::Unavailable),
                };
                require_kind(&stray, false)
                    .and_then(|()| require_single_name(&stray))
                    .map_err(|_| CredentialStoreFailure::Unavailable)?;
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
            #[cfg(test)]
            if super::faults::fires(super::faults::Fault::StagedSync) {
                return Err(std::io::Error::other("injected staged sync failure"));
            }
            match self.file.as_ref() {
                Some(file) => file.sync_all(),
                None => Err(std::io::Error::from(ErrorKind::BrokenPipe)),
            }
        }

        /// Renames the flushed staged file over `name` in the held root, then
        /// flushes the renamed file and the held root before closing it. The
        /// rename names both objects by handle, so an ancestor that points
        /// elsewhere by now cannot redirect it, and it goes ahead only over
        /// an entry [`target_may_be_replaced`] admits.
        ///
        /// The two flushes are the strongest barrier a standard user has: a
        /// file flush persists its data and metadata, and an NTFS directory
        /// flush persists the directory structure ([MS-FSA] 2.1.5.7). They
        /// are not a namespace durability claim: no public Windows API
        /// promises that a same-volume rename survives power loss
        /// (`docs/research/canonical-directory-durability-2026-08-14.md`;
        /// Microsoft Q&A 6057877, November 2026). Activation stays gated on
        /// the evidence profiles instead (#210). A failed flush leaves the
        /// outcome `Unknown`; the held handle carries `FILE_ADD_FILE`, so a
        /// flush is never refused for want of access.
        pub(in crate::credentials::adapters) fn publish(mut self) -> Result<(), PublishFailure> {
            let file = self.file.as_ref().ok_or(PublishFailure::NotPublished)?;
            if !target_may_be_replaced(&self.directory.handle, &self.name) {
                return Err(PublishFailure::NotPublished);
            }
            match rename_within(file, &self.directory.handle, &self.name) {
                Ok(()) => {}
                Err(PublishFailure::Unknown) => {
                    // The rename may have taken effect, so this handle may now
                    // be the published file: it is never disposed. A staged
                    // file left under its staging name goes in the next sweep.
                    self.published = true;
                    return Err(PublishFailure::Unknown);
                }
                Err(failure) => return Err(failure),
            }
            self.published = true;
            let flushed = flush_publication(file, &self.directory.handle);
            drop(self.file.take());
            flushed.map_err(|_| PublishFailure::Unknown)
        }
    }

    /// Flushes a renamed file, then the held directory it was renamed in.
    fn flush_publication(file: &File, directory: &File) -> std::io::Result<()> {
        #[cfg(test)]
        if super::faults::fires(super::faults::Fault::PublishedFileSync) {
            return Err(std::io::Error::other(
                "injected published file sync failure",
            ));
        }
        file.sync_all()?;
        #[cfg(test)]
        if super::faults::fires(super::faults::Fault::HeldDirectorySync) {
            return Err(std::io::Error::other(
                "injected held directory sync failure",
            ));
        }
        flush_directory(directory)
    }

    impl StagedFile<'_> {
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
