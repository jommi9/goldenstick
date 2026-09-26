//! One-call analysis of an inserted drive, shared by the CLI and the app.

use crate::identify::{identify, Identification, UsbCatalog};
use crate::library::{scan_libraries, LibraryReport};
use crate::manifest::{read_manifest, verification_still_valid, Manifest};
use crate::media::filesystem::FilesystemInfo;
use crate::media::partition::{PartitionEntry, PartitionKind};
use crate::media::{InspectedPartition, MediaLayout};
use crate::privileged::{eligibility, DeviceFingerprint, Eligibility};
use crate::rules::{general_headline, DriveFacts, Ruleset};
use crate::scan::{scan_audio, AudioScan, ScanProgress, ScannedTrack};
use boothready_model::{PartitionScheme, PhysicalDevice};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Where the partition/filesystem facts came from. Raw reads need elevated
/// rights; without them we rely on what the OS reports, which lacks some
/// details (dirty flag, MBR type byte).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutSource {
    Raw,
    Os,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContentStats {
    pub files: u64,
    pub used_bytes: u64,
    pub last_modified_unix: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriveReport {
    pub device: PhysicalDevice,
    pub fingerprint: DeviceFingerprint,
    pub identification: Identification,
    pub eligibility: Eligibility,
    pub layout: MediaLayout,
    pub layout_source: LayoutSource,
    pub mount_point: Option<String>,
    pub content: ContentStats,
    pub libraries: LibraryReport,
    pub audio: AudioScan,
    pub facts: DriveFacts,
    pub headline: String,
    pub manifest: Option<Manifest>,
    /// A passing verification exists and nothing changed since.
    pub verified: bool,
    /// A BoothReady preparation started on this drive and never finished.
    pub interrupted: bool,
}

/// Build a layout from what the OS tells us about the device.
pub fn layout_from_os(dev: &PhysicalDevice) -> MediaLayout {
    let partitions = dev
        .volumes
        .iter()
        .enumerate()
        .map(|(i, v)| InspectedPartition {
            entry: PartitionEntry {
                index: i as u32 + 1,
                start_bytes: v.offset_bytes.unwrap_or(0),
                size_bytes: v.size_bytes,
                kind: PartitionKind::Other,
                mbr_type: None,
                gpt_type: None,
                gpt_name: None,
                bootable: false,
            },
            filesystem: v.filesystem.map(|kind| FilesystemInfo {
                kind,
                label: v.label.clone(),
                serial: v.uuid.clone(),
                cluster_size: None,
                bytes_per_sector: None,
                total_bytes: Some(v.size_bytes),
                dirty: None,
                warnings: vec![],
            }),
            aligned_1mib: v.offset_bytes.is_none_or(|o| o % (1 << 20) == 0),
        })
        .collect();
    MediaLayout {
        size_bytes: dev.size_bytes,
        scheme: dev.partition_scheme.unwrap_or(PartitionScheme::Unknown),
        sector_size: dev.logical_sector_size,
        disk_id: None,
        partitions,
        warnings: vec![],
    }
}

pub fn content_stats(root: &Path) -> ContentStats {
    let mut s = ContentStats::default();
    for e in walkdir::WalkDir::new(root).into_iter().filter_map(Result::ok).filter(|e| e.file_type().is_file()) {
        if let Ok(m) = e.metadata() {
            s.files += 1;
            s.used_bytes += m.len();
            if let Some(t) = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()) {
                s.last_modified_unix = Some(s.last_modified_unix.unwrap_or(0).max(t.as_secs()));
            }
        }
    }
    s
}

/// Identify, inspect and scan a drive. `raw_layout` is the helper's raw
/// inspection when available. `on_track` streams audio results as they come.
pub fn analyze_drive(
    dev: &PhysicalDevice,
    raw_layout: Option<MediaLayout>,
    rules: &Ruleset,
    catalog: &UsbCatalog,
    on_track: impl FnMut(&ScanProgress, &ScannedTrack),
) -> DriveReport {
    let identification = identify(dev, catalog);
    let (layout, layout_source) = match raw_layout {
        Some(l) => (l, LayoutSource::Raw),
        None => (layout_from_os(dev), LayoutSource::Os),
    };
    let root = dev.primary_mount().and_then(|v| v.mount_point.clone());
    let (libraries, audio, content, manifest, verified) = match &root {
        Some(r) => {
            let libs = scan_libraries(r);
            let audio = scan_audio(r, on_track);
            let manifest = read_manifest(r);
            let verified = manifest.as_ref().is_some_and(|m| verification_still_valid(r, m));
            (libs, audio, content_stats(r), manifest, verified)
        }
        None => (
            LibraryReport { formats: vec![], rekordbox: None, engine: None, serato: None },
            AudioScan::default(),
            ContentStats::default(),
            None,
            false,
        ),
    };
    let facts = DriveFacts::from_parts(
        &layout,
        &libraries,
        &audio,
        dev.usb.as_ref().and_then(|u| u.max_power_ma),
        identification.catalog_id.clone(),
    );
    let headline = general_headline(&facts, rules);
    let interrupted = manifest.as_ref().is_some_and(|m| m.is_interrupted());
    DriveReport {
        fingerprint: DeviceFingerprint::of(dev),
        eligibility: eligibility(dev),
        device: dev.clone(),
        identification,
        layout,
        layout_source,
        mount_point: root.map(|r| r.to_string_lossy().into_owned()),
        content,
        libraries,
        audio,
        facts,
        headline,
        manifest,
        verified,
        interrupted,
    }
}
