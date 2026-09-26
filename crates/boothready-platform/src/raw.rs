//! Exclusive raw access to a whole physical device, for the privileged
//! helper only.
//!
//! Each OS needs different preparation before raw writes are safe:
//! - Linux opens the block device with `O_EXCL`, which fails if any
//!   partition is still mounted.
//! - macOS writes to `/dev/rdiskN` after `diskutil unmountDisk`.
//! - Windows locks and dismounts every volume on the disk and keeps those
//!   handles open for the duration, then asks the disk driver to re-read the
//!   new layout.

use crate::PlatformError;
use boothready_model::PhysicalDevice;
use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};

pub struct RawDisk {
    file: File,
    pub size: u64,
    pub sector: u32,
    #[cfg(windows)]
    _locks: Vec<File>,
}

impl RawDisk {
    /// Open a regular image file as if it were a device (demo platform, tests).
    pub fn open_image(path: &std::path::Path, size: u64) -> Result<RawDisk, PlatformError> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        if file.metadata()?.len() < size {
            file.set_len(size)?;
        }
        Ok(RawDisk {
            file,
            size,
            sector: 512,
            #[cfg(windows)]
            _locks: vec![],
        })
    }

    /// Open the physical device for exclusive raw writes. The caller must
    /// already have validated the device against the user's confirmation.
    pub fn open_device(dev: &PhysicalDevice) -> Result<RawDisk, PlatformError> {
        let map = |e: io::Error| match e.kind() {
            io::ErrorKind::PermissionDenied => PlatformError::PermissionDenied(format!("{}: {e}", dev.os_path)),
            _ if e.raw_os_error() == Some(16) => PlatformError::Busy { holder: None },
            _ => PlatformError::Io(e),
        };
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            let file =
                OpenOptions::new().read(true).write(true).custom_flags(libc::O_EXCL).open(&dev.os_path).map_err(map)?;
            Ok(RawDisk { file, size: dev.size_bytes, sector: dev.logical_sector_size.max(512) })
        }
        #[cfg(target_os = "macos")]
        {
            let file = OpenOptions::new().read(true).write(true).open(&dev.os_path).map_err(map)?;
            Ok(RawDisk { file, size: dev.size_bytes, sector: dev.logical_sector_size.max(512) })
        }
        #[cfg(windows)]
        {
            use std::os::windows::fs::OpenOptionsExt;
            use std::os::windows::io::AsRawHandle;
            const SHARE_RW: u32 = 0x1 | 0x2;
            let mut locks = Vec::new();
            for v in &dev.volumes {
                let vol = OpenOptions::new()
                    .read(true)
                    .write(true)
                    .share_mode(SHARE_RW)
                    .open(v.os_path.trim_end_matches('\\'))
                    .map_err(map)?;
                let h = windows::Win32::Foundation::HANDLE(vol.as_raw_handle());
                if !win::ioctl_simple(h, windows::Win32::System::Ioctl::FSCTL_LOCK_VOLUME) {
                    return Err(PlatformError::Busy { holder: None });
                }
                win::ioctl_simple(h, windows::Win32::System::Ioctl::FSCTL_DISMOUNT_VOLUME);
                locks.push(vol);
            }
            let file =
                OpenOptions::new().read(true).write(true).share_mode(SHARE_RW).open(&dev.os_path).map_err(map)?;
            Ok(RawDisk { file, size: dev.size_bytes, sector: dev.logical_sector_size.max(512), _locks: locks })
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
        {
            let _ = map;
            Err(PlatformError::Failed("raw disk access is not supported on this OS".into()))
        }
    }

    /// Flush everything and make the OS pick up the new partition table.
    pub fn finish(mut self) -> Result<(), PlatformError> {
        self.file.flush()?;
        self.file.sync_all()?;
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::AsRawFd;
            const BLKRRPART: libc::c_ulong = 0x125F;
            // SAFETY: BLKRRPART takes no argument; the fd is a block device
            // we opened. Failure only means the kernel re-reads later.
            unsafe {
                libc::ioctl(self.file.as_raw_fd(), BLKRRPART as _);
            }
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            let h = windows::Win32::Foundation::HANDLE(self.file.as_raw_handle());
            win::ioctl_simple(h, windows::Win32::System::Ioctl::IOCTL_DISK_UPDATE_PROPERTIES);
        }
        Ok(())
    }
}

impl Read for RawDisk {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.file.read(buf)
    }
}

impl Write for RawDisk {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.file.write(buf)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

impl Seek for RawDisk {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.file.seek(pos)
    }
}

#[cfg(windows)]
mod win {
    use windows::Win32::Foundation::HANDLE;
    use windows::Win32::System::IO::DeviceIoControl;

    pub fn ioctl_simple(h: HANDLE, code: u32) -> bool {
        let mut returned = 0u32;
        // SAFETY: control codes without buffers on a handle we own.
        unsafe { DeviceIoControl(h, code, None, 0, None, 0, Some(&mut returned), None) }.is_ok()
    }
}

/// Name of the first partition's device node, for OS formatters.
pub fn first_partition_node(disk_node: &str) -> String {
    let last = disk_node.chars().last().unwrap_or('a');
    if last.is_ascii_digit() {
        // nvme0n1 -> nvme0n1p1, mmcblk0 -> mmcblk0p1
        format!("{disk_node}p1")
    } else {
        format!("{disk_node}1")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partition_nodes() {
        assert_eq!(first_partition_node("/dev/sdb"), "/dev/sdb1");
        assert_eq!(first_partition_node("/dev/mmcblk0"), "/dev/mmcblk0p1");
    }

    #[test]
    fn image_backed_raw_disk() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("x.img");
        let mut r = RawDisk::open_image(&p, 1 << 20).unwrap();
        r.seek(SeekFrom::Start(510)).unwrap();
        r.write_all(&[0x55, 0xAA]).unwrap();
        r.finish().unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(bytes.len(), 1 << 20);
        assert_eq!(&bytes[510..512], &[0x55, 0xAA]);
    }
}
