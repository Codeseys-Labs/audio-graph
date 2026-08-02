//! Windows metadata detector for credential-v2 filesystem qualification.

use super::{
    DetectorFault, FILESYSTEM_DETECTOR_SCHEMA_VERSION, FilesystemFamily, FilesystemObservation,
    Platform, PlatformRelease, Ternary,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ApiFault {
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WindowsFilesystem {
    Ntfs,
    Refs,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct VolumeSample {
    identity: u64,
    filesystem: WindowsFilesystem,
    read_only: bool,
    persistent_acls: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RemoteProtocolCall {
    Remote,
    NoRemoteProtocol,
}

fn remote_protocol_call_from_protocol(protocol: u32) -> RemoteProtocolCall {
    if protocol == 0 {
        RemoteProtocolCall::NoRemoteProtocol
    } else {
        RemoteProtocolCall::Remote
    }
}

fn windows_filesystem_from_utf16(name: &[u16]) -> WindowsFilesystem {
    let name = &name[..name
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(name.len())];
    if name == [b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16] {
        WindowsFilesystem::Ntfs
    } else if name == [b'R' as u16, b'e' as u16, b'F' as u16, b'S' as u16] {
        WindowsFilesystem::Refs
    } else {
        WindowsFilesystem::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DriveTypeCall {
    Fixed,
    Remote,
    Removable,
    Other,
}

fn drive_type_call_from_native(
    value: u32,
    fixed: u32,
    remote: u32,
    removable: u32,
) -> DriveTypeCall {
    if value == fixed {
        DriveTypeCall::Fixed
    } else if value == remote {
        DriveTypeCall::Remote
    } else if value == removable {
        DriveTypeCall::Removable
    } else {
        DriveTypeCall::Other
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HotplugCall {
    media_removable: bool,
    media_hotplug: bool,
    device_hotplug: bool,
}

const CLOUD_FILE_NOT_UNDER_SYNC_ROOT_HRESULT: i32 = 0x8007_0186_u32 as i32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CloudFilesCall {
    Managed,
    NotUnderSyncRootSentinel,
    Unknown,
}

fn cloud_files_call_from_hresult(code: i32) -> CloudFilesCall {
    if code == CLOUD_FILE_NOT_UNDER_SYNC_ROOT_HRESULT {
        CloudFilesCall::NotUnderSyncRootSentinel
    } else {
        CloudFilesCall::Unknown
    }
}

struct VolumeGuidPaths {
    drive_root: Vec<u16>,
    device: Vec<u16>,
}

fn volume_guid_paths(final_path: &[u16]) -> Option<VolumeGuidPaths> {
    const PREFIX: &[u8] = br"\\?\Volume{";
    const GUID_LENGTH: usize = 36;
    const GUID_HYPHENS: [usize; 4] = [8, 13, 18, 23];

    let final_path = String::from_utf16(final_path).ok()?;
    let bytes = final_path.as_bytes();
    if !bytes.starts_with(PREFIX) {
        return None;
    }

    let guid_start = PREFIX.len();
    let guid_end = guid_start.checked_add(GUID_LENGTH)?;
    let guid = bytes.get(guid_start..guid_end)?;
    for (index, byte) in guid.iter().copied().enumerate() {
        let valid = if GUID_HYPHENS.contains(&index) {
            byte == b'-'
        } else {
            byte.is_ascii_hexdigit()
        };
        if !valid {
            return None;
        }
    }
    if bytes.get(guid_end) != Some(&b'}') || bytes.get(guid_end + 1) != Some(&b'\\') {
        return None;
    }

    let root_end = guid_end + 2;
    let root = std::str::from_utf8(bytes.get(..root_end)?).ok()?;
    let mut drive_root: Vec<u16> = root.encode_utf16().collect();
    drive_root.push(0);
    let mut device = drive_root[..drive_root.len() - 2].to_vec();
    device.push(0);
    Some(VolumeGuidPaths { drive_root, device })
}

trait WindowsMetadataApi {
    type DirectoryIdentity: Eq;
    type HeldDirectory;
    type HeldVolume;

    fn is_directory(&self, target: &Self::HeldDirectory) -> Result<bool, ApiFault>;
    fn directory_identity(
        &self,
        target: &Self::HeldDirectory,
    ) -> Result<Self::DirectoryIdentity, ApiFault>;
    fn volume_sample(&self, target: &Self::HeldDirectory) -> Result<VolumeSample, ApiFault>;
    fn remote_protocol(&self, target: &Self::HeldDirectory)
    -> Result<RemoteProtocolCall, ApiFault>;
    fn open_volume(&self, target: &Self::HeldDirectory) -> Result<Self::HeldVolume, ApiFault>;
    fn drive_type(&self, volume: &Self::HeldVolume) -> Result<DriveTypeCall, ApiFault>;
    fn hotplug(&self, volume: &Self::HeldVolume) -> Result<HotplugCall, ApiFault>;
    fn cloud_files(&self, target: &Self::HeldDirectory) -> CloudFilesCall;
}

fn inspection_unavailable(_: ApiFault) -> DetectorFault {
    DetectorFault::InspectionUnavailable
}

fn inspect_with_api<A: WindowsMetadataApi>(
    api: &A,
    target: &A::HeldDirectory,
    platform_release: PlatformRelease,
) -> Result<FilesystemObservation, DetectorFault> {
    if !api.is_directory(target).map_err(inspection_unavailable)? {
        return Err(DetectorFault::TargetUnavailable);
    }

    let initial_identity = api
        .directory_identity(target)
        .map_err(inspection_unavailable)?;
    let initial_volume = api.volume_sample(target).map_err(inspection_unavailable)?;
    let remote = api
        .remote_protocol(target)
        .map_err(inspection_unavailable)?;
    let volume = api.open_volume(target).map_err(inspection_unavailable)?;
    let drive = api.drive_type(&volume).map_err(inspection_unavailable)?;
    let hotplug = api.hotplug(&volume).map_err(inspection_unavailable)?;
    let cloud = api.cloud_files(target);
    let final_identity = api
        .directory_identity(target)
        .map_err(inspection_unavailable)?;
    let final_volume = api.volume_sample(target).map_err(inspection_unavailable)?;

    let family = match initial_volume.filesystem {
        WindowsFilesystem::Ntfs => FilesystemFamily::WindowsNtfs,
        WindowsFilesystem::Refs => FilesystemFamily::WindowsRefs,
        WindowsFilesystem::Other => FilesystemFamily::Other,
    };
    let kernel_native = match initial_volume.filesystem {
        WindowsFilesystem::Ntfs | WindowsFilesystem::Refs => Ternary::Yes,
        WindowsFilesystem::Other => Ternary::Unknown,
    };
    let local = match (remote, drive) {
        (RemoteProtocolCall::Remote, _) | (_, DriveTypeCall::Remote) => Ternary::No,
        (RemoteProtocolCall::NoRemoteProtocol, DriveTypeCall::Fixed | DriveTypeCall::Removable) => {
            Ternary::Yes
        }
        (RemoteProtocolCall::NoRemoteProtocol, DriveTypeCall::Other) => Ternary::Unknown,
    };
    let internal_fixed = match drive {
        DriveTypeCall::Fixed => {
            if hotplug.media_removable || hotplug.media_hotplug || hotplug.device_hotplug {
                Ternary::No
            } else {
                Ternary::Yes
            }
        }
        DriveTypeCall::Remote | DriveTypeCall::Removable => Ternary::No,
        DriveTypeCall::Other => Ternary::Unknown,
    };
    let os_managed_cloud_root = match cloud {
        CloudFilesCall::Managed => Ternary::Yes,
        CloudFilesCall::NotUnderSyncRootSentinel => Ternary::No,
        CloudFilesCall::Unknown => Ternary::Unknown,
    };

    Ok(FilesystemObservation {
        platform: Platform::Windows,
        platform_release,
        family,
        writable: if initial_volume.read_only {
            Ternary::No
        } else {
            Ternary::Yes
        },
        local,
        kernel_native,
        internal_fixed,
        os_managed_cloud_root,
        access_controls_enforced: if initial_volume.persistent_acls {
            Ternary::Yes
        } else {
            Ternary::No
        },
        identity_stable: if initial_identity == final_identity && initial_volume == final_volume {
            Ternary::Yes
        } else {
            Ternary::No
        },
        detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
    })
}

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    use crate::credentials::filesystem_policy::FilesystemDetector;
    use core::ffi::c_void;
    use std::mem::{size_of, size_of_val};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::CloudFilters::{
        CF_SYNC_ROOT_BASIC_INFO, CF_SYNC_ROOT_INFO_BASIC, CfGetSyncRootInfoByHandle,
    };
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAGS_AND_ATTRIBUTES, FILE_ID_INFO,
        FILE_READ_ATTRIBUTES, FILE_REMOTE_PROTOCOL_INFO, FILE_SHARE_DELETE, FILE_SHARE_MODE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_STANDARD_INFO, FileIdInfo, FileRemoteProtocolInfo,
        FileStandardInfo, GetFileInformationByHandleEx, GetFinalPathNameByHandleW,
        GetVolumeInformationByHandleW, OPEN_EXISTING, READ_CONTROL, VOLUME_NAME_GUID,
    };
    use windows::Win32::System::IO::DeviceIoControl;
    use windows::Win32::System::Ioctl::{IOCTL_STORAGE_GET_HOTPLUG_INFO, STORAGE_HOTPLUG_INFO};
    use windows::Win32::System::SystemServices::{FILE_PERSISTENT_ACLS, FILE_READ_ONLY_VOLUME};
    use windows::Win32::System::WindowsProgramming::{DRIVE_FIXED, DRIVE_REMOTE, DRIVE_REMOVABLE};
    use windows::core::PCWSTR;

    const MAX_FINAL_PATH_UNITS: usize = 32_768;
    const FILE_REMOTE_PROTOCOL_INFO_VERSION: u16 = 1;

    struct OwnedHandle(HANDLE);

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            if !self.0.is_invalid() {
                // SAFETY: the handle was returned by CreateFileW and this guard
                // owns it exclusively. Drop closes it exactly once.
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }

    pub(crate) struct WindowsHeldDirectory {
        handle: OwnedHandle,
    }

    /// Credential-backend capability produced only after the detector has
    /// qualified this exact held directory. Its callback methods are a trusted
    /// crate-private native boundary; callers must not return or persist the
    /// borrowed native pointer/handle they receive.
    pub(crate) struct WindowsQualifiedParent<'a> {
        held: &'a WindowsHeldDirectory,
        normalized_path: Vec<u16>,
        identity: DirectoryIdentity,
        volume: VolumeSample,
    }

    /// Opaque NUL-terminated child path derived from the qualified held parent.
    /// The callback is restricted to trusted crate-private credential code;
    /// callers must not return or persist its borrowed `PCWSTR`.
    pub(crate) struct WindowsChildPath(Vec<u16>);

    impl std::fmt::Debug for WindowsQualifiedParent<'_> {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("WindowsQualifiedParent([REDACTED])")
        }
    }

    impl std::fmt::Debug for WindowsChildPath {
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str("WindowsChildPath([REDACTED])")
        }
    }

    impl WindowsChildPath {
        pub(crate) fn with_pcwstr<R>(&self, operation: impl FnOnce(PCWSTR) -> R) -> R {
            operation(PCWSTR(self.0.as_ptr()))
        }

        #[cfg(test)]
        pub(crate) fn redacted_test_fixture(value: &str) -> Self {
            Self(value.encode_utf16().chain(std::iter::once(0)).collect())
        }
    }

    impl WindowsQualifiedParent<'_> {
        pub(crate) fn with_scoped_handle<R>(&self, operation: impl FnOnce(HANDLE) -> R) -> R {
            operation(self.held.handle.0)
        }

        pub(crate) fn child_path(&self, leaf: &str) -> Result<WindowsChildPath, DetectorFault> {
            if !valid_child_leaf(leaf) {
                return Err(DetectorFault::InspectionUnavailable);
            }

            let mut path = self.normalized_path.clone();
            if path.last().copied() != Some(b'\\' as u16) {
                path.push(b'\\' as u16);
            }
            path.extend(leaf.encode_utf16());
            if path.len() >= MAX_FINAL_PATH_UNITS {
                return Err(DetectorFault::InspectionUnavailable);
            }
            path.push(0);
            Ok(WindowsChildPath(path))
        }

        pub(crate) fn identity_is_unchanged(&self) -> Result<bool, DetectorFault> {
            let api = NativeWindowsMetadataApi;
            let is_directory = api
                .is_directory(self.held)
                .map_err(inspection_unavailable)?;
            let identity = api
                .directory_identity(self.held)
                .map_err(inspection_unavailable)?;
            let volume = api
                .volume_sample(self.held)
                .map_err(inspection_unavailable)?;
            let current_path =
                normalized_handle_path(self.held.handle.0).map_err(inspection_unavailable)?;

            let mut stored_path = self.normalized_path.clone();
            stored_path.push(0);
            let share =
                FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
            // SAFETY: the stored path was derived from this held handle, is
            // NUL-terminated here, and remains live through the call.
            let reopened_handle = unsafe {
                CreateFileW(
                    PCWSTR(stored_path.as_ptr()),
                    FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0,
                    share,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    None,
                )
            }
            .map_err(|_| DetectorFault::InspectionUnavailable)?;
            let reopened = WindowsHeldDirectory {
                handle: OwnedHandle(reopened_handle),
            };
            let reopened_is_directory = api
                .is_directory(&reopened)
                .map_err(inspection_unavailable)?;
            let reopened_identity = api
                .directory_identity(&reopened)
                .map_err(inspection_unavailable)?;
            let reopened_volume = api
                .volume_sample(&reopened)
                .map_err(inspection_unavailable)?;
            Ok(held_parent_is_unchanged(
                current_path == self.normalized_path,
                ParentSnapshot {
                    is_directory: true,
                    identity: self.identity,
                    volume: self.volume,
                },
                ParentSnapshot {
                    is_directory,
                    identity,
                    volume,
                },
                ParentSnapshot {
                    is_directory: reopened_is_directory,
                    identity: reopened_identity,
                    volume: reopened_volume,
                },
            ))
        }

        pub(crate) fn volume_matches(&self, volume_serial: u64) -> bool {
            self.volume.identity == volume_serial
        }
    }

    fn valid_child_leaf(leaf: &str) -> bool {
        !leaf.is_empty()
            && leaf != "."
            && leaf != ".."
            && leaf.len() <= 128
            && leaf
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
    }

    #[derive(Clone, Copy)]
    struct ParentSnapshot {
        is_directory: bool,
        identity: DirectoryIdentity,
        volume: VolumeSample,
    }

    fn held_parent_is_unchanged(
        normalized_path_unchanged: bool,
        initial: ParentSnapshot,
        current: ParentSnapshot,
        reopened: ParentSnapshot,
    ) -> bool {
        initial.is_directory
            && current.is_directory
            && reopened.is_directory
            && normalized_path_unchanged
            && initial.identity == current.identity
            && initial.volume == current.volume
            && initial.identity == reopened.identity
            && initial.volume == reopened.volume
    }

    fn normalized_handle_path(handle: HANDLE) -> Result<Vec<u16>, ApiFault> {
        let mut normalized_path = vec![0u16; MAX_FINAL_PATH_UNITS];
        // SAFETY: the buffer is writable for its full reported length and the
        // caller holds the directory handle for the synchronous query.
        let length =
            unsafe { GetFinalPathNameByHandleW(handle, &mut normalized_path, VOLUME_NAME_GUID) }
                as usize;
        if length == 0 || length >= normalized_path.len() {
            return Err(ApiFault::Unavailable);
        }
        normalized_path.truncate(length);
        Ok(normalized_path)
    }

    struct WindowsHeldVolume {
        handle: OwnedHandle,
        drive_root: Vec<u16>,
    }

    #[derive(Clone, Copy, PartialEq, Eq)]
    struct DirectoryIdentity {
        volume_serial: u64,
        file_id: [u8; 16],
    }

    struct NativeWindowsMetadataApi;

    pub(crate) struct WindowsFilesystemDetector {
        platform_release: PlatformRelease,
    }

    impl WindowsFilesystemDetector {
        pub(crate) const fn new(platform_release: PlatformRelease) -> Self {
            Self { platform_release }
        }

        pub(crate) fn open_target(
            &self,
            path: &Path,
        ) -> Result<WindowsHeldDirectory, DetectorFault> {
            let path: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            let share =
                FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
            // SAFETY: `path` is NUL-terminated and lives through the call. The
            // returned handle is transferred immediately into its RAII guard.
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(path.as_ptr()),
                    FILE_READ_ATTRIBUTES.0 | READ_CONTROL.0,
                    share,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_BACKUP_SEMANTICS,
                    None,
                )
            }
            .map_err(|_| DetectorFault::TargetUnavailable)?;
            Ok(WindowsHeldDirectory {
                handle: OwnedHandle(handle),
            })
        }

        pub(crate) fn qualify_parent<'a>(
            &self,
            target: &'a WindowsHeldDirectory,
        ) -> Result<WindowsQualifiedParent<'a>, DetectorFault> {
            let observation = self.inspect(target)?;
            if observation.family != FilesystemFamily::WindowsNtfs
                || observation.writable != Ternary::Yes
                || observation.local != Ternary::Yes
                || observation.kernel_native != Ternary::Yes
                || observation.internal_fixed != Ternary::Yes
                || observation.os_managed_cloud_root != Ternary::No
                || observation.access_controls_enforced != Ternary::Yes
                || observation.identity_stable != Ternary::Yes
                || observation.detector_schema != FILESYSTEM_DETECTOR_SCHEMA_VERSION
            {
                return Err(DetectorFault::InspectionUnavailable);
            }

            let normalized_path =
                normalized_handle_path(target.handle.0).map_err(inspection_unavailable)?;

            let api = NativeWindowsMetadataApi;
            let identity = api
                .directory_identity(target)
                .map_err(inspection_unavailable)?;
            let volume = api.volume_sample(target).map_err(inspection_unavailable)?;
            Ok(WindowsQualifiedParent {
                held: target,
                normalized_path,
                identity,
                volume,
            })
        }
    }

    impl FilesystemDetector for WindowsFilesystemDetector {
        type HeldTarget = WindowsHeldDirectory;

        fn inspect(
            &self,
            target: &Self::HeldTarget,
        ) -> Result<FilesystemObservation, DetectorFault> {
            inspect_with_api(&NativeWindowsMetadataApi, target, self.platform_release)
        }
    }

    fn file_information<T: Default>(
        handle: HANDLE,
        class: windows::Win32::Storage::FileSystem::FILE_INFO_BY_HANDLE_CLASS,
    ) -> Result<T, ApiFault> {
        let mut value = T::default();
        // SAFETY: `value` is a writable `T` with the exact byte size passed to
        // the API, and remains alive until the synchronous call returns.
        unsafe {
            GetFileInformationByHandleEx(
                handle,
                class,
                std::ptr::addr_of_mut!(value).cast::<c_void>(),
                size_of::<T>() as u32,
            )
        }
        .map_err(|_| ApiFault::Unavailable)?;
        Ok(value)
    }

    impl WindowsMetadataApi for NativeWindowsMetadataApi {
        type DirectoryIdentity = DirectoryIdentity;
        type HeldDirectory = WindowsHeldDirectory;
        type HeldVolume = WindowsHeldVolume;

        fn is_directory(&self, target: &Self::HeldDirectory) -> Result<bool, ApiFault> {
            let info: FILE_STANDARD_INFO = file_information(target.handle.0, FileStandardInfo)?;
            Ok(info.Directory)
        }

        fn directory_identity(
            &self,
            target: &Self::HeldDirectory,
        ) -> Result<Self::DirectoryIdentity, ApiFault> {
            let info: FILE_ID_INFO = file_information(target.handle.0, FileIdInfo)?;
            Ok(DirectoryIdentity {
                volume_serial: info.VolumeSerialNumber,
                file_id: info.FileId.Identifier,
            })
        }

        fn volume_sample(&self, target: &Self::HeldDirectory) -> Result<VolumeSample, ApiFault> {
            let mut identity = 0u32;
            let mut flags = 0u32;
            let mut filesystem_name = [0u16; 32];
            // SAFETY: every optional output pointer references initialized,
            // writable storage for the duration of this synchronous call. No
            // volume label or maximum-component value is requested.
            unsafe {
                GetVolumeInformationByHandleW(
                    target.handle.0,
                    None,
                    Some(&mut identity),
                    None,
                    Some(&mut flags),
                    Some(&mut filesystem_name),
                )
            }
            .map_err(|_| ApiFault::Unavailable)?;
            Ok(VolumeSample {
                identity: u64::from(identity),
                filesystem: windows_filesystem_from_utf16(&filesystem_name),
                read_only: flags & FILE_READ_ONLY_VOLUME != 0,
                persistent_acls: flags & FILE_PERSISTENT_ACLS != 0,
            })
        }

        fn remote_protocol(
            &self,
            target: &Self::HeldDirectory,
        ) -> Result<RemoteProtocolCall, ApiFault> {
            let mut info = FILE_REMOTE_PROTOCOL_INFO {
                StructureVersion: FILE_REMOTE_PROTOCOL_INFO_VERSION,
                StructureSize: size_of::<FILE_REMOTE_PROTOCOL_INFO>() as u16,
                ..Default::default()
            };
            // SAFETY: `info` is the exact output structure required by
            // FileRemoteProtocolInfo and remains valid through the call.
            unsafe {
                GetFileInformationByHandleEx(
                    target.handle.0,
                    FileRemoteProtocolInfo,
                    std::ptr::addr_of_mut!(info).cast::<c_void>(),
                    size_of_val(&info) as u32,
                )
            }
            .map_err(|_| ApiFault::Unavailable)?;
            Ok(remote_protocol_call_from_protocol(info.Protocol))
        }

        fn open_volume(&self, target: &Self::HeldDirectory) -> Result<Self::HeldVolume, ApiFault> {
            let mut final_path = vec![0u16; MAX_FINAL_PATH_UNITS];
            // SAFETY: the buffer is writable for its full reported length and
            // the held directory handle remains open throughout the call.
            let length = unsafe {
                GetFinalPathNameByHandleW(target.handle.0, &mut final_path, VOLUME_NAME_GUID)
            } as usize;
            if length == 0 || length >= final_path.len() {
                return Err(ApiFault::Unavailable);
            }
            let paths = volume_guid_paths(&final_path[..length]).ok_or(ApiFault::Unavailable)?;
            let share =
                FILE_SHARE_MODE(FILE_SHARE_READ.0 | FILE_SHARE_WRITE.0 | FILE_SHARE_DELETE.0);
            // SAFETY: the private device path is NUL-terminated and lives
            // through the call. The returned volume handle enters RAII.
            let handle = unsafe {
                CreateFileW(
                    PCWSTR(paths.device.as_ptr()),
                    0,
                    share,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAGS_AND_ATTRIBUTES(0),
                    None,
                )
            }
            .map_err(|_| ApiFault::Unavailable)?;
            Ok(WindowsHeldVolume {
                handle: OwnedHandle(handle),
                drive_root: paths.drive_root,
            })
        }

        fn drive_type(&self, volume: &Self::HeldVolume) -> Result<DriveTypeCall, ApiFault> {
            // SAFETY: `drive_root` is a NUL-terminated volume GUID root derived
            // from the same held directory handle.
            let drive_type = unsafe {
                windows::Win32::Storage::FileSystem::GetDriveTypeW(PCWSTR(
                    volume.drive_root.as_ptr(),
                ))
            };
            Ok(drive_type_call_from_native(
                drive_type,
                DRIVE_FIXED,
                DRIVE_REMOTE,
                DRIVE_REMOVABLE,
            ))
        }

        fn hotplug(&self, volume: &Self::HeldVolume) -> Result<HotplugCall, ApiFault> {
            let mut info = STORAGE_HOTPLUG_INFO {
                Size: size_of::<STORAGE_HOTPLUG_INFO>() as u32,
                ..Default::default()
            };
            let mut returned = 0u32;
            // SAFETY: the volume handle stays open; `info` is writable for the
            // exact output size and `returned` is a valid result pointer.
            unsafe {
                DeviceIoControl(
                    volume.handle.0,
                    IOCTL_STORAGE_GET_HOTPLUG_INFO,
                    None,
                    0,
                    Some(std::ptr::addr_of_mut!(info).cast::<c_void>()),
                    size_of_val(&info) as u32,
                    Some(&mut returned),
                    None,
                )
            }
            .map_err(|_| ApiFault::Unavailable)?;
            if returned < size_of::<STORAGE_HOTPLUG_INFO>() as u32 {
                return Err(ApiFault::Unavailable);
            }
            Ok(HotplugCall {
                media_removable: info.MediaRemovable,
                media_hotplug: info.MediaHotplug,
                device_hotplug: info.DeviceHotplug,
            })
        }

        fn cloud_files(&self, target: &Self::HeldDirectory) -> CloudFilesCall {
            let mut info = CF_SYNC_ROOT_BASIC_INFO::default();
            // SAFETY: `info` is the exact writable basic-info structure, and
            // the held directory handle remains open for this synchronous call.
            match unsafe {
                CfGetSyncRootInfoByHandle(
                    target.handle.0,
                    CF_SYNC_ROOT_INFO_BASIC,
                    std::ptr::addr_of_mut!(info).cast::<c_void>(),
                    size_of_val(&info) as u32,
                    None,
                )
            } {
                Ok(()) => CloudFilesCall::Managed,
                Err(error) => cloud_files_call_from_hresult(error.code().0),
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use crate::credentials::filesystem_policy::{
            FILE_V2_SUPPORTED_PROFILES, FilesystemStatus, FilesystemStatusCode,
            JOURNAL_SUPPORTED_PROFILES, PersistenceTarget, evaluate_filesystem,
        };

        #[test]
        fn qualified_child_names_are_leaf_only_and_debug_is_redacted() {
            for accepted in [
                "state.json",
                "credentials.json",
                ".audiograph-credential-0123456789abcdef.tmp",
            ] {
                assert!(valid_child_leaf(accepted), "accepted leaf {accepted}");
            }
            for rejected in [
                "",
                ".",
                "..",
                "nested\\state.json",
                "nested/state.json",
                "C:state.json",
                "state json",
                "state.json\0suffix",
            ] {
                assert!(!valid_child_leaf(rejected), "rejected leaf {rejected:?}");
            }
            assert!(!valid_child_leaf(&"a".repeat(129)));

            let path = WindowsChildPath(
                "private-path-volume-file-sid-canary\0"
                    .encode_utf16()
                    .collect(),
            );
            assert_eq!(format!("{path:?}"), "WindowsChildPath([REDACTED])");
        }

        #[test]
        fn qualified_parent_recheck_requires_directory_identity_and_volume_equality() {
            let identity = DirectoryIdentity {
                volume_serial: 11,
                file_id: [0x22; 16],
            };
            let volume = VolumeSample {
                identity: 11,
                filesystem: WindowsFilesystem::Ntfs,
                read_only: false,
                persistent_acls: true,
            };
            let snapshot = ParentSnapshot {
                is_directory: true,
                identity,
                volume,
            };
            assert!(held_parent_is_unchanged(true, snapshot, snapshot, snapshot,));

            let changed_identity = DirectoryIdentity {
                file_id: [0x33; 16],
                ..identity
            };
            let changed_volume = VolumeSample {
                identity: 12,
                ..volume
            };
            assert!(!held_parent_is_unchanged(
                true,
                snapshot,
                ParentSnapshot {
                    is_directory: false,
                    ..snapshot
                },
                snapshot,
            ));
            assert!(!held_parent_is_unchanged(
                true,
                snapshot,
                ParentSnapshot {
                    identity: changed_identity,
                    ..snapshot
                },
                snapshot,
            ));
            assert!(!held_parent_is_unchanged(
                true,
                snapshot,
                ParentSnapshot {
                    volume: changed_volume,
                    ..snapshot
                },
                snapshot,
            ));
            assert!(!held_parent_is_unchanged(
                false, snapshot, snapshot, snapshot,
            ));
            assert!(!held_parent_is_unchanged(
                true,
                snapshot,
                snapshot,
                ParentSnapshot {
                    identity: changed_identity,
                    ..snapshot
                },
            ));
            assert!(!held_parent_is_unchanged(
                true,
                snapshot,
                snapshot,
                ParentSnapshot {
                    volume: changed_volume,
                    ..snapshot
                },
            ));
        }

        #[test]
        #[ignore = "requires native Windows and AUDIO_GRAPH_WINDOWS_FILESYSTEM_SMOKE_DIR"]
        fn native_metadata_smoke_uses_only_closed_observations() {
            let path = std::env::var_os("AUDIO_GRAPH_WINDOWS_FILESYSTEM_SMOKE_DIR")
                .map(std::path::PathBuf::from)
                .expect("set the private smoke target outside test output");
            let platform_release = PlatformRelease::new(10, 0, 0);
            let detector = WindowsFilesystemDetector::new(platform_release);
            let target = detector
                .open_target(&path)
                .expect("open the configured metadata-only smoke target");
            let observation = detector
                .inspect(&target)
                .expect("inspect the metadata-only smoke target");

            assert_eq!(
                observation,
                FilesystemObservation {
                    platform: Platform::Windows,
                    platform_release,
                    family: FilesystemFamily::WindowsNtfs,
                    writable: Ternary::Yes,
                    local: Ternary::Yes,
                    kernel_native: Ternary::Yes,
                    internal_fixed: Ternary::Yes,
                    os_managed_cloud_root: Ternary::No,
                    access_controls_enforced: Ternary::Yes,
                    identity_stable: Ternary::Yes,
                    detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
                }
            );
            assert!(JOURNAL_SUPPORTED_PROFILES.is_empty());
            assert!(FILE_V2_SUPPORTED_PROFILES.is_empty());

            let private_path = path.to_string_lossy();
            for persistence_target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
                let status = evaluate_filesystem(persistence_target, Ok(observation));
                assert_eq!(
                    status,
                    FilesystemStatus {
                        target: persistence_target,
                        code: FilesystemStatusCode::DurabilityUnproved,
                        family: Some(FilesystemFamily::WindowsNtfs),
                        detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
                    }
                );
                let serialized = serde_json::to_string(&status).expect("serialize closed status");
                let debug = format!("{status:?}");
                if private_path.len() > 3 {
                    assert!(!serialized.contains(private_path.as_ref()));
                    assert!(!debug.contains(private_path.as_ref()));
                }
            }
        }
    }
}

#[cfg(target_os = "windows")]
#[allow(unused_imports)]
pub(crate) use native::{
    WindowsChildPath, WindowsFilesystemDetector, WindowsHeldDirectory, WindowsQualifiedParent,
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::filesystem_policy::{
        FilesystemStatusCode, PersistenceTarget, evaluate_filesystem,
    };
    use std::cell::{Cell, RefCell};

    struct ScriptedApi {
        directory: Result<bool, ApiFault>,
        initial_identity: Result<u64, ApiFault>,
        final_identity: Result<u64, ApiFault>,
        initial_volume: Result<VolumeSample, ApiFault>,
        final_volume: Result<VolumeSample, ApiFault>,
        remote: Result<RemoteProtocolCall, ApiFault>,
        open_volume: Result<(), ApiFault>,
        drive: Result<DriveTypeCall, ApiFault>,
        hotplug: Result<HotplugCall, ApiFault>,
        cloud: CloudFilesCall,
        identity_calls: Cell<u8>,
        volume_calls: Cell<u8>,
        calls: RefCell<Vec<&'static str>>,
    }

    impl ScriptedApi {
        fn positive() -> Self {
            let volume = VolumeSample {
                identity: 11,
                filesystem: WindowsFilesystem::Ntfs,
                read_only: false,
                persistent_acls: true,
            };
            Self {
                directory: Ok(true),
                initial_identity: Ok(7),
                final_identity: Ok(7),
                initial_volume: Ok(volume),
                final_volume: Ok(volume),
                remote: Ok(RemoteProtocolCall::NoRemoteProtocol),
                open_volume: Ok(()),
                drive: Ok(DriveTypeCall::Fixed),
                hotplug: Ok(HotplugCall {
                    media_removable: false,
                    media_hotplug: false,
                    device_hotplug: false,
                }),
                cloud: CloudFilesCall::NotUnderSyncRootSentinel,
                identity_calls: Cell::new(0),
                volume_calls: Cell::new(0),
                calls: RefCell::new(Vec::new()),
            }
        }

        fn called(&self, name: &'static str) {
            self.calls.borrow_mut().push(name);
        }
    }

    impl WindowsMetadataApi for ScriptedApi {
        type DirectoryIdentity = u64;
        type HeldDirectory = ();
        type HeldVolume = ();

        fn is_directory(&self, _target: &Self::HeldDirectory) -> Result<bool, ApiFault> {
            self.called("directory");
            self.directory
        }

        fn directory_identity(
            &self,
            _target: &Self::HeldDirectory,
        ) -> Result<Self::DirectoryIdentity, ApiFault> {
            self.called("identity");
            let call = self.identity_calls.get();
            self.identity_calls.set(call + 1);
            if call == 0 {
                self.initial_identity
            } else {
                self.final_identity
            }
        }

        fn volume_sample(&self, _target: &Self::HeldDirectory) -> Result<VolumeSample, ApiFault> {
            self.called("volume");
            let call = self.volume_calls.get();
            self.volume_calls.set(call + 1);
            if call == 0 {
                self.initial_volume
            } else {
                self.final_volume
            }
        }

        fn remote_protocol(
            &self,
            _target: &Self::HeldDirectory,
        ) -> Result<RemoteProtocolCall, ApiFault> {
            self.called("remote");
            self.remote
        }

        fn open_volume(&self, _target: &Self::HeldDirectory) -> Result<Self::HeldVolume, ApiFault> {
            self.called("open_volume");
            self.open_volume
        }

        fn drive_type(&self, _volume: &Self::HeldVolume) -> Result<DriveTypeCall, ApiFault> {
            self.called("drive");
            self.drive
        }

        fn hotplug(&self, _volume: &Self::HeldVolume) -> Result<HotplugCall, ApiFault> {
            self.called("hotplug");
            self.hotplug
        }

        fn cloud_files(&self, _target: &Self::HeldDirectory) -> CloudFilesCall {
            self.called("cloud");
            self.cloud
        }
    }

    #[test]
    fn only_the_exact_not_under_sync_root_hresult_is_favorable() {
        assert_eq!(
            cloud_files_call_from_hresult(0x8007_0186_u32 as i32),
            CloudFilesCall::NotUnderSyncRootSentinel
        );
        assert_eq!(
            cloud_files_call_from_hresult(0x8007_0195_u32 as i32),
            CloudFilesCall::Unknown
        );
        assert_eq!(
            cloud_files_call_from_hresult(0x8000_4005_u32 as i32),
            CloudFilesCall::Unknown
        );
    }

    #[test]
    fn one_held_target_with_every_positive_ntfs_observation_becomes_a_candidate() {
        let api = ScriptedApi::positive();
        let observation = inspect_with_api(&api, &(), PlatformRelease::new(10, 0, 22_631))
            .expect("closed positive observation");

        assert_eq!(
            observation,
            FilesystemObservation {
                platform: Platform::Windows,
                platform_release: PlatformRelease::new(10, 0, 22_631),
                family: FilesystemFamily::WindowsNtfs,
                writable: Ternary::Yes,
                local: Ternary::Yes,
                kernel_native: Ternary::Yes,
                internal_fixed: Ternary::Yes,
                os_managed_cloud_root: Ternary::No,
                access_controls_enforced: Ternary::Yes,
                identity_stable: Ternary::Yes,
                detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
            }
        );
        assert_eq!(
            api.calls.into_inner(),
            [
                "directory",
                "identity",
                "volume",
                "remote",
                "open_volume",
                "drive",
                "hotplug",
                "cloud",
                "identity",
                "volume",
            ]
        );
    }

    #[test]
    fn unavailable_remote_protocol_inspection_denies_the_target() {
        let mut api = ScriptedApi::positive();
        api.remote = Err(ApiFault::Unavailable);

        assert_eq!(
            inspect_with_api(&api, &(), PlatformRelease::new(10, 0, 22_631)),
            Err(DetectorFault::InspectionUnavailable)
        );
    }

    #[test]
    fn successful_remote_protocol_metadata_is_reduced_to_a_closed_result() {
        assert_eq!(
            remote_protocol_call_from_protocol(0),
            RemoteProtocolCall::NoRemoteProtocol
        );
        assert_eq!(
            remote_protocol_call_from_protocol(1),
            RemoteProtocolCall::Remote
        );
    }

    #[test]
    fn volume_guid_derivation_accepts_only_the_final_handle_guid_form() {
        let final_path: Vec<u16> = r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\private"
            .encode_utf16()
            .collect();
        let paths = volume_guid_paths(&final_path).expect("strict volume GUID form");

        assert_eq!(
            String::from_utf16(&paths.drive_root[..paths.drive_root.len() - 1]).unwrap(),
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\"
        );
        assert_eq!(
            String::from_utf16(&paths.device[..paths.device.len() - 1]).unwrap(),
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}"
        );

        for rejected in [
            r"C:\private",
            r"\\?\UNC\host\share\private",
            r"\\?\Volume{not-a-guid}\private",
            r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}",
        ] {
            assert!(
                volume_guid_paths(&rejected.encode_utf16().collect::<Vec<_>>()).is_none(),
                "non-GUID final form must deny"
            );
        }
    }

    #[test]
    fn every_required_api_boundary_failure_is_inspection_unavailable() {
        type FaultInjection = fn(&mut ScriptedApi);
        let cases: &[(&str, FaultInjection)] = &[
            ("directory", |api| {
                api.directory = Err(ApiFault::Unavailable)
            }),
            ("initial_identity", |api| {
                api.initial_identity = Err(ApiFault::Unavailable)
            }),
            ("initial_volume", |api| {
                api.initial_volume = Err(ApiFault::Unavailable)
            }),
            ("remote", |api| api.remote = Err(ApiFault::Unavailable)),
            ("open_volume", |api| {
                api.open_volume = Err(ApiFault::Unavailable)
            }),
            ("drive", |api| api.drive = Err(ApiFault::Unavailable)),
            ("hotplug", |api| api.hotplug = Err(ApiFault::Unavailable)),
            ("final_identity", |api| {
                api.final_identity = Err(ApiFault::Unavailable)
            }),
            ("final_volume", |api| {
                api.final_volume = Err(ApiFault::Unavailable)
            }),
        ];

        for (name, inject) in cases {
            let mut api = ScriptedApi::positive();
            inject(&mut api);
            assert_eq!(
                inspect_with_api(&api, &(), PlatformRelease::new(10, 0, 22_631)),
                Err(DetectorFault::InspectionUnavailable),
                "{name} failure must stay closed"
            );
        }

        let mut not_directory = ScriptedApi::positive();
        not_directory.directory = Ok(false);
        assert_eq!(
            inspect_with_api(&not_directory, &(), PlatformRelease::new(10, 0, 22_631)),
            Err(DetectorFault::TargetUnavailable)
        );
    }

    #[test]
    fn windows_result_classes_map_to_the_normative_closed_denials() {
        type ResultMutation = fn(&mut ScriptedApi);
        let cases: &[(&str, ResultMutation, FilesystemStatusCode)] = &[
            (
                "remote_protocol",
                |api| api.remote = Ok(RemoteProtocolCall::Remote),
                FilesystemStatusCode::Remote,
            ),
            (
                "remote_drive",
                |api| api.drive = Ok(DriveTypeCall::Remote),
                FilesystemStatusCode::Remote,
            ),
            (
                "removable_drive",
                |api| api.drive = Ok(DriveTypeCall::Removable),
                FilesystemStatusCode::RemovableOrHotplug,
            ),
            (
                "unknown_drive",
                |api| api.drive = Ok(DriveTypeCall::Other),
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "media_removable",
                |api| api.hotplug.as_mut().unwrap().media_removable = true,
                FilesystemStatusCode::RemovableOrHotplug,
            ),
            (
                "media_hotplug",
                |api| api.hotplug.as_mut().unwrap().media_hotplug = true,
                FilesystemStatusCode::RemovableOrHotplug,
            ),
            (
                "device_hotplug",
                |api| api.hotplug.as_mut().unwrap().device_hotplug = true,
                FilesystemStatusCode::RemovableOrHotplug,
            ),
            (
                "cloud_managed",
                |api| api.cloud = CloudFilesCall::Managed,
                FilesystemStatusCode::CloudManaged,
            ),
            (
                "cloud_unknown",
                |api| api.cloud = CloudFilesCall::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "read_only",
                |api| {
                    api.initial_volume.as_mut().unwrap().read_only = true;
                    api.final_volume.as_mut().unwrap().read_only = true;
                },
                FilesystemStatusCode::ReadOnly,
            ),
            (
                "acl_absent",
                |api| {
                    api.initial_volume.as_mut().unwrap().persistent_acls = false;
                    api.final_volume.as_mut().unwrap().persistent_acls = false;
                },
                FilesystemStatusCode::AccessControlUnproved,
            ),
            (
                "refs",
                |api| {
                    api.initial_volume.as_mut().unwrap().filesystem = WindowsFilesystem::Refs;
                    api.final_volume.as_mut().unwrap().filesystem = WindowsFilesystem::Refs;
                },
                FilesystemStatusCode::FilesystemUnproved,
            ),
            (
                "other_filesystem",
                |api| {
                    api.initial_volume.as_mut().unwrap().filesystem = WindowsFilesystem::Other;
                    api.final_volume.as_mut().unwrap().filesystem = WindowsFilesystem::Other;
                },
                FilesystemStatusCode::InspectionUnavailable,
            ),
        ];

        for (name, mutate, expected) in cases {
            let mut api = ScriptedApi::positive();
            mutate(&mut api);
            let inspection = inspect_with_api(&api, &(), PlatformRelease::new(10, 0, 22_631));
            assert_eq!(
                evaluate_filesystem(PersistenceTarget::Journal, inspection).code,
                *expected,
                "{name}"
            );
        }
    }

    #[test]
    fn final_identity_or_volume_disagreement_is_target_changed() {
        type RaceMutation = fn(&mut ScriptedApi);
        let races: &[(&str, RaceMutation)] = &[
            ("file_identity", |api| api.final_identity = Ok(8)),
            ("volume_identity", |api| {
                api.final_volume.as_mut().unwrap().identity = 12
            }),
            ("filesystem_family", |api| {
                api.final_volume.as_mut().unwrap().filesystem = WindowsFilesystem::Refs
            }),
            ("read_only_flag", |api| {
                api.final_volume.as_mut().unwrap().read_only = true
            }),
            ("acl_flag", |api| {
                api.final_volume.as_mut().unwrap().persistent_acls = false
            }),
        ];

        for (name, race) in races {
            let mut api = ScriptedApi::positive();
            race(&mut api);
            let inspection = inspect_with_api(&api, &(), PlatformRelease::new(10, 0, 22_631));
            assert_eq!(
                evaluate_filesystem(PersistenceTarget::Journal, inspection).code,
                FilesystemStatusCode::TargetChanged,
                "{name}"
            );
        }
    }

    #[test]
    fn filesystem_name_mapping_is_exact_and_closed() {
        assert_eq!(
            windows_filesystem_from_utf16(&[b'N' as u16, b'T' as u16, b'F' as u16, b'S' as u16, 0]),
            WindowsFilesystem::Ntfs
        );
        assert_eq!(
            windows_filesystem_from_utf16(&[b'R' as u16, b'e' as u16, b'F' as u16, b'S' as u16, 0]),
            WindowsFilesystem::Refs
        );
        for unknown in ["ntfs", "CSVFS", "FAT32", ""] {
            assert_eq!(
                windows_filesystem_from_utf16(&unknown.encode_utf16().collect::<Vec<_>>()),
                WindowsFilesystem::Other
            );
        }
    }

    #[test]
    fn drive_unknown_and_no_root_dir_are_closed_other_results() {
        const DRIVE_UNKNOWN_VALUE: u32 = 0;
        const DRIVE_NO_ROOT_DIR_VALUE: u32 = 1;
        const DRIVE_REMOVABLE_VALUE: u32 = 2;
        const DRIVE_FIXED_VALUE: u32 = 3;
        const DRIVE_REMOTE_VALUE: u32 = 4;

        for denied in [DRIVE_UNKNOWN_VALUE, DRIVE_NO_ROOT_DIR_VALUE, 5, 6] {
            assert_eq!(
                drive_type_call_from_native(
                    denied,
                    DRIVE_FIXED_VALUE,
                    DRIVE_REMOTE_VALUE,
                    DRIVE_REMOVABLE_VALUE,
                ),
                DriveTypeCall::Other
            );
        }
    }
}
