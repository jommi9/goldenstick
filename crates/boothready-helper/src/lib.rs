//! The privileged helper's request handling, as a library.
//!
//! The release binary wires this to the native platform and runs elevated.
//! Demo and test builds call the same code in-process against disk images,
//! so the erase path that ships is the one the tests exercise.

use boothready_core::format::{build_fat32, normalize_label, plan_layout, write_mbr};
use boothready_core::io::AlignedIo;
use boothready_core::media::{inspect, MediaLayout};
use boothready_core::privileged::{check_target, HelperErrorCode, HelperEvent, HelperRequest, PROTOCOL_VERSION};
use boothready_model::{FilesystemKind, PartitionScheme, PhysicalDevice};
use boothready_platform::demo::DemoPlatform;
use boothready_platform::raw::RawDisk;
use boothready_platform::{Platform, PlatformError};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub type Emit<'a> = &'a mut dyn FnMut(HelperEvent);

/// OS-specific pieces of preparing a drive.
pub trait Backend: Send + Sync {
    fn platform(&self) -> &dyn Platform;
    fn open_raw(&self, dev: &PhysicalDevice) -> Result<RawDisk, PlatformError>;
    /// Build an MBR + exFAT drive. There's no vetted pure-Rust exFAT
    /// formatter, so each OS uses its own.
    fn build_exfat(&self, dev: &PhysicalDevice, label: &str, seed: u64) -> Result<(), String>;
    /// Open the device read-only for the post-format check.
    fn open_read(&self, dev: &PhysicalDevice) -> std::io::Result<fs::File> {
        fs::File::open(&dev.os_path)
    }
    fn after_prepare(&self, _dev: &PhysicalDevice, _layout: &MediaLayout) -> Result<(), String> {
        Ok(())
    }
}

fn err(code: HelperErrorCode, message: impl Into<String>) -> HelperEvent {
    HelperEvent::Error { code, message: message.into() }
}

fn platform_err(e: PlatformError) -> HelperEvent {
    match e {
        PlatformError::NotFound(m) => err(HelperErrorCode::NotFound, format!("device {m} not found")),
        PlatformError::Busy { holder } => err(
            HelperErrorCode::Busy,
            match holder {
                Some(h) => format!("{h} is using this USB. Close it and try again."),
                None => "Another application is using this USB. Close it and try again.".into(),
            },
        ),
        PlatformError::PermissionDenied(m) => err(HelperErrorCode::PermissionDenied, m),
        other => err(HelperErrorCode::Failed, other.to_string()),
    }
}

fn progress(emit: Emit<'_>, step: &str, detail: &str) {
    emit(HelperEvent::Progress { step: step.into(), detail: detail.into() });
}

pub fn handle(backend: &dyn Backend, req: HelperRequest, emit: Emit<'_>) {
    match req {
        HelperRequest::Hello { protocol } => {
            if protocol != PROTOCOL_VERSION {
                emit(err(
                    HelperErrorCode::BadRequest,
                    format!("protocol {protocol} not supported (helper speaks {PROTOCOL_VERSION})"),
                ));
            } else {
                emit(HelperEvent::Hello { protocol: PROTOCOL_VERSION, version: env!("CARGO_PKG_VERSION").into() });
            }
        }
        HelperRequest::ListDevices => match backend.platform().list_devices() {
            Ok(devices) => emit(HelperEvent::Devices { devices }),
            Err(e) => emit(platform_err(e)),
        },
        HelperRequest::Inspect { device_id } => match find(backend, &device_id) {
            Ok(dev) => match backend.open_read(&dev).and_then(inspect) {
                Ok(layout) => emit(HelperEvent::Done { detail: serde_json::to_string(&layout).unwrap_or_default() }),
                Err(e) => emit(err(HelperErrorCode::Failed, format!("could not read the drive: {e}"))),
            },
            Err(e) => emit(e),
        },
        HelperRequest::Unmount { device_id } => match backend.platform().unmount(&device_id) {
            Ok(()) => emit(HelperEvent::Done { detail: "unmounted".into() }),
            Err(e) => emit(platform_err(e)),
        },
        HelperRequest::Eject { device_id } => match backend.platform().eject(&device_id) {
            Ok(()) => emit(HelperEvent::Done { detail: "ejected".into() }),
            Err(e) => emit(platform_err(e)),
        },
        HelperRequest::Prepare { expected, confirmation, scheme, filesystem, label } => {
            match prepare(backend, &expected, &confirmation, scheme, filesystem, &label, emit) {
                Ok(detail) => emit(HelperEvent::Done { detail }),
                Err(e) => emit(e),
            }
        }
    }
}

fn find(backend: &dyn Backend, id: &str) -> Result<PhysicalDevice, HelperEvent> {
    backend
        .platform()
        .list_devices()
        .map_err(platform_err)?
        .into_iter()
        .find(|d| d.id == id)
        .ok_or_else(|| err(HelperErrorCode::NotFound, "The USB drive is no longer connected. Nothing was erased."))
}

#[allow(clippy::too_many_arguments)]
fn prepare(
    backend: &dyn Backend,
    expected: &boothready_core::privileged::DeviceFingerprint,
    confirmation: &str,
    scheme: PartitionScheme,
    filesystem: FilesystemKind,
    label: &str,
    emit: Emit<'_>,
) -> Result<String, HelperEvent> {
    let label = normalize_label(label).map_err(|e| err(HelperErrorCode::BadRequest, e.to_string()))?;
    if scheme != PartitionScheme::Mbr {
        return Err(err(HelperErrorCode::Refused, "Only MBR layouts are supported."));
    }
    if !matches!(filesystem, FilesystemKind::Fat32 | FilesystemKind::Exfat) {
        return Err(err(HelperErrorCode::Refused, format!("{} is not a supported DJ format.", filesystem.label())));
    }
    progress(emit, "checking", "Confirming this is the drive you selected");
    let dev = find(backend, &expected.device_id)?;
    check_target(&dev, expected, confirmation).map_err(|m| err(HelperErrorCode::Refused, m))?;

    progress(emit, "unmounting", "Unmounting the drive");
    backend.platform().unmount(&dev.id).map_err(platform_err)?;

    let seed = u64::from_str_radix(&confirmation[..16.min(confirmation.len())], 16).unwrap_or(0x5EED)
        ^ std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos() as u64).unwrap_or(0);
    let sector = dev.logical_sector_size.max(512);
    match filesystem {
        FilesystemKind::Fat32 => {
            progress(emit, "partitioning", "Writing a new MBR partition table");
            let raw = backend.open_raw(&dev).map_err(platform_err)?;
            let size = raw.size;
            let mut io = AlignedIo::new(raw, 4096.max(sector as u64), size);
            progress(emit, "formatting", "Formatting as FAT32");
            build_fat32(&mut io, size, sector, &label, seed)
                .map_err(|e| err(HelperErrorCode::Failed, e.to_string()))?;
            let raw = io.into_inner().map_err(|e| err(HelperErrorCode::Failed, e.to_string()))?;
            raw.finish().map_err(platform_err)?;
        }
        _ => {
            progress(emit, "formatting", "Partitioning and formatting as exFAT");
            backend.build_exfat(&dev, &label, seed).map_err(|m| err(HelperErrorCode::Failed, m))?;
        }
    }

    progress(emit, "verifying", "Checking the new layout");
    let layout = backend
        .open_read(&dev)
        .and_then(inspect)
        .map_err(|e| err(HelperErrorCode::Failed, format!("could not re-read the drive: {e}")))?;
    let got_fs = layout.primary_filesystem();
    let got_label = layout.primary().and_then(|p| p.filesystem.as_ref()).and_then(|f| f.label.clone());
    if layout.scheme != PartitionScheme::Mbr
        || got_fs != Some(filesystem)
        || got_label.as_deref() != Some(label.as_str())
    {
        return Err(err(
            HelperErrorCode::Failed,
            format!(
                "The drive doesn't look right after formatting ({} / {} / {}). Don't use it until it's rebuilt.",
                layout.scheme.label(),
                got_fs.map(|f| f.label()).unwrap_or("no filesystem"),
                got_label.unwrap_or_default()
            ),
        ));
    }
    backend.after_prepare(&dev, &layout).map_err(|m| err(HelperErrorCode::Failed, m))?;
    Ok(serde_json::to_string(&layout).unwrap_or_default())
}

/// Backend for the real OS.
pub struct NativeBackend {
    platform: Box<dyn Platform>,
}

impl NativeBackend {
    pub fn new() -> NativeBackend {
        NativeBackend { platform: boothready_platform::native() }
    }
}

impl Default for NativeBackend {
    fn default() -> Self {
        Self::new()
    }
}

fn run_fixed(program: &str, args: &[&str]) -> Result<(), String> {
    let out = Command::new(program).args(args).output().map_err(|e| format!("{program}: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed: {}", String::from_utf8_lossy(&out.stderr).trim()))
    }
}

impl Backend for NativeBackend {
    fn platform(&self) -> &dyn Platform {
        self.platform.as_ref()
    }

    fn open_raw(&self, dev: &PhysicalDevice) -> Result<RawDisk, PlatformError> {
        RawDisk::open_device(dev)
    }

    fn build_exfat(&self, dev: &PhysicalDevice, label: &str, seed: u64) -> Result<(), String> {
        if cfg!(target_os = "macos") {
            // diskutil builds the MBR and exFAT volume in one step.
            return run_fixed("/usr/sbin/diskutil", &["eraseDisk", "ExFAT", label, "MBRFormat", &dev.id]);
        }
        if cfg!(windows) {
            let n = dev.id.trim_start_matches("PhysicalDrive");
            if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
                return Err("unexpected device id".into());
            }
            // The label is already restricted to [A-Z0-9_-], so it can't
            // escape this script.
            let script = format!(
                "select disk {n}\nclean\ncreate partition primary align=1024\nformat fs=exfat label=\"{label}\" quick\nassign\nexit\n"
            );
            let path = std::env::temp_dir().join(format!("boothready-{seed:x}.txt"));
            fs::write(&path, script).map_err(|e| e.to_string())?;
            let r = run_fixed("diskpart", &["/s", &path.to_string_lossy()]);
            let _ = fs::remove_file(&path);
            return r;
        }
        // Linux: our MBR, then mkfs.exfat on the new partition node.
        let raw = RawDisk::open_device(dev).map_err(|e| e.to_string())?;
        let size = raw.size;
        let layout =
            plan_layout(size, dev.logical_sector_size.max(512), FilesystemKind::Exfat).map_err(|e| e.to_string())?;
        let mut io = AlignedIo::new(raw, 4096, size);
        write_mbr(&mut io, size, &layout, seed as u32).map_err(|e| e.to_string())?;
        io.into_inner().map_err(|e| e.to_string())?.finish().map_err(|e| e.to_string())?;
        let node = boothready_platform::raw::first_partition_node(&dev.os_path);
        for _ in 0..50 {
            if Path::new(&node).exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        run_fixed("mkfs.exfat", &["-L", label, &node])
    }
}

/// Backend over the folder-based demo platform. Formats `disk.img` and
/// resets the demo volume folder, so the whole flow can be exercised
/// without touching real hardware.
pub struct DemoBackend {
    pub platform: DemoPlatform,
}

impl DemoBackend {
    pub fn new(root: PathBuf) -> DemoBackend {
        DemoBackend { platform: DemoPlatform::new(root) }
    }
}

impl Backend for DemoBackend {
    fn platform(&self) -> &dyn Platform {
        &self.platform
    }

    fn open_raw(&self, dev: &PhysicalDevice) -> Result<RawDisk, PlatformError> {
        RawDisk::open_image(Path::new(&dev.os_path), dev.size_bytes)
    }

    fn open_read(&self, dev: &PhysicalDevice) -> std::io::Result<fs::File> {
        fs::File::open(&dev.os_path)
    }

    fn build_exfat(&self, dev: &PhysicalDevice, label: &str, seed: u64) -> Result<(), String> {
        let size = dev.size_bytes;
        let layout = plan_layout(size, 512, FilesystemKind::Exfat).map_err(|e| e.to_string())?;
        let mut raw = RawDisk::open_image(Path::new(&dev.os_path), size).map_err(|e| e.to_string())?;
        write_mbr(&mut raw, size, &layout, seed as u32).map_err(|e| e.to_string())?;
        // Format a scratch partition image, then copy its non-zero blocks in.
        let tmp = PathBuf::from(format!("{}.part", dev.os_path));
        fs::File::create(&tmp).and_then(|f| f.set_len(layout.partition_len)).map_err(|e| e.to_string())?;
        let mkfs = ["/usr/sbin/mkfs.exfat", "/sbin/mkfs.exfat", "mkfs.exfat"]
            .into_iter()
            .find(|p| Path::new(p).exists() || !p.starts_with('/'))
            .unwrap_or("mkfs.exfat");
        let r = run_fixed(mkfs, &["-L", label, &tmp.to_string_lossy()]);
        if r.is_ok() {
            use std::io::{Read, Seek, SeekFrom, Write};
            let mut src = fs::File::open(&tmp).map_err(|e| e.to_string())?;
            let mut buf = vec![0u8; 1 << 20];
            let mut pos = 0u64;
            loop {
                let n = src.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                if buf[..n].iter().any(|&b| b != 0) {
                    raw.seek(SeekFrom::Start(layout.partition_start + pos)).map_err(|e| e.to_string())?;
                    raw.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                }
                pos += n as u64;
            }
        }
        let _ = fs::remove_file(&tmp);
        r?;
        raw.finish().map_err(|e| e.to_string())
    }

    fn after_prepare(&self, dev: &PhysicalDevice, layout: &MediaLayout) -> Result<(), String> {
        // Mirror what an OS does after formatting: empty volume, new label,
        // remounted.
        let dir = self.platform.device_dir(&dev.id);
        let vol = dir.join("volume");
        if vol.exists() {
            fs::remove_dir_all(&vol).map_err(|e| e.to_string())?;
        }
        fs::create_dir_all(&vol).map_err(|e| e.to_string())?;
        let mut updated = dev.clone();
        let fs_info = layout.primary().and_then(|p| p.filesystem.clone());
        updated.partition_scheme = Some(layout.scheme);
        if let Some(v) = updated.volumes.first_mut() {
            v.filesystem = fs_info.as_ref().map(|f| f.kind);
            v.label = fs_info.as_ref().and_then(|f| f.label.clone());
            v.uuid = fs_info.as_ref().and_then(|f| f.serial.clone());
            v.mount_point = None;
        }
        updated.volumes.truncate(1);
        fs::write(dir.join("device.json"), serde_json::to_vec_pretty(&updated).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let _ = fs::remove_file(dir.join(".unmounted"));
        Ok(())
    }
}

#[cfg(test)]
mod tests;
