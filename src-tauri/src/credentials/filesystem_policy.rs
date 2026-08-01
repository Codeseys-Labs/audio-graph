//! Closed, backend-private filesystem eligibility policy for credential-v2.
//!
//! Target-native adapters own paths, handles, and native observations. Only the
//! closed values in this module may cross the detector seam. Evidence profiles
//! are reviewed release data compiled into the binary, never runtime input.

use serde::Serialize;

#[cfg(any(test, target_os = "macos"))]
pub(crate) mod macos;

#[cfg(any(target_os = "windows", test))]
pub(crate) mod windows;

#[cfg(target_os = "linux")]
pub(crate) mod linux;

pub(crate) const FILESYSTEM_DETECTOR_SCHEMA_VERSION: u16 = 1;
pub(crate) const PERSISTENCE_WRAPPER_PROTOCOL_VERSION: u16 = 1;
const EVIDENCE_MANIFEST_SCHEMA_VERSION: u16 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum PersistenceTarget {
    Journal,
    FileV2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Platform {
    Windows,
    MacOs,
    Linux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FilesystemFamily {
    WindowsNtfs,
    WindowsRefs,
    MacApfs,
    MacHfsPlus,
    LinuxExt4,
    LinuxBtrfs,
    LinuxXfs,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ternary {
    Yes,
    No,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct PlatformRelease {
    pub(crate) major: u16,
    pub(crate) minor: u16,
    pub(crate) patch: u32,
}

impl PlatformRelease {
    pub(crate) const fn new(major: u16, minor: u16, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FilesystemObservation {
    pub(crate) platform: Platform,
    pub(crate) platform_release: PlatformRelease,
    pub(crate) family: FilesystemFamily,
    pub(crate) writable: Ternary,
    pub(crate) local: Ternary,
    pub(crate) kernel_native: Ternary,
    pub(crate) internal_fixed: Ternary,
    pub(crate) os_managed_cloud_root: Ternary,
    pub(crate) access_controls_enforced: Ternary,
    pub(crate) identity_stable: Ternary,
    pub(crate) detector_schema: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetectorFault {
    TargetUnavailable,
    InspectionUnavailable,
}

pub(crate) trait FilesystemDetector {
    type HeldTarget;

    fn inspect(&self, target: &Self::HeldTarget) -> Result<FilesystemObservation, DetectorFault>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum FilesystemStatusCode {
    Supported,
    TargetUnavailable,
    InspectionUnavailable,
    TargetChanged,
    ReadOnly,
    Remote,
    UserspaceFilesystem,
    CloudManaged,
    RemovableOrHotplug,
    FilesystemUnproved,
    AccessControlUnproved,
    DurabilityUnproved,
    ConfidentialityUnproved,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct FilesystemStatus {
    pub(crate) target: PersistenceTarget,
    pub(crate) code: FilesystemStatusCode,
    pub(crate) family: Option<FilesystemFamily>,
    pub(crate) detector_schema: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvidenceProfileKey {
    pub(crate) target: PersistenceTarget,
    pub(crate) platform: Platform,
    pub(crate) family: FilesystemFamily,
    pub(crate) detector_schema: u16,
    pub(crate) minimum_os_release: PlatformRelease,
    pub(crate) persistence_protocol: u16,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct EvidenceManifestRef {
    pub(crate) schema_version: u16,
    pub(crate) digest: [u8; 32],
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct JournalEvidenceProfile {
    pub(crate) key: EvidenceProfileKey,
    pub(crate) journal_evidence: EvidenceManifestRef,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileV2EvidenceProfile {
    pub(crate) key: EvidenceProfileKey,
    pub(crate) journal_evidence: EvidenceManifestRef,
    pub(crate) confidentiality_evidence: EvidenceManifestRef,
}

pub(crate) const JOURNAL_SUPPORTED_PROFILES: &[JournalEvidenceProfile] = &[];
pub(crate) const FILE_V2_SUPPORTED_PROFILES: &[FileV2EvidenceProfile] = &[];

#[derive(Clone, Copy)]
struct EvidenceProfiles<'a> {
    journal: &'a [JournalEvidenceProfile],
    file_v2: &'a [FileV2EvidenceProfile],
}

impl EvidenceProfiles<'static> {
    const fn compiled() -> Self {
        Self {
            journal: JOURNAL_SUPPORTED_PROFILES,
            file_v2: FILE_V2_SUPPORTED_PROFILES,
        }
    }
}

const JOURNAL_CANDIDATE_FAMILIES: &[(Platform, FilesystemFamily)] = &[
    (Platform::Windows, FilesystemFamily::WindowsNtfs),
    (Platform::MacOs, FilesystemFamily::MacApfs),
    (Platform::Linux, FilesystemFamily::LinuxExt4),
];
const FILE_V2_CANDIDATE_FAMILIES: &[(Platform, FilesystemFamily)] = &[
    (Platform::Windows, FilesystemFamily::WindowsNtfs),
    (Platform::MacOs, FilesystemFamily::MacApfs),
    (Platform::Linux, FilesystemFamily::LinuxExt4),
];

fn is_candidate_family(
    target: PersistenceTarget,
    platform: Platform,
    family: FilesystemFamily,
) -> bool {
    let candidates = match target {
        PersistenceTarget::Journal => JOURNAL_CANDIDATE_FAMILIES,
        PersistenceTarget::FileV2 => FILE_V2_CANDIDATE_FAMILIES,
    };
    candidates.contains(&(platform, family))
}

fn profile_key_matches(
    key: EvidenceProfileKey,
    expected_target: PersistenceTarget,
    observation: FilesystemObservation,
) -> bool {
    key.target == expected_target
        && key.platform == observation.platform
        && key.family == observation.family
        && key.detector_schema == observation.detector_schema
        && key.persistence_protocol == PERSISTENCE_WRAPPER_PROTOCOL_VERSION
        && observation.platform_release >= key.minimum_os_release
}

fn journal_profile_matches(
    profile: JournalEvidenceProfile,
    observation: FilesystemObservation,
) -> bool {
    profile_key_matches(profile.key, PersistenceTarget::Journal, observation)
        && profile.journal_evidence.schema_version == EVIDENCE_MANIFEST_SCHEMA_VERSION
}

fn file_v2_journal_evidence_matches(
    profile: FileV2EvidenceProfile,
    observation: FilesystemObservation,
) -> bool {
    profile_key_matches(profile.key, PersistenceTarget::FileV2, observation)
        && profile.journal_evidence.schema_version == EVIDENCE_MANIFEST_SCHEMA_VERSION
}

fn file_v2_profile_matches(
    profile: FileV2EvidenceProfile,
    observation: FilesystemObservation,
) -> bool {
    file_v2_journal_evidence_matches(profile, observation)
        && profile.confidentiality_evidence.schema_version == EVIDENCE_MANIFEST_SCHEMA_VERSION
}

pub(crate) fn evaluate_filesystem(
    target: PersistenceTarget,
    inspection: Result<FilesystemObservation, DetectorFault>,
) -> FilesystemStatus {
    evaluate_with_profiles(target, inspection, EvidenceProfiles::compiled())
}

fn evaluate_with_profiles(
    target: PersistenceTarget,
    inspection: Result<FilesystemObservation, DetectorFault>,
    profiles: EvidenceProfiles<'_>,
) -> FilesystemStatus {
    match inspection {
        Err(DetectorFault::TargetUnavailable) => FilesystemStatus {
            target,
            code: FilesystemStatusCode::TargetUnavailable,
            family: None,
            detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
        },
        Err(DetectorFault::InspectionUnavailable) => FilesystemStatus {
            target,
            code: FilesystemStatusCode::InspectionUnavailable,
            family: None,
            detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
        },
        Ok(observation) => {
            let has_required_unknown = [
                observation.writable,
                observation.local,
                observation.kernel_native,
                observation.internal_fixed,
                observation.os_managed_cloud_root,
                observation.identity_stable,
            ]
            .contains(&Ternary::Unknown);
            let code = if has_required_unknown {
                FilesystemStatusCode::InspectionUnavailable
            } else if observation.identity_stable != Ternary::Yes {
                FilesystemStatusCode::TargetChanged
            } else if observation.writable != Ternary::Yes {
                FilesystemStatusCode::ReadOnly
            } else if observation.local != Ternary::Yes {
                FilesystemStatusCode::Remote
            } else if observation.kernel_native != Ternary::Yes {
                FilesystemStatusCode::UserspaceFilesystem
            } else if observation.os_managed_cloud_root != Ternary::No {
                FilesystemStatusCode::CloudManaged
            } else if observation.internal_fixed != Ternary::Yes {
                FilesystemStatusCode::RemovableOrHotplug
            } else if !is_candidate_family(target, observation.platform, observation.family) {
                FilesystemStatusCode::FilesystemUnproved
            } else if observation.access_controls_enforced != Ternary::Yes {
                FilesystemStatusCode::AccessControlUnproved
            } else if observation.detector_schema != FILESYSTEM_DETECTOR_SCHEMA_VERSION {
                FilesystemStatusCode::DurabilityUnproved
            } else {
                let has_journal_evidence = profiles
                    .journal
                    .iter()
                    .copied()
                    .any(|profile| journal_profile_matches(profile, observation))
                    || profiles
                        .file_v2
                        .iter()
                        .copied()
                        .any(|profile| file_v2_journal_evidence_matches(profile, observation));
                if !has_journal_evidence {
                    FilesystemStatusCode::DurabilityUnproved
                } else if target == PersistenceTarget::FileV2
                    && !profiles
                        .file_v2
                        .iter()
                        .copied()
                        .any(|profile| file_v2_profile_matches(profile, observation))
                {
                    FilesystemStatusCode::ConfidentialityUnproved
                } else {
                    FilesystemStatusCode::Supported
                }
            };
            FilesystemStatus {
                target,
                code,
                family: Some(observation.family),
                detector_schema: observation.detector_schema,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type ObservationMutation = fn(&mut FilesystemObservation);
    type StatusCase = (&'static str, ObservationMutation, FilesystemStatusCode);

    fn positive_ntfs_observation() -> FilesystemObservation {
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
    }

    fn manifest(digest_byte: u8) -> EvidenceManifestRef {
        EvidenceManifestRef {
            schema_version: EVIDENCE_MANIFEST_SCHEMA_VERSION,
            digest: [digest_byte; 32],
        }
    }

    fn profile_key(
        target: PersistenceTarget,
        observation: FilesystemObservation,
    ) -> EvidenceProfileKey {
        EvidenceProfileKey {
            target,
            platform: observation.platform,
            family: observation.family,
            detector_schema: observation.detector_schema,
            minimum_os_release: observation.platform_release,
            persistence_protocol: PERSISTENCE_WRAPPER_PROTOCOL_VERSION,
        }
    }

    fn journal_profile(observation: FilesystemObservation) -> JournalEvidenceProfile {
        JournalEvidenceProfile {
            key: profile_key(PersistenceTarget::Journal, observation),
            journal_evidence: manifest(0x11),
        }
    }

    fn file_v2_profile(observation: FilesystemObservation) -> FileV2EvidenceProfile {
        FileV2EvidenceProfile {
            key: profile_key(PersistenceTarget::FileV2, observation),
            journal_evidence: manifest(0x22),
            confidentiality_evidence: manifest(0x33),
        }
    }

    #[test]
    fn empty_compiled_profiles_keep_positive_candidates_unproved() {
        assert!(JOURNAL_SUPPORTED_PROFILES.is_empty());
        assert!(FILE_V2_SUPPORTED_PROFILES.is_empty());

        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            assert_eq!(
                evaluate_filesystem(target, Ok(positive_ntfs_observation())),
                FilesystemStatus {
                    target,
                    code: FilesystemStatusCode::DurabilityUnproved,
                    family: Some(FilesystemFamily::WindowsNtfs),
                    detector_schema: FILESYSTEM_DETECTOR_SCHEMA_VERSION,
                }
            );
        }
    }

    #[test]
    fn detector_faults_and_unknown_observations_fail_closed() {
        for (fault, expected) in [
            (
                DetectorFault::TargetUnavailable,
                FilesystemStatusCode::TargetUnavailable,
            ),
            (
                DetectorFault::InspectionUnavailable,
                FilesystemStatusCode::InspectionUnavailable,
            ),
        ] {
            let status = evaluate_filesystem(PersistenceTarget::Journal, Err(fault));
            assert_eq!(status.code, expected);
            assert_eq!(status.family, None);
        }

        let required_unknowns: &[StatusCase] = &[
            (
                "writable",
                |observation| observation.writable = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "local",
                |observation| observation.local = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "kernel_native",
                |observation| observation.kernel_native = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "internal_fixed",
                |observation| observation.internal_fixed = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "os_managed_cloud_root",
                |observation| observation.os_managed_cloud_root = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "identity_stable",
                |observation| observation.identity_stable = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "access_controls_enforced",
                |observation| observation.access_controls_enforced = Ternary::Unknown,
                FilesystemStatusCode::AccessControlUnproved,
            ),
        ];

        for (name, make_unknown, expected) in required_unknowns {
            let mut observation = positive_ntfs_observation();
            make_unknown(&mut observation);
            assert_eq!(
                evaluate_filesystem(PersistenceTarget::Journal, Ok(observation)).code,
                *expected,
                "unknown {name} must not become favorable"
            );
        }
    }

    #[test]
    fn runtime_denials_follow_the_normative_precedence() {
        let cases: &[StatusCase] = &[
            (
                "target_changed_before_read_only",
                |observation| {
                    observation.identity_stable = Ternary::No;
                    observation.writable = Ternary::No;
                },
                FilesystemStatusCode::TargetChanged,
            ),
            (
                "read_only_before_remote",
                |observation| {
                    observation.writable = Ternary::No;
                    observation.local = Ternary::No;
                },
                FilesystemStatusCode::ReadOnly,
            ),
            (
                "remote_before_userspace",
                |observation| {
                    observation.local = Ternary::No;
                    observation.kernel_native = Ternary::No;
                },
                FilesystemStatusCode::Remote,
            ),
            (
                "userspace_before_cloud",
                |observation| {
                    observation.kernel_native = Ternary::No;
                    observation.os_managed_cloud_root = Ternary::Yes;
                },
                FilesystemStatusCode::UserspaceFilesystem,
            ),
            (
                "cloud_before_removable",
                |observation| {
                    observation.os_managed_cloud_root = Ternary::Yes;
                    observation.internal_fixed = Ternary::No;
                },
                FilesystemStatusCode::CloudManaged,
            ),
            (
                "removable_before_family",
                |observation| {
                    observation.internal_fixed = Ternary::No;
                    observation.family = FilesystemFamily::Other;
                },
                FilesystemStatusCode::RemovableOrHotplug,
            ),
            (
                "family_before_access_control",
                |observation| {
                    observation.family = FilesystemFamily::Other;
                    observation.access_controls_enforced = Ternary::No;
                },
                FilesystemStatusCode::FilesystemUnproved,
            ),
            (
                "access_control_before_evidence",
                |observation| observation.access_controls_enforced = Ternary::No,
                FilesystemStatusCode::AccessControlUnproved,
            ),
        ];

        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            for (name, deny, expected) in cases {
                let mut observation = positive_ntfs_observation();
                deny(&mut observation);
                assert_eq!(
                    evaluate_filesystem(target, Ok(observation)).code,
                    *expected,
                    "{name} for {target:?}"
                );
            }
        }
    }

    #[test]
    fn journal_and_file_v2_evidence_are_separate_authorities() {
        for (platform, family) in [
            (Platform::Windows, FilesystemFamily::WindowsNtfs),
            (Platform::MacOs, FilesystemFamily::MacApfs),
            (Platform::Linux, FilesystemFamily::LinuxExt4),
        ] {
            let mut observation = positive_ntfs_observation();
            observation.platform = platform;
            observation.family = family;
            let journal = [journal_profile(observation)];
            let file_v2 = [file_v2_profile(observation)];

            let journal_only = EvidenceProfiles {
                journal: &journal,
                file_v2: &[],
            };
            assert_eq!(
                evaluate_with_profiles(PersistenceTarget::Journal, Ok(observation), journal_only,)
                    .code,
                FilesystemStatusCode::Supported
            );
            assert_eq!(
                evaluate_with_profiles(PersistenceTarget::FileV2, Ok(observation), journal_only,)
                    .code,
                FilesystemStatusCode::ConfidentialityUnproved,
                "journal evidence must never authorize file-v2"
            );

            let both = EvidenceProfiles {
                journal: &journal,
                file_v2: &file_v2,
            };
            for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
                assert_eq!(
                    evaluate_with_profiles(target, Ok(observation), both).code,
                    FilesystemStatusCode::Supported
                );
            }

            let file_v2_with_embedded_journal_evidence = EvidenceProfiles {
                journal: &[],
                file_v2: &file_v2,
            };
            for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
                assert_eq!(
                    evaluate_with_profiles(
                        target,
                        Ok(observation),
                        file_v2_with_embedded_journal_evidence,
                    )
                    .code,
                    FilesystemStatusCode::Supported,
                    "file-v2 evidence explicitly includes the journal gates"
                );
            }
        }
    }

    #[test]
    fn evidence_profiles_are_bound_to_current_versions_and_os_floor() {
        let observation = positive_ntfs_observation();

        let mut foreign_schema_observation = observation;
        foreign_schema_observation.detector_schema = FILESYSTEM_DETECTOR_SCHEMA_VERSION + 1;
        let foreign_schema = [journal_profile(foreign_schema_observation)];
        assert_eq!(
            evaluate_with_profiles(
                PersistenceTarget::Journal,
                Ok(foreign_schema_observation),
                EvidenceProfiles {
                    journal: &foreign_schema,
                    file_v2: &[],
                },
            )
            .code,
            FilesystemStatusCode::DurabilityUnproved,
            "a foreign detector schema cannot authorize the current evaluator"
        );

        let mut wrong_target = journal_profile(observation);
        wrong_target.key.target = PersistenceTarget::FileV2;
        let mut wrong_platform = journal_profile(observation);
        wrong_platform.key.platform = Platform::Linux;
        let mut wrong_family = journal_profile(observation);
        wrong_family.key.family = FilesystemFamily::LinuxExt4;
        let mut wrong_profile_schema = journal_profile(observation);
        wrong_profile_schema.key.detector_schema = FILESYSTEM_DETECTOR_SCHEMA_VERSION + 1;
        for (name, profile) in [
            ("target", wrong_target),
            ("platform", wrong_platform),
            ("family", wrong_family),
            ("detector_schema", wrong_profile_schema),
        ] {
            let journal = [profile];
            assert_eq!(
                evaluate_with_profiles(
                    PersistenceTarget::Journal,
                    Ok(observation),
                    EvidenceProfiles {
                        journal: &journal,
                        file_v2: &[],
                    },
                )
                .code,
                FilesystemStatusCode::DurabilityUnproved,
                "profile {name} is part of the authorization key"
            );
        }

        let mut wrong_protocol = journal_profile(observation);
        wrong_protocol.key.persistence_protocol = PERSISTENCE_WRAPPER_PROTOCOL_VERSION + 1;
        let wrong_protocol = [wrong_protocol];
        assert_eq!(
            evaluate_with_profiles(
                PersistenceTarget::Journal,
                Ok(observation),
                EvidenceProfiles {
                    journal: &wrong_protocol,
                    file_v2: &[],
                },
            )
            .code,
            FilesystemStatusCode::DurabilityUnproved
        );

        let mut wrong_manifest = journal_profile(observation);
        wrong_manifest.journal_evidence.schema_version = EVIDENCE_MANIFEST_SCHEMA_VERSION + 1;
        let wrong_manifest = [wrong_manifest];
        assert_eq!(
            evaluate_with_profiles(
                PersistenceTarget::Journal,
                Ok(observation),
                EvidenceProfiles {
                    journal: &wrong_manifest,
                    file_v2: &[],
                },
            )
            .code,
            FilesystemStatusCode::DurabilityUnproved
        );

        let mut future_os = journal_profile(observation);
        future_os.key.minimum_os_release = PlatformRelease::new(11, 0, 0);
        let future_os = [future_os];
        assert_eq!(
            evaluate_with_profiles(
                PersistenceTarget::Journal,
                Ok(observation),
                EvidenceProfiles {
                    journal: &future_os,
                    file_v2: &[],
                },
            )
            .code,
            FilesystemStatusCode::DurabilityUnproved
        );
    }

    #[test]
    fn newer_runtime_release_satisfies_journal_and_file_v2_profile_minimums() {
        let profile_floor = positive_ntfs_observation();
        let mut newer_runtime = profile_floor;
        newer_runtime.platform_release = PlatformRelease::new(10, 0, 22_632);
        assert!(newer_runtime.platform_release > profile_floor.platform_release);

        let journal = [journal_profile(profile_floor)];
        assert_eq!(
            evaluate_with_profiles(
                PersistenceTarget::Journal,
                Ok(newer_runtime),
                EvidenceProfiles {
                    journal: &journal,
                    file_v2: &[],
                },
            )
            .code,
            FilesystemStatusCode::Supported,
            "a runtime newer than the journal profile floor remains authorized"
        );

        let file_v2 = [file_v2_profile(profile_floor)];
        let file_v2_only = EvidenceProfiles {
            journal: &[],
            file_v2: &file_v2,
        };
        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            assert_eq!(
                evaluate_with_profiles(target, Ok(newer_runtime), file_v2_only).code,
                FilesystemStatusCode::Supported,
                "a runtime newer than the file-v2 profile floor remains authorized for {target:?}"
            );
        }
    }

    #[test]
    fn stale_embedded_file_v2_journal_schema_cannot_authorize_durability() {
        let observation = positive_ntfs_observation();
        let mut stale_embedded_journal = file_v2_profile(observation);
        stale_embedded_journal.journal_evidence.schema_version =
            EVIDENCE_MANIFEST_SCHEMA_VERSION + 1;
        let file_v2 = [stale_embedded_journal];
        let stale_file_v2_only = EvidenceProfiles {
            journal: &[],
            file_v2: &file_v2,
        };

        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            assert_eq!(
                evaluate_with_profiles(target, Ok(observation), stale_file_v2_only).code,
                FilesystemStatusCode::DurabilityUnproved,
                "stale embedded journal evidence cannot authorize durability for {target:?}"
            );
        }

        let journal = [journal_profile(observation)];
        let valid_journal_with_stale_file_v2 = EvidenceProfiles {
            journal: &journal,
            file_v2: &file_v2,
        };
        assert_eq!(
            evaluate_with_profiles(
                PersistenceTarget::Journal,
                Ok(observation),
                valid_journal_with_stale_file_v2,
            )
            .code,
            FilesystemStatusCode::Supported,
            "separate valid journal evidence still authorizes journal durability"
        );
        assert_eq!(
            evaluate_with_profiles(
                PersistenceTarget::FileV2,
                Ok(observation),
                valid_journal_with_stale_file_v2,
            )
            .code,
            FilesystemStatusCode::ConfidentialityUnproved,
            "stale embedded journal evidence cannot authorize file-v2 confidentiality"
        );
    }

    #[test]
    fn invalid_file_v2_confidentiality_evidence_preserves_embedded_journal_authority() {
        let observation = positive_ntfs_observation();
        let mut profile = file_v2_profile(observation);
        profile.confidentiality_evidence.schema_version = EVIDENCE_MANIFEST_SCHEMA_VERSION + 1;
        let file_v2 = [profile];
        let profiles = EvidenceProfiles {
            journal: &[],
            file_v2: &file_v2,
        };

        assert_eq!(
            evaluate_with_profiles(PersistenceTarget::Journal, Ok(observation), profiles).code,
            FilesystemStatusCode::Supported
        );
        assert_eq!(
            evaluate_with_profiles(PersistenceTarget::FileV2, Ok(observation), profiles).code,
            FilesystemStatusCode::ConfidentialityUnproved
        );
    }

    #[test]
    fn only_initial_candidate_families_reach_empty_profile_lookup() {
        let candidates = [
            (Platform::Windows, FilesystemFamily::WindowsNtfs),
            (Platform::MacOs, FilesystemFamily::MacApfs),
            (Platform::Linux, FilesystemFamily::LinuxExt4),
        ];
        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            for (platform, family) in candidates {
                let mut observation = positive_ntfs_observation();
                observation.platform = platform;
                observation.family = family;
                assert_eq!(
                    evaluate_filesystem(target, Ok(observation)).code,
                    FilesystemStatusCode::DurabilityUnproved,
                    "candidate {platform:?}/{family:?} remains unproved for {target:?}"
                );
            }
        }

        let non_candidates = [
            (Platform::Windows, FilesystemFamily::WindowsRefs),
            (Platform::MacOs, FilesystemFamily::MacHfsPlus),
            (Platform::Linux, FilesystemFamily::LinuxBtrfs),
            (Platform::Linux, FilesystemFamily::LinuxXfs),
            (Platform::Windows, FilesystemFamily::MacApfs),
            (Platform::MacOs, FilesystemFamily::LinuxExt4),
            (Platform::Linux, FilesystemFamily::WindowsNtfs),
            (Platform::Windows, FilesystemFamily::Other),
        ];
        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            for (platform, family) in non_candidates {
                let mut observation = positive_ntfs_observation();
                observation.platform = platform;
                observation.family = family;
                assert_eq!(
                    evaluate_filesystem(target, Ok(observation)).code,
                    FilesystemStatusCode::FilesystemUnproved,
                    "non-candidate {platform:?}/{family:?} must deny {target:?}"
                );
            }
        }
    }

    #[test]
    fn exact_fake_profiles_never_override_runtime_denials() {
        let denied: &[StatusCase] = &[
            (
                "unknown_writable",
                |observation| observation.writable = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "unknown_local",
                |observation| observation.local = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "unknown_native",
                |observation| observation.kernel_native = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "unknown_cloud",
                |observation| observation.os_managed_cloud_root = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "unknown_fixed",
                |observation| observation.internal_fixed = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "unknown_identity",
                |observation| observation.identity_stable = Ternary::Unknown,
                FilesystemStatusCode::InspectionUnavailable,
            ),
            (
                "changed_identity",
                |observation| observation.identity_stable = Ternary::No,
                FilesystemStatusCode::TargetChanged,
            ),
            (
                "read_only",
                |observation| observation.writable = Ternary::No,
                FilesystemStatusCode::ReadOnly,
            ),
            (
                "remote",
                |observation| observation.local = Ternary::No,
                FilesystemStatusCode::Remote,
            ),
            (
                "userspace",
                |observation| observation.kernel_native = Ternary::No,
                FilesystemStatusCode::UserspaceFilesystem,
            ),
            (
                "cloud_managed",
                |observation| observation.os_managed_cloud_root = Ternary::Yes,
                FilesystemStatusCode::CloudManaged,
            ),
            (
                "removable",
                |observation| observation.internal_fixed = Ternary::No,
                FilesystemStatusCode::RemovableOrHotplug,
            ),
            (
                "recognized_but_not_candidate",
                |observation| observation.family = FilesystemFamily::WindowsRefs,
                FilesystemStatusCode::FilesystemUnproved,
            ),
            (
                "mac_hfs_plus_not_candidate",
                |observation| {
                    observation.platform = Platform::MacOs;
                    observation.family = FilesystemFamily::MacHfsPlus;
                },
                FilesystemStatusCode::FilesystemUnproved,
            ),
            (
                "linux_btrfs_not_candidate",
                |observation| {
                    observation.platform = Platform::Linux;
                    observation.family = FilesystemFamily::LinuxBtrfs;
                },
                FilesystemStatusCode::FilesystemUnproved,
            ),
            (
                "linux_xfs_not_candidate",
                |observation| {
                    observation.platform = Platform::Linux;
                    observation.family = FilesystemFamily::LinuxXfs;
                },
                FilesystemStatusCode::FilesystemUnproved,
            ),
            (
                "other_family",
                |observation| observation.family = FilesystemFamily::Other,
                FilesystemStatusCode::FilesystemUnproved,
            ),
            (
                "access_control_absent",
                |observation| observation.access_controls_enforced = Ternary::No,
                FilesystemStatusCode::AccessControlUnproved,
            ),
            (
                "access_control_unknown",
                |observation| observation.access_controls_enforced = Ternary::Unknown,
                FilesystemStatusCode::AccessControlUnproved,
            ),
        ];

        for (name, deny, expected) in denied {
            let mut observation = positive_ntfs_observation();
            deny(&mut observation);
            let journal = [journal_profile(observation)];
            let file_v2 = [file_v2_profile(observation)];
            let profiles = EvidenceProfiles {
                journal: &journal,
                file_v2: &file_v2,
            };
            for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
                assert_eq!(
                    evaluate_with_profiles(target, Ok(observation), profiles).code,
                    *expected,
                    "{name} must deny {target:?} before exact profile lookup"
                );
            }
        }
    }

    #[test]
    fn required_unknowns_precede_every_coexistent_runtime_denial_with_exact_profiles() {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum ObservationField {
            Writable,
            Local,
            KernelNative,
            InternalFixed,
            OsManagedCloudRoot,
            IdentityStable,
            Family,
            AccessControlsEnforced,
        }

        struct MutationCase {
            name: &'static str,
            field: ObservationField,
            apply: ObservationMutation,
        }

        let required_unknowns: &[MutationCase] = &[
            MutationCase {
                name: "writable",
                field: ObservationField::Writable,
                apply: |observation| observation.writable = Ternary::Unknown,
            },
            MutationCase {
                name: "local",
                field: ObservationField::Local,
                apply: |observation| observation.local = Ternary::Unknown,
            },
            MutationCase {
                name: "kernel_native",
                field: ObservationField::KernelNative,
                apply: |observation| observation.kernel_native = Ternary::Unknown,
            },
            MutationCase {
                name: "internal_fixed",
                field: ObservationField::InternalFixed,
                apply: |observation| observation.internal_fixed = Ternary::Unknown,
            },
            MutationCase {
                name: "os_managed_cloud_root",
                field: ObservationField::OsManagedCloudRoot,
                apply: |observation| observation.os_managed_cloud_root = Ternary::Unknown,
            },
            MutationCase {
                name: "identity_stable",
                field: ObservationField::IdentityStable,
                apply: |observation| observation.identity_stable = Ternary::Unknown,
            },
        ];
        let lower_denials: &[MutationCase] = &[
            MutationCase {
                name: "target_changed",
                field: ObservationField::IdentityStable,
                apply: |observation| observation.identity_stable = Ternary::No,
            },
            MutationCase {
                name: "read_only",
                field: ObservationField::Writable,
                apply: |observation| observation.writable = Ternary::No,
            },
            MutationCase {
                name: "remote",
                field: ObservationField::Local,
                apply: |observation| observation.local = Ternary::No,
            },
            MutationCase {
                name: "userspace_filesystem",
                field: ObservationField::KernelNative,
                apply: |observation| observation.kernel_native = Ternary::No,
            },
            MutationCase {
                name: "cloud_managed",
                field: ObservationField::OsManagedCloudRoot,
                apply: |observation| observation.os_managed_cloud_root = Ternary::Yes,
            },
            MutationCase {
                name: "removable_or_hotplug",
                field: ObservationField::InternalFixed,
                apply: |observation| observation.internal_fixed = Ternary::No,
            },
            MutationCase {
                name: "filesystem_unproved",
                field: ObservationField::Family,
                apply: |observation| observation.family = FilesystemFamily::Other,
            },
            MutationCase {
                name: "access_control_unproved",
                field: ObservationField::AccessControlsEnforced,
                apply: |observation| observation.access_controls_enforced = Ternary::No,
            },
        ];

        let mut checked_pairs = 0;
        for required_unknown in required_unknowns {
            for lower_denial in lower_denials {
                if required_unknown.field == lower_denial.field {
                    // One ternary field cannot simultaneously hold Unknown and its
                    // denying value. Singleton coverage above proves those six cases.
                    continue;
                }

                let mut observation = positive_ntfs_observation();
                (required_unknown.apply)(&mut observation);
                (lower_denial.apply)(&mut observation);
                let journal = [journal_profile(observation)];
                let file_v2 = [file_v2_profile(observation)];
                let profiles = EvidenceProfiles {
                    journal: &journal,
                    file_v2: &file_v2,
                };

                for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
                    assert_eq!(
                        evaluate_with_profiles(target, Ok(observation), profiles).code,
                        FilesystemStatusCode::InspectionUnavailable,
                        "{}=Unknown must precede lower denial {} for {target:?}",
                        required_unknown.name,
                        lower_denial.name,
                    );
                }
                checked_pairs += 1;
            }
        }

        assert_eq!(
            checked_pairs, 42,
            "the bounded pair matrix must stay exhaustive"
        );
    }

    #[test]
    fn access_control_unknown_keeps_deliberate_precedence_with_exact_profiles() {
        let mut observation = positive_ntfs_observation();
        observation.access_controls_enforced = Ternary::Unknown;
        let journal = [journal_profile(observation)];
        let file_v2 = [file_v2_profile(observation)];
        let profiles = EvidenceProfiles {
            journal: &journal,
            file_v2: &file_v2,
        };

        for target in [PersistenceTarget::Journal, PersistenceTarget::FileV2] {
            assert_eq!(
                evaluate_with_profiles(target, Ok(observation), profiles).code,
                FilesystemStatusCode::AccessControlUnproved,
                "access-control Unknown is deliberate policy, not inspection failure for {target:?}"
            );
        }
    }

    #[test]
    fn detector_internals_cannot_escape_closed_status_serialization_or_debug() {
        const PATH_CANARY: &str = "private-path-canary";
        const VOLUME_CANARY: &str = "volume-identity-canary";
        const DEVICE_CANARY: &str = "device-identity-canary";
        const PROVIDER_CANARY: &str = "provider-identity-canary";
        const NATIVE_PROSE_CANARY: &str = "native-error-prose-canary";
        const NATIVE_CODE_CANARY: u32 = 4_294_000_001;

        struct AdapterOnlyTarget {
            path: &'static str,
            volume_id: &'static str,
            device_id: &'static str,
            provider_id: &'static str,
            native_code: u32,
            native_prose: &'static str,
        }

        struct ClosedFakeDetector;

        impl FilesystemDetector for ClosedFakeDetector {
            type HeldTarget = AdapterOnlyTarget;

            fn inspect(
                &self,
                target: &Self::HeldTarget,
            ) -> Result<FilesystemObservation, DetectorFault> {
                let transient_adapter_only_values = (
                    target.path,
                    target.volume_id,
                    target.device_id,
                    target.provider_id,
                    target.native_code,
                    target.native_prose,
                );
                assert!(!transient_adapter_only_values.0.is_empty());
                Err(DetectorFault::InspectionUnavailable)
            }
        }

        let target = AdapterOnlyTarget {
            path: PATH_CANARY,
            volume_id: VOLUME_CANARY,
            device_id: DEVICE_CANARY,
            provider_id: PROVIDER_CANARY,
            native_code: NATIVE_CODE_CANARY,
            native_prose: NATIVE_PROSE_CANARY,
        };
        let detector = ClosedFakeDetector;
        let status = evaluate_filesystem(PersistenceTarget::Journal, detector.inspect(&target));
        let serialized = serde_json::to_string(&status).expect("closed status JSON");
        let debug = format!("{status:?}");

        assert_eq!(
            serde_json::to_value(status).expect("closed status value"),
            serde_json::json!({
                "target": "journal",
                "code": "inspection_unavailable",
                "family": null,
                "detector_schema": FILESYSTEM_DETECTOR_SCHEMA_VERSION,
            })
        );
        for forbidden in [
            PATH_CANARY,
            VOLUME_CANARY,
            DEVICE_CANARY,
            PROVIDER_CANARY,
            NATIVE_PROSE_CANARY,
        ] {
            assert!(!serialized.contains(forbidden));
            assert!(!debug.contains(forbidden));
        }
        let native_code = NATIVE_CODE_CANARY.to_string();
        assert!(!serialized.contains(&native_code));
        assert!(!debug.contains(&native_code));
    }
}
