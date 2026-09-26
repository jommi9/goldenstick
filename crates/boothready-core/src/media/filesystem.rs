//! Filesystem detection from raw volume bytes.

use crate::io::{be_u32, le_u16, le_u32, le_u64, Disk};
use crate::media::partition::is_hfs_signature;
use boothready_model::FilesystemKind;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilesystemInfo {
    pub kind: FilesystemKind,
    pub label: Option<String>,
    /// Volume serial / ID, formatted the way the OS usually shows it.
    pub serial: Option<String>,
    pub cluster_size: Option<u32>,
    pub bytes_per_sector: Option<u32>,
    /// Size the filesystem believes it has.
    pub total_bytes: Option<u64>,
    /// `Some(true)` when the volume was not cleanly unmounted (pulled
    /// without ejecting, or a crash while writing).
    pub dirty: Option<bool>,
    pub warnings: Vec<String>,
}

impl FilesystemInfo {
    fn new(kind: FilesystemKind) -> Self {
        FilesystemInfo {
            kind,
            label: None,
            serial: None,
            cluster_size: None,
            bytes_per_sector: None,
            total_bytes: None,
            dirty: None,
            warnings: vec![],
        }
    }
}

/// Detect the filesystem of the volume starting at `offset` and spanning
/// `len` bytes.
pub fn detect_filesystem<R: Read + Seek>(
    disk: &mut Disk<R>,
    offset: u64,
    len: u64,
) -> io::Result<Option<FilesystemInfo>> {
    if len < 2048 || offset.checked_add(2048).is_none_or(|end| end > disk.size()) {
        return Ok(None);
    }
    let head = disk.read_at(offset, 2048)?;
    let boot = &head[..512];

    if &boot[3..11] == b"EXFAT   " {
        return read_exfat(disk, offset, len, boot).map(Some);
    }
    if &boot[3..11] == b"NTFS    " {
        let mut fs = FilesystemInfo::new(FilesystemKind::Ntfs);
        let bps = le_u16(boot, 11) as u32;
        let spc = boot[13] as u32;
        fs.bytes_per_sector = Some(bps);
        fs.cluster_size = Some(bps * spc);
        fs.total_bytes = Some(le_u64(boot, 40) * bps as u64);
        fs.serial = Some(format!("{:016X}", le_u64(boot, 72)));
        return Ok(Some(fs));
    }
    if let Some(fs) = read_fat(disk, offset, len, boot)? {
        return Ok(Some(fs));
    }
    if is_hfs_signature(&head[1024..]) {
        let mut fs = FilesystemInfo::new(FilesystemKind::HfsPlus);
        let block_size = be_u32(&head, 1024 + 40);
        let total_blocks = be_u32(&head, 1024 + 44) as u64;
        fs.cluster_size = Some(block_size);
        fs.total_bytes = Some(total_blocks * block_size as u64);
        // kHFSVolumeUnmountedBit (bit 8) set means cleanly unmounted.
        let attributes = be_u32(&head, 1024 + 4);
        fs.dirty = Some(attributes & (1 << 8) == 0);
        return Ok(Some(fs));
    }
    // HFS wrapper ("BD") around an embedded HFS+ volume.
    if &head[1024..1026] == b"BD" {
        let mut fs = FilesystemInfo::new(FilesystemKind::HfsPlus);
        fs.warnings.push("HFS+ volume inside a legacy HFS wrapper".into());
        return Ok(Some(fs));
    }
    if &head[32..36] == b"NXSB" {
        let mut fs = FilesystemInfo::new(FilesystemKind::Apfs);
        let block_size = le_u32(&head, 36);
        let block_count = le_u64(&head, 40);
        fs.cluster_size = Some(block_size);
        fs.total_bytes = Some(block_count.saturating_mul(block_size as u64));
        return Ok(Some(fs));
    }
    if le_u16(&head, 1024 + 56) == 0xEF53 {
        return Ok(Some(FilesystemInfo::new(FilesystemKind::Ext)));
    }
    Ok(None)
}

fn trim_label(raw: &[u8]) -> Option<String> {
    let s: String = raw.iter().map(|&b| if b.is_ascii_graphic() || b == b' ' { b as char } else { '?' }).collect();
    let s = s.trim_end().to_string();
    if s.is_empty() || s == "NO NAME" {
        None
    } else {
        Some(s)
    }
}

fn read_fat<R: Read + Seek>(disk: &mut Disk<R>, offset: u64, len: u64, b: &[u8]) -> io::Result<Option<FilesystemInfo>> {
    let bps = le_u16(b, 11) as u64;
    let spc = b[13] as u64;
    let reserved = le_u16(b, 14) as u64;
    let num_fats = b[16] as u64;
    let root_entries = le_u16(b, 17) as u64;
    let total16 = le_u16(b, 19) as u64;
    let fat16_size = le_u16(b, 22) as u64;
    let total32 = le_u32(b, 32) as u64;
    let jump_ok = b[0] == 0xEB || b[0] == 0xE9;
    let sig_ok = b[510] == 0x55 && b[511] == 0xAA;
    if !jump_ok
        || !matches!(bps, 512 | 1024 | 2048 | 4096)
        || spc == 0
        || !spc.is_power_of_two()
        || reserved == 0
        || !(1..=2).contains(&num_fats)
    {
        return Ok(None);
    }
    let fat_size = if fat16_size != 0 { fat16_size } else { le_u32(b, 36) as u64 };
    let total = if total16 != 0 { total16 } else { total32 };
    if fat_size == 0 || total == 0 {
        return Ok(None);
    }
    let root_dir_sectors = (root_entries * 32).div_ceil(bps);
    let meta = reserved + num_fats * fat_size + root_dir_sectors;
    if meta >= total {
        return Ok(None);
    }
    let clusters = (total - meta) / spc;
    // Microsoft's rule: the type is decided by cluster count alone.
    let kind = if clusters < 4085 {
        FilesystemKind::Fat12
    } else if clusters < 65525 {
        FilesystemKind::Fat16
    } else {
        FilesystemKind::Fat32
    };
    let mut fs = FilesystemInfo::new(kind);
    fs.bytes_per_sector = Some(bps as u32);
    fs.cluster_size = Some((bps * spc) as u32);
    fs.total_bytes = Some(total * bps);
    if !sig_ok {
        fs.warnings.push("Boot sector is missing its 0x55AA signature".into());
    }
    if total * bps > len {
        fs.warnings.push("Filesystem claims to be larger than its partition".into());
    } else if len - total * bps > 16 * 1024 * 1024 {
        fs.warnings.push(format!("Filesystem uses {} MB less than its partition", (len - total * bps) / (1024 * 1024)));
    }
    if bps != 512 {
        fs.warnings.push(format!("Uses {bps}-byte sectors; older players expect 512"));
    }
    let (label_off, serial_off) = if kind == FilesystemKind::Fat32 { (71, 67) } else { (43, 39) };
    let bpb_label = trim_label(&b[label_off..label_off + 11]);
    let serial = le_u32(b, serial_off);
    fs.serial = Some(format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF));

    // Root-directory volume label wins over the BPB copy; that's what Windows
    // and macOS display.
    let fat_start = offset + reserved * bps;
    let root_start = if kind == FilesystemKind::Fat32 {
        let root_cluster = le_u32(b, 44) as u64;
        let data_start = offset + (reserved + num_fats * fat_size) * bps;
        data_start + root_cluster.saturating_sub(2) * spc * bps
    } else {
        fat_start + num_fats * fat_size * bps
    };
    let root_len = if kind == FilesystemKind::Fat32 { spc * bps } else { root_dir_sectors * bps }.min(64 * 1024);
    let mut dir_label = None;
    if root_start + root_len <= disk.size() && root_len > 0 {
        let dir = disk.read_at(root_start, root_len as usize)?;
        for entry in dir.chunks_exact(32) {
            if entry[0] == 0 {
                break;
            }
            if entry[0] == 0xE5 {
                continue;
            }
            let attr = entry[11];
            if attr & 0x0F == 0x0F {
                continue; // long-name entry
            }
            if attr & 0x08 != 0 {
                dir_label = trim_label(&entry[0..11]);
                break;
            }
        }
    }
    fs.label = dir_label.or(bpb_label);

    // Clean-shutdown bit in FAT[1].
    if fat_start + 8 <= disk.size() {
        let fat = disk.read_at(fat_start, 8)?;
        fs.dirty = match kind {
            FilesystemKind::Fat32 => Some(le_u32(&fat, 4) & 0x0800_0000 == 0),
            FilesystemKind::Fat16 => Some(le_u16(&fat, 2) & 0x8000 == 0),
            _ => None,
        };
    }
    Ok(Some(fs))
}

fn read_exfat<R: Read + Seek>(disk: &mut Disk<R>, offset: u64, len: u64, b: &[u8]) -> io::Result<FilesystemInfo> {
    let mut fs = FilesystemInfo::new(FilesystemKind::Exfat);
    let sector_shift = b[108] as u32;
    let cluster_shift = b[109] as u32;
    if !(9..=12).contains(&sector_shift) || sector_shift + cluster_shift > 25 {
        fs.warnings.push("exFAT boot sector has invalid geometry".into());
        return Ok(fs);
    }
    let bps = 1u64 << sector_shift;
    let cluster = bps << cluster_shift;
    let volume_len = le_u64(b, 72);
    let heap_offset = le_u32(b, 88) as u64;
    let root_cluster = le_u32(b, 96) as u64;
    let serial = le_u32(b, 100);
    let flags = le_u16(b, 106);
    fs.bytes_per_sector = Some(bps as u32);
    fs.cluster_size = Some(cluster as u32);
    fs.total_bytes = Some(volume_len * bps);
    fs.serial = Some(format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF));
    fs.dirty = Some(flags & 0x0002 != 0);
    if volume_len * bps > len {
        fs.warnings.push("Filesystem claims to be larger than its partition".into());
    }
    let root = offset + heap_offset * bps + root_cluster.saturating_sub(2) * cluster;
    let root_len = cluster.min(64 * 1024);
    if root_cluster >= 2 && root + root_len <= disk.size() {
        let dir = disk.read_at(root, root_len as usize)?;
        for entry in dir.chunks_exact(32) {
            match entry[0] {
                0x00 => break,
                0x83 => {
                    let n = (entry[1] as usize).min(11);
                    let chars: Vec<u16> = (0..n).map(|i| le_u16(entry, 2 + i * 2)).collect();
                    let label = String::from_utf16_lossy(&chars);
                    if !label.is_empty() {
                        fs.label = Some(label);
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    Ok(fs)
}
