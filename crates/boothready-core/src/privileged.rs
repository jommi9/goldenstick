//! Types shared across the privilege boundary, and the safety checks that
//! guard destructive operations (PRD §25).
//!
//! The unprivileged app never sends a path or command string to the helper.
//! It sends one of a few structured requests naming a device ID and the
//! fingerprint the user confirmed. The helper re-enumerates devices itself,
//! and refuses unless the device it finds matches that fingerprint and passes
//! every eligibility rule.

use boothready_model::{BusType, FilesystemKind, PartitionScheme, PhysicalDevice};
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u32 = 1;

/// MBR with 512-byte sectors tops out at 2 TiB; nothing bigger is a USB stick
/// we should be formatting through the standard flow.
pub const MAX_ELIGIBLE_BYTES: u64 = 2 * 1024 * 1024 * 1024 * 1024;

/// Identity of a physical device at the moment the user confirmed it. If
/// any of this differs when the helper runs, the operation is refused.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceFingerprint {
    pub device_id: String,
    pub size_bytes: u64,
    pub serial: Option<String>,
    pub vendor_id: Option<u16>,
    pub product_id: Option<u16>,
    /// Volume UUIDs/serials present at confirmation time, sorted.
    pub volume_ids: Vec<String>,
}

impl DeviceFingerprint {
    pub fn of(dev: &PhysicalDevice) -> DeviceFingerprint {
        let usb = dev.usb.as_ref();
        let mut volume_ids: Vec<String> = dev.volumes.iter().filter_map(|v| v.uuid.clone()).collect();
        volume_ids.sort();
        DeviceFingerprint {
            device_id: dev.id.clone(),
            size_bytes: dev.size_bytes,
            serial: usb.and_then(|u| u.serial.clone()),
            vendor_id: usb.and_then(|u| u.vendor_id),
            product_id: usb.and_then(|u| u.product_id),
            volume_ids,
        }
    }

    /// Short token shown nowhere but compared everywhere: the confirmation
    /// screen computes it, the helper recomputes it from what it sees.
    pub fn token(&self) -> String {
        let json = serde_json::to_vec(self).expect("fingerprint serialises");
        blake3::hash(&json).to_hex()[..32].to_string()
    }

    /// Last four characters of the serial, for "Serial ending: 4F82".
    pub fn serial_tail(&self) -> Option<String> {
        self.serial.as_ref().filter(|s| s.len() >= 4).map(|s| s[s.len() - 4..].to_uppercase())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Eligibility {
    pub eligible: bool,
    pub reasons: Vec<String>,
}

/// Mount points that must never belong to a device we erase.
const SYSTEM_MOUNTS: &[&str] = &["/", "/boot", "/boot/efi", "/usr", "/home", "/System/Volumes/Data", "/private/var/vm"];

fn is_system_mount(p: &std::path::Path) -> bool {
    let s = p.to_string_lossy();
    SYSTEM_MOUNTS.contains(&s.as_ref())
        || (s.len() <= 3 && s.to_ascii_uppercase().starts_with("C:"))
        || s.eq_ignore_ascii_case("C:\\")
}

/// Whether a device may appear in the standard erase-and-prepare flow.
pub fn eligibility(dev: &PhysicalDevice) -> Eligibility {
    let mut reasons = Vec::new();
    if dev.is_system {
        reasons.push("This is a system disk.".to_string());
    }
    if dev.bus != BusType::Usb {
        reasons.push("Only USB drives can be prepared.".to_string());
    }
    if !dev.removable {
        reasons.push("The operating system doesn't report this drive as removable.".to_string());
    }
    if dev.size_bytes == 0 {
        reasons.push("The drive reports no capacity (no media inserted?).".to_string());
    }
    if dev.size_bytes > MAX_ELIGIBLE_BYTES {
        reasons.push("Drives larger than 2 TB are outside what BoothReady prepares.".to_string());
    }
    if dev.volumes.iter().filter_map(|v| v.mount_point.as_deref()).any(is_system_mount) {
        reasons.push("A volume on this drive is mounted as part of the operating system.".to_string());
    }
    Eligibility { eligible: reasons.is_empty(), reasons }
}

/// Check the device the helper found against what the user confirmed.
pub fn check_target(found: &PhysicalDevice, expected: &DeviceFingerprint, token: &str) -> Result<(), String> {
    let now = DeviceFingerprint::of(found);
    if now.device_id != expected.device_id {
        return Err("A different device is now at this location.".into());
    }
    if now.size_bytes != expected.size_bytes
        || now.serial != expected.serial
        || now.vendor_id != expected.vendor_id
        || now.product_id != expected.product_id
    {
        return Err("The drive changed since you confirmed it. Nothing was erased.".into());
    }
    if expected.token() != token || now.token() != token {
        return Err("Confirmation doesn't match this drive. Nothing was erased.".into());
    }
    let e = eligibility(found);
    if !e.eligible {
        return Err(e.reasons.join(" "));
    }
    Ok(())
}

/// Everything the helper will do. There is deliberately no variant that
/// carries a path, a command or arbitrary arguments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum HelperRequest {
    Hello {
        protocol: u32,
    },
    ListDevices,
    /// Erase the whole device and create a single-partition layout.
    Prepare {
        expected: DeviceFingerprint,
        confirmation: String,
        scheme: PartitionScheme,
        filesystem: FilesystemKind,
        label: String,
    },
    /// Read-only structural inspection of the raw device.
    Inspect {
        device_id: String,
    },
    Unmount {
        device_id: String,
    },
    Eject {
        device_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HelperEvent {
    Hello { protocol: u32, version: String },
    Devices { devices: Vec<PhysicalDevice> },
    Progress { step: String, detail: String },
    Done { detail: String },
    Error { code: HelperErrorCode, message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HelperErrorCode {
    BadRequest,
    NotFound,
    Refused,
    Busy,
    Failed,
    PermissionDenied,
}

#[cfg(test)]
mod tests {
    use super::*;
    use boothready_model::{UsbDescriptor, Volume};
    use std::path::PathBuf;

    pub(crate) fn usb_stick() -> PhysicalDevice {
        PhysicalDevice {
            id: "sdb".into(),
            os_path: "/dev/sdb".into(),
            bus: BusType::Usb,
            removable: true,
            is_system: false,
            size_bytes: 64_000_000_000,
            logical_sector_size: 512,
            storage_vendor: Some("SanDisk".into()),
            storage_model: Some("Ultra".into()),
            storage_revision: None,
            usb: Some(UsbDescriptor {
                vendor_id: Some(0x0781),
                product_id: Some(0x5581),
                serial: Some("4C530001230518104F82".into()),
                ..Default::default()
            }),
            partition_scheme: Some(PartitionScheme::Mbr),
            volumes: vec![Volume {
                os_path: "/dev/sdb1".into(),
                mount_point: Some(PathBuf::from("/media/FESTIVAL26")),
                label: Some("FESTIVAL26".into()),
                filesystem: Some(FilesystemKind::Fat32),
                size_bytes: 63_000_000_000,
                offset_bytes: Some(1 << 20),
                uuid: Some("1234-ABCD".into()),
                efi_system: false,
            }],
        }
    }

    #[test]
    fn eligibility_rules() {
        let ok = usb_stick();
        assert!(eligibility(&ok).eligible);
        let mut sys = ok.clone();
        sys.is_system = true;
        assert!(!eligibility(&sys).eligible);
        let mut sata = ok.clone();
        sata.bus = BusType::Sata;
        assert!(!eligibility(&sata).eligible);
        let mut fixed = ok.clone();
        fixed.removable = false;
        assert!(!eligibility(&fixed).eligible);
        let mut root = ok.clone();
        root.volumes[0].mount_point = Some(PathBuf::from("/"));
        assert!(!eligibility(&root).eligible);
        let mut huge = ok;
        huge.size_bytes = 4_000_000_000_000;
        assert!(!eligibility(&huge).eligible);
    }

    #[test]
    fn swapped_or_changed_drive_is_refused() {
        let dev = usb_stick();
        let fp = DeviceFingerprint::of(&dev);
        let token = fp.token();
        assert_eq!(fp.serial_tail().as_deref(), Some("4F82"));
        assert!(check_target(&dev, &fp, &token).is_ok());

        // User swapped sticks between confirming and erasing.
        let mut other = dev.clone();
        other.usb.as_mut().unwrap().serial = Some("OTHER0001".into());
        assert!(check_target(&other, &fp, &token).is_err());
        // Same stick, reformatted elsewhere in the meantime.
        let mut changed = dev.clone();
        changed.volumes[0].uuid = Some("9999-0000".into());
        assert!(check_target(&changed, &fp, &token).is_err());
        // Forged or stale token.
        assert!(check_target(&dev, &fp, "0000").is_err());
    }

    #[test]
    fn requests_have_no_free_form_commands() {
        let json = serde_json::to_string(&HelperRequest::Eject { device_id: "sdb".into() }).unwrap();
        assert_eq!(json, r#"{"op":"eject","device_id":"sdb"}"#);
        assert!(serde_json::from_str::<HelperRequest>(r#"{"op":"shell","cmd":"rm -rf /"}"#).is_err());
    }
}
