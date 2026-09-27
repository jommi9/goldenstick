//! macOS backend.
//!
//! Device details come from `diskutil ... -plist` (the same data Disk
//! Arbitration exposes) and USB identity from `ioreg -a`, both parsed as
//! plists. The parsers are plain functions over bytes so they're tested on
//! every OS; only the command execution is macOS-specific.
//!
//! A future revision can subscribe to Disk Arbitration appear/disappear
//! callbacks instead of polling; the data model stays the same.

use boothready_model::{BusType, FilesystemKind, PartitionScheme, PhysicalDevice, UsbDescriptor, Volume};
use plist::{Dictionary, Value};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskListEntry {
    pub id: String,
    pub content: Option<String>,
    pub size: u64,
    pub partitions: Vec<PartitionListEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionListEntry {
    pub id: String,
    pub content: Option<String>,
    pub size: u64,
    pub volume_name: Option<String>,
    pub mount_point: Option<String>,
    pub volume_uuid: Option<String>,
}

fn s(d: &Dictionary, k: &str) -> Option<String> {
    d.get(k).and_then(Value::as_string).map(str::to_string).filter(|v| !v.is_empty())
}

fn u(d: &Dictionary, k: &str) -> Option<u64> {
    d.get(k).and_then(|v| v.as_unsigned_integer().or_else(|| v.as_signed_integer().map(|i| i as u64)))
}

fn b(d: &Dictionary, k: &str) -> Option<bool> {
    d.get(k).and_then(Value::as_boolean)
}

/// Parse `diskutil list -plist [external physical]`.
pub fn parse_disk_list(bytes: &[u8]) -> Result<Vec<DiskListEntry>, plist::Error> {
    let root: Value = plist::from_bytes(bytes)?;
    let mut out = Vec::new();
    let Some(all) = root.as_dictionary().and_then(|d| d.get("AllDisksAndPartitions")).and_then(Value::as_array) else {
        return Ok(out);
    };
    for disk in all.iter().filter_map(Value::as_dictionary) {
        let Some(id) = s(disk, "DeviceIdentifier") else { continue };
        let partitions = disk
            .get("Partitions")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_dictionary)
                    .filter_map(|p| {
                        Some(PartitionListEntry {
                            id: s(p, "DeviceIdentifier")?,
                            content: s(p, "Content"),
                            size: u(p, "Size").unwrap_or(0),
                            volume_name: s(p, "VolumeName"),
                            mount_point: s(p, "MountPoint"),
                            volume_uuid: s(p, "VolumeUUID"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.push(DiskListEntry { id, content: s(disk, "Content"), size: u(disk, "Size").unwrap_or(0), partitions });
    }
    Ok(out)
}

/// A volume inside an APFS container.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApfsVolume {
    pub id: String,
    pub name: Option<String>,
    pub mount_point: Option<String>,
    pub uuid: Option<String>,
}

/// Parse `diskutil list -plist virtual` into the volumes of each APFS
/// container, keyed by the physical partition that stores it ("disk4s2").
/// APFS volumes live on a synthesized disk, so this is the only place their
/// names and mount points show up.
pub fn parse_apfs_containers(bytes: &[u8]) -> Result<HashMap<String, Vec<ApfsVolume>>, plist::Error> {
    let root: Value = plist::from_bytes(bytes)?;
    let mut out: HashMap<String, Vec<ApfsVolume>> = HashMap::new();
    let Some(all) = root.as_dictionary().and_then(|d| d.get("AllDisksAndPartitions")).and_then(Value::as_array) else {
        return Ok(out);
    };
    for disk in all.iter().filter_map(Value::as_dictionary) {
        let volumes: Vec<ApfsVolume> = disk
            .get("APFSVolumes")
            .and_then(Value::as_array)
            .map(|vols| {
                vols.iter()
                    .filter_map(Value::as_dictionary)
                    .filter_map(|v| {
                        Some(ApfsVolume {
                            id: s(v, "DeviceIdentifier")?,
                            name: s(v, "VolumeName"),
                            mount_point: s(v, "MountPoint"),
                            uuid: s(v, "VolumeUUID"),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let stores = disk.get("APFSPhysicalStores").and_then(Value::as_array).into_iter().flatten();
        for store in stores.filter_map(Value::as_dictionary).filter_map(|d| s(d, "DeviceIdentifier")) {
            out.entry(store).or_default().extend(volumes.iter().cloned());
        }
    }
    Ok(out)
}

/// The subset of `diskutil info -plist <disk>` we use.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskInfo {
    pub bus_protocol: Option<String>,
    pub internal: Option<bool>,
    pub removable: Option<bool>,
    pub ejectable: Option<bool>,
    pub block_size: Option<u32>,
    pub size: Option<u64>,
    pub media_name: Option<String>,
    pub device_node: Option<String>,
    pub content: Option<String>,
    pub filesystem_type: Option<String>,
    pub filesystem_name: Option<String>,
    pub volume_name: Option<String>,
    pub mount_point: Option<String>,
    pub volume_uuid: Option<String>,
    pub partition_offset: Option<u64>,
    pub system_image: Option<bool>,
}

pub fn parse_disk_info(bytes: &[u8]) -> Result<DiskInfo, plist::Error> {
    let root: Value = plist::from_bytes(bytes)?;
    let d = root.as_dictionary().cloned().unwrap_or_default();
    Ok(DiskInfo {
        bus_protocol: s(&d, "BusProtocol"),
        internal: b(&d, "Internal"),
        removable: b(&d, "Removable").or(b(&d, "RemovableMedia")).or(b(&d, "RemovableMediaOrExternalDevice")),
        ejectable: b(&d, "Ejectable"),
        block_size: u(&d, "DeviceBlockSize").map(|v| v as u32),
        size: u(&d, "TotalSize").or(u(&d, "Size")),
        media_name: s(&d, "MediaName"),
        device_node: s(&d, "DeviceNode"),
        content: s(&d, "Content"),
        filesystem_type: s(&d, "FilesystemType"),
        filesystem_name: s(&d, "FilesystemName"),
        volume_name: s(&d, "VolumeName"),
        mount_point: s(&d, "MountPoint"),
        volume_uuid: s(&d, "VolumeUUID"),
        partition_offset: u(&d, "PartitionMapPartitionOffset"),
        system_image: b(&d, "SystemImage"),
    })
}

/// Walk `ioreg -r -c IOUSBHostDevice -l -a` output and map BSD disk names
/// ("disk4") to the USB device above them.
pub fn parse_ioreg_usb(bytes: &[u8]) -> Result<HashMap<String, UsbDescriptor>, plist::Error> {
    let root: Value = plist::from_bytes(bytes)?;
    let mut out = HashMap::new();
    let devices: Vec<&Value> = match &root {
        Value::Array(a) => a.iter().collect(),
        v => vec![v],
    };
    for dev in devices.iter().filter_map(|v| v.as_dictionary()) {
        let desc = UsbDescriptor {
            vendor_id: u(dev, "idVendor").map(|v| v as u16),
            product_id: u(dev, "idProduct").map(|v| v as u16),
            manufacturer: s(dev, "USB Vendor Name").or_else(|| s(dev, "kUSBVendorString")),
            product: s(dev, "USB Product Name").or_else(|| s(dev, "kUSBProductString")),
            serial: s(dev, "USB Serial Number").or_else(|| s(dev, "kUSBSerialNumberString")),
            bcd_device: u(dev, "bcdDevice").map(|v| v as u16),
            usb_version: u(dev, "bcdUSB").map(|v| format!("{}.{:02x}", v >> 8, v & 0xFF)),
            speed_mbps: u(dev, "Device Speed").map(|sp| match sp {
                0 => 1,
                1 => 12,
                2 => 480,
                3 => 5000,
                4 => 10000,
                _ => 20000,
            }),
            max_power_ma: None,
        };
        let mut names = Vec::new();
        collect_bsd_names(dev, &mut names);
        for n in names {
            // Only whole disks ("disk4", not "disk4s1").
            if n.starts_with("disk") && !n[4..].contains('s') {
                out.insert(n, desc.clone());
            }
        }
    }
    Ok(out)
}

fn collect_bsd_names(d: &Dictionary, out: &mut Vec<String>) {
    if let Some(n) = s(d, "BSD Name") {
        out.push(n);
    }
    if let Some(children) = d.get("IORegistryEntryChildren").and_then(Value::as_array) {
        for c in children.iter().filter_map(Value::as_dictionary) {
            collect_bsd_names(c, out);
        }
    }
}

fn scheme(content: Option<&str>) -> Option<PartitionScheme> {
    content.map(|c| match c {
        "FDisk_partition_scheme" => PartitionScheme::Mbr,
        "GUID_partition_scheme" => PartitionScheme::Gpt,
        "Apple_partition_scheme" => PartitionScheme::Apm,
        _ => PartitionScheme::Superfloppy,
    })
}

fn fs_kind(fs_type: Option<&str>, fs_name: Option<&str>) -> Option<FilesystemKind> {
    match fs_type? {
        "msdos" => Some(match fs_name {
            Some(n) if n.contains("FAT16") => FilesystemKind::Fat16,
            Some(n) if n.contains("FAT12") => FilesystemKind::Fat12,
            _ => FilesystemKind::Fat32,
        }),
        "exfat" => Some(FilesystemKind::Exfat),
        "hfs" => Some(FilesystemKind::HfsPlus),
        "apfs" => Some(FilesystemKind::Apfs),
        "ntfs" => Some(FilesystemKind::Ntfs),
        _ => None,
    }
}

/// Apple's GPT partition types name the filesystem. That matters for APFS,
/// whose partition carries no `FilesystemType` because its volumes live on a
/// synthesized container disk. MBR types like `DOS_FAT_32` are only a type
/// byte that can disagree with the partition's contents, so they're ignored.
fn fs_from_content(content: Option<&str>) -> Option<FilesystemKind> {
    match content? {
        "Apple_APFS" | "Apple_APFS_ISC" | "Apple_APFS_Recovery" => Some(FilesystemKind::Apfs),
        "Apple_HFS" | "Apple_HFSX" => Some(FilesystemKind::HfsPlus),
        _ => None,
    }
}

/// Assemble a `PhysicalDevice` from parsed diskutil/ioreg data.
pub fn build_device(
    entry: &DiskListEntry,
    whole: &DiskInfo,
    parts: &[DiskInfo],
    usb: Option<&UsbDescriptor>,
    apfs: &HashMap<String, Vec<ApfsVolume>>,
) -> PhysicalDevice {
    let bus = match whole.bus_protocol.as_deref() {
        Some("USB") => BusType::Usb,
        Some("SATA") => BusType::Sata,
        Some("PCI-Express") | Some("Apple Fabric") => BusType::Nvme,
        Some("Secure Digital") => BusType::Sd,
        Some("Thunderbolt") => BusType::Thunderbolt,
        Some("Disk Image") | Some("Virtual Interface") => BusType::Virtual,
        Some(o) => BusType::Other(o.to_string()),
        None => BusType::Other("unknown".into()),
    };
    let volumes: Vec<Volume> = entry
        .partitions
        .iter()
        .zip(parts.iter().map(Some).chain(std::iter::repeat(None)))
        .map(|(p, info)| {
            let content = info.and_then(|i| i.content.as_deref()).or(p.content.as_deref());
            // An APFS partition shows the name and mount point of the
            // container volume a user would see: the first mounted one.
            let inner = apfs.get(&p.id).and_then(|vols| vols.iter().find(|v| v.mount_point.is_some()).or(vols.first()));
            Volume {
                os_path: format!("/dev/{}", p.id),
                mount_point: info
                    .and_then(|i| i.mount_point.clone())
                    .or_else(|| p.mount_point.clone())
                    .or_else(|| inner.and_then(|v| v.mount_point.clone()))
                    .map(PathBuf::from),
                label: info
                    .and_then(|i| i.volume_name.clone())
                    .or_else(|| p.volume_name.clone())
                    .or_else(|| inner.and_then(|v| v.name.clone())),
                filesystem: info
                    .and_then(|i| fs_kind(i.filesystem_type.as_deref(), i.filesystem_name.as_deref()))
                    .or_else(|| fs_from_content(content)),
                size_bytes: p.size,
                offset_bytes: info.and_then(|i| i.partition_offset),
                uuid: info
                    .and_then(|i| i.volume_uuid.clone())
                    .or_else(|| p.volume_uuid.clone())
                    .or_else(|| inner.and_then(|v| v.uuid.clone())),
                efi_system: content == Some("EFI"),
            }
        })
        .collect();
    let system_mount = |m: &str| m == "/" || m.starts_with("/System");
    // A macOS startup disk mounts "/" from an APFS volume that isn't the one
    // shown for its partition, so every container volume counts here.
    let mut container_mounts =
        entry.partitions.iter().filter_map(|p| apfs.get(&p.id)).flatten().filter_map(|v| v.mount_point.as_deref());
    let is_system = whole.internal == Some(true)
        || whole.system_image == Some(true)
        || volumes.iter().filter_map(|v| v.mount_point.as_deref()).any(|m| m.to_str().is_some_and(system_mount))
        || container_mounts.any(system_mount);
    PhysicalDevice {
        id: entry.id.clone(),
        os_path: format!("/dev/r{}", entry.id),
        bus,
        removable: whole.removable.unwrap_or(false) || whole.ejectable.unwrap_or(false),
        is_system,
        size_bytes: whole.size.unwrap_or(entry.size),
        logical_sector_size: whole.block_size.unwrap_or(512),
        storage_vendor: None,
        storage_model: whole.media_name.clone(),
        storage_revision: None,
        usb: usb.cloned(),
        partition_scheme: scheme(entry.content.as_deref().or(whole.content.as_deref())),
        volumes,
    }
}

/// Build the device list from `diskutil list -plist physical`, the APFS
/// containers from `diskutil list -plist virtual`, `ioreg` output and a way
/// to get `diskutil info -plist <id>` for each disk and partition. The live
/// backend and diagnostics replay both go through here.
pub fn assemble(
    list: &[DiskListEntry],
    apfs: &HashMap<String, Vec<ApfsVolume>>,
    ioreg: &[u8],
    info: impl Fn(&str) -> Option<Vec<u8>>,
) -> Result<Vec<PhysicalDevice>, crate::PlatformError> {
    let usb = parse_ioreg_usb(ioreg).unwrap_or_default();
    let disk_info = |id: &str| info(id).and_then(|b| parse_disk_info(&b).ok());
    let mut devs = Vec::new();
    for entry in list {
        let whole = disk_info(&entry.id)
            .ok_or_else(|| crate::PlatformError::Failed(format!("diskutil info {} failed", entry.id)))?;
        let parts: Vec<DiskInfo> = entry.partitions.iter().map(|p| disk_info(&p.id).unwrap_or_default()).collect();
        devs.push(build_device(entry, &whole, &parts, usb.get(&entry.id), apfs));
    }
    Ok(devs)
}

const DISKUTIL: &str = "/usr/sbin/diskutil";
const IOREG: &str = "/usr/sbin/ioreg";
const LIST_PHYSICAL: [&str; 4] = [DISKUTIL, "list", "-plist", "physical"];
const LIST_VIRTUAL: [&str; 4] = [DISKUTIL, "list", "-plist", "virtual"];
const IOREG_USB: [&str; 6] = [IOREG, "-r", "-c", "IOUSBHostDevice", "-l", "-a"];

/// Keep only the USB devices that carry a disk, so a report doesn't list
/// the keyboards, phones and hubs on the machine.
pub fn storage_only_ioreg(bytes: &[u8]) -> Option<Vec<u8>> {
    let devices = match plist::from_bytes::<Value>(bytes).ok()? {
        Value::Array(a) => a,
        v => vec![v],
    };
    let kept: Vec<Value> = devices
        .into_iter()
        .filter(|v| {
            v.as_dictionary().is_some_and(|d| {
                let mut names = Vec::new();
                collect_bsd_names(d, &mut names);
                !names.is_empty()
            })
        })
        .collect();
    let mut out = Vec::new();
    Value::Array(kept).to_writer_xml(&mut out).ok()?;
    Some(out)
}

/// Rebuild a Mac's device list from a diagnostics report.
pub fn replay(d: &crate::diagnostics::Diagnostics) -> Option<Result<Vec<PhysicalDevice>, crate::PlatformError>> {
    let list = parse_disk_list(&d.output(&LIST_PHYSICAL)?).ok()?;
    let apfs = d.output(&LIST_VIRTUAL).and_then(|v| parse_apfs_containers(&v).ok()).unwrap_or_default();
    let ioreg = d.output(&IOREG_USB).unwrap_or_default();
    Some(assemble(&list, &apfs, &ioreg, |id| d.output(&[DISKUTIL, "info", "-plist", id])))
}

/// The raw output a report needs to replay this Mac's listing.
#[cfg(target_os = "macos")]
pub(crate) fn captures() -> Vec<crate::diagnostics::Capture> {
    use crate::diagnostics::Capture;
    let list = Capture::command(LIST_PHYSICAL[0], &LIST_PHYSICAL[1..]);
    let ids: Vec<String> = list
        .decoded()
        .and_then(|b| parse_disk_list(&b).ok())
        .unwrap_or_default()
        .into_iter()
        .flat_map(|e| std::iter::once(e.id).chain(e.partitions.into_iter().map(|p| p.id)))
        .collect();
    let mut out = vec![list, Capture::command(LIST_VIRTUAL[0], &LIST_VIRTUAL[1..])];
    let mut ioreg = Capture::command(IOREG_USB[0], &IOREG_USB[1..]);
    if let Some(text) = ioreg.text.take() {
        ioreg.text = storage_only_ioreg(text.as_bytes()).map(|b| String::from_utf8_lossy(&b).into_owned());
    }
    out.push(ioreg);
    out.extend(ids.iter().map(|id| Capture::command(DISKUTIL, &["info", "-plist", id])));
    out.push(Capture::command("/usr/bin/sw_vers", &[]));
    out
}

#[cfg(target_os = "macos")]
pub use imp::MacPlatform;

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use crate::{run, Platform, PlatformError};
    use std::sync::Mutex;

    pub struct MacPlatform;

    type Listing = (Vec<DiskListEntry>, HashMap<String, Vec<ApfsVolume>>);
    static CACHE: Mutex<Option<(Listing, Vec<PhysicalDevice>)>> = Mutex::new(None);

    fn output(cmd: &[&str]) -> Result<Vec<u8>, PlatformError> {
        Ok(std::process::Command::new(cmd[0]).args(&cmd[1..]).output()?.stdout)
    }

    impl Platform for MacPlatform {
        fn list_devices(&self) -> Result<Vec<PhysicalDevice>, PlatformError> {
            let list = parse_disk_list(&output(&LIST_PHYSICAL)?).map_err(|e| PlatformError::Failed(e.to_string()))?;
            // APFS volumes mount on a synthesized disk, so their mount points
            // only change in the virtual listing.
            let apfs = parse_apfs_containers(&output(&LIST_VIRTUAL)?).unwrap_or_default();
            let listing = (list, apfs);
            // diskutil info is slow; only re-query when a listing changed.
            let mut cache = CACHE.lock().unwrap();
            if let Some((prev, devs)) = cache.as_ref() {
                if *prev == listing {
                    return Ok(devs.clone());
                }
            }
            let (list, apfs) = &listing;
            let ioreg = output(&IOREG_USB)?;
            let devs = assemble(list, apfs, &ioreg, |id| output(&[DISKUTIL, "info", "-plist", id]).ok())?;
            *cache = Some((listing, devs.clone()));
            Ok(devs)
        }

        fn unmount(&self, device_id: &str) -> Result<(), PlatformError> {
            run("/usr/sbin/diskutil", &["unmountDisk", device_id]).map(|_| ())
        }

        fn eject(&self, device_id: &str) -> Result<(), PlatformError> {
            match run("/usr/sbin/diskutil", &["eject", device_id]) {
                Err(PlatformError::Busy { .. }) | Err(PlatformError::Failed(_)) => {
                    // Name the process holding the volume, if lsof can tell.
                    let holder = std::process::Command::new("/usr/sbin/lsof")
                        .args(["-Fc", "+f", "--", &format!("/dev/{device_id}")])
                        .output()
                        .ok()
                        .and_then(|o| {
                            String::from_utf8_lossy(&o.stdout)
                                .lines()
                                .find_map(|l| l.strip_prefix('c').map(str::to_string))
                        });
                    Err(PlatformError::Busy { holder })
                }
                other => other.map(|_| ()),
            }
        }

        fn name(&self) -> &'static str {
            "macos"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>AllDisksAndPartitions</key><array>
 <dict>
  <key>Content</key><string>GUID_partition_scheme</string>
  <key>DeviceIdentifier</key><string>disk0</string>
  <key>OSInternal</key><false/>
  <key>Partitions</key><array>
   <dict><key>Content</key><string>EFI</string><key>DeviceIdentifier</key><string>disk0s1</string><key>Size</key><integer>524288000</integer></dict>
   <dict><key>Content</key><string>Apple_APFS</string><key>DeviceIdentifier</key><string>disk0s2</string><key>Size</key><integer>499000000000</integer></dict>
  </array>
  <key>Size</key><integer>500277790720</integer>
 </dict>
 <dict>
  <key>Content</key><string>FDisk_partition_scheme</string>
  <key>DeviceIdentifier</key><string>disk4</string>
  <key>OSInternal</key><false/>
  <key>Partitions</key><array>
   <dict>
    <key>Content</key><string>DOS_FAT_32</string>
    <key>DeviceIdentifier</key><string>disk4s1</string>
    <key>MountPoint</key><string>/Volumes/JOMMI_DJ</string>
    <key>Size</key><integer>31914983424</integer>
    <key>VolumeName</key><string>JOMMI_DJ</string>
    <key>VolumeUUID</key><string>0E239BC6-F960-3107-89CF-1C97F78BB46B</string>
   </dict>
  </array>
  <key>Size</key><integer>31915507712</integer>
 </dict>
</array>
<key>WholeDisks</key><array><string>disk0</string><string>disk4</string></array>
</dict></plist>"#;

    const INFO_DISK4: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>BusProtocol</key><string>USB</string>
<key>Content</key><string>FDisk_partition_scheme</string>
<key>DeviceBlockSize</key><integer>512</integer>
<key>DeviceIdentifier</key><string>disk4</string>
<key>DeviceNode</key><string>/dev/disk4</string>
<key>Ejectable</key><true/>
<key>Internal</key><false/>
<key>MediaName</key><string>SanDisk 3.2Gen1</string>
<key>Removable</key><true/>
<key>RemovableMedia</key><true/>
<key>Size</key><integer>31915507712</integer>
<key>SystemImage</key><false/>
<key>TotalSize</key><integer>31915507712</integer>
</dict></plist>"#;

    const INFO_DISK4S1: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict>
<key>FilesystemName</key><string>MS-DOS (FAT32)</string>
<key>FilesystemType</key><string>msdos</string>
<key>MountPoint</key><string>/Volumes/JOMMI_DJ</string>
<key>PartitionMapPartitionOffset</key><integer>1048576</integer>
<key>VolumeName</key><string>JOMMI_DJ</string>
<key>VolumeUUID</key><string>0E239BC6-F960-3107-89CF-1C97F78BB46B</string>
</dict></plist>"#;

    const IOREG: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><array>
 <dict>
  <key>idVendor</key><integer>1921</integer>
  <key>idProduct</key><integer>21889</integer>
  <key>bcdDevice</key><integer>256</integer>
  <key>bcdUSB</key><integer>800</integer>
  <key>Device Speed</key><integer>3</integer>
  <key>USB Product Name</key><string>SanDisk 3.2Gen1</string>
  <key>USB Vendor Name</key><string> USB</string>
  <key>USB Serial Number</key><string>4C530001230518104F82</string>
  <key>IORegistryEntryChildren</key><array>
   <dict><key>IORegistryEntryChildren</key><array>
     <dict><key>BSD Name</key><string>disk4</string>
       <key>IORegistryEntryChildren</key><array><dict><key>BSD Name</key><string>disk4s1</string></dict></array>
     </dict>
   </array></dict>
  </array>
 </dict>
</array></plist>"#;

    #[test]
    fn a_diagnostics_report_replays_on_any_os() {
        use crate::diagnostics::{replay, Capture, Diagnostics};
        const INFO_DISK0: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>BusProtocol</key><string>Apple Fabric</string><key>Internal</key><true/></dict></plist>"#;
        // Only USB devices carrying a disk make it into a report.
        let with_keyboard = IOREG.replace(
            "</array></plist>",
            "<dict><key>USB Product Name</key><string>Magic Keyboard</string></dict></array></plist>",
        );
        let ioreg = String::from_utf8(storage_only_ioreg(with_keyboard.as_bytes()).unwrap()).unwrap();
        assert!(ioreg.contains("SanDisk 3.2Gen1") && !ioreg.contains("Magic Keyboard"), "{ioreg}");

        let mut captures = vec![Capture::text(&LIST_PHYSICAL, LIST), Capture::text(&IOREG_USB, ioreg)];
        for (id, info) in [("disk0", INFO_DISK0), ("disk4", INFO_DISK4), ("disk4s1", INFO_DISK4S1)] {
            captures.push(Capture::text(&[DISKUTIL, "info", "-plist", id], info));
        }
        let mut report = Diagnostics {
            format: crate::diagnostics::FORMAT,
            boothready_version: "0.1.0".into(),
            created_unix: 0,
            os: "macos".into(),
            arch: "aarch64".into(),
            backend: "macos".into(),
            devices: Vec::new(),
            error: None,
            captures,
        };
        let devs = replay(&report).unwrap().unwrap();
        let list = parse_disk_list(LIST.as_bytes()).unwrap();
        let usb = parse_ioreg_usb(IOREG.as_bytes()).unwrap();
        let whole = parse_disk_info(INFO_DISK4.as_bytes()).unwrap();
        let part = parse_disk_info(INFO_DISK4S1.as_bytes()).unwrap();
        assert_eq!(devs.len(), 2);
        assert_eq!(devs[1], build_device(&list[1], &whole, &[part], usb.get("disk4"), &HashMap::new()));
        assert!(devs[0].is_system);

        // Missing whole-disk output fails the listing, as it does live.
        report.captures.retain(|c| c.source.last().map(String::as_str) != Some("disk0"));
        assert!(replay(&report).unwrap().is_err());
    }

    #[test]
    fn assembles_usb_stick_from_diskutil_and_ioreg() {
        let list = parse_disk_list(LIST.as_bytes()).unwrap();
        assert_eq!(list.len(), 2);
        let usb = parse_ioreg_usb(IOREG.as_bytes()).unwrap();
        assert_eq!(usb.len(), 1);
        let whole = parse_disk_info(INFO_DISK4.as_bytes()).unwrap();
        let part = parse_disk_info(INFO_DISK4S1.as_bytes()).unwrap();
        let dev = build_device(&list[1], &whole, &[part], usb.get("disk4"), &HashMap::new());
        assert_eq!(dev.id, "disk4");
        assert_eq!(dev.os_path, "/dev/rdisk4");
        assert_eq!(dev.bus, BusType::Usb);
        assert!(dev.removable && !dev.is_system);
        assert_eq!(dev.partition_scheme, Some(PartitionScheme::Mbr));
        let u = dev.usb.as_ref().unwrap();
        assert_eq!((u.vendor_id, u.product_id), (Some(0x0781), Some(0x5581)));
        assert_eq!(u.speed_mbps, Some(5000));
        assert_eq!(u.usb_version.as_deref(), Some("3.20"));
        let v = &dev.volumes[0];
        assert_eq!(v.filesystem, Some(FilesystemKind::Fat32));
        assert_eq!(v.label.as_deref(), Some("JOMMI_DJ"));
        assert_eq!(v.mount_point.as_deref(), Some(std::path::Path::new("/Volumes/JOMMI_DJ")));
        assert_eq!(v.offset_bytes, Some(1 << 20));
    }

    #[test]
    fn internal_disk_is_system() {
        let list = parse_disk_list(LIST.as_bytes()).unwrap();
        let whole = DiskInfo { internal: Some(true), bus_protocol: Some("Apple Fabric".into()), ..Default::default() };
        let dev = build_device(&list[0], &whole, &[], None, &HashMap::new());
        assert!(dev.is_system);
        assert_eq!(dev.partition_scheme, Some(PartitionScheme::Gpt));
        // diskutil gives an APFS partition no FilesystemType; its GPT type
        // still says what it is.
        assert!(dev.volumes[0].efi_system);
        assert!(!dev.volumes[1].efi_system);
        assert_eq!(dev.volumes[1].filesystem, Some(FilesystemKind::Apfs));
    }

    const APFS_STICK_LIST: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>AllDisksAndPartitions</key><array>
 <dict>
  <key>Content</key><string>GUID_partition_scheme</string>
  <key>DeviceIdentifier</key><string>disk4</string>
  <key>Partitions</key><array>
   <dict><key>Content</key><string>EFI</string><key>DeviceIdentifier</key><string>disk4s1</string><key>Size</key><integer>209715200</integer><key>VolumeName</key><string>EFI</string></dict>
   <dict><key>Content</key><string>Apple_APFS</string><key>DeviceIdentifier</key><string>disk4s2</string><key>Size</key><integer>63812476928</integer></dict>
  </array>
  <key>Size</key><integer>64023257088</integer>
 </dict>
</array></dict></plist>"#;

    const APFS_VIRTUAL: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0"><dict><key>AllDisksAndPartitions</key><array>
 <dict>
  <key>APFSPhysicalStores</key><array><dict><key>DeviceIdentifier</key><string>disk4s2</string></dict></array>
  <key>APFSVolumes</key><array>
   <dict><key>DeviceIdentifier</key><string>disk5s1</string><key>MountPoint</key><string>/Volumes/DJ STICK</string><key>VolumeName</key><string>DJ STICK</string><key>VolumeUUID</key><string>5D2C9E7A-0B61-4F0E-9C3B-7A1E2D4F6A80</string></dict>
  </array>
  <key>Content</key><string>EF57347C-0000-11AA-AA11-00306543ECAC</string>
  <key>DeviceIdentifier</key><string>disk5</string>
 </dict>
 <dict>
  <key>APFSPhysicalStores</key><array><dict><key>DeviceIdentifier</key><string>disk0s2</string></dict></array>
  <key>APFSVolumes</key><array>
   <dict><key>DeviceIdentifier</key><string>disk3s1</string><key>VolumeName</key><string>Macintosh HD</string></dict>
   <dict><key>DeviceIdentifier</key><string>disk3s5</string><key>MountPoint</key><string>/System/Volumes/Data</string><key>VolumeName</key><string>Data</string></dict>
  </array>
  <key>DeviceIdentifier</key><string>disk3</string>
 </dict>
 <dict><key>Content</key><string>GUID_partition_scheme</string><key>DeviceIdentifier</key><string>disk6</string></dict>
</array></dict></plist>"#;

    #[test]
    fn apfs_stick_shows_its_container_volume() {
        let list = parse_disk_list(APFS_STICK_LIST.as_bytes()).unwrap();
        let apfs = parse_apfs_containers(APFS_VIRTUAL.as_bytes()).unwrap();
        assert_eq!(apfs.len(), 2);
        let whole = DiskInfo { bus_protocol: Some("USB".into()), removable: Some(true), ..Default::default() };
        let dev = build_device(&list[0], &whole, &[], None, &apfs);
        assert!(!dev.is_system);
        let v = dev.primary_volume().unwrap();
        assert_eq!(v.os_path, "/dev/disk4s2");
        assert_eq!(v.label.as_deref(), Some("DJ STICK"));
        assert_eq!(v.mount_point.as_deref(), Some(std::path::Path::new("/Volumes/DJ STICK")));
        assert_eq!(v.filesystem, Some(FilesystemKind::Apfs));
        assert!(dev.volumes[0].efi_system);
    }

    #[test]
    fn startup_disk_is_system_through_its_apfs_container() {
        let list = parse_disk_list(LIST.as_bytes()).unwrap();
        let apfs = parse_apfs_containers(APFS_VIRTUAL.as_bytes()).unwrap();
        // Even without the Internal flag, a container mounted under /System
        // marks the disk as the running system.
        let whole = DiskInfo { bus_protocol: Some("USB".into()), ..Default::default() };
        let dev = build_device(&list[0], &whole, &[], None, &apfs);
        assert!(dev.is_system);
        assert_eq!(dev.volumes[1].label.as_deref(), Some("Data"));
    }

    #[test]
    fn mbr_type_byte_alone_does_not_name_the_filesystem() {
        let list = parse_disk_list(LIST.as_bytes()).unwrap();
        let dev = build_device(&list[1], &DiskInfo::default(), &[], None, &HashMap::new());
        assert_eq!(dev.volumes[0].filesystem, None);
        assert!(!dev.volumes[0].efi_system);
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(parse_disk_list(b"not a plist").is_err());
        assert!(parse_apfs_containers(b"not a plist").is_err());
        let _ = parse_ioreg_usb(b"<plist><array><dict><key>x</key></dict></array></plist>");
    }
}
