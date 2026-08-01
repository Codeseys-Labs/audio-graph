//! macOS held-target detector for credential-v2 filesystem eligibility.

use super::{
    FILESYSTEM_DETECTOR_SCHEMA_VERSION, FilesystemFamily, FilesystemObservation, Platform,
    PlatformRelease, Ternary,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NativeFilesystemKind {
    Apfs,
    HfsPlus,
    Userspace,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FoundationVolumeKind {
    Apfs,
    HfsPlus,
    Other,
}

// Darwin's public mount flags are stable ABI values. Keep the numeric surface
// private and immediately reduce it to the closed policy vocabulary.
const DARWIN_MNT_RDONLY: u32 = 0x0000_0001;
const DARWIN_MNT_LOCAL: u32 = 0x0000_1000;
const DARWIN_MNT_IGNORE_OWNERSHIP: u32 = 0x0020_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DarwinMountTraits {
    writable: Ternary,
    local: Ternary,
    access_controls_enforced: Ternary,
}

fn nul_terminated_prefix(value: &[u8]) -> &[u8] {
    let end = value
        .iter()
        .position(|byte| *byte == 0)
        .unwrap_or(value.len());
    &value[..end]
}

fn classify_native_filesystem_name(value: &[u8]) -> NativeFilesystemKind {
    match nul_terminated_prefix(value) {
        b"apfs" => NativeFilesystemKind::Apfs,
        b"hfs" => NativeFilesystemKind::HfsPlus,
        b"osxfuse" | b"macfuse" => NativeFilesystemKind::Userspace,
        _ => NativeFilesystemKind::Other,
    }
}

fn classify_foundation_volume_name(value: &[u8]) -> FoundationVolumeKind {
    match value {
        b"apfs" => FoundationVolumeKind::Apfs,
        b"hfs" => FoundationVolumeKind::HfsPlus,
        _ => FoundationVolumeKind::Other,
    }
}

fn classify_darwin_mount_flags(flags: u32) -> DarwinMountTraits {
    DarwinMountTraits {
        writable: if flags & DARWIN_MNT_RDONLY == 0 {
            Ternary::Yes
        } else {
            Ternary::No
        },
        local: if flags & DARWIN_MNT_LOCAL != 0 {
            Ternary::Yes
        } else {
            Ternary::No
        },
        access_controls_enforced: if flags & DARWIN_MNT_IGNORE_OWNERSHIP == 0 {
            Ternary::Yes
        } else {
            Ternary::No
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResourceValue<T> {
    Value(T),
    Missing,
    WrongType,
    QueryFailed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderBoundaryResult {
    Managed,
    /// Reserved for deterministic fixtures until `audio-graph-0e08` proves
    /// the ordinary packaged-app File Provider negative.
    #[cfg(test)]
    ProvedNotManaged,
    UnprovedNotManaged,
    UnexpectedFailure,
    TimedOut,
    LateCompletion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TargetIdentity {
    device: u64,
    inode: u64,
    filesystem: [i32; 2],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FdSnapshot {
    identity: TargetIdentity,
    filesystem: NativeFilesystemKind,
    writable: Ternary,
    local: Ternary,
    access_controls_enforced: Ternary,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FoundationSnapshot {
    volume_local: ResourceValue<bool>,
    volume_internal: ResourceValue<bool>,
    volume_removable: ResourceValue<bool>,
    volume_ejectable: ResourceValue<bool>,
    volume_read_only: ResourceValue<bool>,
    supports_access_permissions: ResourceValue<bool>,
    volume_kind: ResourceValue<FoundationVolumeKind>,
    is_ubiquitous: ResourceValue<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MacOsSnapshot {
    platform_release: PlatformRelease,
    initial_held_fd: FdSnapshot,
    reopened_url_fd: FdSnapshot,
    final_held_fd: FdSnapshot,
    foundation: FoundationSnapshot,
    provider: ProviderBoundaryResult,
}

fn resource_bool(value: ResourceValue<bool>) -> Ternary {
    match value {
        ResourceValue::Value(true) => Ternary::Yes,
        ResourceValue::Value(false) => Ternary::No,
        ResourceValue::Missing | ResourceValue::WrongType | ResourceValue::QueryFailed => {
            Ternary::Unknown
        }
    }
}

fn require_agreement(left: Ternary, right: Ternary) -> Ternary {
    if left == right {
        left
    } else {
        Ternary::Unknown
    }
}

fn classify_family(
    native: NativeFilesystemKind,
    foundation: ResourceValue<FoundationVolumeKind>,
) -> (FilesystemFamily, Ternary) {
    match (native, foundation) {
        (NativeFilesystemKind::Apfs, ResourceValue::Value(FoundationVolumeKind::Apfs)) => {
            (FilesystemFamily::MacApfs, Ternary::Yes)
        }
        (NativeFilesystemKind::HfsPlus, ResourceValue::Value(FoundationVolumeKind::HfsPlus)) => {
            (FilesystemFamily::MacHfsPlus, Ternary::Yes)
        }
        (NativeFilesystemKind::Userspace, ResourceValue::Value(_)) => {
            (FilesystemFamily::Other, Ternary::No)
        }
        _ => (FilesystemFamily::Other, Ternary::Unknown),
    }
}

fn classify_internal_fixed(foundation: FoundationSnapshot) -> Ternary {
    let internal = resource_bool(foundation.volume_internal);
    let removable = resource_bool(foundation.volume_removable);
    let ejectable = resource_bool(foundation.volume_ejectable);
    if [internal, removable, ejectable].contains(&Ternary::Unknown) {
        Ternary::Unknown
    } else if internal == Ternary::Yes && removable == Ternary::No && ejectable == Ternary::No {
        Ternary::Yes
    } else {
        Ternary::No
    }
}

fn classify_cloud(ubiquitous: ResourceValue<bool>, provider: ProviderBoundaryResult) -> Ternary {
    if provider == ProviderBoundaryResult::Managed {
        return Ternary::Yes;
    }
    match resource_bool(ubiquitous) {
        Ternary::Yes => Ternary::Yes,
        Ternary::Unknown => Ternary::Unknown,
        Ternary::No => match provider {
            ProviderBoundaryResult::Managed => unreachable!("handled above"),
            #[cfg(test)]
            ProviderBoundaryResult::ProvedNotManaged => Ternary::No,
            ProviderBoundaryResult::UnprovedNotManaged
            | ProviderBoundaryResult::UnexpectedFailure
            | ProviderBoundaryResult::TimedOut
            | ProviderBoundaryResult::LateCompletion => Ternary::Unknown,
        },
    }
}

fn classify_snapshot(snapshot: MacOsSnapshot) -> FilesystemObservation {
    let identity_stable = if snapshot.initial_held_fd == snapshot.reopened_url_fd
        && snapshot.reopened_url_fd == snapshot.final_held_fd
    {
        Ternary::Yes
    } else {
        Ternary::No
    };
    let (family, kernel_native) = classify_family(
        snapshot.initial_held_fd.filesystem,
        snapshot.foundation.volume_kind,
    );
    let writable = require_agreement(
        snapshot.initial_held_fd.writable,
        match resource_bool(snapshot.foundation.volume_read_only) {
            Ternary::Yes => Ternary::No,
            Ternary::No => Ternary::Yes,
            Ternary::Unknown => Ternary::Unknown,
        },
    );

    FilesystemObservation {
        platform: Platform::MacOs,
        platform_release: snapshot.platform_release,
        family,
        writable,
        local: require_agreement(
            snapshot.initial_held_fd.local,
            resource_bool(snapshot.foundation.volume_local),
        ),
        kernel_native,
        internal_fixed: classify_internal_fixed(snapshot.foundation),
        os_managed_cloud_root: classify_cloud(snapshot.foundation.is_ubiquitous, snapshot.provider),
        access_controls_enforced: require_agreement(
            snapshot.initial_held_fd.access_controls_enforced,
            resource_bool(snapshot.foundation.supports_access_permissions),
        ),
        identity_stable,
        detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
    }
}

#[cfg(target_os = "macos")]
mod native {
    use super::super::{DetectorFault, FilesystemDetector};
    use super::*;
    use objc2_foundation::{
        NSArray, NSNumber, NSString, NSURL, NSURLIsUbiquitousItemKey, NSURLResourceKey,
        NSURLVolumeIsEjectableKey, NSURLVolumeIsInternalKey, NSURLVolumeIsLocalKey,
        NSURLVolumeIsReadOnlyKey, NSURLVolumeIsRemovableKey,
        NSURLVolumeSupportsAccessPermissionsKey, NSURLVolumeTypeNameKey,
    };
    use rustix::fd::OwnedFd;
    use rustix::fs::{CWD, FileType, Mode, OFlags, fstat, fstatfs, getpath, openat};
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::path::Path;

    const DIRECTORY_OPEN_FLAGS: OFlags = OFlags::RDONLY
        .union(OFlags::DIRECTORY)
        .union(OFlags::CLOEXEC)
        .union(OFlags::NOFOLLOW);

    /// The only long-lived native value is the opened directory descriptor.
    /// No path is retained after acquisition.
    pub(crate) struct HeldMacOsTarget {
        directory: OwnedFd,
    }

    impl HeldMacOsTarget {
        pub(crate) fn open(path: &Path) -> Result<Self, DetectorFault> {
            let directory = open_directory(path).map_err(|_| DetectorFault::TargetUnavailable)?;
            Ok(Self { directory })
        }
    }

    #[derive(Debug, Clone, Copy)]
    pub(crate) struct MacOsFilesystemDetector {
        platform_release: PlatformRelease,
    }

    impl MacOsFilesystemDetector {
        pub(crate) const fn new(platform_release: PlatformRelease) -> Self {
            Self { platform_release }
        }
    }

    impl FilesystemDetector for MacOsFilesystemDetector {
        type HeldTarget = HeldMacOsTarget;

        fn inspect(
            &self,
            target: &Self::HeldTarget,
        ) -> Result<FilesystemObservation, DetectorFault> {
            let initial_held_fd = fd_snapshot(&target.directory)?;

            // F_GETPATH is used only to bridge the already-held descriptor to
            // Foundation. The CString, NSURL path, and reopened PathBuf all die
            // in this call and never enter the closed observation.
            let transient_fd_path =
                getpath(&target.directory).map_err(|_| DetectorFault::InspectionUnavailable)?;
            let transient_path = Path::new(OsStr::from_bytes(transient_fd_path.as_bytes()));
            let url = NSURL::from_directory_path(transient_path)
                .ok_or(DetectorFault::InspectionUnavailable)?;

            let foundation = foundation_snapshot(&url);
            let transient_url_path = url
                .to_file_path()
                .ok_or(DetectorFault::InspectionUnavailable)?;
            let reopened = open_directory(&transient_url_path)
                .map_err(|_| DetectorFault::InspectionUnavailable)?;
            let reopened_url_fd = fd_snapshot(&reopened)?;
            let final_held_fd = fd_snapshot(&target.directory)?;

            Ok(classify_snapshot(MacOsSnapshot {
                platform_release: self.platform_release,
                initial_held_fd,
                reopened_url_fd,
                final_held_fd,
                foundation,
                // `audio-graph-0e08` has not yet proved that an ordinary
                // packaged app can establish a universal favorable File
                // Provider negative. Production therefore cannot emit one.
                provider: ProviderBoundaryResult::UnprovedNotManaged,
            }))
        }
    }

    fn open_directory(path: &Path) -> rustix::io::Result<OwnedFd> {
        openat(CWD, path, DIRECTORY_OPEN_FLAGS, Mode::empty())
    }

    fn fd_snapshot(directory: &OwnedFd) -> Result<FdSnapshot, DetectorFault> {
        let metadata = fstat(directory).map_err(|_| DetectorFault::InspectionUnavailable)?;
        if !FileType::from_raw_mode(metadata.st_mode).is_dir() {
            return Err(DetectorFault::InspectionUnavailable);
        }

        let filesystem = fstatfs(directory).map_err(|_| DetectorFault::InspectionUnavailable)?;
        let raw_name = filesystem.f_fstypename.map(|byte| byte as u8);
        let mount = classify_darwin_mount_flags(filesystem.f_flags);

        // rustix 1.1.4 exposes Darwin's libc fsid_t, whose locked definition is
        // exactly two private i32 fields. `transmute` preserves those opaque
        // identity bits and statically rejects any future size change.
        let filesystem_identity =
            unsafe { std::mem::transmute::<rustix::fs::Fsid, [i32; 2]>(filesystem.f_fsid) };

        Ok(FdSnapshot {
            identity: TargetIdentity {
                device: metadata.st_dev as u64,
                inode: metadata.st_ino as u64,
                filesystem: filesystem_identity,
            },
            filesystem: classify_native_filesystem_name(&raw_name),
            writable: mount.writable,
            local: mount.local,
            access_controls_enforced: mount.access_controls_enforced,
        })
    }

    fn failed_foundation_snapshot() -> FoundationSnapshot {
        FoundationSnapshot {
            volume_local: ResourceValue::QueryFailed,
            volume_internal: ResourceValue::QueryFailed,
            volume_removable: ResourceValue::QueryFailed,
            volume_ejectable: ResourceValue::QueryFailed,
            volume_read_only: ResourceValue::QueryFailed,
            supports_access_permissions: ResourceValue::QueryFailed,
            volume_kind: ResourceValue::QueryFailed,
            is_ubiquitous: ResourceValue::QueryFailed,
        }
    }

    fn foundation_snapshot(url: &NSURL) -> FoundationSnapshot {
        // SAFETY: These are immutable Foundation framework constants available
        // on every supported macOS release. Copy their references once, then
        // keep all foreign-static access inside this narrow adapter.
        let resource_keys = unsafe {
            [
                NSURLVolumeIsLocalKey,
                NSURLVolumeIsInternalKey,
                NSURLVolumeIsRemovableKey,
                NSURLVolumeIsEjectableKey,
                NSURLVolumeIsReadOnlyKey,
                NSURLVolumeSupportsAccessPermissionsKey,
                NSURLVolumeTypeNameKey,
                NSURLIsUbiquitousItemKey,
            ]
        };
        let [
            volume_local_key,
            volume_internal_key,
            volume_removable_key,
            volume_ejectable_key,
            volume_read_only_key,
            supports_access_permissions_key,
            volume_type_name_key,
            is_ubiquitous_key,
        ] = resource_keys;
        let keys = NSArray::<NSURLResourceKey>::from_slice(&[
            volume_local_key,
            volume_internal_key,
            volume_removable_key,
            volume_ejectable_key,
            volume_read_only_key,
            supports_access_permissions_key,
            volume_type_name_key,
            is_ubiquitous_key,
        ]);
        let values = match url.resourceValuesForKeys_error(&keys) {
            Ok(values) => values,
            Err(_private_error) => return failed_foundation_snapshot(),
        };

        let read_bool = |key: &NSURLResourceKey| match values.objectForKey(key) {
            None => ResourceValue::Missing,
            Some(value) => match value.downcast_ref::<NSNumber>() {
                Some(number) => ResourceValue::Value(number.as_bool()),
                None => ResourceValue::WrongType,
            },
        };
        let volume_kind = match values.objectForKey(volume_type_name_key) {
            None => ResourceValue::Missing,
            Some(value) => match value.downcast_ref::<NSString>() {
                Some(name) => ResourceValue::Value(classify_foundation_volume_name(
                    name.to_string().as_bytes(),
                )),
                None => ResourceValue::WrongType,
            },
        };

        FoundationSnapshot {
            volume_local: read_bool(volume_local_key),
            volume_internal: read_bool(volume_internal_key),
            volume_removable: read_bool(volume_removable_key),
            volume_ejectable: read_bool(volume_ejectable_key),
            volume_read_only: read_bool(volume_read_only_key),
            supports_access_permissions: read_bool(supports_access_permissions_key),
            volume_kind,
            is_ubiquitous: read_bool(is_ubiquitous_key),
        }
    }
}

#[cfg(target_os = "macos")]
// The AppState/persistence integration slice consumes this interface later;
// keep the detector dark without making the intended adapter path ambiguous.
#[allow(unused_imports)]
pub(crate) use native::{HeldMacOsTarget, MacOsFilesystemDetector};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::filesystem_policy::{
        FILE_V2_SUPPORTED_PROFILES, FilesystemStatusCode, JOURNAL_SUPPORTED_PROFILES,
        PersistenceTarget, evaluate_filesystem,
    };

    fn candidate_fd_snapshot() -> FdSnapshot {
        FdSnapshot {
            identity: TargetIdentity {
                device: 7,
                inode: 11,
                filesystem: [13, 17],
            },
            filesystem: NativeFilesystemKind::Apfs,
            writable: Ternary::Yes,
            local: Ternary::Yes,
            access_controls_enforced: Ternary::Yes,
        }
    }

    fn candidate_snapshot(provider: ProviderBoundaryResult) -> MacOsSnapshot {
        let fd = candidate_fd_snapshot();
        MacOsSnapshot {
            platform_release: PlatformRelease::new(15, 6, 0),
            initial_held_fd: fd,
            reopened_url_fd: fd,
            final_held_fd: fd,
            foundation: FoundationSnapshot {
                volume_local: ResourceValue::Value(true),
                volume_internal: ResourceValue::Value(true),
                volume_removable: ResourceValue::Value(false),
                volume_ejectable: ResourceValue::Value(false),
                volume_read_only: ResourceValue::Value(false),
                supports_access_permissions: ResourceValue::Value(true),
                volume_kind: ResourceValue::Value(FoundationVolumeKind::Apfs),
                is_ubiquitous: ResourceValue::Value(false),
            },
            provider,
        }
    }

    #[test]
    fn native_filesystem_names_are_mapped_exactly() {
        assert_eq!(
            classify_native_filesystem_name(b"apfs\0ignored"),
            NativeFilesystemKind::Apfs,
        );
        assert_eq!(
            classify_native_filesystem_name(b"hfs\0ignored"),
            NativeFilesystemKind::HfsPlus,
        );
        assert_eq!(
            classify_native_filesystem_name(b"osxfuse\0"),
            NativeFilesystemKind::Userspace,
        );
        assert_eq!(
            classify_native_filesystem_name(b"macfuse\0"),
            NativeFilesystemKind::Userspace,
        );
        assert_eq!(
            classify_native_filesystem_name(b"APFS\0"),
            NativeFilesystemKind::Other,
        );
        assert_eq!(
            classify_native_filesystem_name(b"apfs-shadow\0"),
            NativeFilesystemKind::Other,
        );
        assert_eq!(
            classify_foundation_volume_name(b"apfs"),
            FoundationVolumeKind::Apfs,
        );
        assert_eq!(
            classify_foundation_volume_name(b"hfs"),
            FoundationVolumeKind::HfsPlus,
        );
        assert_eq!(
            classify_foundation_volume_name(b"APFS"),
            FoundationVolumeKind::Other,
        );
    }

    #[test]
    fn darwin_mount_flags_are_reduced_to_closed_traits() {
        let favorable = classify_darwin_mount_flags(DARWIN_MNT_LOCAL);
        assert_eq!(favorable.writable, Ternary::Yes);
        assert_eq!(favorable.local, Ternary::Yes);
        assert_eq!(favorable.access_controls_enforced, Ternary::Yes);

        let denied = classify_darwin_mount_flags(DARWIN_MNT_RDONLY | DARWIN_MNT_IGNORE_OWNERSHIP);
        assert_eq!(denied.writable, Ternary::No);
        assert_eq!(denied.local, Ternary::No);
        assert_eq!(denied.access_controls_enforced, Ternary::No);
    }

    #[test]
    fn proved_local_apfs_is_only_a_candidate_observation() {
        let observation =
            classify_snapshot(candidate_snapshot(ProviderBoundaryResult::ProvedNotManaged));

        assert_eq!(observation.platform, Platform::MacOs);
        assert_eq!(observation.platform_release, PlatformRelease::new(15, 6, 0));
        assert_eq!(observation.family, FilesystemFamily::MacApfs);
        assert_eq!(observation.writable, Ternary::Yes);
        assert_eq!(observation.local, Ternary::Yes);
        assert_eq!(observation.kernel_native, Ternary::Yes);
        assert_eq!(observation.internal_fixed, Ternary::Yes);
        assert_eq!(observation.os_managed_cloud_root, Ternary::No);
        assert_eq!(observation.access_controls_enforced, Ternary::Yes);
        assert_eq!(observation.identity_stable, Ternary::Yes);
    }

    #[test]
    fn positive_file_provider_identification_denies_even_if_icloud_query_failed() {
        let mut snapshot = candidate_snapshot(ProviderBoundaryResult::Managed);
        snapshot.foundation.is_ubiquitous = ResourceValue::QueryFailed;

        assert_eq!(
            classify_snapshot(snapshot).os_managed_cloud_root,
            Ternary::Yes,
        );
    }

    #[test]
    fn unproved_or_failed_file_provider_negatives_never_become_favorable() {
        for provider in [
            ProviderBoundaryResult::UnprovedNotManaged,
            ProviderBoundaryResult::UnexpectedFailure,
            ProviderBoundaryResult::TimedOut,
            ProviderBoundaryResult::LateCompletion,
        ] {
            assert_eq!(
                classify_snapshot(candidate_snapshot(provider)).os_managed_cloud_root,
                Ternary::Unknown,
                "{provider:?} must remain fail-closed",
            );
        }

        let mut ubiquitous = candidate_snapshot(ProviderBoundaryResult::ProvedNotManaged);
        ubiquitous.foundation.is_ubiquitous = ResourceValue::Value(true);
        assert_eq!(
            classify_snapshot(ubiquitous).os_managed_cloud_root,
            Ternary::Yes,
        );
    }

    #[test]
    fn every_missing_wrong_typed_or_failed_foundation_value_denies() {
        type BoolSetter = fn(&mut FoundationSnapshot, ResourceValue<bool>);
        type BoolCase = (
            &'static str,
            BoolSetter,
            fn(FilesystemObservation) -> Ternary,
        );
        let bool_cases: &[BoolCase] = &[
            (
                "volume_local",
                |foundation, value| foundation.volume_local = value,
                |observation| observation.local,
            ),
            (
                "volume_internal",
                |foundation, value| foundation.volume_internal = value,
                |observation| observation.internal_fixed,
            ),
            (
                "volume_removable",
                |foundation, value| foundation.volume_removable = value,
                |observation| observation.internal_fixed,
            ),
            (
                "volume_ejectable",
                |foundation, value| foundation.volume_ejectable = value,
                |observation| observation.internal_fixed,
            ),
            (
                "volume_read_only",
                |foundation, value| foundation.volume_read_only = value,
                |observation| observation.writable,
            ),
            (
                "supports_access_permissions",
                |foundation, value| foundation.supports_access_permissions = value,
                |observation| observation.access_controls_enforced,
            ),
            (
                "is_ubiquitous",
                |foundation, value| foundation.is_ubiquitous = value,
                |observation| observation.os_managed_cloud_root,
            ),
        ];
        for invalid in [
            ResourceValue::Missing,
            ResourceValue::WrongType,
            ResourceValue::QueryFailed,
        ] {
            for (name, set, read) in bool_cases {
                let mut snapshot = candidate_snapshot(ProviderBoundaryResult::ProvedNotManaged);
                set(&mut snapshot.foundation, invalid);
                assert_eq!(
                    read(classify_snapshot(snapshot)),
                    Ternary::Unknown,
                    "{name} {invalid:?} must deny",
                );
            }

            let mut snapshot = candidate_snapshot(ProviderBoundaryResult::ProvedNotManaged);
            snapshot.foundation.volume_kind = match invalid {
                ResourceValue::Missing => ResourceValue::Missing,
                ResourceValue::WrongType => ResourceValue::WrongType,
                ResourceValue::QueryFailed => ResourceValue::QueryFailed,
                ResourceValue::Value(_) => unreachable!(),
            };
            assert_eq!(
                classify_snapshot(snapshot).kernel_native,
                Ternary::Unknown,
                "volume kind {invalid:?} must deny",
            );
        }
    }

    #[test]
    fn closed_negative_storage_traits_are_preserved() {
        let provider = ProviderBoundaryResult::ProvedNotManaged;

        let mut remote = candidate_snapshot(provider);
        remote.initial_held_fd.local = Ternary::No;
        remote.reopened_url_fd.local = Ternary::No;
        remote.final_held_fd.local = Ternary::No;
        remote.foundation.volume_local = ResourceValue::Value(false);
        assert_eq!(classify_snapshot(remote).local, Ternary::No);

        for change in [
            |foundation: &mut FoundationSnapshot| {
                foundation.volume_internal = ResourceValue::Value(false);
            },
            |foundation: &mut FoundationSnapshot| {
                foundation.volume_removable = ResourceValue::Value(true);
            },
            |foundation: &mut FoundationSnapshot| {
                foundation.volume_ejectable = ResourceValue::Value(true);
            },
        ] {
            let mut snapshot = candidate_snapshot(provider);
            change(&mut snapshot.foundation);
            assert_eq!(classify_snapshot(snapshot).internal_fixed, Ternary::No);
        }

        let mut read_only = candidate_snapshot(provider);
        read_only.initial_held_fd.writable = Ternary::No;
        read_only.reopened_url_fd.writable = Ternary::No;
        read_only.final_held_fd.writable = Ternary::No;
        read_only.foundation.volume_read_only = ResourceValue::Value(true);
        assert_eq!(classify_snapshot(read_only).writable, Ternary::No);

        let mut userspace = candidate_snapshot(provider);
        userspace.initial_held_fd.filesystem = NativeFilesystemKind::Userspace;
        userspace.reopened_url_fd.filesystem = NativeFilesystemKind::Userspace;
        userspace.final_held_fd.filesystem = NativeFilesystemKind::Userspace;
        userspace.foundation.volume_kind = ResourceValue::Value(FoundationVolumeKind::Other);
        assert_eq!(classify_snapshot(userspace).kernel_native, Ternary::No);

        let mut hfs = candidate_snapshot(provider);
        hfs.initial_held_fd.filesystem = NativeFilesystemKind::HfsPlus;
        hfs.reopened_url_fd.filesystem = NativeFilesystemKind::HfsPlus;
        hfs.final_held_fd.filesystem = NativeFilesystemKind::HfsPlus;
        hfs.foundation.volume_kind = ResourceValue::Value(FoundationVolumeKind::HfsPlus);
        let hfs = classify_snapshot(hfs);
        assert_eq!(hfs.family, FilesystemFamily::MacHfsPlus);
        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            assert_eq!(
                evaluate_filesystem(target, Ok(hfs)).code,
                FilesystemStatusCode::FilesystemUnproved,
            );
        }
    }

    #[test]
    fn any_fd_or_filesystem_recheck_disagreement_marks_the_target_changed() {
        type SnapshotMutation = fn(&mut MacOsSnapshot);
        let cases: &[(&str, SnapshotMutation)] = &[
            ("device", |snapshot| {
                snapshot.reopened_url_fd.identity.device += 1;
            }),
            ("inode", |snapshot| {
                snapshot.reopened_url_fd.identity.inode += 1;
            }),
            ("filesystem_identity", |snapshot| {
                snapshot.final_held_fd.identity.filesystem[0] += 1;
            }),
            ("filesystem_kind", |snapshot| {
                snapshot.final_held_fd.filesystem = NativeFilesystemKind::Other;
            }),
            ("mount_traits", |snapshot| {
                snapshot.final_held_fd.writable = Ternary::No;
            }),
        ];

        for (name, mutate) in cases {
            let mut snapshot = candidate_snapshot(ProviderBoundaryResult::ProvedNotManaged);
            mutate(&mut snapshot);
            assert_eq!(
                classify_snapshot(snapshot).identity_stable,
                Ternary::No,
                "{name} disagreement must deny",
            );
        }
    }

    #[test]
    fn empty_profiles_and_unproved_provider_boundary_prevent_runtime_support() {
        assert!(JOURNAL_SUPPORTED_PROFILES.is_empty());
        assert!(FILE_V2_SUPPORTED_PROFILES.is_empty());

        let future_candidate =
            classify_snapshot(candidate_snapshot(ProviderBoundaryResult::ProvedNotManaged));
        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            assert_eq!(
                evaluate_filesystem(target, Ok(future_candidate)).code,
                FilesystemStatusCode::DurabilityUnproved,
            );
        }

        let current_runtime = classify_snapshot(candidate_snapshot(
            ProviderBoundaryResult::UnprovedNotManaged,
        ));
        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            assert_eq!(
                evaluate_filesystem(target, Ok(current_runtime)).code,
                FilesystemStatusCode::InspectionUnavailable,
            );
        }
    }
}
