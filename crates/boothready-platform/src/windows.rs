//! Windows backend using native storage APIs.
//!
//! Physical drives are addressed as `\\.\PhysicalDriveN` and identified by
//! the storage descriptor and the USB device instance above them; drive
//! letters are only ever reported, never used as identity (PRD §50).
//!
//! Status: type-checked in CI for `x86_64-pc-windows-msvc` and smoke-tested
//! on CI runners (enumeration only). Needs hardware testing with real USB
//! drives before release.

use crate::windows_layout::{self, DriveLayout};
use crate::{Platform, PlatformError};
use boothready_model::{BusType, FilesystemKind, PhysicalDevice, UsbDescriptor, Volume};
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::PathBuf;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_IDW, CM_Get_Parent, CM_Request_Device_EjectW, SetupDiDestroyDeviceInfoList,
    SetupDiEnumDeviceInterfaces, SetupDiGetClassDevsW, SetupDiGetDeviceInterfaceDetailW, CR_SUCCESS,
    DIGCF_DEVICEINTERFACE, DIGCF_PRESENT, PNP_VETO_TYPE, SP_DEVICE_INTERFACE_DATA, SP_DEVICE_INTERFACE_DETAIL_DATA_W,
    SP_DEVINFO_DATA,
};
use windows::Win32::Foundation::{
    CloseHandle, ERROR_INSUFFICIENT_BUFFER, ERROR_MORE_DATA, GENERIC_READ, GENERIC_WRITE, HANDLE,
};
use windows::Win32::Storage::FileSystem::{
    BusTypeUsb, CreateFileW, FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDiskFreeSpaceExW,
    GetDiskFreeSpaceW, GetVolumeInformationW, GetVolumePathNameW, GetVolumePathNamesForVolumeNameW,
    FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, OPEN_EXISTING,
};
use windows::Win32::System::Ioctl::{
    PropertyStandardQuery, StorageDeviceProperty, DISK_GEOMETRY, DISK_GEOMETRY_EX, DRIVE_LAYOUT_INFORMATION_EX,
    FSCTL_DISMOUNT_VOLUME, FSCTL_LOCK_VOLUME, GET_LENGTH_INFORMATION, GUID_DEVINTERFACE_DISK,
    IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, IOCTL_DISK_GET_LENGTH_INFO,
    IOCTL_STORAGE_GET_DEVICE_NUMBER, IOCTL_STORAGE_QUERY_PROPERTY, PARTITION_INFORMATION_EX, STORAGE_DEVICE_DESCRIPTOR,
    STORAGE_DEVICE_NUMBER, STORAGE_PROPERTY_QUERY,
};
use windows::Win32::System::SystemInformation::GetSystemWindowsDirectoryW;
use windows::Win32::System::IO::DeviceIoControl;

pub struct WindowsPlatform;

struct Handle(HANDLE);

impl Drop for Handle {
    fn drop(&mut self) {
        // SAFETY: the handle came from CreateFileW and is closed exactly once.
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn open(path: &str, access: u32) -> Option<Handle> {
    let w = wide(path);
    // SAFETY: `w` is a valid NUL-terminated UTF-16 string for the call.
    let h = unsafe {
        CreateFileW(
            PCWSTR(w.as_ptr()),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .ok()?;
    Some(Handle(h))
}

/// Issue an IOCTL and return the output buffer, growing it if needed.
fn ioctl(h: &Handle, code: u32, input: Option<&[u8]>, initial: usize) -> Option<Vec<u8>> {
    let mut size = initial;
    for _ in 0..6 {
        let mut out = vec![0u8; size];
        let mut returned = 0u32;
        // SAFETY: buffers are valid for the sizes passed; the handle is open.
        let r = unsafe {
            DeviceIoControl(
                h.0,
                code,
                input.map(|i| i.as_ptr() as *const c_void),
                input.map(|i| i.len() as u32).unwrap_or(0),
                Some(out.as_mut_ptr() as *mut c_void),
                out.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        match r {
            Ok(()) => {
                out.truncate(returned as usize);
                return Some(out);
            }
            // Only a short buffer is worth another try. Anything else, such
            // as access denied or a refused volume lock, fails again.
            Err(e)
                if e.code() == ERROR_INSUFFICIENT_BUFFER.to_hresult() || e.code() == ERROR_MORE_DATA.to_hresult() =>
            {
                size = (size * 4).max(256)
            }
            Err(_) => return None,
        }
    }
    None
}

fn read_struct<T: Copy>(buf: &[u8]) -> Option<T> {
    if buf.len() < std::mem::size_of::<T>() {
        return None;
    }
    // SAFETY: length checked; `read_unaligned` tolerates any alignment.
    Some(unsafe { std::ptr::read_unaligned(buf.as_ptr() as *const T) })
}

fn cstr_at(buf: &[u8], off: u32) -> Option<String> {
    let off = off as usize;
    if off == 0 || off >= buf.len() {
        return None;
    }
    let end = buf[off..].iter().position(|&b| b == 0).map(|e| off + e).unwrap_or(buf.len());
    Some(String::from_utf8_lossy(&buf[off..end]).trim().to_string()).filter(|s| !s.is_empty())
}

struct Descriptor {
    bus_usb: bool,
    removable: bool,
    vendor: Option<String>,
    product: Option<String>,
    revision: Option<String>,
    serial: Option<String>,
}

/// The raw `STORAGE_DEVICE_DESCRIPTOR` buffer, strings included.
fn descriptor_raw(h: &Handle) -> Option<Vec<u8>> {
    let q = STORAGE_PROPERTY_QUERY {
        PropertyId: StorageDeviceProperty,
        QueryType: PropertyStandardQuery,
        AdditionalParameters: [0],
    };
    // SAFETY: STORAGE_PROPERTY_QUERY is plain old data.
    let qb = unsafe {
        std::slice::from_raw_parts(&q as *const _ as *const u8, std::mem::size_of::<STORAGE_PROPERTY_QUERY>())
    };
    ioctl(h, IOCTL_STORAGE_QUERY_PROPERTY, Some(qb), 1024)
}

fn descriptor(h: &Handle) -> Option<Descriptor> {
    let buf = descriptor_raw(h)?;
    let d: STORAGE_DEVICE_DESCRIPTOR = read_struct(&buf)?;
    Some(Descriptor {
        bus_usb: d.BusType == BusTypeUsb,
        removable: d.RemovableMedia,
        vendor: cstr_at(&buf, d.VendorIdOffset),
        product: cstr_at(&buf, d.ProductIdOffset),
        revision: cstr_at(&buf, d.ProductRevisionOffset),
        serial: cstr_at(&buf, d.SerialNumberOffset),
    })
}

struct VolumeRec {
    guid_path: String,
    disk: u32,
    offset: u64,
    length: u64,
}

/// Every mounted volume and the physical disk it lives on.
/// The little-endian `u32` at `o`. Like indexing, it panics when `b` is too
/// short; callers check the length first.
fn le_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// The little-endian `i64` at `o`, with the same bounds behaviour as `le_u32`.
fn le_i64(b: &[u8], o: usize) -> i64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    i64::from_le_bytes(a)
}

fn volumes() -> Vec<VolumeRec> {
    let mut out = Vec::new();
    let mut name = [0u16; 512];
    // SAFETY: `name` is a valid writable buffer.
    let Ok(find) = (unsafe { FindFirstVolumeW(&mut name) }) else { return out };
    loop {
        let guid_path = from_wide(&name);
        // Opening a volume requires the path without its trailing backslash.
        if let Some(h) = open(guid_path.trim_end_matches('\\'), 0) {
            if let Some(buf) = ioctl(&h, IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS, None, 256) {
                if buf.len() >= 8 {
                    let count = le_u32(&buf, 0) as usize;
                    // DISK_EXTENT { DiskNumber u32, pad u32, StartingOffset i64, ExtentLength i64 }
                    for i in 0..count {
                        let o = 8 + i * 24;
                        if o + 24 > buf.len() {
                            break;
                        }
                        out.push(VolumeRec {
                            guid_path: guid_path.clone(),
                            disk: le_u32(&buf, o),
                            offset: le_i64(&buf, o + 8) as u64,
                            length: le_i64(&buf, o + 16) as u64,
                        });
                    }
                }
            }
        }
        // SAFETY: `find` is a valid search handle and `name` is writable.
        if unsafe { FindNextVolumeW(find, &mut name) }.is_err() {
            break;
        }
    }
    // SAFETY: closing the handle FindFirstVolumeW returned.
    unsafe {
        let _ = FindVolumeClose(find);
    }
    out
}

struct VolumeDetails {
    mount: Option<PathBuf>,
    label: Option<String>,
    fs: Option<String>,
    serial: Option<u32>,
}

fn volume_details(guid_path: &str) -> VolumeDetails {
    let w = wide(guid_path);
    let mut paths = vec![0u16; 1024];
    let mut len = 0u32;
    // SAFETY: buffers are valid and sized as passed.
    let mount = unsafe { GetVolumePathNamesForVolumeNameW(PCWSTR(w.as_ptr()), Some(&mut paths), &mut len) }
        .ok()
        .map(|_| from_wide(&paths))
        .filter(|p| !p.is_empty())
        .map(PathBuf::from);
    let mut label = [0u16; 261];
    let mut fs = [0u16; 261];
    let mut serial = 0u32;
    // SAFETY: buffers are valid and sized as passed.
    let ok = unsafe {
        GetVolumeInformationW(PCWSTR(w.as_ptr()), Some(&mut label), Some(&mut serial), None, None, Some(&mut fs))
    }
    .is_ok();
    VolumeDetails {
        mount,
        label: ok.then(|| from_wide(&label)).filter(|s| !s.is_empty()),
        fs: ok.then(|| from_wide(&fs)).filter(|s| !s.is_empty()),
        serial: ok.then_some(serial),
    }
}

fn fs_kind(name: &str) -> Option<FilesystemKind> {
    match name.to_ascii_uppercase().as_str() {
        "FAT32" => Some(FilesystemKind::Fat32),
        "FAT" => Some(FilesystemKind::Fat16),
        "EXFAT" => Some(FilesystemKind::Exfat),
        "NTFS" => Some(FilesystemKind::Ntfs),
        _ => None,
    }
}

/// Disk numbers holding the Windows directory.
fn system_disks(vols: &[VolumeRec]) -> HashSet<u32> {
    let mut buf = [0u16; 260];
    // SAFETY: `buf` is writable.
    let n = unsafe { GetSystemWindowsDirectoryW(Some(&mut buf)) } as usize;
    let win = String::from_utf16_lossy(&buf[..n.min(buf.len())]).to_ascii_uppercase();
    let drive = win.get(..3).unwrap_or("C:\\").to_string();
    vols.iter()
        .filter(|v| {
            volume_details(&v.guid_path)
                .mount
                .map(|m| m.to_string_lossy().to_ascii_uppercase().starts_with(&drive))
                .unwrap_or(false)
        })
        .map(|v| v.disk)
        .collect()
}

struct UsbIdentity {
    desc: UsbDescriptor,
    devinst: u32,
    /// "USB\VID_0781&PID_5581\<serial>", kept for diagnostics.
    instance_id: String,
}

/// Map disk numbers to the USB device instance above each disk.
fn usb_by_disk_number() -> HashMap<u32, UsbIdentity> {
    let mut out = HashMap::new();
    // SAFETY: standard SetupAPI enumeration; every buffer is sized as passed
    // and the info list is destroyed before returning.
    unsafe {
        let guid: GUID = GUID_DEVINTERFACE_DISK;
        let Ok(info) = SetupDiGetClassDevsW(Some(&guid), PCWSTR::null(), None, DIGCF_PRESENT | DIGCF_DEVICEINTERFACE)
        else {
            return out;
        };
        let mut index = 0;
        loop {
            let mut iface = SP_DEVICE_INTERFACE_DATA {
                cbSize: std::mem::size_of::<SP_DEVICE_INTERFACE_DATA>() as u32,
                ..Default::default()
            };
            if SetupDiEnumDeviceInterfaces(info, None, &guid, index, &mut iface).is_err() {
                break;
            }
            index += 1;
            let mut needed = 0u32;
            let _ = SetupDiGetDeviceInterfaceDetailW(info, &iface, None, 0, Some(&mut needed), None);
            if needed == 0 {
                continue;
            }
            let mut buf = vec![0u8; needed as usize + 8];
            let detail = buf.as_mut_ptr() as *mut SP_DEVICE_INTERFACE_DETAIL_DATA_W;
            (*detail).cbSize = std::mem::size_of::<SP_DEVICE_INTERFACE_DETAIL_DATA_W>() as u32;
            let mut devinfo =
                SP_DEVINFO_DATA { cbSize: std::mem::size_of::<SP_DEVINFO_DATA>() as u32, ..Default::default() };
            if SetupDiGetDeviceInterfaceDetailW(info, &iface, Some(detail), needed, None, Some(&mut devinfo)).is_err() {
                continue;
            }
            let path_ptr = std::ptr::addr_of!((*detail).DevicePath) as *const u16;
            let path_len = (needed as usize - 4) / 2;
            let path = from_wide(std::slice::from_raw_parts(path_ptr, path_len));
            let Some(h) = open(&path, 0) else { continue };
            let Some(num) = ioctl(&h, IOCTL_STORAGE_GET_DEVICE_NUMBER, None, 64)
                .and_then(|b| read_struct::<STORAGE_DEVICE_NUMBER>(&b))
            else {
                continue;
            };
            // Walk up: disk -> USBSTOR/UAS -> USB device ("USB\VID_0781&PID_5581\<serial>").
            let mut inst = devinfo.DevInst;
            for _ in 0..4 {
                let mut parent = 0u32;
                if CM_Get_Parent(&mut parent, inst, 0) != CR_SUCCESS {
                    break;
                }
                inst = parent;
                let mut id = [0u16; 512];
                if CM_Get_Device_IDW(inst, &mut id, 0) != CR_SUCCESS {
                    break;
                }
                let id = from_wide(&id);
                if let Some(desc) = parse_usb_instance_id(&id) {
                    out.insert(num.DeviceNumber, UsbIdentity { desc, devinst: inst, instance_id: id });
                    break;
                }
            }
        }
        let _ = SetupDiDestroyDeviceInfoList(info);
    }
    out
}

/// PowerShell's own view of the disks, to hold our reading against.
const POWERSHELL_DISKS: &str = "Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,OSArchitecture | ConvertTo-Json; \
Get-Disk | Select-Object Number,FriendlyName,BusType,PartitionStyle,Size,LogicalSectorSize,IsSystem,IsBoot,IsOffline | ConvertTo-Json; \
Get-Partition | Select-Object DiskNumber,PartitionNumber,Offset,Size,Type,GptType,MbrType,DriveLetter,IsSystem | ConvertTo-Json; \
Get-Volume | Select-Object DriveLetter,FileSystemLabel,FileSystem,DriveType,Size,SizeRemaining,AllocationUnitSize | ConvertTo-Json";

/// Raw output for a diagnostics report: the IOCTL buffers each drive
/// answered with, every volume with its disk extent, the USB instance above
/// each disk, and PowerShell's view of the same disks.
pub(crate) fn captures() -> Vec<crate::diagnostics::Capture> {
    use crate::diagnostics::Capture;
    let mut out = Vec::new();
    for n in 0..64u32 {
        let Some(h) = open(&format!("\\\\.\\PhysicalDrive{n}"), 0) else { continue };
        let drive = format!("PhysicalDrive{n}");
        let buffers = [
            ("IOCTL_STORAGE_QUERY_PROPERTY", descriptor_raw(&h)),
            ("IOCTL_DISK_GET_DRIVE_GEOMETRY_EX", ioctl(&h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, None, 256)),
            ("IOCTL_DISK_GET_DRIVE_LAYOUT_EX", ioctl(&h, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, None, 4096)),
        ];
        for (call, buf) in buffers {
            out.push(match buf {
                Some(b) => Capture::bytes(&[call, &drive], &b),
                None => Capture::failed(&[call, &drive], "no answer"),
            });
        }
    }
    for v in volumes() {
        let d = volume_details(&v.guid_path);
        let rec = serde_json::json!({
            "disk": v.disk,
            "offset": v.offset,
            "length": v.length,
            "mount": d.mount,
            "label": d.label,
            "filesystem": d.fs,
        });
        out.push(Capture::text(&["IOCTL_VOLUME_GET_VOLUME_DISK_EXTENTS", &v.guid_path], rec.to_string()));
    }
    let mut usb: Vec<(u32, UsbIdentity)> = usb_by_disk_number().into_iter().collect();
    usb.sort_by_key(|(n, _)| *n);
    for (n, id) in usb {
        out.push(Capture::text(&["CM_Get_Device_IDW", &format!("PhysicalDrive{n}")], id.instance_id));
    }
    out.push(Capture::command("powershell.exe", &["-NoProfile", "-NonInteractive", "-Command", POWERSHELL_DISKS]));
    out
}

/// Parse "USB\VID_0781&PID_5581\4C530001230518104F82".
pub fn parse_usb_instance_id(id: &str) -> Option<UsbDescriptor> {
    let upper = id.to_ascii_uppercase();
    if !upper.starts_with("USB\\VID_") {
        return None;
    }
    let mut parts = id.split('\\');
    parts.next();
    let ids = parts.next()?;
    let serial = parts.next().map(str::to_string).filter(|s| !s.contains('&'));
    let hex = |key: &str| {
        ids.to_ascii_uppercase()
            .split('&')
            .find_map(|p| p.strip_prefix(key).and_then(|v| u16::from_str_radix(v, 16).ok()))
    };
    Some(UsbDescriptor { vendor_id: hex("VID_"), product_id: hex("PID_"), serial, ..Default::default() })
}

// The layout parser reads these structs as bytes; hold it to the real layout.
const _: () = {
    use crate::windows_layout::{ENTRY_SIZE, ENTRY_START, ENTRY_TYPE, LAYOUT_ENTRIES};
    use std::mem::{offset_of, size_of};
    assert!(offset_of!(DRIVE_LAYOUT_INFORMATION_EX, PartitionEntry) == LAYOUT_ENTRIES);
    assert!(size_of::<PARTITION_INFORMATION_EX>() == ENTRY_SIZE);
    assert!(offset_of!(PARTITION_INFORMATION_EX, StartingOffset) == ENTRY_START);
    assert!(offset_of!(PARTITION_INFORMATION_EX, Anonymous) == ENTRY_TYPE);
};

/// Logical sector size and capacity. `DISK_GEOMETRY_EX` ends in a
/// variable-length member, so a driver may return fewer bytes than the
/// struct's size; the fixed fields are read by offset instead.
fn geometry(h: &Handle) -> Option<(u32, u64)> {
    const SECTOR: usize =
        std::mem::offset_of!(DISK_GEOMETRY_EX, Geometry) + std::mem::offset_of!(DISK_GEOMETRY, BytesPerSector);
    const SIZE: usize = std::mem::offset_of!(DISK_GEOMETRY_EX, DiskSize);
    let b = ioctl(h, IOCTL_DISK_GET_DRIVE_GEOMETRY_EX, None, 256)?;
    let sector = u32::from_le_bytes(b.get(SECTOR..SECTOR + 4)?.try_into().ok()?);
    let size = i64::from_le_bytes(b.get(SIZE..SIZE + 8)?.try_into().ok()?);
    Some((sector, size.max(0) as u64))
}

fn drive_layout(h: &Handle) -> Option<DriveLayout> {
    windows_layout::parse(&ioctl(h, IOCTL_DISK_GET_DRIVE_LAYOUT_EX, None, 4096)?)
}

impl WindowsPlatform {
    fn devices(&self) -> Vec<(PhysicalDevice, Option<u32>)> {
        let vols = volumes();
        let system = system_disks(&vols);
        let usb = usb_by_disk_number();
        let mut out = Vec::new();
        for n in 0..64u32 {
            let path = format!("\\\\.\\PhysicalDrive{n}");
            let Some(h) = open(&path, 0) else { continue };
            let Some(d) = descriptor(&h) else { continue };
            let geo = geometry(&h);
            let sector = geo.map(|g| g.0).filter(|&s| s > 0).unwrap_or(512);
            // The handle is opened without read access so listing drives
            // doesn't need admin. The geometry IOCTL works on such a handle;
            // IOCTL_DISK_GET_LENGTH_INFO needs read access.
            let size = geo
                .map(|g| g.1)
                .filter(|&s| s > 0)
                .or_else(|| {
                    ioctl(&h, IOCTL_DISK_GET_LENGTH_INFO, None, 64)
                        .and_then(|b| read_struct::<GET_LENGTH_INFORMATION>(&b))
                        .map(|l| l.Length as u64)
                })
                .unwrap_or(0);
            let layout = drive_layout(&h);
            let volumes: Vec<Volume> = vols
                .iter()
                .filter(|v| v.disk == n)
                .map(|v| {
                    let det = volume_details(&v.guid_path);
                    Volume {
                        os_path: v.guid_path.clone(),
                        mount_point: det.mount,
                        label: det.label,
                        filesystem: det.fs.as_deref().and_then(fs_kind),
                        size_bytes: v.length,
                        offset_bytes: Some(v.offset),
                        uuid: det.serial.map(|s| format!("{:04X}-{:04X}", s >> 16, s & 0xFFFF)),
                        efi_system: layout.as_ref().is_some_and(|l| l.efi_starts.contains(&v.offset)),
                    }
                })
                .collect();
            let usb_id = usb.get(&n);
            let mut usb_desc = usb_id.map(|u| u.desc.clone());
            if let Some(u) = usb_desc.as_mut() {
                u.product = u.product.clone().or_else(|| d.product.clone());
                u.manufacturer = u.manufacturer.clone().or_else(|| d.vendor.clone());
                if u.serial.is_none() {
                    u.serial = d.serial.clone();
                }
            }
            let dev = PhysicalDevice {
                id: format!("PhysicalDrive{n}"),
                os_path: path,
                bus: if d.bus_usb { BusType::Usb } else { BusType::Other("non-usb".into()) },
                removable: d.removable,
                is_system: system.contains(&n),
                size_bytes: size,
                logical_sector_size: sector,
                storage_vendor: d.vendor,
                storage_model: d.product,
                storage_revision: d.revision,
                usb: usb_desc,
                partition_scheme: layout.as_ref().map(|l| l.scheme),
                volumes,
            };
            out.push((dev, usb_id.map(|u| u.devinst)));
        }
        out
    }
}

/// Free space for the current user and the cluster size of the volume
/// holding `path`.
pub(crate) fn space(path: &std::path::Path) -> std::io::Result<crate::Space> {
    let w = wide(&path.to_string_lossy());
    let mut root = vec![0u16; 1024];
    // SAFETY: `w` is NUL-terminated and `root` is writable for its length.
    unsafe { GetVolumePathNameW(PCWSTR(w.as_ptr()), &mut root) }.map_err(std::io::Error::other)?;
    let mut free = 0u64;
    // SAFETY: valid path and out-pointer.
    unsafe { GetDiskFreeSpaceExW(PCWSTR(w.as_ptr()), Some(&mut free), None, None) }.map_err(std::io::Error::other)?;
    let (mut spc, mut bps) = (0u32, 0u32);
    // SAFETY: `root` holds the NUL-terminated volume root; out-pointers are valid.
    unsafe { GetDiskFreeSpaceW(PCWSTR(root.as_ptr()), Some(&mut spc), Some(&mut bps), None, None) }
        .map_err(std::io::Error::other)?;
    Ok(crate::Space { free_bytes: free, cluster_bytes: spc as u64 * bps as u64 })
}

impl Platform for WindowsPlatform {
    fn list_devices(&self) -> Result<Vec<PhysicalDevice>, PlatformError> {
        Ok(self.devices().into_iter().map(|(d, _)| d).collect())
    }

    fn unmount(&self, device_id: &str) -> Result<(), PlatformError> {
        let (dev, _) = self
            .devices()
            .into_iter()
            .find(|(d, _)| d.id == device_id)
            .ok_or_else(|| PlatformError::NotFound(device_id.into()))?;
        for v in &dev.volumes {
            let h = open(v.os_path.trim_end_matches('\\'), GENERIC_READ.0 | GENERIC_WRITE.0)
                .ok_or_else(|| PlatformError::PermissionDenied(format!("can't open {}", v.os_path)))?;
            if ioctl(&h, FSCTL_LOCK_VOLUME, None, 0).is_none() {
                return Err(PlatformError::Busy { holder: None });
            }
            let _ = ioctl(&h, FSCTL_DISMOUNT_VOLUME, None, 0);
        }
        Ok(())
    }

    fn eject(&self, device_id: &str) -> Result<(), PlatformError> {
        let (_, devinst) = self
            .devices()
            .into_iter()
            .find(|(d, _)| d.id == device_id)
            .ok_or_else(|| PlatformError::NotFound(device_id.into()))?;
        let devinst = devinst.ok_or_else(|| PlatformError::Failed("no USB device found above this disk".into()))?;
        let mut veto = PNP_VETO_TYPE(0);
        let mut name = [0u16; 512];
        // SAFETY: valid devinst from CfgMgr32 and writable buffers.
        let r = unsafe { CM_Request_Device_EjectW(devinst, Some(&mut veto), Some(&mut name), 0) };
        if r == CR_SUCCESS && veto.0 == 0 {
            return Ok(());
        }
        // Windows names the vetoing driver or process path when it can.
        let holder = Some(from_wide(&name)).filter(|s| !s.is_empty());
        Err(PlatformError::Busy { holder })
    }

    fn name(&self) -> &'static str {
        "windows"
    }
}
