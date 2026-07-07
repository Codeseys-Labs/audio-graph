//! Small Rust-owned IPC contract surfaces that can be exported without linking
//! the full Tauri application.

use serde::{Deserialize, Serialize};

pub mod endpoint_credential_routing;
pub mod session_data_movement;

/// Audio source information emitted by the backend source discovery path.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AudioSourceInfo {
    pub id: String,
    pub name: String,
    pub source_type: AudioSourceType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capture_target: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_kind: Option<AudioDeviceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supported_formats: Vec<AudioFormatInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_format: Option<AudioFormatInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub channel_provenance: Option<AudioSourceChannelProvenance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<AudioSourceCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_status: Option<AudioPermissionStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_recovery: Option<AudioPermissionRecoveryHint>,
    pub is_active: bool,
}

/// Type of audio source.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type")]
pub enum AudioSourceType {
    SystemDefault,
    Device {
        device_id: String,
    },
    Application {
        pid: u32,
        app_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        bundle_id: Option<String>,
    },
    ApplicationName {
        app_name: String,
    },
    ProcessTree {
        pid: u32,
    },
}

/// Endpoint direction for a device source when the platform backend can resolve it.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioDeviceKind {
    Input,
    Output,
}

/// A serializable snapshot of an rsac-supported capture format.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct AudioFormatInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub sample_format: AudioSampleFormat,
}

/// How AudioGraph should interpret source channel identity.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioSourceChannelLayout {
    Unknown,
    Mono,
    SourceNative,
    MixedMono,
    GeneratedSpeakerLanes,
}

/// Provenance for a source/channel identity claim.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioChannelProvenanceKind {
    Unknown,
    Physical,
    AppProcessDerived,
    VirtualMeetingLane,
    GeneratedSourceSeparation,
    Mixed,
}

/// One channel lane in a source descriptor.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct AudioSourceChannelInfo {
    pub index: u16,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub provenance: AudioChannelProvenanceKind,
}

/// Source-level channel provenance and admission metadata.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct AudioSourceChannelProvenance {
    pub layout: AudioSourceChannelLayout,
    pub provenance: AudioChannelProvenanceKind,
    pub source_native: bool,
    pub channel_count: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<AudioSourceChannelInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub negotiated_format: Option<AudioFormatInfo>,
}

impl AudioSourceChannelProvenance {
    pub fn unknown_for_format(negotiated_format: Option<AudioFormatInfo>) -> Self {
        Self::fallback_for_format(AudioChannelProvenanceKind::Unknown, negotiated_format)
    }

    pub fn fallback_for_format(
        provenance: AudioChannelProvenanceKind,
        negotiated_format: Option<AudioFormatInfo>,
    ) -> Self {
        let channel_count = negotiated_format
            .as_ref()
            .map(|format| format.channels.max(1))
            .unwrap_or(1);
        Self {
            layout: if channel_count == 1 {
                AudioSourceChannelLayout::Mono
            } else {
                AudioSourceChannelLayout::MixedMono
            },
            provenance,
            source_native: false,
            channel_count,
            channels: channel_infos(channel_count, provenance),
            negotiated_format,
        }
    }

    pub fn source_native(
        provenance: AudioChannelProvenanceKind,
        channels: Vec<AudioSourceChannelInfo>,
        negotiated_format: Option<AudioFormatInfo>,
    ) -> Self {
        Self {
            layout: AudioSourceChannelLayout::SourceNative,
            provenance,
            source_native: true,
            channel_count: channels.len() as u16,
            channels,
            negotiated_format,
        }
    }

    pub fn is_source_native_admissible(&self) -> bool {
        if self.layout != AudioSourceChannelLayout::SourceNative
            || !self.source_native
            || self.channel_count < 2
            || self.channels.len() != self.channel_count as usize
        {
            return false;
        }

        if matches!(
            self.provenance,
            AudioChannelProvenanceKind::Unknown
                | AudioChannelProvenanceKind::Mixed
                | AudioChannelProvenanceKind::GeneratedSourceSeparation
        ) {
            return false;
        }

        self.channels.iter().enumerate().all(|(expected, channel)| {
            channel.index as usize == expected
                && !matches!(
                    channel.provenance,
                    AudioChannelProvenanceKind::Unknown
                        | AudioChannelProvenanceKind::Mixed
                        | AudioChannelProvenanceKind::GeneratedSourceSeparation
                )
        })
    }

    pub fn requires_mono_fallback(&self) -> bool {
        !self.is_source_native_admissible()
    }
}

fn channel_infos(
    channel_count: u16,
    provenance: AudioChannelProvenanceKind,
) -> Vec<AudioSourceChannelInfo> {
    (0..channel_count)
        .map(|index| AudioSourceChannelInfo {
            index,
            id: format!("ch{index}"),
            label: None,
            provenance,
        })
        .collect()
}

/// PCM sample format exposed to the frontend.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioSampleFormat {
    I16,
    I24,
    I32,
    F32,
}

/// Source-specific platform capability projection for UI gating.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct AudioSourceCapabilities {
    pub backend_name: String,
    pub capture_supported: bool,
    pub supports_system_capture: bool,
    pub supports_application_capture: bool,
    pub supports_process_tree_capture: bool,
    pub supports_device_selection: bool,
    pub supports_device_change_notifications: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unsupported_reason: Option<String>,
}

/// Capture permission status for a source when the platform has an OS gate.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioPermissionStatus {
    Granted,
    NotDetermined,
    Denied,
    NotRequired,
    Unknown,
}

/// Backend-issued display metadata for recovering from capture permission blocks.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct AudioPermissionRecoveryHint {
    pub platform: AudioPermissionRecoveryPlatform,
    pub permission_kind: AudioPermissionKind,
    pub summary: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actions: Vec<AudioPermissionRecoveryAction>,
}

/// Platform family that owns the permission recovery copy.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioPermissionRecoveryPlatform {
    Macos,
    Linux,
    Windows,
    Unknown,
}

/// Permission surface the user needs to repair.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioPermissionKind {
    AudioCapture,
    PipewireAccess,
    WindowsAccess,
    Unknown,
}

/// Display-only recovery action. The frontend must not assume these are native commands.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub struct AudioPermissionRecoveryAction {
    pub kind: AudioPermissionRecoveryActionKind,
    pub label: String,
}

/// Non-executable action kind for permission recovery UX.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, schemars::JsonSchema, PartialEq, Eq)]
pub enum AudioPermissionRecoveryActionKind {
    GrantPermissionManually,
    RelaunchApp,
    RefreshSources,
}

pub fn audio_source_info_schema_json() -> serde_json::Value {
    serde_json::to_value(schemars::schema_for!(AudioSourceInfo))
        .expect("AudioSourceInfo schema should serialize")
}

pub fn audio_source_contract_typescript_module() -> String {
    let schema = serde_json::to_string_pretty(&audio_source_info_schema_json())
        .expect("AudioSourceInfo schema should serialize");
    let schema_literal = js_single_quoted_string_literal(&schema);
    format!(
        r#"// @generated by src-tauri/crates/ipc-contract/src/lib.rs. Do not edit manually.

export type SourceId = string;

export type AudioDeviceKind = "Input" | "Output";

export type AudioSampleFormat = "I16" | "I24" | "I32" | "F32";

export interface AudioFormatInfo {{
  sample_rate: number;
  channels: number;
  sample_format: AudioSampleFormat;
}}

export type AudioSourceChannelLayout =
  | "Unknown"
  | "Mono"
  | "SourceNative"
  | "MixedMono"
  | "GeneratedSpeakerLanes";

export type AudioChannelProvenanceKind =
  | "Unknown"
  | "Physical"
  | "AppProcessDerived"
  | "VirtualMeetingLane"
  | "GeneratedSourceSeparation"
  | "Mixed";

export interface AudioSourceChannelInfo {{
  index: number;
  id: string;
  label?: string | null;
  provenance: AudioChannelProvenanceKind;
}}

export interface AudioSourceChannelProvenance {{
  layout: AudioSourceChannelLayout;
  provenance: AudioChannelProvenanceKind;
  source_native: boolean;
  channel_count: number;
  channels?: AudioSourceChannelInfo[];
  negotiated_format?: AudioFormatInfo | null;
}}

export type AudioPermissionStatus =
  | "Granted"
  | "NotDetermined"
  | "Denied"
  | "NotRequired"
  | "Unknown";

export type AudioPermissionRecoveryPlatform =
  | "Macos"
  | "Linux"
  | "Windows"
  | "Unknown";

export type AudioPermissionKind =
  | "AudioCapture"
  | "PipewireAccess"
  | "WindowsAccess"
  | "Unknown";

export type AudioPermissionRecoveryActionKind =
  | "GrantPermissionManually"
  | "RelaunchApp"
  | "RefreshSources";

export interface AudioPermissionRecoveryAction {{
  kind: AudioPermissionRecoveryActionKind;
  label: string;
}}

export interface AudioPermissionRecoveryHint {{
  platform: AudioPermissionRecoveryPlatform;
  permission_kind: AudioPermissionKind;
  summary: string;
  body: string;
  actions?: AudioPermissionRecoveryAction[];
}}

export interface AudioSourceCapabilities {{
  backend_name: string;
  capture_supported: boolean;
  supports_system_capture: boolean;
  supports_application_capture: boolean;
  supports_process_tree_capture: boolean;
  supports_device_selection: boolean;
  supports_device_change_notifications: boolean;
  unsupported_reason?: string | null;
}}

export type AudioSourceType =
  | {{ type: "SystemDefault" }}
  | {{ type: "Device"; device_id: string }}
  | {{
      type: "Application";
      pid: number;
      app_name: string;
      bundle_id?: string | null;
    }}
  | {{ type: "ApplicationName"; app_name: string }}
  | {{ type: "ProcessTree"; pid: number }};

export interface AudioSourceInfo {{
  id: SourceId;
  name: string;
  source_type: AudioSourceType;
  capture_target?: SourceId | null;
  device_kind?: AudioDeviceKind | null;
  is_default?: boolean | null;
  supported_formats?: AudioFormatInfo[];
  default_format?: AudioFormatInfo | null;
  channel_provenance?: AudioSourceChannelProvenance | null;
  capabilities?: AudioSourceCapabilities | null;
  permission_status?: AudioPermissionStatus | null;
  permission_recovery?: AudioPermissionRecoveryHint | null;
  is_active: boolean;
}}

export const AUDIO_SOURCE_INFO_SCHEMA_JSON =
  {schema_literal};

export const AUDIO_SOURCE_INFO_SCHEMA = JSON.parse(
  AUDIO_SOURCE_INFO_SCHEMA_JSON,
) as Record<string, unknown>;
"#
    )
}

pub(crate) fn js_single_quoted_string_literal(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('\'');
    for ch in value.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '\'' => out.push_str("\\'"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch.is_control() => {
                use std::fmt::Write;
                write!(&mut out, "\\u{:04x}", ch as u32).expect("write to string");
            }
            ch => out.push(ch),
        }
    }
    out.push('\'');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_audio_source_contract_contains_schema_and_process_tree_variant() {
        let module = audio_source_contract_typescript_module();
        assert!(module.contains("export interface AudioSourceInfo"));
        assert!(module.contains("export interface AudioSourceChannelProvenance"));
        assert!(module.contains("channel_provenance?: AudioSourceChannelProvenance | null"));
        assert!(module.contains(r#"{ type: "ProcessTree"; pid: number }"#));
        assert!(module.contains("export interface AudioPermissionRecoveryHint"));
        assert!(module.contains("permission_recovery?: AudioPermissionRecoveryHint | null"));
        assert!(module.contains("AUDIO_SOURCE_INFO_SCHEMA_JSON"));
        assert!(module.contains("AudioSourceInfo"));
    }

    #[test]
    fn audio_source_info_accepts_omitted_optional_recovery_and_bundle_id() {
        let raw = serde_json::json!({
            "id": "app:42",
            "name": "Design Tool",
            "source_type": {
                "type": "Application",
                "pid": 42,
                "app_name": "Design Tool"
            },
            "permission_status": "Denied",
            "is_active": false
        });

        let source: AudioSourceInfo =
            serde_json::from_value(raw).expect("optional fields should deserialize");
        assert_eq!(source.permission_recovery, None);
        assert!(matches!(
            source.source_type,
            AudioSourceType::Application {
                bundle_id: None,
                ..
            }
        ));
    }

    #[test]
    fn audio_source_info_round_trips_permission_recovery() {
        let source = AudioSourceInfo {
            id: "app:42".to_string(),
            name: "Design Tool".to_string(),
            source_type: AudioSourceType::Application {
                pid: 42,
                app_name: "Design Tool".to_string(),
                bundle_id: Some("com.example.DesignTool".to_string()),
            },
            capture_target: Some("app:42".to_string()),
            device_kind: None,
            is_default: Some(false),
            supported_formats: Vec::new(),
            default_format: None,
            channel_provenance: Some(AudioSourceChannelProvenance::fallback_for_format(
                AudioChannelProvenanceKind::AppProcessDerived,
                None,
            )),
            capabilities: None,
            permission_status: Some(AudioPermissionStatus::Denied),
            permission_recovery: Some(AudioPermissionRecoveryHint {
                platform: AudioPermissionRecoveryPlatform::Macos,
                permission_kind: AudioPermissionKind::AudioCapture,
                summary: "Audio Capture permission is denied.".to_string(),
                body: "Grant permission, relaunch AudioGraph, then refresh sources.".to_string(),
                actions: vec![AudioPermissionRecoveryAction {
                    kind: AudioPermissionRecoveryActionKind::GrantPermissionManually,
                    label: "Grant permission manually".to_string(),
                }],
            }),
            is_active: false,
        };

        let value = serde_json::to_value(&source).expect("source should serialize");
        assert_eq!(value["permission_recovery"]["platform"], "Macos");
        assert_eq!(
            value["permission_recovery"]["permission_kind"],
            "AudioCapture"
        );
        assert_eq!(value["source_type"]["bundle_id"], "com.example.DesignTool");
        assert_eq!(value["channel_provenance"]["source_native"], false);

        let round_trip: AudioSourceInfo =
            serde_json::from_value(value).expect("source should deserialize");
        assert_eq!(round_trip.permission_recovery, source.permission_recovery);
        assert_eq!(round_trip.channel_provenance, source.channel_provenance);
    }

    #[test]
    fn channel_provenance_rejects_misleading_stereo_and_generated_lanes() {
        let stereo = AudioSourceChannelProvenance::unknown_for_format(Some(AudioFormatInfo {
            sample_rate: 48_000,
            channels: 2,
            sample_format: AudioSampleFormat::F32,
        }));
        assert_eq!(stereo.channel_count, 2);
        assert!(stereo.requires_mono_fallback());

        let generated = AudioSourceChannelProvenance {
            layout: AudioSourceChannelLayout::GeneratedSpeakerLanes,
            provenance: AudioChannelProvenanceKind::GeneratedSourceSeparation,
            source_native: true,
            channel_count: 2,
            channels: vec![
                AudioSourceChannelInfo {
                    index: 0,
                    id: "speaker-lane-0".to_string(),
                    label: Some("Speaker lane 0".to_string()),
                    provenance: AudioChannelProvenanceKind::GeneratedSourceSeparation,
                },
                AudioSourceChannelInfo {
                    index: 1,
                    id: "speaker-lane-1".to_string(),
                    label: Some("Speaker lane 1".to_string()),
                    provenance: AudioChannelProvenanceKind::GeneratedSourceSeparation,
                },
            ],
            negotiated_format: None,
        };
        assert!(generated.requires_mono_fallback());
    }

    #[test]
    fn source_native_channel_provenance_preserves_channel_order() {
        let source_native = AudioSourceChannelProvenance::source_native(
            AudioChannelProvenanceKind::VirtualMeetingLane,
            vec![
                AudioSourceChannelInfo {
                    index: 0,
                    id: "meeting-host".to_string(),
                    label: Some("Host lane".to_string()),
                    provenance: AudioChannelProvenanceKind::VirtualMeetingLane,
                },
                AudioSourceChannelInfo {
                    index: 1,
                    id: "meeting-guest".to_string(),
                    label: Some("Guest lane".to_string()),
                    provenance: AudioChannelProvenanceKind::VirtualMeetingLane,
                },
            ],
            Some(AudioFormatInfo {
                sample_rate: 48_000,
                channels: 2,
                sample_format: AudioSampleFormat::F32,
            }),
        );

        assert!(source_native.is_source_native_admissible());
        let ordered_ids: Vec<&str> = source_native
            .channels
            .iter()
            .map(|channel| channel.id.as_str())
            .collect();
        assert_eq!(ordered_ids, vec!["meeting-host", "meeting-guest"]);
    }
}
