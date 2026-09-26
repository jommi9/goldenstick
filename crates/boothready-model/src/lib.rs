//! Shared data types describing physical storage as BoothReady sees it.
//!
//! These types are deliberately free of any OS or parsing logic so that the
//! platform layer, the privileged helper and the core engine can all agree on
//! them without depending on each other.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::PathBuf;

/// Partition map found at the start of a disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionScheme {
    /// Master Boot Record ("FDisk" on macOS).
    Mbr,
    /// GUID Partition Table. A protective MBR is expected in front of it.
    Gpt,
    /// Apple Partition Map, only seen on very old Mac-formatted media.
    Apm,
    /// No partition table: the filesystem starts at sector 0 ("superfloppy").
    Superfloppy,
    Unknown,
}

impl PartitionScheme {
    pub fn label(self) -> &'static str {
        match self {
            PartitionScheme::Mbr => "MBR",
            PartitionScheme::Gpt => "GPT",
            PartitionScheme::Apm => "Apple Partition Map",
            PartitionScheme::Superfloppy => "No partition table",
            PartitionScheme::Unknown => "Unknown",
        }
    }
}

impl fmt::Display for PartitionScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Filesystem found on a volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesystemKind {
    Fat12,
    Fat16,
    Fat32,
    Exfat,
    Ntfs,
    HfsPlus,
    Apfs,
    Ext,
    Unknown,
}

impl FilesystemKind {
    pub fn label(self) -> &'static str {
        match self {
            FilesystemKind::Fat12 => "FAT12",
            FilesystemKind::Fat16 => "FAT16",
            FilesystemKind::Fat32 => "FAT32",
            FilesystemKind::Exfat => "exFAT",
            FilesystemKind::Ntfs => "NTFS",
            FilesystemKind::HfsPlus => "HFS+",
            FilesystemKind::Apfs => "APFS",
            FilesystemKind::Ext => "ext2/3/4",
            FilesystemKind::Unknown => "Unknown",
        }
    }

    /// Parse the names operating systems and rule files commonly use.
    pub fn from_name(name: &str) -> Option<FilesystemKind> {
        let n = name.trim().to_ascii_lowercase().replace(['-', '_', ' '], "");
        Some(match n.as_str() {
            "fat12" => FilesystemKind::Fat12,
            "fat16" | "fat" | "msdos" => FilesystemKind::Fat16,
            "fat32" | "vfat" => FilesystemKind::Fat32,
            "exfat" => FilesystemKind::Exfat,
            "ntfs" => FilesystemKind::Ntfs,
            "hfs+" | "hfsplus" | "hfs" | "machfs" | "journaledhfs+" => FilesystemKind::HfsPlus,
            "apfs" => FilesystemKind::Apfs,
            "ext2" | "ext3" | "ext4" => FilesystemKind::Ext,
            _ => return None,
        })
    }
}

impl fmt::Display for FilesystemKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// How the storage device is attached to the host.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BusType {
    Usb,
    Sata,
    Nvme,
    Sd,
    Thunderbolt,
    Virtual,
    Other(String),
}

/// What the USB layer tells us about the device, when the OS exposes it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsbDescriptor {
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    pub manufacturer: Option<String>,
    pub product: Option<String>,
    pub serial: Option<String>,
    /// bcdDevice, often the closest thing to a hardware revision we can read.
    pub bcd_device: Option<u16>,
    /// USB specification version the device reports, e.g. "3.20".
    pub usb_version: Option<String>,
    /// Negotiated link speed in Mbit/s.
    pub speed_mbps: Option<u32>,
    /// bMaxPower from the active configuration, in mA.
    pub max_power_ma: Option<u32>,
}

/// A mounted or mountable filesystem on a physical device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    /// OS node for the volume (`/dev/sdb1`, `disk4s1`, `\\?\Volume{...}\`).
    pub os_path: String,
    pub mount_point: Option<PathBuf>,
    pub label: Option<String>,
    pub filesystem: Option<FilesystemKind>,
    pub size_bytes: u64,
    /// Byte offset of the volume on the physical device, if known.
    pub offset_bytes: Option<u64>,
    pub uuid: Option<String>,
}

/// A whole physical storage device such as a USB stick.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PhysicalDevice {
    /// Stable-for-this-session identifier chosen by the platform layer.
    pub id: String,
    /// Node the helper opens for raw access (`/dev/sdb`, `/dev/rdisk4`,
    /// `\\.\PhysicalDrive2`).
    pub os_path: String,
    pub bus: BusType,
    pub removable: bool,
    /// Hosts the running OS, a boot volume or anything else we must never touch.
    pub is_system: bool,
    pub size_bytes: u64,
    pub logical_sector_size: u32,
    /// Vendor/model/revision as reported by the storage (SCSI inquiry) layer.
    pub storage_vendor: Option<String>,
    pub storage_model: Option<String>,
    pub storage_revision: Option<String>,
    pub usb: Option<UsbDescriptor>,
    pub partition_scheme: Option<PartitionScheme>,
    pub volumes: Vec<Volume>,
}

impl PhysicalDevice {
    /// Best human-readable name the OS gave us, before catalog matching.
    pub fn raw_display_name(&self) -> String {
        let usb = self.usb.as_ref();
        let vendor = usb
            .and_then(|u| u.manufacturer.clone())
            .or_else(|| self.storage_vendor.clone())
            .unwrap_or_default();
        let product = usb
            .and_then(|u| u.product.clone())
            .or_else(|| self.storage_model.clone())
            .unwrap_or_default();
        let name = format!("{} {}", vendor.trim(), product.trim());
        let name = name.trim();
        if name.is_empty() {
            "Unknown USB drive".to_string()
        } else {
            name.to_string()
        }
    }

    pub fn serial(&self) -> Option<&str> {
        self.usb.as_ref().and_then(|u| u.serial.as_deref())
    }

    /// First mounted volume, which is where DJ libraries live on single
    /// partition USB sticks.
    pub fn primary_mount(&self) -> Option<&Volume> {
        self.volumes.iter().find(|v| v.mount_point.is_some())
    }
}

/// Events the platform layer emits as devices come and go.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DeviceEvent {
    Appeared { device: PhysicalDevice },
    Changed { device: PhysicalDevice },
    Disappeared { id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filesystem_names_parse() {
        assert_eq!(FilesystemKind::from_name("FAT32"), Some(FilesystemKind::Fat32));
        assert_eq!(FilesystemKind::from_name("vfat"), Some(FilesystemKind::Fat32));
        assert_eq!(FilesystemKind::from_name("ExFAT"), Some(FilesystemKind::Exfat));
        assert_eq!(FilesystemKind::from_name("HFS+"), Some(FilesystemKind::HfsPlus));
        assert_eq!(FilesystemKind::from_name("Journaled HFS+"), Some(FilesystemKind::HfsPlus));
        assert_eq!(FilesystemKind::from_name("zfs"), None);
    }

    #[test]
    fn display_name_falls_back() {
        let dev = PhysicalDevice {
            id: "x".into(),
            os_path: "/dev/sdz".into(),
            bus: BusType::Usb,
            removable: true,
            is_system: false,
            size_bytes: 1,
            logical_sector_size: 512,
            storage_vendor: Some("SanDisk ".into()),
            storage_model: Some("Ultra".into()),
            storage_revision: None,
            usb: None,
            partition_scheme: None,
            volumes: vec![],
        };
        assert_eq!(dev.raw_display_name(), "SanDisk Ultra");
    }
}
