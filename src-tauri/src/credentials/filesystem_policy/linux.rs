//! Linux held-target filesystem detector.

use super::{
    DetectorFault, FILESYSTEM_DETECTOR_SCHEMA_VERSION, FilesystemDetector, FilesystemFamily,
    FilesystemObservation, Platform, PlatformRelease, Ternary,
};
use rustix::fd::OwnedFd;
use rustix::fs::{
    AtFlags, CWD, FileType, Mode, OFlags, StatxFlags, fstat, fstatfs, major, minor, openat, statx,
};
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

const MAX_MOUNTINFO_BYTES: usize = 1024 * 1024;
const MAX_MOUNTINFO_ROWS: usize = 4096;
const MAX_MOUNTINFO_ROW_BYTES: usize = 16 * 1024;
const MAX_MOUNTINFO_FIELDS: usize = 128;
const MAX_TOPOLOGY_NODES: usize = 32;
const MAX_TOPOLOGY_DEPTH: usize = 16;
const MAX_TOPOLOGY_ENTRIES: usize = 64;
const MAX_ATTRIBUTE_BYTES: usize = 4 * 1024;
const MAX_TOPOLOGY_IDENTITY_BYTES: usize = 256 * 1024;

const EXT_FAMILY_MAGIC: u64 = 0x0000_ef53;
const BTRFS_MAGIC: u64 = 0x9123_683e;
const XFS_MAGIC: u64 = 0x5846_5342;
const NFS_MAGIC: u64 = 0x0000_6969;
const CIFS_MAGIC: u64 = 0xff53_4d42;
const SMB2_MAGIC: u64 = 0xfe53_4d42;
const FUSE_MAGIC: u64 = 0x6573_5546;
const OVERLAY_MAGIC: u64 = 0x794c_7630;
const TMPFS_MAGIC: u64 = 0x0102_1994;
const V9FS_MAGIC: u64 = 0x0102_1997;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct DeviceId {
    major: u32,
    minor: u32,
}

impl DeviceId {
    const fn new(major: u32, minor: u32) -> Self {
        Self { major, minor }
    }
}

impl fmt::Debug for DeviceId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("DeviceId(<redacted>)")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MountFilesystem {
    Ext4,
    Btrfs,
    Xfs,
    Remote,
    Userspace,
    Overlay,
    Volatile,
    Unknown,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct MountRecord {
    mount_id: u64,
    device: DeviceId,
    filesystem: MountFilesystem,
    mount_writable: bool,
    superblock_writable: bool,
}

impl fmt::Debug for MountRecord {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MountRecord")
            .field("filesystem", &self.filesystem)
            .field("mount_writable", &self.mount_writable)
            .field("superblock_writable", &self.superblock_writable)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClosedFault {
    Missing,
    Ambiguous,
    Malformed,
    LimitExceeded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PhysicalAncestry {
    FixedPciNvme,
    FixedPciSata,
    FixedPciScsi,
    RemovablePci,
    Usb,
    FireWire,
    Thunderbolt,
    Mmc,
    Virtio,
    VmBus,
    Network,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeviceRemovability {
    Fixed,
    Removable,
    Unknown,
}

#[derive(Default)]
struct PhysicalAncestryEvidence {
    saw_fixed_pci: bool,
    saw_nvme: bool,
    saw_ata: bool,
    saw_scsi: bool,
}

impl PhysicalAncestryEvidence {
    fn observe(
        &mut self,
        subsystem: &[u8],
        pci_removability: Option<DeviceRemovability>,
    ) -> Option<PhysicalAncestry> {
        match subsystem {
            b"usb" => Some(PhysicalAncestry::Usb),
            b"firewire" | b"ieee1394" => Some(PhysicalAncestry::FireWire),
            b"thunderbolt" => Some(PhysicalAncestry::Thunderbolt),
            b"mmc" | b"mmc_host" => Some(PhysicalAncestry::Mmc),
            b"virtio" | b"xen" => Some(PhysicalAncestry::Virtio),
            b"vmbus" => Some(PhysicalAncestry::VmBus),
            b"net" | b"nbd" => Some(PhysicalAncestry::Network),
            b"pci" => match pci_removability {
                Some(DeviceRemovability::Fixed) => {
                    self.saw_fixed_pci = true;
                    None
                }
                Some(DeviceRemovability::Removable) => Some(PhysicalAncestry::RemovablePci),
                Some(DeviceRemovability::Unknown) | None => Some(PhysicalAncestry::Unknown),
            },
            b"nvme" => {
                self.saw_nvme = true;
                None
            }
            b"ata" => {
                self.saw_ata = true;
                None
            }
            b"scsi" | b"scsi_device" | b"scsi_host" | b"scsi_target" => {
                self.saw_scsi = true;
                None
            }
            b"block" => None,
            _ => Some(PhysicalAncestry::Unknown),
        }
    }

    fn finish(self) -> PhysicalAncestry {
        match (
            self.saw_fixed_pci,
            self.saw_nvme,
            self.saw_ata,
            self.saw_scsi,
        ) {
            (true, true, false, false) => PhysicalAncestry::FixedPciNvme,
            (true, false, true, _) => PhysicalAncestry::FixedPciSata,
            (true, false, false, true) => PhysicalAncestry::FixedPciScsi,
            _ => PhysicalAncestry::Unknown,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopologyNodeKind {
    Physical(PhysicalAncestry),
    Stacked,
    Loop,
}

#[derive(Clone, PartialEq, Eq)]
struct PrivateKernelObjectIdentity {
    path: Box<[u8]>,
    stat_device: u64,
    inode: u64,
    ctime_seconds: i64,
    ctime_nanoseconds: i64,
}

impl PrivateKernelObjectIdentity {
    fn byte_len(&self) -> usize {
        self.path.len()
    }
}

impl fmt::Debug for PrivateKernelObjectIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivateKernelObjectIdentity(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PrivateTopologyIdentity {
    node: PrivateKernelObjectIdentity,
    physical_ancestors: Vec<PrivateKernelObjectIdentity>,
}

impl PrivateTopologyIdentity {
    fn byte_len(&self) -> Result<usize, ClosedFault> {
        if self.physical_ancestors.len() > MAX_TOPOLOGY_DEPTH {
            return Err(ClosedFault::LimitExceeded);
        }
        std::iter::once(&self.node)
            .chain(self.physical_ancestors.iter())
            .try_fold(0usize, |total, identity| {
                if identity.path.is_empty() {
                    return Err(ClosedFault::Malformed);
                }
                if identity.byte_len() > MAX_ATTRIBUTE_BYTES {
                    return Err(ClosedFault::LimitExceeded);
                }
                total
                    .checked_add(identity.byte_len())
                    .ok_or(ClosedFault::LimitExceeded)
            })
    }
}

fn add_topology_identity_bytes(
    consumed: usize,
    identity: &PrivateTopologyIdentity,
) -> Result<usize, ClosedFault> {
    let total = consumed
        .checked_add(identity.byte_len()?)
        .ok_or(ClosedFault::LimitExceeded)?;
    if total > MAX_TOPOLOGY_IDENTITY_BYTES {
        return Err(ClosedFault::LimitExceeded);
    }
    Ok(total)
}

impl fmt::Debug for PrivateTopologyIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PrivateTopologyIdentity(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq)]
struct PhysicalAncestrySnapshot {
    closed: PhysicalAncestry,
    ancestors: Vec<PrivateKernelObjectIdentity>,
}

#[derive(Clone, PartialEq, Eq)]
struct TopologyNodeStabilitySample {
    device: DeviceId,
    removable: bool,
    backers: Vec<DeviceId>,
    is_loop: bool,
    node_identity: PrivateKernelObjectIdentity,
    physical: Option<PhysicalAncestrySnapshot>,
}

fn ensure_stable_node_samples(
    initial: &TopologyNodeStabilitySample,
    after: &TopologyNodeStabilitySample,
) -> Result<(), ClosedFault> {
    if initial == after {
        Ok(())
    } else {
        Err(ClosedFault::Ambiguous)
    }
}

#[derive(Clone, PartialEq, Eq)]
struct TopologyNode {
    device: DeviceId,
    backers: Vec<DeviceId>,
    removable: Option<bool>,
    kind: TopologyNodeKind,
    private_identity: PrivateTopologyIdentity,
}

#[derive(Clone, PartialEq, Eq)]
struct TopologyGraph {
    root: DeviceId,
    nodes: Vec<TopologyNode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TopologyDisposition {
    Fixed,
    Denied,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct HandleIdentity {
    device: DeviceId,
    inode: u64,
    mount_id: u64,
}

impl fmt::Debug for HandleIdentity {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("HandleIdentity(<redacted>)")
    }
}

#[derive(Clone, PartialEq, Eq)]
enum TopologyEvidence {
    NotRequired,
    Samples {
        initial: TopologyGraph,
        after: TopologyGraph,
    },
}

struct InspectionFacts {
    platform_release: PlatformRelease,
    initial_identity: HandleIdentity,
    final_identity: HandleIdentity,
    initial_magic: u64,
    final_magic: u64,
    initial_mount: MountRecord,
    final_mount: MountRecord,
    topology: TopologyEvidence,
}

fn classify_facts(facts: InspectionFacts) -> Result<FilesystemObservation, DetectorFault> {
    if !magic_matches(facts.initial_mount.filesystem, facts.initial_magic)
        || !magic_matches(facts.final_mount.filesystem, facts.final_magic)
    {
        return Err(DetectorFault::InspectionUnavailable);
    }

    let (topology, topology_stable) = match &facts.topology {
        TopologyEvidence::NotRequired => (None, true),
        TopologyEvidence::Samples { initial, after } => {
            let initial_disposition =
                classify_topology(initial).map_err(|_| DetectorFault::InspectionUnavailable)?;
            let final_disposition =
                classify_topology(after).map_err(|_| DetectorFault::InspectionUnavailable)?;
            (
                Some(initial_disposition),
                initial == after && initial_disposition == final_disposition,
            )
        }
    };

    let mount_identity_matches = facts.initial_mount.mount_id == facts.initial_identity.mount_id
        && facts.initial_mount.device == facts.initial_identity.device
        && facts.final_mount.mount_id == facts.final_identity.mount_id
        && facts.final_mount.device == facts.final_identity.device;
    let identity_stable = mount_identity_matches
        && topology_stable
        && facts.initial_identity == facts.final_identity
        && facts.initial_mount == facts.final_mount
        && facts.initial_magic == facts.final_magic;

    let writable = if facts.initial_mount.mount_writable && facts.initial_mount.superblock_writable
    {
        Ternary::Yes
    } else {
        Ternary::No
    };
    let (family, local, kernel_native, internal_fixed, access_controls_enforced) =
        match facts.initial_mount.filesystem {
            MountFilesystem::Ext4 | MountFilesystem::Btrfs | MountFilesystem::Xfs => {
                let family = match facts.initial_mount.filesystem {
                    MountFilesystem::Ext4 => FilesystemFamily::LinuxExt4,
                    MountFilesystem::Btrfs => FilesystemFamily::LinuxBtrfs,
                    MountFilesystem::Xfs => FilesystemFamily::LinuxXfs,
                    _ => unreachable!(),
                };
                let internal_fixed = if writable == Ternary::No {
                    Ternary::No
                } else {
                    match topology.ok_or(DetectorFault::InspectionUnavailable)? {
                        TopologyDisposition::Fixed => Ternary::Yes,
                        TopologyDisposition::Denied => Ternary::No,
                    }
                };
                (
                    family,
                    Ternary::Yes,
                    Ternary::Yes,
                    internal_fixed,
                    Ternary::Yes,
                )
            }
            MountFilesystem::Remote => (
                FilesystemFamily::Other,
                Ternary::No,
                Ternary::Yes,
                Ternary::No,
                Ternary::No,
            ),
            MountFilesystem::Userspace | MountFilesystem::Overlay => (
                FilesystemFamily::Other,
                Ternary::Yes,
                Ternary::No,
                Ternary::No,
                Ternary::No,
            ),
            MountFilesystem::Volatile => (
                FilesystemFamily::Other,
                Ternary::Yes,
                Ternary::Yes,
                Ternary::No,
                Ternary::Yes,
            ),
            MountFilesystem::Unknown => return Err(DetectorFault::InspectionUnavailable),
        };

    Ok(FilesystemObservation {
        platform: Platform::Linux,
        platform_release: facts.platform_release,
        family,
        writable,
        local,
        kernel_native,
        internal_fixed,
        // Linux cloud-boundary-v1: no kernel-visible denied storage/provider
        // layer was observed. This says nothing about ordinary user-space
        // copiers, backup tools, indexers, or endpoint-security software.
        os_managed_cloud_root: Ternary::No,
        access_controls_enforced,
        identity_stable: if identity_stable {
            Ternary::Yes
        } else {
            Ternary::No
        },
        detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
    })
}

fn magic_matches(filesystem: MountFilesystem, magic: u64) -> bool {
    match filesystem {
        MountFilesystem::Ext4 => magic == EXT_FAMILY_MAGIC,
        MountFilesystem::Btrfs => magic == BTRFS_MAGIC,
        MountFilesystem::Xfs => magic == XFS_MAGIC,
        MountFilesystem::Remote => {
            matches!(
                magic,
                NFS_MAGIC | CIFS_MAGIC | SMB2_MAGIC | FUSE_MAGIC | V9FS_MAGIC
            )
        }
        MountFilesystem::Userspace => magic == FUSE_MAGIC,
        MountFilesystem::Overlay => magic == OVERLAY_MAGIC,
        MountFilesystem::Volatile => magic == TMPFS_MAGIC,
        MountFilesystem::Unknown => false,
    }
}

pub(crate) struct LinuxHeldTarget {
    directory: OwnedFd,
}

impl LinuxHeldTarget {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self, DetectorFault> {
        let directory = openat(
            CWD,
            path.as_ref(),
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(|_| DetectorFault::TargetUnavailable)?;
        let metadata = fstat(&directory).map_err(|_| DetectorFault::TargetUnavailable)?;
        if FileType::from_raw_mode(metadata.st_mode) != FileType::Directory {
            return Err(DetectorFault::TargetUnavailable);
        }
        Ok(Self { directory })
    }
}

pub(crate) struct LinuxFilesystemDetector;

impl FilesystemDetector for LinuxFilesystemDetector {
    type HeldTarget = LinuxHeldTarget;

    fn inspect(&self, target: &Self::HeldTarget) -> Result<FilesystemObservation, DetectorFault> {
        inspect_held_target(target)
    }
}

fn inspect_held_target(target: &LinuxHeldTarget) -> Result<FilesystemObservation, DetectorFault> {
    let initial_identity = sample_identity(&target.directory)?;
    let initial_magic = sample_filesystem_magic(&target.directory)?;
    let initial_mount = read_mount_record(initial_identity.mount_id)?;

    let topology = if topology_required(initial_mount) {
        let initial = snapshot_topology(initial_identity.device)
            .map_err(|_| DetectorFault::InspectionUnavailable)?;
        let platform_release = read_platform_release()?;
        let after = snapshot_topology(initial_identity.device)
            .map_err(|_| DetectorFault::InspectionUnavailable)?;
        let final_mount = read_mount_record(initial_identity.mount_id)?;
        let final_magic = sample_filesystem_magic(&target.directory)?;
        let final_identity = sample_identity(&target.directory)?;
        return classify_facts(InspectionFacts {
            platform_release,
            initial_identity,
            final_identity,
            initial_magic,
            final_magic,
            initial_mount,
            final_mount,
            topology: TopologyEvidence::Samples { initial, after },
        });
    } else {
        TopologyEvidence::NotRequired
    };

    let platform_release = read_platform_release()?;
    let final_mount = read_mount_record(initial_identity.mount_id)?;
    let final_magic = sample_filesystem_magic(&target.directory)?;
    let final_identity = sample_identity(&target.directory)?;
    classify_facts(InspectionFacts {
        platform_release,
        initial_identity,
        final_identity,
        initial_magic,
        final_magic,
        initial_mount,
        final_mount,
        topology,
    })
}

fn topology_required(mount: MountRecord) -> bool {
    mount.mount_writable
        && mount.superblock_writable
        && matches!(
            mount.filesystem,
            MountFilesystem::Ext4 | MountFilesystem::Btrfs | MountFilesystem::Xfs
        )
}

fn sample_identity(directory: &OwnedFd) -> Result<HandleIdentity, DetectorFault> {
    let metadata = fstat(directory).map_err(|_| DetectorFault::InspectionUnavailable)?;
    let extended = statx(
        directory,
        c"",
        AtFlags::EMPTY_PATH | AtFlags::NO_AUTOMOUNT,
        StatxFlags::BASIC_STATS | StatxFlags::MNT_ID,
    )
    .map_err(|_| DetectorFault::InspectionUnavailable)?;
    let returned = StatxFlags::from_bits_retain(extended.stx_mask);
    if !returned.contains(StatxFlags::BASIC_STATS | StatxFlags::MNT_ID)
        || FileType::from_raw_mode(metadata.st_mode) != FileType::Directory
        || FileType::from_raw_mode(extended.stx_mode.into()) != FileType::Directory
        || extended.stx_mnt_id == 0
    {
        return Err(DetectorFault::InspectionUnavailable);
    }

    let stat_device = DeviceId::new(major(metadata.st_dev), minor(metadata.st_dev));
    let statx_device = DeviceId::new(extended.stx_dev_major, extended.stx_dev_minor);
    let stat_inode = metadata.st_ino;
    if stat_device != statx_device || stat_inode != extended.stx_ino {
        return Err(DetectorFault::InspectionUnavailable);
    }
    Ok(HandleIdentity {
        device: stat_device,
        inode: stat_inode,
        mount_id: extended.stx_mnt_id,
    })
}

fn sample_filesystem_magic(directory: &OwnedFd) -> Result<u64, DetectorFault> {
    let metadata = fstatfs(directory).map_err(|_| DetectorFault::InspectionUnavailable)?;
    Ok(normalize_filesystem_magic(metadata.f_type))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn normalize_filesystem_magic(raw: rustix::fs::FsWord) -> u64 {
    u64::from(raw as u32)
}

fn read_mount_record(mount_id: u64) -> Result<MountRecord, DetectorFault> {
    let input = read_bounded_file(Path::new("/proc/self/mountinfo"), MAX_MOUNTINFO_BYTES)
        .map_err(|_| DetectorFault::InspectionUnavailable)?;
    parse_mountinfo(&input, mount_id).map_err(|_| DetectorFault::InspectionUnavailable)
}

fn read_platform_release() -> Result<PlatformRelease, DetectorFault> {
    let input = read_bounded_file(Path::new("/proc/sys/kernel/osrelease"), MAX_ATTRIBUTE_BYTES)
        .map_err(|_| DetectorFault::InspectionUnavailable)?;
    parse_platform_release(&input).map_err(|_| DetectorFault::InspectionUnavailable)
}

fn parse_platform_release(input: &[u8]) -> Result<PlatformRelease, ClosedFault> {
    if input.is_empty() || input.len() > MAX_ATTRIBUTE_BYTES {
        return Err(if input.is_empty() {
            ClosedFault::Malformed
        } else {
            ClosedFault::LimitExceeded
        });
    }
    let release = input.strip_suffix(b"\n").unwrap_or(input);
    let mut components = release.split(|byte| *byte == b'.');
    let major = components.next().ok_or(ClosedFault::Malformed)?;
    let minor = components.next().ok_or(ClosedFault::Malformed)?;
    let patch = components.next().ok_or(ClosedFault::Malformed)?;
    let patch_digits: Vec<u8> = patch
        .iter()
        .copied()
        .take_while(u8::is_ascii_digit)
        .collect();
    if patch_digits.is_empty() {
        return Err(ClosedFault::Malformed);
    }
    Ok(PlatformRelease::new(
        u16::try_from(parse_ascii_u64(major)?).map_err(|_| ClosedFault::Malformed)?,
        u16::try_from(parse_ascii_u64(minor)?).map_err(|_| ClosedFault::Malformed)?,
        u32::try_from(parse_ascii_u64(&patch_digits)?).map_err(|_| ClosedFault::Malformed)?,
    ))
}

fn snapshot_topology(root: DeviceId) -> Result<TopologyGraph, ClosedFault> {
    let mut pending = vec![(root, 0usize)];
    let mut nodes = Vec::new();
    let mut identity_bytes = 0usize;
    while let Some((device, depth)) = pending.pop() {
        if depth > MAX_TOPOLOGY_DEPTH {
            return Err(ClosedFault::LimitExceeded);
        }
        if nodes
            .iter()
            .any(|node: &TopologyNode| node.device == device)
        {
            continue;
        }
        if nodes.len() >= MAX_TOPOLOGY_NODES {
            return Err(ClosedFault::LimitExceeded);
        }
        let node = read_topology_node(device)?;
        identity_bytes = add_topology_identity_bytes(identity_bytes, &node.private_identity)?;
        for backer in node.backers.iter().rev() {
            pending.push((*backer, depth + 1));
        }
        nodes.push(node);
    }
    nodes.sort_by_key(|node| node.device);
    Ok(TopologyGraph { root, nodes })
}

fn read_topology_node(device: DeviceId) -> Result<TopologyNode, ClosedFault> {
    let link = PathBuf::from(format!("/sys/dev/block/{}:{}", device.major, device.minor));
    let initial_path = resolve_topology_node(&link)?;
    let initial = read_topology_node_sample(&initial_path)?;
    if initial.device != device {
        return Err(ClosedFault::Ambiguous);
    }
    let final_path = resolve_topology_node(&link)?;
    let after = read_topology_node_sample(&final_path)?;
    if after.device != device {
        return Err(ClosedFault::Ambiguous);
    }
    ensure_stable_node_samples(&initial, &after)?;

    let TopologyNodeStabilitySample {
        device,
        removable,
        backers,
        is_loop,
        node_identity,
        physical,
    } = initial;
    let (kind, physical_ancestors) = match (is_loop, backers.is_empty(), physical) {
        (true, _, None) => (TopologyNodeKind::Loop, Vec::new()),
        (false, false, None) => (TopologyNodeKind::Stacked, Vec::new()),
        (false, true, Some(physical)) => (
            TopologyNodeKind::Physical(physical.closed),
            physical.ancestors,
        ),
        _ => return Err(ClosedFault::Malformed),
    };
    Ok(TopologyNode {
        device,
        backers,
        removable: Some(removable),
        kind,
        private_identity: PrivateTopologyIdentity {
            node: node_identity,
            physical_ancestors,
        },
    })
}

fn resolve_topology_node(link: &Path) -> Result<PathBuf, ClosedFault> {
    let node_path = fs::canonicalize(link).map_err(|_| ClosedFault::Missing)?;
    if node_path.as_os_str().as_bytes().len() > MAX_ATTRIBUTE_BYTES
        || !node_path.starts_with("/sys/devices")
    {
        return Err(ClosedFault::LimitExceeded);
    }
    Ok(node_path)
}

fn read_topology_node_sample(node_path: &Path) -> Result<TopologyNodeStabilitySample, ClosedFault> {
    let node_identity = read_private_kernel_object_identity(node_path)?;
    let device = read_device_attribute(&node_path.join("dev"))?;
    let removable = read_bool_attribute(&node_path.join("removable"))?;
    let mut backers = read_backers(&node_path.join("slaves"))?;
    backers.sort_unstable();
    if backers.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ClosedFault::Ambiguous);
    }
    let is_loop = path_exists_as_directory(&node_path.join("loop"))?;
    let physical = if !is_loop && backers.is_empty() {
        Some(classify_physical_ancestry(node_path)?)
    } else {
        None
    };
    Ok(TopologyNodeStabilitySample {
        device,
        removable,
        backers,
        is_loop,
        node_identity,
        physical,
    })
}

fn read_backers(directory: &Path) -> Result<Vec<DeviceId>, ClosedFault> {
    let entries = fs::read_dir(directory).map_err(|_| ClosedFault::Missing)?;
    let mut backers = Vec::new();
    for entry in entries {
        if backers.len() >= MAX_TOPOLOGY_ENTRIES {
            return Err(ClosedFault::LimitExceeded);
        }
        let entry = entry.map_err(|_| ClosedFault::Missing)?;
        let file_type = entry.file_type().map_err(|_| ClosedFault::Missing)?;
        if !file_type.is_symlink() && !file_type.is_dir() {
            return Err(ClosedFault::Malformed);
        }
        backers.push(read_device_attribute(&entry.path().join("dev"))?);
    }
    Ok(backers)
}

fn classify_physical_ancestry(node_path: &Path) -> Result<PhysicalAncestrySnapshot, ClosedFault> {
    let mut evidence = PhysicalAncestryEvidence::default();
    let mut ancestors = Vec::new();
    let mut current = Some(node_path);
    for depth in 0..=MAX_TOPOLOGY_DEPTH {
        let path = current.ok_or(ClosedFault::Missing)?;
        if path != node_path {
            if ancestors.len() >= MAX_TOPOLOGY_DEPTH {
                return Err(ClosedFault::LimitExceeded);
            }
            ancestors.push(read_private_kernel_object_identity(path)?);
        }
        if let Some(subsystem) = read_optional_subsystem(path)? {
            let removability = if subsystem == b"pci" {
                read_optional_device_removability(path)?
            } else {
                None
            };
            if let Some(terminal) = evidence.observe(&subsystem, removability) {
                return Ok(PhysicalAncestrySnapshot {
                    closed: terminal,
                    ancestors,
                });
            }
        }
        if path == Path::new("/sys/devices") {
            return Ok(PhysicalAncestrySnapshot {
                closed: evidence.finish(),
                ancestors,
            });
        }
        if depth == MAX_TOPOLOGY_DEPTH {
            return Err(ClosedFault::LimitExceeded);
        }
        current = path.parent();
    }
    Err(ClosedFault::LimitExceeded)
}

fn read_private_kernel_object_identity(
    path: &Path,
) -> Result<PrivateKernelObjectIdentity, ClosedFault> {
    let path_bytes = path.as_os_str().as_bytes();
    if path_bytes.is_empty() || path_bytes.len() > MAX_ATTRIBUTE_BYTES {
        return Err(if path_bytes.is_empty() {
            ClosedFault::Malformed
        } else {
            ClosedFault::LimitExceeded
        });
    }
    let metadata = fs::metadata(path).map_err(|_| ClosedFault::Missing)?;
    if !metadata.is_dir() {
        return Err(ClosedFault::Malformed);
    }
    Ok(PrivateKernelObjectIdentity {
        path: path_bytes.into(),
        stat_device: metadata.dev(),
        inode: metadata.ino(),
        ctime_seconds: metadata.ctime(),
        ctime_nanoseconds: metadata.ctime_nsec(),
    })
}

fn read_optional_subsystem(path: &Path) -> Result<Option<Vec<u8>>, ClosedFault> {
    let target = match fs::read_link(path.join("subsystem")) {
        Ok(target) => target,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            if path_exists(&path.join("uevent"))? {
                return Err(ClosedFault::Missing);
            }
            return Ok(None);
        }
        Err(_) => return Err(ClosedFault::Missing),
    };
    if target.as_os_str().as_bytes().len() > MAX_ATTRIBUTE_BYTES {
        return Err(ClosedFault::LimitExceeded);
    }
    let name = target.file_name().ok_or(ClosedFault::Malformed)?;
    Ok(Some(name.as_bytes().to_vec()))
}

fn path_exists(path: &Path) -> Result<bool, ClosedFault> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ClosedFault::Missing),
    }
}

fn read_optional_device_removability(
    device_path: &Path,
) -> Result<Option<DeviceRemovability>, ClosedFault> {
    let attribute = device_path.join("removable");
    if !path_exists(&attribute)? {
        return Ok(None);
    }
    let input = read_bounded_file(&attribute, MAX_ATTRIBUTE_BYTES)?;
    parse_device_removability(&input).map(Some)
}

fn parse_device_removability(input: &[u8]) -> Result<DeviceRemovability, ClosedFault> {
    if input.is_empty() || input.len() > MAX_ATTRIBUTE_BYTES {
        return Err(if input.is_empty() {
            ClosedFault::Malformed
        } else {
            ClosedFault::LimitExceeded
        });
    }
    match input.strip_suffix(b"\n").unwrap_or(input) {
        b"fixed" => Ok(DeviceRemovability::Fixed),
        b"removable" => Ok(DeviceRemovability::Removable),
        b"unknown" => Ok(DeviceRemovability::Unknown),
        _ => Err(ClosedFault::Malformed),
    }
}

fn path_exists_as_directory(path: &Path) -> Result<bool, ClosedFault> {
    match fs::metadata(path) {
        Ok(metadata) if metadata.is_dir() => Ok(true),
        Ok(_) => Err(ClosedFault::Malformed),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(ClosedFault::Missing),
    }
}

fn read_device_attribute(path: &Path) -> Result<DeviceId, ClosedFault> {
    let input = read_bounded_file(path, MAX_ATTRIBUTE_BYTES)?;
    let value = input.strip_suffix(b"\n").unwrap_or(&input);
    parse_device(value)
}

fn read_bool_attribute(path: &Path) -> Result<bool, ClosedFault> {
    let input = read_bounded_file(path, MAX_ATTRIBUTE_BYTES)?;
    match input.strip_suffix(b"\n").unwrap_or(&input) {
        b"0" => Ok(false),
        b"1" => Ok(true),
        _ => Err(ClosedFault::Malformed),
    }
}

fn read_bounded_file(path: &Path, limit: usize) -> Result<Vec<u8>, ClosedFault> {
    let file = File::open(path).map_err(|_| ClosedFault::Missing)?;
    let read_limit = u64::try_from(limit)
        .ok()
        .and_then(|limit| limit.checked_add(1))
        .ok_or(ClosedFault::LimitExceeded)?;
    let mut reader = file.take(read_limit);
    let mut input = Vec::new();
    reader
        .read_to_end(&mut input)
        .map_err(|_| ClosedFault::Missing)?;
    if input.len() > limit {
        return Err(ClosedFault::LimitExceeded);
    }
    Ok(input)
}

fn classify_topology(graph: &TopologyGraph) -> Result<TopologyDisposition, ClosedFault> {
    if graph.nodes.is_empty() {
        return Err(ClosedFault::Missing);
    }
    if graph.nodes.len() > MAX_TOPOLOGY_NODES {
        return Err(ClosedFault::LimitExceeded);
    }
    let mut identity_bytes = 0usize;
    for (index, node) in graph.nodes.iter().enumerate() {
        if node.backers.len() > MAX_TOPOLOGY_ENTRIES {
            return Err(ClosedFault::LimitExceeded);
        }
        identity_bytes = add_topology_identity_bytes(identity_bytes, &node.private_identity)?;
        if graph.nodes[..index]
            .iter()
            .any(|candidate| candidate.device == node.device)
        {
            return Err(ClosedFault::Ambiguous);
        }
    }

    let mut active = Vec::new();
    let mut visited = Vec::new();
    let disposition = classify_topology_node(graph, graph.root, 0, &mut active, &mut visited)?;
    if visited.len() != graph.nodes.len() {
        return Err(ClosedFault::Ambiguous);
    }
    Ok(disposition)
}

fn classify_topology_node(
    graph: &TopologyGraph,
    device: DeviceId,
    depth: usize,
    active: &mut Vec<DeviceId>,
    visited: &mut Vec<DeviceId>,
) -> Result<TopologyDisposition, ClosedFault> {
    if depth > MAX_TOPOLOGY_DEPTH {
        return Err(ClosedFault::LimitExceeded);
    }
    if active.contains(&device) {
        return Err(ClosedFault::Ambiguous);
    }
    if visited.contains(&device) {
        return Err(ClosedFault::Ambiguous);
    }
    let node = graph
        .nodes
        .iter()
        .find(|node| node.device == device)
        .ok_or(ClosedFault::Missing)?;
    let removable = node.removable.ok_or(ClosedFault::Missing)?;
    if removable {
        visited.push(device);
        return Ok(TopologyDisposition::Denied);
    }

    active.push(device);
    let disposition = match node.kind {
        TopologyNodeKind::Loop => {
            if !node.backers.is_empty() {
                return Err(ClosedFault::Ambiguous);
            }
            TopologyDisposition::Denied
        }
        TopologyNodeKind::Physical(ancestry) => {
            if !node.backers.is_empty() {
                return Err(ClosedFault::Ambiguous);
            }
            match ancestry {
                PhysicalAncestry::FixedPciNvme
                | PhysicalAncestry::FixedPciSata
                | PhysicalAncestry::FixedPciScsi => TopologyDisposition::Fixed,
                PhysicalAncestry::RemovablePci
                | PhysicalAncestry::Usb
                | PhysicalAncestry::FireWire
                | PhysicalAncestry::Thunderbolt
                | PhysicalAncestry::Mmc
                | PhysicalAncestry::Virtio
                | PhysicalAncestry::VmBus
                | PhysicalAncestry::Network => TopologyDisposition::Denied,
                PhysicalAncestry::Unknown => return Err(ClosedFault::Missing),
            }
        }
        TopologyNodeKind::Stacked => {
            let [backer] = node.backers.as_slice() else {
                return Err(if node.backers.is_empty() {
                    ClosedFault::Missing
                } else {
                    ClosedFault::Ambiguous
                });
            };
            classify_topology_node(graph, *backer, depth + 1, active, visited)?
        }
    };
    active.pop();
    visited.push(device);
    Ok(disposition)
}

fn parse_mountinfo(input: &[u8], expected_mount_id: u64) -> Result<MountRecord, ClosedFault> {
    if input.len() > MAX_MOUNTINFO_BYTES {
        return Err(ClosedFault::LimitExceeded);
    }

    let mut rows = 0usize;
    let mut selected = None;
    for row in input.split(|byte| *byte == b'\n') {
        if row.is_empty() {
            continue;
        }
        rows = rows.checked_add(1).ok_or(ClosedFault::LimitExceeded)?;
        if rows > MAX_MOUNTINFO_ROWS || row.len() > MAX_MOUNTINFO_ROW_BYTES {
            return Err(ClosedFault::LimitExceeded);
        }

        let fields: Vec<&[u8]> = row
            .split(|byte| byte.is_ascii_whitespace())
            .filter(|field| !field.is_empty())
            .take(MAX_MOUNTINFO_FIELDS + 1)
            .collect();
        if fields.len() > MAX_MOUNTINFO_FIELDS {
            return Err(ClosedFault::LimitExceeded);
        }
        if fields.len() < 10 {
            return Err(ClosedFault::Malformed);
        }

        let separators = fields
            .iter()
            .enumerate()
            .filter(|(_, field)| **field == b"-")
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let [separator] = separators.as_slice() else {
            return Err(ClosedFault::Malformed);
        };
        if *separator < 6 || *separator + 4 != fields.len() {
            return Err(ClosedFault::Malformed);
        }

        let mount_id = parse_ascii_u64(fields[0])?;
        let parent_id = parse_ascii_u64(fields[1])?;
        if mount_id == 0 || parent_id == 0 {
            return Err(ClosedFault::Malformed);
        }
        let device = parse_device(fields[2])?;
        let mount_writable = parse_writability(fields[5])?;
        let superblock_writable = parse_writability(fields[*separator + 3])?;
        let filesystem = classify_mount_filesystem(fields[*separator + 1]);

        if mount_id == expected_mount_id {
            let record = MountRecord {
                mount_id,
                device,
                filesystem,
                mount_writable,
                superblock_writable,
            };
            if selected.replace(record).is_some() {
                return Err(ClosedFault::Ambiguous);
            }
        }
    }

    selected.ok_or(ClosedFault::Missing)
}

fn parse_ascii_u64(field: &[u8]) -> Result<u64, ClosedFault> {
    if field.is_empty() || !field.iter().all(u8::is_ascii_digit) {
        return Err(ClosedFault::Malformed);
    }
    field.iter().try_fold(0u64, |value, byte| {
        value
            .checked_mul(10)
            .and_then(|value| value.checked_add(u64::from(byte - b'0')))
            .ok_or(ClosedFault::Malformed)
    })
}

fn parse_device(field: &[u8]) -> Result<DeviceId, ClosedFault> {
    let mut parts = field.split(|byte| *byte == b':');
    let major = parts.next().ok_or(ClosedFault::Malformed)?;
    let minor = parts.next().ok_or(ClosedFault::Malformed)?;
    if parts.next().is_some() {
        return Err(ClosedFault::Malformed);
    }
    let major = u32::try_from(parse_ascii_u64(major)?).map_err(|_| ClosedFault::Malformed)?;
    let minor = u32::try_from(parse_ascii_u64(minor)?).map_err(|_| ClosedFault::Malformed)?;
    Ok(DeviceId::new(major, minor))
}

fn parse_writability(options: &[u8]) -> Result<bool, ClosedFault> {
    let mut writable = false;
    let mut read_only = false;
    for option in options.split(|byte| *byte == b',') {
        writable |= option == b"rw";
        read_only |= option == b"ro";
    }
    match (writable, read_only) {
        (true, false) => Ok(true),
        (false, true) => Ok(false),
        _ => Err(ClosedFault::Malformed),
    }
}

fn classify_mount_filesystem(filesystem: &[u8]) -> MountFilesystem {
    match filesystem {
        b"ext4" => MountFilesystem::Ext4,
        b"btrfs" => MountFilesystem::Btrfs,
        b"xfs" => MountFilesystem::Xfs,
        b"nfs" | b"nfs4" | b"cifs" | b"smb3" | b"9p" | b"virtiofs" => MountFilesystem::Remote,
        b"fuse" | b"fuseblk" => MountFilesystem::Userspace,
        value if value.starts_with(b"fuse.") => MountFilesystem::Userspace,
        b"overlay" => MountFilesystem::Overlay,
        b"tmpfs" => MountFilesystem::Volatile,
        _ => MountFilesystem::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credentials::filesystem_policy::{
        FilesystemStatusCode, PersistenceTarget, evaluate_filesystem,
    };

    const MOUNT_ID: u64 = 41;
    const PRIVATE_ROOT_CANARY: &str = "private-root-canary";
    const PRIVATE_TARGET_CANARY: &str = "private-target-canary";
    const PRIVATE_SOURCE_CANARY: &str = "private-source-canary";

    fn mountinfo_row(
        mount_id: u64,
        device: &str,
        mount_options: &str,
        optional_fields: &str,
        filesystem: &str,
        super_options: &str,
    ) -> String {
        format!(
            "{mount_id} 1 {device} {PRIVATE_ROOT_CANARY} {PRIVATE_TARGET_CANARY} {mount_options} {optional_fields}- {filesystem} {PRIVATE_SOURCE_CANARY} {super_options}\n"
        )
    }

    #[test]
    fn mountinfo_selects_exact_mount_id_without_retaining_private_fields() {
        let input = format!(
            "{}{}",
            mountinfo_row(7, "8:1", "rw", "", "ext4", "rw,errors=remount-ro"),
            mountinfo_row(
                MOUNT_ID,
                "259:3",
                "rw,nosuid",
                "shared:9 master:2 ",
                "ext4",
                "rw,discard"
            )
        );

        let record = parse_mountinfo(input.as_bytes(), MOUNT_ID).expect("exact mount row");
        assert_eq!(record.mount_id, MOUNT_ID);
        assert_eq!(record.device, DeviceId::new(259, 3));
        assert_eq!(record.filesystem, MountFilesystem::Ext4);
        assert!(record.mount_writable);
        assert!(record.superblock_writable);

        let closed_debug = format!("{record:?}");
        for private in [
            PRIVATE_ROOT_CANARY,
            PRIVATE_TARGET_CANARY,
            PRIVATE_SOURCE_CANARY,
        ] {
            assert!(!closed_debug.contains(private));
        }
    }

    #[test]
    fn mountinfo_rejects_missing_duplicate_malformed_and_oversized_inputs() {
        let valid = mountinfo_row(MOUNT_ID, "8:3", "rw", "", "ext4", "rw");
        assert_eq!(
            parse_mountinfo(valid.as_bytes(), MOUNT_ID + 1),
            Err(ClosedFault::Missing)
        );

        let duplicate = format!("{valid}{valid}");
        assert_eq!(
            parse_mountinfo(duplicate.as_bytes(), MOUNT_ID),
            Err(ClosedFault::Ambiguous)
        );

        let malformed = valid.replace(" - ext4 ", " ext4 ");
        assert_eq!(
            parse_mountinfo(malformed.as_bytes(), MOUNT_ID),
            Err(ClosedFault::Malformed)
        );

        let oversized = vec![b'x'; MAX_MOUNTINFO_BYTES + 1];
        assert_eq!(
            parse_mountinfo(&oversized, MOUNT_ID),
            Err(ClosedFault::LimitExceeded)
        );

        let too_many_rows =
            mountinfo_row(7, "8:1", "rw", "", "ext4", "rw").repeat(MAX_MOUNTINFO_ROWS + 1);
        assert_eq!(
            parse_mountinfo(too_many_rows.as_bytes(), MOUNT_ID),
            Err(ClosedFault::LimitExceeded)
        );

        let too_many_fields = mountinfo_row(
            MOUNT_ID,
            "8:1",
            "rw",
            &"future:1 ".repeat(MAX_MOUNTINFO_FIELDS),
            "ext4",
            "rw",
        );
        assert_eq!(
            parse_mountinfo(too_many_fields.as_bytes(), MOUNT_ID),
            Err(ClosedFault::LimitExceeded)
        );

        let oversized_row = mountinfo_row(
            MOUNT_ID,
            "8:1",
            "rw",
            &format!("{} ", "x".repeat(MAX_MOUNTINFO_ROW_BYTES)),
            "ext4",
            "rw",
        );
        assert_eq!(
            parse_mountinfo(oversized_row.as_bytes(), MOUNT_ID),
            Err(ClosedFault::LimitExceeded)
        );

        let duplicate_separator = valid.replace(" - ext4 ", " - - ext4 ");
        assert_eq!(
            parse_mountinfo(duplicate_separator.as_bytes(), MOUNT_ID),
            Err(ClosedFault::Malformed)
        );

        let malformed_parent = valid.replacen(" 1 ", " invalid ", 1);
        assert_eq!(
            parse_mountinfo(malformed_parent.as_bytes(), MOUNT_ID),
            Err(ClosedFault::Malformed)
        );
    }

    #[test]
    fn mountinfo_preserves_read_only_view_and_closed_denied_types() {
        let cases = [
            ("ext4", MountFilesystem::Ext4),
            ("btrfs", MountFilesystem::Btrfs),
            ("xfs", MountFilesystem::Xfs),
            ("nfs4", MountFilesystem::Remote),
            ("cifs", MountFilesystem::Remote),
            ("fuse.sshfs", MountFilesystem::Userspace),
            ("overlay", MountFilesystem::Overlay),
            ("tmpfs", MountFilesystem::Volatile),
            ("futurefs", MountFilesystem::Unknown),
        ];

        for (filesystem, expected) in cases {
            let input = mountinfo_row(MOUNT_ID, "8:3", "ro,nosuid", "", filesystem, "rw");
            let record = parse_mountinfo(input.as_bytes(), MOUNT_ID).expect("closed row");
            assert_eq!(record.filesystem, expected, "{filesystem}");
            assert!(!record.mount_writable);
            assert!(record.superblock_writable);
        }
    }

    fn physical(device: DeviceId, ancestry: PhysicalAncestry) -> TopologyNode {
        let node_path = format!("/fixture/physical/{}:{}", device.major, device.minor);
        let ancestor_path = format!("/fixture/ancestor/{}:{}", device.major, device.minor);
        let inode = (u64::from(device.major) << 32) | u64::from(device.minor);
        TopologyNode {
            device,
            backers: Vec::new(),
            removable: Some(false),
            kind: TopologyNodeKind::Physical(ancestry),
            private_identity: private_topology_identity(
                node_path.as_bytes(),
                ancestor_path.as_bytes(),
                inode,
            ),
        }
    }

    fn stacked(device: DeviceId, backers: Vec<DeviceId>) -> TopologyNode {
        let node_path = format!("/fixture/stacked/{}:{}", device.major, device.minor);
        let inode = (u64::from(device.major) << 32) | u64::from(device.minor);
        TopologyNode {
            device,
            backers,
            removable: Some(false),
            kind: TopologyNodeKind::Stacked,
            private_identity: private_nonphysical_identity(node_path.as_bytes(), inode),
        }
    }

    fn private_kernel_identity(path: &[u8], inode: u64) -> PrivateKernelObjectIdentity {
        PrivateKernelObjectIdentity {
            path: path.into(),
            stat_device: 23,
            inode,
            ctime_seconds: 1_722_000_000,
            ctime_nanoseconds: 117,
        }
    }

    fn private_topology_identity(
        node_path: &[u8],
        ancestor_path: &[u8],
        inode: u64,
    ) -> PrivateTopologyIdentity {
        PrivateTopologyIdentity {
            node: private_kernel_identity(node_path, inode),
            physical_ancestors: vec![private_kernel_identity(ancestor_path, inode + 1)],
        }
    }

    fn private_nonphysical_identity(node_path: &[u8], inode: u64) -> PrivateTopologyIdentity {
        PrivateTopologyIdentity {
            node: private_kernel_identity(node_path, inode),
            physical_ancestors: Vec::new(),
        }
    }

    #[test]
    fn topology_accepts_only_a_single_fixed_pci_backer_chain() {
        let root = DeviceId::new(253, 0);
        let leaf = DeviceId::new(259, 1);
        let graph = TopologyGraph {
            root,
            nodes: vec![
                stacked(root, vec![leaf]),
                physical(leaf, PhysicalAncestry::FixedPciNvme),
            ],
        };

        assert_eq!(classify_topology(&graph), Ok(TopologyDisposition::Fixed));

        for ancestry in [
            PhysicalAncestry::FixedPciSata,
            PhysicalAncestry::FixedPciScsi,
        ] {
            let direct = TopologyGraph {
                root: leaf,
                nodes: vec![physical(leaf, ancestry)],
            };
            assert_eq!(
                classify_topology(&direct),
                Ok(TopologyDisposition::Fixed),
                "{ancestry:?}"
            );
        }
    }

    #[test]
    fn topology_denies_removable_loop_and_hotplug_or_virtual_ancestry() {
        let root = DeviceId::new(8, 1);
        let mut removable = physical(root, PhysicalAncestry::FixedPciScsi);
        removable.removable = Some(true);
        assert_eq!(
            classify_topology(&TopologyGraph {
                root,
                nodes: vec![removable]
            }),
            Ok(TopologyDisposition::Denied)
        );

        let loop_node = TopologyNode {
            device: root,
            backers: Vec::new(),
            removable: Some(false),
            kind: TopologyNodeKind::Loop,
            private_identity: private_nonphysical_identity(b"/fixture/loop", 701),
        };
        assert_eq!(
            classify_topology(&TopologyGraph {
                root,
                nodes: vec![loop_node]
            }),
            Ok(TopologyDisposition::Denied)
        );

        for ancestry in [
            PhysicalAncestry::RemovablePci,
            PhysicalAncestry::Usb,
            PhysicalAncestry::FireWire,
            PhysicalAncestry::Thunderbolt,
            PhysicalAncestry::Mmc,
            PhysicalAncestry::Virtio,
            PhysicalAncestry::VmBus,
            PhysicalAncestry::Network,
        ] {
            let graph = TopologyGraph {
                root,
                nodes: vec![physical(root, ancestry)],
            };
            assert_eq!(
                classify_topology(&graph),
                Ok(TopologyDisposition::Denied),
                "{ancestry:?}"
            );
        }
    }

    #[test]
    fn block_removable_zero_cannot_substitute_for_generic_device_fixed() {
        let root = DeviceId::new(8, 1);
        let block_zero_without_generic_fixed = TopologyGraph {
            root,
            nodes: vec![physical(root, PhysicalAncestry::Unknown)],
        };

        assert_eq!(
            classify_topology(&block_zero_without_generic_fixed),
            Err(ClosedFault::Missing)
        );
    }

    #[test]
    fn topology_fails_closed_for_missing_multiple_cyclic_and_bounded_graphs() {
        let root = DeviceId::new(253, 0);
        let first = DeviceId::new(8, 1);
        let second = DeviceId::new(8, 2);

        let missing = TopologyGraph {
            root,
            nodes: vec![stacked(root, vec![first])],
        };
        assert_eq!(classify_topology(&missing), Err(ClosedFault::Missing));

        let multiple = TopologyGraph {
            root,
            nodes: vec![stacked(root, vec![first, second])],
        };
        assert_eq!(classify_topology(&multiple), Err(ClosedFault::Ambiguous));

        let cycle = TopologyGraph {
            root,
            nodes: vec![stacked(root, vec![first]), stacked(first, vec![root])],
        };
        assert_eq!(classify_topology(&cycle), Err(ClosedFault::Ambiguous));

        let oversized = TopologyGraph {
            root,
            nodes: (0..=MAX_TOPOLOGY_NODES)
                .map(|minor| {
                    physical(
                        DeviceId::new(8, minor as u32),
                        PhysicalAncestry::FixedPciScsi,
                    )
                })
                .collect(),
        };
        assert_eq!(
            classify_topology(&oversized),
            Err(ClosedFault::LimitExceeded)
        );

        let unknown = TopologyGraph {
            root,
            nodes: vec![physical(root, PhysicalAncestry::Unknown)],
        };
        assert_eq!(classify_topology(&unknown), Err(ClosedFault::Missing));

        let unknown_removable = TopologyGraph {
            root,
            nodes: vec![TopologyNode {
                device: root,
                backers: Vec::new(),
                removable: None,
                kind: TopologyNodeKind::Physical(PhysicalAncestry::FixedPciScsi),
                private_identity: private_topology_identity(
                    b"/fixture/unknown-removable",
                    b"/fixture/unknown-removable-pci",
                    801,
                ),
            }],
        };
        assert_eq!(
            classify_topology(&unknown_removable),
            Err(ClosedFault::Missing)
        );

        let devices: Vec<DeviceId> = (0..=(MAX_TOPOLOGY_DEPTH + 1))
            .map(|minor| DeviceId::new(8, minor as u32))
            .collect();
        let mut nodes: Vec<TopologyNode> = devices
            .windows(2)
            .map(|pair| stacked(pair[0], vec![pair[1]]))
            .collect();
        nodes.push(physical(
            *devices.last().expect("bounded fixture"),
            PhysicalAncestry::FixedPciScsi,
        ));
        let too_deep = TopologyGraph {
            root: devices[0],
            nodes,
        };
        assert_eq!(
            classify_topology(&too_deep),
            Err(ClosedFault::LimitExceeded)
        );
    }

    fn favorable_facts() -> InspectionFacts {
        let device = DeviceId::new(259, 3);
        let identity = HandleIdentity {
            device,
            inode: 117,
            mount_id: MOUNT_ID,
        };
        let mount = MountRecord {
            mount_id: MOUNT_ID,
            device,
            filesystem: MountFilesystem::Ext4,
            mount_writable: true,
            superblock_writable: true,
        };
        let topology = TopologyGraph {
            root: device,
            nodes: vec![physical(device, PhysicalAncestry::FixedPciNvme)],
        };
        InspectionFacts {
            platform_release: PlatformRelease::new(6, 12, 0),
            initial_identity: identity,
            final_identity: identity,
            initial_magic: EXT_FAMILY_MAGIC,
            final_magic: EXT_FAMILY_MAGIC,
            initial_mount: mount,
            final_mount: mount,
            topology: TopologyEvidence::Samples {
                initial: topology.clone(),
                after: topology,
            },
        }
    }

    #[test]
    fn facts_classify_exact_ext4_fixed_candidate_without_granting_a_profile() {
        let observation = classify_facts(favorable_facts()).expect("closed observation");
        assert_eq!(observation.platform, Platform::Linux);
        assert_eq!(observation.family, FilesystemFamily::LinuxExt4);
        assert_eq!(observation.writable, Ternary::Yes);
        assert_eq!(observation.local, Ternary::Yes);
        assert_eq!(observation.kernel_native, Ternary::Yes);
        assert_eq!(observation.internal_fixed, Ternary::Yes);
        assert_eq!(observation.os_managed_cloud_root, Ternary::No);
        assert_eq!(observation.access_controls_enforced, Ternary::Yes);
        assert_eq!(observation.identity_stable, Ternary::Yes);
        let journal_status = evaluate_filesystem(PersistenceTarget::Journal, Ok(observation));
        assert_eq!(
            journal_status.code,
            FilesystemStatusCode::DurabilityUnproved
        );
        assert_eq!(
            serde_json::to_value(journal_status).expect("closed status"),
            serde_json::json!({
                "target": "journal",
                "code": "durability_unproved",
                "family": "linux_ext4",
                "detector_schema": FILESYSTEM_DETECTOR_SCHEMA_VERSION,
            })
        );
        assert_eq!(
            evaluate_filesystem(PersistenceTarget::FileV2, Ok(observation)).code,
            FilesystemStatusCode::DurabilityUnproved
        );
    }

    #[test]
    fn facts_preserve_read_only_and_content_free_target_change_denials() {
        let mut read_only = favorable_facts();
        read_only.initial_mount.mount_writable = false;
        read_only.final_mount.mount_writable = false;
        let observation = classify_facts(read_only).expect("read-only observation");
        assert_eq!(observation.writable, Ternary::No);
        assert_eq!(
            evaluate_filesystem(PersistenceTarget::Journal, Ok(observation)).code,
            FilesystemStatusCode::ReadOnly
        );

        let mut changed = favorable_facts();
        changed.final_identity.inode += 1;
        let observation = classify_facts(changed).expect("changed observation");
        assert_eq!(observation.identity_stable, Ternary::No);
        assert_eq!(
            evaluate_filesystem(PersistenceTarget::Journal, Ok(observation)).code,
            FilesystemStatusCode::TargetChanged
        );

        let mut mount_changed = favorable_facts();
        mount_changed.final_mount.device = DeviceId::new(259, 4);
        let observation = classify_facts(mount_changed).expect("mount identity disagreement");
        assert_eq!(observation.identity_stable, Ternary::No);

        let mut topology_changed = favorable_facts();
        if let TopologyEvidence::Samples { initial, after } = &mut topology_changed.topology {
            assert_eq!(classify_topology(initial), Ok(TopologyDisposition::Fixed));
            assert_eq!(classify_topology(after), Ok(TopologyDisposition::Fixed));
            after.nodes[0].kind = TopologyNodeKind::Physical(PhysicalAncestry::Thunderbolt);
        }
        let observation = classify_facts(topology_changed).expect("topology race denial");
        assert_eq!(observation.identity_stable, Ternary::No);
    }

    #[test]
    fn facts_detect_private_topology_replacement_when_closed_fields_are_reused() {
        const INITIAL_NODE_CANARY: &[u8] = b"private-initial-node-canary";
        const INITIAL_ANCESTOR_CANARY: &[u8] = b"private-initial-pci-canary";
        const AFTER_NODE_CANARY: &[u8] = b"private-after-node-canary";
        const AFTER_ANCESTOR_CANARY: &[u8] = b"private-after-pci-canary";

        let mut facts = favorable_facts();
        let TopologyEvidence::Samples { initial, after } = &mut facts.topology else {
            panic!("favorable facts retain two topology samples");
        };
        initial.nodes[0].private_identity =
            private_topology_identity(INITIAL_NODE_CANARY, INITIAL_ANCESTOR_CANARY, 301);
        after.nodes[0].private_identity =
            private_topology_identity(AFTER_NODE_CANARY, AFTER_ANCESTOR_CANARY, 401);

        assert_eq!(initial.nodes[0].device, after.nodes[0].device);
        assert_eq!(initial.nodes[0].backers, after.nodes[0].backers);
        assert_eq!(initial.nodes[0].removable, after.nodes[0].removable);
        assert_eq!(initial.nodes[0].kind, after.nodes[0].kind);
        assert_ne!(
            initial.nodes[0].private_identity,
            after.nodes[0].private_identity
        );

        let observation = classify_facts(facts).expect("closed topology replacement");
        assert_eq!(observation.identity_stable, Ternary::No);
        assert_eq!(
            evaluate_filesystem(PersistenceTarget::Journal, Ok(observation)).code,
            FilesystemStatusCode::TargetChanged
        );
    }

    #[test]
    fn private_topology_and_handle_debug_never_reveal_identity_canaries() {
        const NODE_CANARY: &[u8] = b"private-node-identity-canary";
        const ANCESTOR_CANARY: &[u8] = b"private-ancestor-identity-canary";
        const STAT_DEVICE_CANARY: u64 = 8_001_117_223;
        const INODE_CANARY: u64 = 8_001_117_224;
        const MOUNT_CANARY: u64 = 8_001_117_225;
        const CTIME_SECONDS_CANARY: i64 = 8_001_117_226;
        const CTIME_NANOSECONDS_CANARY: i64 = 8_001_117_227;

        let private_identity = PrivateTopologyIdentity {
            node: PrivateKernelObjectIdentity {
                path: NODE_CANARY.into(),
                stat_device: STAT_DEVICE_CANARY,
                inode: INODE_CANARY,
                ctime_seconds: CTIME_SECONDS_CANARY,
                ctime_nanoseconds: CTIME_NANOSECONDS_CANARY,
            },
            physical_ancestors: vec![PrivateKernelObjectIdentity {
                path: ANCESTOR_CANARY.into(),
                stat_device: STAT_DEVICE_CANARY + 1,
                inode: INODE_CANARY + 1,
                ctime_seconds: 8_001_117_228,
                ctime_nanoseconds: 8_001_117_229,
            }],
        };
        let handle = HandleIdentity {
            device: DeviceId::new(8_001, 1_117),
            inode: INODE_CANARY,
            mount_id: MOUNT_CANARY,
        };

        assert_eq!(
            format!("{:?}", private_identity.node),
            "PrivateKernelObjectIdentity(<redacted>)"
        );
        assert_eq!(
            format!("{private_identity:?}"),
            "PrivateTopologyIdentity(<redacted>)"
        );
        assert_eq!(format!("{handle:?}"), "HandleIdentity(<redacted>)");

        let mut facts = favorable_facts();
        let TopologyEvidence::Samples { initial, after } = &mut facts.topology else {
            panic!("favorable facts retain two topology samples");
        };
        initial.nodes[0].private_identity = private_identity.clone();
        after.nodes[0].private_identity = private_identity.clone();
        let status = evaluate_filesystem(PersistenceTarget::Journal, classify_facts(facts));
        let status_json = serde_json::to_string(&status).expect("closed status");
        let stat_device_canary = STAT_DEVICE_CANARY.to_string();
        let inode_canary = INODE_CANARY.to_string();
        let mount_canary = MOUNT_CANARY.to_string();
        let ctime_seconds_canary = CTIME_SECONDS_CANARY.to_string();
        let ctime_nanoseconds_canary = CTIME_NANOSECONDS_CANARY.to_string();
        let debug_values = [
            std::str::from_utf8(NODE_CANARY).expect("ASCII fixture"),
            std::str::from_utf8(ANCESTOR_CANARY).expect("ASCII fixture"),
            stat_device_canary.as_str(),
            inode_canary.as_str(),
            mount_canary.as_str(),
            ctime_seconds_canary.as_str(),
            ctime_nanoseconds_canary.as_str(),
        ];
        for forbidden in debug_values {
            assert!(!format!("{:?}", private_identity.node).contains(forbidden));
            assert!(!format!("{handle:?}").contains(forbidden));
            assert!(!status_json.contains(forbidden));
        }
    }

    #[test]
    fn topology_rejects_an_oversized_private_identity_path() {
        let root = DeviceId::new(259, 17);
        let mut node = physical(root, PhysicalAncestry::FixedPciNvme);
        node.private_identity.node.path = vec![b'x'; MAX_ATTRIBUTE_BYTES + 1].into_boxed_slice();

        assert_eq!(
            classify_topology(&TopologyGraph {
                root,
                nodes: vec![node],
            }),
            Err(ClosedFault::LimitExceeded)
        );
    }

    #[test]
    fn topology_identity_budget_accumulates_individually_bounded_identities() {
        let identities: Vec<PrivateTopologyIdentity> = (0_u8..4)
            .map(|identity_index| PrivateTopologyIdentity {
                node: private_kernel_identity(
                    &vec![b'a' + identity_index; MAX_ATTRIBUTE_BYTES],
                    1_000 + u64::from(identity_index),
                ),
                physical_ancestors: (0..MAX_TOPOLOGY_DEPTH)
                    .map(|ancestor_index| {
                        private_kernel_identity(
                            &vec![b'A' + identity_index; MAX_ATTRIBUTE_BYTES],
                            2_000
                                + u64::from(identity_index) * 100
                                + u64::try_from(ancestor_index).expect("bounded ancestor index"),
                        )
                    })
                    .collect(),
            })
            .collect();

        for identity in &identities {
            assert_eq!(identity.physical_ancestors.len(), 16);
            assert!(
                std::iter::once(&identity.node)
                    .chain(identity.physical_ancestors.iter())
                    .all(|object| object.path.len() <= 4_096)
            );
            assert_eq!(identity.byte_len(), Ok(69_632));
        }

        let mut consumed = 0;
        for (identity, expected) in identities.iter().take(3).zip([69_632, 139_264, 208_896]) {
            consumed = add_topology_identity_bytes(consumed, identity)
                .expect("three individually bounded identities fit the aggregate budget");
            assert_eq!(consumed, expected);
        }
        assert_eq!(
            add_topology_identity_bytes(consumed, &identities[3]),
            Err(ClosedFault::LimitExceeded)
        );
    }

    #[test]
    fn topology_node_reread_rejects_every_identity_bearing_field_change() {
        let device = DeviceId::new(259, 19);
        let initial = TopologyNodeStabilitySample {
            device,
            removable: false,
            backers: Vec::new(),
            is_loop: false,
            node_identity: private_kernel_identity(b"/fixture/node-a", 901),
            physical: Some(PhysicalAncestrySnapshot {
                closed: PhysicalAncestry::FixedPciNvme,
                ancestors: vec![private_kernel_identity(b"/fixture/pci-a", 902)],
            }),
        };
        assert_eq!(ensure_stable_node_samples(&initial, &initial), Ok(()));

        let mut changes = Vec::new();
        let mut removable = initial.clone();
        removable.removable = true;
        changes.push(removable);
        let mut backers = initial.clone();
        backers.backers = vec![DeviceId::new(259, 20)];
        changes.push(backers);
        let mut loop_state = initial.clone();
        loop_state.is_loop = true;
        changes.push(loop_state);
        let mut node_identity = initial.clone();
        node_identity.node_identity = private_kernel_identity(b"/fixture/node-b", 903);
        changes.push(node_identity);
        let mut ancestry_identity = initial.clone();
        ancestry_identity.physical = Some(PhysicalAncestrySnapshot {
            closed: PhysicalAncestry::FixedPciNvme,
            ancestors: vec![private_kernel_identity(b"/fixture/pci-b", 904)],
        });
        changes.push(ancestry_identity);

        for after in changes {
            assert_eq!(
                ensure_stable_node_samples(&initial, &after),
                Err(ClosedFault::Ambiguous)
            );
        }
    }

    #[test]
    fn facts_reject_filesystem_magic_disagreement_and_topology_unknowns() {
        let mut mismatched = favorable_facts();
        mismatched.initial_magic = XFS_MAGIC;
        mismatched.final_magic = XFS_MAGIC;
        assert_eq!(
            classify_facts(mismatched),
            Err(DetectorFault::InspectionUnavailable)
        );

        let mut missing = favorable_facts();
        missing.topology = TopologyEvidence::Samples {
            initial: TopologyGraph {
                root: DeviceId::new(8, 1),
                nodes: Vec::new(),
            },
            after: TopologyGraph {
                root: DeviceId::new(8, 1),
                nodes: Vec::new(),
            },
        };
        assert_eq!(
            classify_facts(missing),
            Err(DetectorFault::InspectionUnavailable)
        );
    }

    #[test]
    fn facts_recognize_unproved_families_and_closed_negative_mounts() {
        for (filesystem, magic, family) in [
            (
                MountFilesystem::Btrfs,
                BTRFS_MAGIC,
                FilesystemFamily::LinuxBtrfs,
            ),
            (MountFilesystem::Xfs, XFS_MAGIC, FilesystemFamily::LinuxXfs),
        ] {
            let mut facts = favorable_facts();
            facts.initial_mount.filesystem = filesystem;
            facts.final_mount.filesystem = filesystem;
            facts.initial_magic = magic;
            facts.final_magic = magic;
            let observation = classify_facts(facts).expect("recognized family");
            assert_eq!(observation.family, family);
            assert_eq!(
                evaluate_filesystem(PersistenceTarget::Journal, Ok(observation)).code,
                FilesystemStatusCode::FilesystemUnproved
            );
        }

        for (filesystem, magic, local, kernel_native) in [
            (
                MountFilesystem::Remote,
                NFS_MAGIC,
                Ternary::No,
                Ternary::Yes,
            ),
            (
                MountFilesystem::Userspace,
                FUSE_MAGIC,
                Ternary::Yes,
                Ternary::No,
            ),
            (
                MountFilesystem::Overlay,
                OVERLAY_MAGIC,
                Ternary::Yes,
                Ternary::No,
            ),
        ] {
            let mut facts = favorable_facts();
            facts.initial_mount.filesystem = filesystem;
            facts.final_mount.filesystem = filesystem;
            facts.initial_magic = magic;
            facts.final_magic = magic;
            facts.topology = TopologyEvidence::NotRequired;
            let observation = classify_facts(facts).expect("closed negative observation");
            assert_eq!(observation.local, local);
            assert_eq!(observation.kernel_native, kernel_native);
        }

        assert_eq!(classify_mount_filesystem(b"ext3"), MountFilesystem::Unknown);
        assert!(!magic_matches(MountFilesystem::Unknown, EXT_FAMILY_MAGIC));
    }

    #[test]
    fn kernel_release_parser_keeps_only_closed_numeric_components() {
        assert_eq!(
            parse_platform_release(b"6.12.17-audio-host\n"),
            Ok(PlatformRelease::new(6, 12, 17))
        );
        assert_eq!(
            parse_platform_release(b"6.6.87.2-microsoft-standard-WSL2\n"),
            Ok(PlatformRelease::new(6, 6, 87))
        );
        assert_eq!(
            parse_platform_release(PRIVATE_TARGET_CANARY.as_bytes()),
            Err(ClosedFault::Malformed)
        );
    }

    #[test]
    fn generic_device_removability_requires_the_exact_fixed_value() {
        assert_eq!(
            parse_device_removability(b"fixed\n"),
            Ok(DeviceRemovability::Fixed)
        );
        assert_eq!(
            parse_device_removability(b"removable\n"),
            Ok(DeviceRemovability::Removable)
        );
        assert_eq!(
            parse_device_removability(b"unknown\n"),
            Ok(DeviceRemovability::Unknown)
        );
        for unproved in [b"0\n".as_slice(), b"1\n", b"fixed extra\n", b"future\n"] {
            assert_eq!(
                parse_device_removability(unproved),
                Err(ClosedFault::Malformed)
            );
        }
    }

    #[test]
    fn hotplug_ancestor_overrides_a_fixed_pci_descendant() {
        let mut evidence = PhysicalAncestryEvidence::default();
        assert_eq!(evidence.observe(b"nvme", None), None);
        assert_eq!(
            evidence.observe(b"pci", Some(DeviceRemovability::Fixed)),
            None
        );
        assert_eq!(
            evidence.observe(b"thunderbolt", None),
            Some(PhysicalAncestry::Thunderbolt)
        );
    }

    #[test]
    fn missing_generic_device_removability_keeps_pci_ancestry_unproved() {
        let mut evidence = PhysicalAncestryEvidence::default();
        assert_eq!(evidence.observe(b"nvme", None), None);
        assert_eq!(
            evidence.observe(b"pci", None),
            Some(PhysicalAncestry::Unknown)
        );
    }

    #[test]
    fn native_probe_holds_the_directory_and_never_grants_an_empty_profile() {
        let held = LinuxHeldTarget::open(".").expect("open current test directory");
        let result = LinuxFilesystemDetector.inspect(&held);
        match result {
            Ok(observation) => {
                assert_eq!(observation.platform, Platform::Linux);
                assert_ne!(
                    evaluate_filesystem(PersistenceTarget::Journal, Ok(observation)).code,
                    FilesystemStatusCode::Supported
                );
                assert_ne!(
                    evaluate_filesystem(PersistenceTarget::FileV2, Ok(observation)).code,
                    FilesystemStatusCode::Supported
                );
            }
            Err(DetectorFault::InspectionUnavailable) => {}
            Err(DetectorFault::TargetUnavailable) => {
                panic!("a held current directory became unavailable")
            }
        }
    }
}
