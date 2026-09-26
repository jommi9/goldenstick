//! Linux backend: sysfs for devices and USB identity, mountinfo for mounts,
//! udev's database for partition-table and filesystem details.
//!
//! Linux isn't a release target (PRD §77), but this backend lets the whole
//! stack run and be tested on Linux CI and developer machines.

use crate::{run, Platform, PlatformError};
use boothready_model::{BusType, FilesystemKind, PartitionScheme, PhysicalDevice, UsbDescriptor, Volume};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

pub struct LinuxPlatform {
    sys: PathBuf,
    mountinfo: PathBuf,
    udev: PathBuf,
    dev: PathBuf,
}

/// Mount points whose presence marks a disk as the running system.
const SYSTEM_MOUNTS: &[&str] = &["/", "/boot", "/boot/efi", "/efi", "/usr", "/var", "/home", "/nix"];

#[derive(Debug, Clone)]
struct Mount {
    point: PathBuf,
    fstype: String,
}

impl LinuxPlatform {
    pub fn system() -> LinuxPlatform {
        LinuxPlatform::with_roots("/sys", "/proc/self/mountinfo", "/run/udev/data", "/dev")
    }

    pub fn with_roots(
        sys: impl Into<PathBuf>,
        mountinfo: impl Into<PathBuf>,
        udev: impl Into<PathBuf>,
        dev: impl Into<PathBuf>,
    ) -> LinuxPlatform {
        LinuxPlatform { sys: sys.into(), mountinfo: mountinfo.into(), udev: udev.into(), dev: dev.into() }
    }

    fn mounts(&self) -> HashMap<String, Vec<Mount>> {
        let mut out: HashMap<String, Vec<Mount>> = HashMap::new();
        let Ok(text) = fs::read_to_string(&self.mountinfo) else { return out };
        for line in text.lines() {
            // id parent maj:min root mountpoint opts [optional...] - fstype source superopts
            let fields: Vec<&str> = line.split(' ').collect();
            let Some(sep) = fields.iter().position(|f| *f == "-") else { continue };
            if fields.len() < 5 || sep + 1 >= fields.len() {
                continue;
            }
            out.entry(fields[2].to_string())
                .or_default()
                .push(Mount { point: PathBuf::from(unescape_mount(fields[4])), fstype: fields[sep + 1].to_string() });
        }
        out
    }

    fn udev_props(&self, majmin: &str) -> HashMap<String, String> {
        let mut out = HashMap::new();
        if let Ok(text) = fs::read_to_string(self.udev.join(format!("b{majmin}"))) {
            for line in text.lines() {
                if let Some(kv) = line.strip_prefix("E:") {
                    if let Some((k, v)) = kv.split_once('=') {
                        out.insert(k.to_string(), v.to_string());
                    }
                }
            }
        }
        out
    }

    fn device(&self, name: &str, mounts: &HashMap<String, Vec<Mount>>) -> Option<PhysicalDevice> {
        let dir = self.sys.join("block").join(name);
        let size_sectors: u64 = read_trim(&dir.join("size"))?.parse().ok()?;
        let removable = read_trim(&dir.join("removable")).as_deref() == Some("1");
        let lbs: u32 = read_trim(&dir.join("queue/logical_block_size")).and_then(|s| s.parse().ok()).unwrap_or(512);
        let real = fs::canonicalize(dir.join("device")).unwrap_or_default();
        let real_s = real.to_string_lossy();
        let bus = if real_s.contains("/usb") {
            BusType::Usb
        } else if name.starts_with("nvme") {
            BusType::Nvme
        } else if name.starts_with("mmcblk") {
            BusType::Sd
        } else if real_s.contains("/ata") {
            BusType::Sata
        } else if real_s.contains("virtio") || name.starts_with("vd") {
            BusType::Virtual
        } else {
            BusType::Other("unknown".into())
        };
        let usb = if bus == BusType::Usb { usb_descriptor(&real) } else { None };
        let majmin = read_trim(&dir.join("dev")).unwrap_or_default();
        let disk_props = self.udev_props(&majmin);
        let partition_scheme = disk_props.get("ID_PART_TABLE_TYPE").map(|t| match t.as_str() {
            "dos" => PartitionScheme::Mbr,
            "gpt" => PartitionScheme::Gpt,
            "atari" | "mac" => PartitionScheme::Apm,
            _ => PartitionScheme::Unknown,
        });

        let mut volumes = Vec::new();
        let mut parts: Vec<(String, PathBuf)> = fs::read_dir(&dir)
            .ok()?
            .filter_map(Result::ok)
            .filter(|e| e.path().join("partition").exists())
            .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
            .collect();
        parts.sort();
        let mut is_system = false;
        let mut volume_from = |node: String, pdir: &Path, offset: Option<u64>, size: u64| {
            let mm = read_trim(&pdir.join("dev")).unwrap_or_default();
            let props = self.udev_props(&mm);
            let mount = mounts.get(&mm).and_then(|m| m.first()).cloned();
            if mounts
                .get(&mm)
                .is_some_and(|ms| ms.iter().any(|m| SYSTEM_MOUNTS.contains(&m.point.to_string_lossy().as_ref())))
            {
                is_system = true;
            }
            let fs_name = props.get("ID_FS_TYPE").cloned().or_else(|| mount.as_ref().map(|m| m.fstype.clone()));
            let filesystem =
                fs_name.as_deref().and_then(|t| fs_kind(t, props.get("ID_FS_VERSION").map(String::as_str)));
            Volume {
                os_path: node,
                mount_point: mount.map(|m| m.point),
                label: props.get("ID_FS_LABEL").cloned().filter(|l| !l.is_empty()),
                filesystem,
                size_bytes: size,
                offset_bytes: offset,
                uuid: props.get("ID_FS_UUID").cloned(),
            }
        };
        for (pname, pdir) in &parts {
            let start: u64 = read_trim(&pdir.join("start")).and_then(|s| s.parse().ok()).unwrap_or(0);
            let psize: u64 = read_trim(&pdir.join("size")).and_then(|s| s.parse().ok()).unwrap_or(0);
            let node = self.dev.join(pname).to_string_lossy().into_owned();
            volumes.push(volume_from(node, pdir, Some(start * 512), psize * 512));
        }
        if parts.is_empty() && (mounts.contains_key(&majmin) || disk_props.contains_key("ID_FS_TYPE")) {
            // Superfloppy: filesystem directly on the whole device.
            let node = self.dev.join(name).to_string_lossy().into_owned();
            volumes.push(volume_from(node, &dir, Some(0), size_sectors * 512));
        }
        Some(PhysicalDevice {
            id: name.to_string(),
            os_path: self.dev.join(name).to_string_lossy().into_owned(),
            bus,
            removable,
            is_system,
            size_bytes: size_sectors * 512,
            logical_sector_size: lbs,
            storage_vendor: read_trim(&dir.join("device/vendor")),
            storage_model: read_trim(&dir.join("device/model")),
            storage_revision: read_trim(&dir.join("device/rev")),
            usb,
            partition_scheme,
            volumes,
        })
    }
}

fn read_trim(p: &Path) -> Option<String> {
    fs::read_to_string(p).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// mountinfo escapes space, tab, newline and backslash as octal.
fn unescape_mount(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            let v = (b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0');
            out.push(v);
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn fs_kind(t: &str, version: Option<&str>) -> Option<FilesystemKind> {
    Some(match t {
        "vfat" | "msdos" => match version {
            Some("FAT12") => FilesystemKind::Fat12,
            Some("FAT16") => FilesystemKind::Fat16,
            _ => FilesystemKind::Fat32,
        },
        "exfat" => FilesystemKind::Exfat,
        "ntfs" | "ntfs3" => FilesystemKind::Ntfs,
        "hfsplus" => FilesystemKind::HfsPlus,
        "apfs" => FilesystemKind::Apfs,
        "ext2" | "ext3" | "ext4" => FilesystemKind::Ext,
        _ => return None,
    })
}

/// Walk up from the SCSI device to the USB device node and read its
/// descriptor attributes.
fn usb_descriptor(scsi: &Path) -> Option<UsbDescriptor> {
    let mut cur = scsi.to_path_buf();
    for _ in 0..8 {
        if cur.join("idVendor").exists() {
            let hex = |f: &str| read_trim(&cur.join(f)).and_then(|s| u16::from_str_radix(&s, 16).ok());
            return Some(UsbDescriptor {
                vendor_id: hex("idVendor"),
                product_id: hex("idProduct"),
                manufacturer: read_trim(&cur.join("manufacturer")),
                product: read_trim(&cur.join("product")),
                serial: read_trim(&cur.join("serial")),
                bcd_device: hex("bcdDevice"),
                usb_version: read_trim(&cur.join("version")),
                speed_mbps: read_trim(&cur.join("speed")).and_then(|s| s.parse::<f64>().ok()).map(|v| v as u32),
                max_power_ma: read_trim(&cur.join("bMaxPower"))
                    .and_then(|s| s.trim_end_matches("mA").trim().parse().ok()),
            });
        }
        if !cur.pop() {
            break;
        }
    }
    None
}

fn skip_block(name: &str) -> bool {
    ["loop", "ram", "zram", "dm-", "md", "sr", "nbd", "fd", "mtdblock"].iter().any(|p| name.starts_with(p))
}

fn have(cmd: &str) -> bool {
    std::env::var_os("PATH").map(|paths| std::env::split_paths(&paths).any(|d| d.join(cmd).is_file())).unwrap_or(false)
}

impl Platform for LinuxPlatform {
    fn list_devices(&self) -> Result<Vec<PhysicalDevice>, PlatformError> {
        let mounts = self.mounts();
        let mut names: Vec<String> = fs::read_dir(self.sys.join("block"))?
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !skip_block(n))
            .collect();
        names.sort();
        Ok(names.iter().filter_map(|n| self.device(n, &mounts)).filter(|d| d.size_bytes > 0 || d.removable).collect())
    }

    fn unmount(&self, device_id: &str) -> Result<(), PlatformError> {
        let dev = self
            .list_devices()?
            .into_iter()
            .find(|d| d.id == device_id)
            .ok_or_else(|| PlatformError::NotFound(device_id.into()))?;
        for v in dev.volumes.iter().filter(|v| v.mount_point.is_some()) {
            if have("udisksctl") {
                run("udisksctl", &["unmount", "--no-user-interaction", "-b", &v.os_path])?;
            } else {
                let mp = v.mount_point.as_ref().unwrap().to_string_lossy().into_owned();
                run("umount", &[&mp])?;
            }
        }
        Ok(())
    }

    fn eject(&self, device_id: &str) -> Result<(), PlatformError> {
        // SAFETY: sync(2) takes no arguments and cannot fail.
        unsafe { libc::sync() };
        self.unmount(device_id)?;
        let node = self.dev.join(device_id).to_string_lossy().into_owned();
        if have("udisksctl") {
            run("udisksctl", &["power-off", "--no-user-interaction", "-b", &node])?;
            return Ok(());
        }
        let delete = self.sys.join("block").join(device_id).join("device/delete");
        fs::write(&delete, b"1").map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                PlatformError::PermissionDenied("removing the device needs root or udisks2".into())
            } else {
                PlatformError::Io(e)
            }
        })
    }

    fn name(&self) -> &'static str {
        "linux"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    /// Minimal fake of what sysfs, udev and mountinfo look like with one
    /// internal virtio disk and one SanDisk stick plugged in.
    fn fake_system(root: &Path) -> LinuxPlatform {
        let sys = root.join("sys");
        let usbdev = sys.join("devices/pci0000:00/0000:00:14.0/usb2/2-1");
        let scsi = usbdev.join("2-1:1.0/host6/target6:0:0/6:0:0:0");
        for (f, v) in [
            ("idVendor", "0781"),
            ("idProduct", "5581"),
            ("manufacturer", " USB"),
            ("product", " SanDisk 3.2Gen1"),
            ("serial", "4C530001230518104F82"),
            ("bcdDevice", "0100"),
            ("version", " 3.20"),
            ("speed", "5000"),
            ("bMaxPower", "224mA"),
        ] {
            write(&usbdev.join(f), &format!("{v}\n"));
        }
        write(&scsi.join("vendor"), "SanDisk \n");
        write(&scsi.join("model"), "Ultra           \n");
        write(&scsi.join("rev"), "1.00\n");
        let sdb = sys.join("block/sdb");
        fs::create_dir_all(&sdb).unwrap();
        symlink(&scsi, sdb.join("device")).unwrap();
        write(&sdb.join("size"), "60063744\n");
        write(&sdb.join("removable"), "1\n");
        write(&sdb.join("dev"), "8:16\n");
        write(&sdb.join("queue/logical_block_size"), "512\n");
        write(&sdb.join("sdb1/partition"), "1\n");
        write(&sdb.join("sdb1/start"), "2048\n");
        write(&sdb.join("sdb1/size"), "60061696\n");
        write(&sdb.join("sdb1/dev"), "8:17\n");

        let vda_dev = sys.join("devices/pci0000:00/0000:00:04.0/virtio2");
        fs::create_dir_all(&vda_dev).unwrap();
        let vda = sys.join("block/vda");
        fs::create_dir_all(&vda).unwrap();
        symlink(&vda_dev, vda.join("device")).unwrap();
        write(&vda.join("size"), "209715200\n");
        write(&vda.join("removable"), "0\n");
        write(&vda.join("dev"), "252:0\n");
        write(&vda.join("vda1/partition"), "1\n");
        write(&vda.join("vda1/start"), "2048\n");
        write(&vda.join("vda1/size"), "209713152\n");
        write(&vda.join("vda1/dev"), "252:1\n");
        fs::create_dir_all(sys.join("block/loop0")).unwrap();

        write(
            &root.join("mountinfo"),
            "22 1 252:1 / / rw,relatime - ext4 /dev/vda1 rw\n\
             90 22 8:17 / /media/dj/JOMMI\\040DJ rw,nosuid - vfat /dev/sdb1 rw,uid=1000\n",
        );
        write(&root.join("udev/b8:16"), "E:ID_PART_TABLE_TYPE=dos\n");
        write(
            &root.join("udev/b8:17"),
            "E:ID_FS_TYPE=vfat\nE:ID_FS_VERSION=FAT32\nE:ID_FS_LABEL=JOMMI_DJ\nE:ID_FS_UUID=1234-ABCD\n",
        );
        LinuxPlatform::with_roots(sys, root.join("mountinfo"), root.join("udev"), "/dev")
    }

    #[test]
    fn enumerates_usb_stick_and_flags_system_disk() {
        let d = tempfile::tempdir().unwrap();
        let p = fake_system(d.path());
        let devs = p.list_devices().unwrap();
        assert_eq!(devs.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["sdb", "vda"]);

        let stick = &devs[0];
        assert_eq!(stick.bus, BusType::Usb);
        assert!(stick.removable && !stick.is_system);
        assert_eq!(stick.size_bytes, 60063744 * 512);
        assert_eq!(stick.partition_scheme, Some(PartitionScheme::Mbr));
        let usb = stick.usb.as_ref().unwrap();
        assert_eq!((usb.vendor_id, usb.product_id), (Some(0x0781), Some(0x5581)));
        assert_eq!(usb.max_power_ma, Some(224));
        assert_eq!(usb.speed_mbps, Some(5000));
        assert_eq!(stick.storage_model.as_deref(), Some("Ultra"));
        let v = &stick.volumes[0];
        assert_eq!(v.os_path, "/dev/sdb1");
        assert_eq!(v.mount_point.as_deref(), Some(Path::new("/media/dj/JOMMI DJ")));
        assert_eq!(v.filesystem, Some(FilesystemKind::Fat32));
        assert_eq!(v.label.as_deref(), Some("JOMMI_DJ"));
        assert_eq!(v.offset_bytes, Some(1 << 20));

        let vda = &devs[1];
        assert!(vda.is_system);
        assert_eq!(vda.bus, BusType::Virtual);
    }

    #[test]
    fn real_system_enumeration_does_not_crash() {
        // Whatever machine runs the tests: no panics, and anything mounted
        // at / is flagged as a system disk.
        let devs = LinuxPlatform::system().list_devices().unwrap_or_default();
        for d in &devs {
            if d.volumes.iter().any(|v| v.mount_point.as_deref() == Some(Path::new("/"))) {
                assert!(d.is_system, "{d:?}");
            }
        }
    }

    #[test]
    fn mount_unescape() {
        assert_eq!(unescape_mount("/media/a\\040b\\134c"), "/media/a b\\c");
        assert_eq!(unescape_mount("/x\\04"), "/x\\04");
    }
}
