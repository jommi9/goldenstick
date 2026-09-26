//! A folder-backed platform for demos, UI development and tests.
//!
//! Each sub-directory of the demo root is one "USB drive":
//!
//! ```text
//! demo-root/
//!   stick1/
//!     device.json   PhysicalDevice description (id, USB identity, size)
//!     disk.img      optional raw image (inspected and formatted like a device)
//!     volume/       the mounted filesystem contents
//! ```
//!
//! Creating a folder "inserts" a drive; deleting it "removes" it.

use crate::{Platform, PlatformError};
use boothready_model::{BusType, FilesystemKind, PartitionScheme, PhysicalDevice, UsbDescriptor, Volume};
use std::fs;
use std::path::{Path, PathBuf};

pub struct DemoPlatform {
    root: PathBuf,
}

impl DemoPlatform {
    pub fn new(root: PathBuf) -> DemoPlatform {
        DemoPlatform { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn device_dir(&self, id: &str) -> PathBuf {
        self.root.join(id.trim_start_matches("demo:"))
    }

    /// Create a demo stick with an empty volume.
    pub fn create_stick(
        root: &Path,
        name: &str,
        maker: &str,
        product: &str,
        vid: u16,
        pid: u16,
        size: u64,
    ) -> std::io::Result<PathBuf> {
        let dir = root.join(name);
        fs::create_dir_all(dir.join("volume"))?;
        let dev = PhysicalDevice {
            id: format!("demo:{name}"),
            os_path: dir.join("disk.img").to_string_lossy().into_owned(),
            bus: BusType::Usb,
            removable: true,
            is_system: false,
            size_bytes: size,
            logical_sector_size: 512,
            storage_vendor: Some(maker.to_string()),
            storage_model: Some(product.to_string()),
            storage_revision: Some("1.00".into()),
            usb: Some(UsbDescriptor {
                vendor_id: Some(vid),
                product_id: Some(pid),
                manufacturer: Some(maker.to_string()),
                product: Some(product.to_string()),
                serial: Some(format!("DEMO{:08X}", fxhash(name))),
                bcd_device: Some(0x0100),
                usb_version: Some("3.20".into()),
                speed_mbps: Some(5000),
                max_power_ma: Some(224),
            }),
            partition_scheme: Some(PartitionScheme::Mbr),
            volumes: vec![Volume {
                os_path: format!("demo:{name}:1"),
                mount_point: None,
                label: Some(name.to_uppercase()),
                filesystem: Some(FilesystemKind::Fat32),
                size_bytes: size.saturating_sub(1 << 20),
                offset_bytes: Some(1 << 20),
                uuid: Some(format!("{:04X}-{:04X}", fxhash(name) >> 16 & 0xFFFF, fxhash(name) & 0xFFFF)),
                efi_system: false,
            }],
        };
        fs::write(dir.join("device.json"), serde_json::to_vec_pretty(&dev)?)?;
        Ok(dir)
    }

    fn load(&self, dir: &Path) -> Option<PhysicalDevice> {
        let text = fs::read_to_string(dir.join("device.json")).ok()?;
        let mut dev: PhysicalDevice = serde_json::from_str(&text).ok()?;
        let name = dir.file_name()?.to_string_lossy().into_owned();
        dev.id = format!("demo:{name}");
        dev.os_path = dir.join("disk.img").to_string_lossy().into_owned();
        let mounted = !dir.join(".unmounted").exists();
        // Like macOS, leave the EFI system partition unmounted.
        for v in &mut dev.volumes {
            v.mount_point = (mounted && !v.efi_system).then(|| dir.join("volume"));
        }
        Some(dev)
    }
}

/// Small stable hash for demo serial numbers.
fn fxhash(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

impl Platform for DemoPlatform {
    fn list_devices(&self) -> Result<Vec<PhysicalDevice>, PlatformError> {
        let mut out = Vec::new();
        let Ok(rd) = fs::read_dir(&self.root) else { return Ok(out) };
        let mut dirs: Vec<PathBuf> = rd.filter_map(Result::ok).map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        for d in dirs {
            if let Some(dev) = self.load(&d) {
                out.push(dev);
            }
        }
        Ok(out)
    }

    fn unmount(&self, device_id: &str) -> Result<(), PlatformError> {
        let dir = self.device_dir(device_id);
        if !dir.exists() {
            return Err(PlatformError::NotFound(device_id.into()));
        }
        fs::write(dir.join(".unmounted"), b"")?;
        Ok(())
    }

    fn eject(&self, device_id: &str) -> Result<(), PlatformError> {
        let dir = self.device_dir(device_id);
        if !dir.exists() {
            return Err(PlatformError::NotFound(device_id.into()));
        }
        if dir.join(".busy").exists() {
            let holder = fs::read_to_string(dir.join(".busy")).ok().filter(|s| !s.trim().is_empty());
            return Err(PlatformError::Busy { holder: holder.map(|s| s.trim().to_string()) });
        }
        fs::write(dir.join(".unmounted"), b"")?;
        fs::write(dir.join(".ejected"), b"")?;
        Ok(())
    }

    fn name(&self) -> &'static str {
        "demo"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_list_eject() {
        let d = tempfile::tempdir().unwrap();
        DemoPlatform::create_stick(
            d.path(),
            "kingston",
            "Kingston",
            "DataTraveler 3.0",
            0x0951,
            0x1666,
            32_000_000_000,
        )
        .unwrap();
        let p = DemoPlatform::new(d.path().to_path_buf());
        let devs = p.list_devices().unwrap();
        assert_eq!(devs.len(), 1);
        assert_eq!(devs[0].id, "demo:kingston");
        assert!(devs[0].volumes[0].mount_point.is_some());
        fs::write(d.path().join("kingston/.busy"), "rekordbox (pid 4312)").unwrap();
        match p.eject("demo:kingston") {
            Err(PlatformError::Busy { holder }) => assert_eq!(holder.as_deref(), Some("rekordbox (pid 4312)")),
            other => panic!("{other:?}"),
        }
        fs::remove_file(d.path().join("kingston/.busy")).unwrap();
        p.eject("demo:kingston").unwrap();
        assert!(p.list_devices().unwrap()[0].volumes[0].mount_point.is_none());
    }
}
