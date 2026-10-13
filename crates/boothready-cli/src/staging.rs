use anyhow::{bail, Context, Result};
use boothready_core::staging::{checked_path, inventory, stage, verify_collection, Collection, SourceKind};
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
    /// Rehash a staged collection while its source is offline.
    Verify { collection: PathBuf },
    /// Read a source folder into a verified local collection. Rerun to resume.
    Stage {
        source: PathBuf,
        destination: PathBuf,
        #[arg(long, value_enum, default_value = "local")]
        kind: Kind,
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

pub fn run(command: &LibraryCmd, json: bool) -> Result<()> {
    match command {
        LibraryCmd::Verify { collection } => {
            let collection = verify_collection(collection)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&collection)?);
            } else {
                println!("Verified {} files. Collection revision: {}", collection.files.len(), collection.revision);
            }
        }
        LibraryCmd::Stage { source, destination, kind } => {
            let destination = checked_path(destination)?;
            guard_destination(&destination, &boothready_platform::native().list_devices()?)?;
            let kind = match kind {
                Kind::Local => SourceKind::Local,
                Kind::GoogleDriveDesktop => SourceKind::GoogleDriveDesktop,
            };
            let plan = inventory(source, kind)?;
            let parent = destination.parent().context("destination needs a parent")?;
            guard_capacity(&plan, boothready_platform::space(parent)?)?;
            let collection = stage(source, kind, &destination, &AtomicBool::new(false))?;
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
    use boothready_model::Volume;
    fn device(mount: PathBuf, bus: BusType, removable: bool, is_system: bool) -> PhysicalDevice {
        PhysicalDevice {
            id: "synthetic".into(),
            os_path: "synthetic".into(),
            bus,
            removable,
            is_system,
            size_bytes: 1000,
            logical_sector_size: 512,
            storage_vendor: None,
            storage_model: None,
            storage_revision: None,
            usb: None,
            partition_scheme: None,
            volumes: vec![Volume {
                os_path: "synthetic".into(),
                mount_point: Some(mount),
                label: None,
                filesystem: None,
                size_bytes: 1000,
                offset_bytes: None,
                uuid: None,
                efi_system: false,
            }],
        }
    }
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
