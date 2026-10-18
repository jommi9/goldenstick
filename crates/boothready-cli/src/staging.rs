use anyhow::{bail, Context, Result};
use boothready_core::rules::Ruleset;
use boothready_core::sources::{cloud_provider, count_states, device_for_path, os_placeholder, scan_source};
use boothready_core::staging::{
    checked_path, inventory, stage, stage_selected, verify_collection, Collection, SourceKind, StagePhase,
};
use boothready_model::{BusType, PhysicalDevice};
use clap::{Subcommand, ValueEnum};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

#[derive(Clone, Copy, ValueEnum)]
pub enum Kind {
    Local,
    GoogleDriveDesktop,
}
#[derive(Subcommand)]
pub enum LibraryCmd {
    /// List the music in a folder and check it against the selected players.
    /// Cloud-only files are counted from their metadata and never downloaded.
    Scan {
        source: PathBuf,
        #[command(flatten)]
        targets: crate::Targets,
    },
    /// Rehash a staged collection while its source is offline.
    Verify { collection: PathBuf },
    /// Read a source folder into a verified local collection. Rerun to resume.
    Stage {
        source: PathBuf,
        destination: PathBuf,
        #[arg(long, value_enum, default_value = "local")]
        kind: Kind,
        /// Stage only this file, given relative to the source. Repeat for more.
        /// Nothing else in the source is read.
        #[arg(long, value_name = "RELATIVE_PATH")]
        only: Vec<PathBuf>,
    },
}
fn guard_destination(path: &Path, devices: &[PhysicalDevice]) -> Result<()> {
    let mut ancestor = path;
    while !ancestor.exists() {
        ancestor = ancestor.parent().context("no existing destination ancestor")?;
    }
    let ancestor = ancestor.canonicalize()?;
    #[cfg(windows)]
    if !matches!(ancestor.components().next(), Some(std::path::Component::Prefix(p)) if matches!(p.kind(), std::path::Prefix::Disk(_) | std::path::Prefix::VerbatimDisk(_)))
    {
        bail!("network destinations are unsupported");
    }
    let mut candidates = Vec::new();
    for device in devices {
        for volume in &device.volumes {
            if let Some(mount) = &volume.mount_point {
                let mount = mount.canonicalize()?;
                let prefix = ancestor.starts_with(&mount);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if std::fs::metadata(&ancestor)?.dev() != std::fs::metadata(&mount)?.dev() {
                        continue;
                    }
                }
                #[cfg(not(unix))]
                if !prefix {
                    continue;
                }
                // APFS firmlinks can share a filesystem without a textual prefix.
                candidates.push((if prefix { mount.components().count() } else { 0 }, device));
            }
        }
    }
    let (_, device) = candidates
        .into_iter()
        .max_by_key(|(depth, _)| *depth)
        .context("destination storage is unknown; staging refused")?;
    if device.bus == BusType::Usb || device.removable || !device.is_system {
        bail!("staging requires a folder on the computer's system volume");
    }
    Ok(())
}
fn guard_capacity(collection: &Collection, space: boothready_platform::Space) -> Result<()> {
    let cluster = space.cluster_bytes.max(1);
    let mut required = 1024_u64 * 1024; // directory and manifest allowance
    for file in &collection.files {
        let allocated = file.bytes.div_ceil(cluster).checked_mul(cluster).context("collection size overflow")?;
        required = required.checked_add(allocated).context("collection size overflow")?;
    }
    let manifest_bytes = u64::try_from(serde_json::to_vec_pretty(collection)?.len())?;
    required = required
        .checked_add(manifest_bytes.checked_mul(2).context("manifest size overflow")?)
        .context("collection size overflow")?;
    if required > space.free_bytes {
        bail!("insufficient local space: need {required} bytes, available {}", space.free_bytes);
    }
    Ok(())
}

pub fn run(command: &LibraryCmd, json: bool, rules: &Ruleset) -> Result<()> {
    match command {
        LibraryCmd::Scan { source, targets } => {
            let players = crate::resolve_targets(rules, targets)?;
            let source = source.canonicalize().with_context(|| format!("can't open {}", source.display()))?;
            let scan = scan_source(&source, &os_placeholder, &AtomicBool::new(false), |_| {})?;
            if json {
                println!("{}", serde_json::to_string_pretty(&scan)?);
                return Ok(());
            }
            let devices = boothready_platform::native().list_devices().unwrap_or_default();
            let location = match (cloud_provider(&source), device_for_path(&source, &devices)) {
                (Some(provider), _) => format!("{provider} folder"),
                (None, Some(device)) if device.is_system => "this computer".to_string(),
                (None, Some(_)) => "an external drive".to_string(),
                (None, None) => "unknown storage".to_string(),
            };
            let totals = scan.totals();
            let counts = count_states(&scan, &players);
            println!("{} tracks, {} bytes, on {location}", totals.tracks, totals.bytes);
            println!("Checked against {} players:", players.len());
            println!("  ready to export: {}", counts.ready);
            println!("  still in the cloud, format unknown until downloaded: {}", counts.cloud_only);
            println!("  won't play on at least one player: {}", counts.blocked);
            println!("  no compatibility data on at least one player: {}", counts.unchecked);
            println!("  damaged or not audio: {}", counts.unreadable);
            if scan.skipped > 0 {
                println!("  folders or files that could not be read: {}", scan.skipped);
            }
        }
        LibraryCmd::Verify { collection } => {
            let collection = verify_collection(collection)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&collection)?);
            } else {
                println!("Verified {} files. Collection revision: {}", collection.files.len(), collection.revision);
            }
        }
        LibraryCmd::Stage { source, destination, kind, only } => {
            let destination = checked_path(destination)?;
            guard_destination(&destination, &boothready_platform::native().list_devices()?)?;
            let kind = match kind {
                Kind::Local => SourceKind::Local,
                Kind::GoogleDriveDesktop => SourceKind::GoogleDriveDesktop,
            };
            let parent = destination.parent().context("destination needs a parent")?;
            let space = boothready_platform::space(parent)?;
            let collection = if only.is_empty() {
                let plan = inventory(source, kind)?;
                guard_capacity(&plan, space)?;
                stage(source, kind, &destination, &AtomicBool::new(false))?
            } else {
                // Sizes come from metadata, so the check downloads nothing.
                let mut required = 4 * 1024 * 1024_u64;
                for relative in only {
                    let size = std::fs::metadata(source.join(relative))
                        .with_context(|| format!("can't find {} in the source", relative.display()))?
                        .len();
                    required = required.saturating_add(size).saturating_add(space.cluster_bytes.max(1));
                }
                if required > space.free_bytes {
                    bail!("insufficient local space: need {required} bytes, available {}", space.free_bytes);
                }
                let mut phase = None;
                stage_selected(source, kind, Some(only), &destination, &AtomicBool::new(false), &mut |p| {
                    if !json && phase != Some(p.phase) {
                        phase = Some(p.phase);
                        eprintln!(
                            "{}",
                            match p.phase {
                                StagePhase::Reading => "Reading the selected files",
                                StagePhase::Copying => "Copying",
                                StagePhase::Verifying => "Checking every copy",
                            }
                        );
                    }
                })?
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&collection)?);
            } else {
                println!("Verified {} files. Collection revision: {}", collection.files.len(), collection.revision);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::device;
    #[test]
    fn insufficient_space_is_rejected_before_staging() {
        let plan = Collection {
            version: 1,
            kind: SourceKind::Local,
            source: PathBuf::from("synthetic"),
            revision: "synthetic".into(),
            files: vec![],
        };
        assert!(guard_capacity(&plan, boothready_platform::Space { free_bytes: 1, cluster_bytes: 4096 }).is_err());
        assert!(guard_capacity(&plan, boothready_platform::Space { free_bytes: 2 * 1024 * 1024, cluster_bytes: 4096 })
            .is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn same_filesystem_without_prefix_supports_firmlinks() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let mount = root.join("mount");
        std::fs::create_dir(&mount).unwrap();
        assert!(guard_destination(&root.join("collection"), &[device(mount, BusType::Nvme, false, true)]).is_ok());
    }
    #[test]
    fn destination_requires_known_system_storage_and_rejects_nested_usb() {
        let root = tempfile::tempdir().unwrap();
        let mount = root.path().canonicalize().unwrap();
        let dest = mount.join("collection");
        assert!(guard_destination(&dest, &[]).is_err());
        let internal = device(mount.clone(), BusType::Nvme, false, true);
        assert!(guard_destination(&dest, std::slice::from_ref(&internal)).is_ok());
        for (bus, removable, system) in
            [(BusType::Usb, false, true), (BusType::Nvme, true, true), (BusType::Nvme, false, false)]
        {
            assert!(guard_destination(&dest, &[device(mount.clone(), bus, removable, system)]).is_err());
        }
        let nested = mount.join("usb");
        std::fs::create_dir(&nested).unwrap();
        assert!(guard_destination(&nested.join("collection"), &[internal, device(nested, BusType::Usb, true, false)])
            .is_err());
    }
}
