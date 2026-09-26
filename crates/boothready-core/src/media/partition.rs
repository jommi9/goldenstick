//! Partition map parsing (MBR, GPT, APM, superfloppy).

use crate::io::{be_u16, crc32, le_u16, le_u32, le_u64, Disk};
use boothready_model::PartitionScheme;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Seek};

/// One entry from the partition map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionEntry {
    /// 1-based index as the OS would number it.
    pub index: u32,
    pub start_bytes: u64,
    pub size_bytes: u64,
    pub kind: PartitionKind,
    /// MBR type byte, when the entry came from an MBR.
    pub mbr_type: Option<u8>,
    /// GPT type GUID in canonical text form, when the entry came from a GPT.
    pub gpt_type: Option<String>,
    pub gpt_name: Option<String>,
    pub bootable: bool,
}

/// What a partition entry declares itself to be. The filesystem actually
/// present is detected separately and may disagree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PartitionKind {
    Fat12,
    Fat16,
    Fat32,
    /// MBR type 0x07: NTFS, exFAT or HPFS.
    NtfsOrExfat,
    /// GPT Microsoft basic data: FAT, exFAT or NTFS.
    BasicData,
    EfiSystem,
    HfsPlus,
    Apfs,
    Linux,
    Extended,
    GptProtective,
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartitionMap {
    pub scheme: PartitionScheme,
    /// Sector size the map was interpreted with.
    pub sector_size: u32,
    pub partitions: Vec<PartitionEntry>,
    /// Things that are wrong or unusual about the map itself.
    pub warnings: Vec<String>,
    /// MBR disk signature or GPT disk GUID.
    pub disk_id: Option<String>,
}

const GUID_BASIC_DATA: &str = "EBD0A0A2-B9E5-4433-87C0-68B6B72699C7";
const GUID_EFI_SYSTEM: &str = "C12A7328-F81F-11D2-BA4B-00A0C93EC93B";
const GUID_HFS_PLUS: &str = "48465300-0000-11AA-AA11-00306543ECAC";
const GUID_APFS: &str = "7C3457EF-0000-11AA-AA11-00306543ECAC";
const GUID_LINUX_DATA: &str = "0FC63DAF-8483-4772-8E79-3D69D8477DE4";

/// Read the partition map of a disk.
pub fn read_partition_map<R: Read + Seek>(disk: &mut Disk<R>) -> io::Result<PartitionMap> {
    let size = disk.size();
    if size < 512 {
        return Ok(PartitionMap {
            scheme: PartitionScheme::Unknown,
            sector_size: 512,
            partitions: vec![],
            warnings: vec!["Device is too small to hold a partition table".into()],
            disk_id: None,
        });
    }
    let sector0 = disk.read_at(0, 512)?;

    // GPT first: a protective MBR plus "EFI PART" at LBA 1. Try 512 and 4096
    // byte logical sectors.
    for &ss in &[512u64, 4096] {
        if size < ss * 2 {
            continue;
        }
        let hdr = disk.read_at(ss, 512)?;
        if &hdr[0..8] == b"EFI PART" {
            return read_gpt(disk, ss, &sector0, &hdr);
        }
    }

    // Apple Partition Map: driver descriptor "ER" at 0, partition map "PM" at 512.
    if &sector0[0..2] == b"ER" && size >= 1024 {
        let pm = disk.read_at(512, 512)?;
        if &pm[0..2] == b"PM" {
            return Ok(PartitionMap {
                scheme: PartitionScheme::Apm,
                sector_size: 512,
                partitions: vec![],
                warnings: vec!["Apple Partition Map is not supported by DJ hardware".into()],
                disk_id: None,
            });
        }
    }

    let has_boot_sig = sector0[510] == 0x55 && sector0[511] == 0xAA;
    if looks_like_boot_sector(&sector0) {
        return Ok(PartitionMap {
            scheme: PartitionScheme::Superfloppy,
            sector_size: 512,
            partitions: vec![PartitionEntry {
                index: 1,
                start_bytes: 0,
                size_bytes: size,
                kind: PartitionKind::Other,
                mbr_type: None,
                gpt_type: None,
                gpt_name: None,
                bootable: false,
            }],
            warnings: vec!["Filesystem starts at sector 0 without a partition table".into()],
            disk_id: None,
        });
    }
    if has_boot_sig {
        if let Some(map) = read_mbr(disk, &sector0)? {
            return Ok(map);
        }
    }

    // A filesystem can sit at sector 0 without a 0x55AA signature (HFS+,
    // APFS). The caller probes offset 0 for those.
    Ok(PartitionMap {
        scheme: PartitionScheme::Unknown,
        sector_size: 512,
        partitions: vec![],
        warnings: vec!["No recognisable partition table".into()],
        disk_id: None,
    })
}

/// FAT, exFAT and NTFS boot sectors also end in 0x55AA. Tell them apart from
/// an MBR by the jump instruction and the filesystem identifiers.
fn looks_like_boot_sector(s: &[u8]) -> bool {
    let jump = s[0] == 0xEB && s[2] == 0x90 || s[0] == 0xE9;
    if !jump {
        return false;
    }
    if &s[3..11] == b"EXFAT   " || &s[3..11] == b"NTFS    " {
        return true;
    }
    let bps = le_u16(s, 11);
    let spc = s[13];
    let reserved = le_u16(s, 14);
    let fats = s[16];
    matches!(bps, 512 | 1024 | 2048 | 4096)
        && spc.is_power_of_two()
        && reserved > 0
        && (fats == 1 || fats == 2)
        && (&s[54..59] == b"FAT12" || &s[54..59] == b"FAT16" || &s[82..87] == b"FAT32" || &s[54..57] == b"FAT")
}

fn mbr_kind(t: u8) -> PartitionKind {
    match t {
        0x01 => PartitionKind::Fat12,
        0x04 | 0x06 | 0x0E => PartitionKind::Fat16,
        0x0B | 0x0C => PartitionKind::Fat32,
        0x07 => PartitionKind::NtfsOrExfat,
        0xAF => PartitionKind::HfsPlus,
        0x83 => PartitionKind::Linux,
        0x05 | 0x0F | 0x85 => PartitionKind::Extended,
        0xEE => PartitionKind::GptProtective,
        0xEF => PartitionKind::EfiSystem,
        _ => PartitionKind::Other,
    }
}

fn read_mbr<R: Read + Seek>(disk: &mut Disk<R>, s0: &[u8]) -> io::Result<Option<PartitionMap>> {
    let size = disk.size();
    let total_sectors = size / 512;
    let mut partitions = Vec::new();
    let mut warnings = Vec::new();
    let mut extended: Option<(u64, u64)> = None;
    for i in 0..4 {
        let e = &s0[446 + i * 16..446 + (i + 1) * 16];
        let status = e[0];
        let ptype = e[4];
        let lba = le_u32(e, 8) as u64;
        let count = le_u32(e, 12) as u64;
        if ptype == 0 || count == 0 {
            continue;
        }
        if status != 0x00 && status != 0x80 {
            // Not an MBR we can trust. Probably random data in sector 0.
            return Ok(None);
        }
        if lba + count > total_sectors {
            warnings.push(format!(
                "Partition {} extends past the end of the device ({} > {} sectors)",
                i + 1,
                lba + count,
                total_sectors
            ));
        }
        let kind = mbr_kind(ptype);
        if kind == PartitionKind::Extended {
            extended = Some((lba, count));
            continue;
        }
        partitions.push(PartitionEntry {
            index: i as u32 + 1,
            start_bytes: lba * 512,
            size_bytes: count * 512,
            kind,
            mbr_type: Some(ptype),
            gpt_type: None,
            gpt_name: None,
            bootable: status == 0x80,
        });
    }
    if let Some((ext_lba, _)) = extended {
        read_ebr_chain(disk, ext_lba, &mut partitions, &mut warnings)?;
        warnings.push("Uses logical partitions inside an extended partition".into());
    }
    if partitions.iter().any(|p| p.kind == PartitionKind::GptProtective) {
        warnings.push("Protective MBR found but the GPT header is missing or damaged".into());
    }
    if partitions.len() > 1 {
        warnings.push(format!("{} partitions found. DJ players generally read only the first one", partitions.len()));
    }
    let sig = le_u32(s0, 440);
    Ok(Some(PartitionMap {
        scheme: PartitionScheme::Mbr,
        sector_size: 512,
        partitions,
        warnings,
        disk_id: (sig != 0).then(|| format!("{sig:08X}")),
    }))
}

fn read_ebr_chain<R: Read + Seek>(
    disk: &mut Disk<R>,
    ext_lba: u64,
    out: &mut Vec<PartitionEntry>,
    warnings: &mut Vec<String>,
) -> io::Result<()> {
    let mut next = ext_lba;
    let mut index = 5;
    for _ in 0..128 {
        if (next + 1) * 512 > disk.size() {
            warnings.push("Extended partition chain points past the end of the device".into());
            break;
        }
        let ebr = disk.read_at(next * 512, 512)?;
        if ebr[510] != 0x55 || ebr[511] != 0xAA {
            warnings.push("Broken extended partition chain".into());
            break;
        }
        let e1 = &ebr[446..462];
        let e2 = &ebr[462..478];
        if e1[4] != 0 && le_u32(e1, 12) != 0 {
            out.push(PartitionEntry {
                index,
                start_bytes: (next + le_u32(e1, 8) as u64) * 512,
                size_bytes: le_u32(e1, 12) as u64 * 512,
                kind: mbr_kind(e1[4]),
                mbr_type: Some(e1[4]),
                gpt_type: None,
                gpt_name: None,
                bootable: e1[0] == 0x80,
            });
            index += 1;
        }
        let link = le_u32(e2, 8) as u64;
        if e2[4] == 0 || link == 0 {
            break;
        }
        next = ext_lba + link;
    }
    Ok(())
}

/// Format a GPT on-disk GUID (mixed endian) as canonical text.
pub fn guid_to_string(b: &[u8]) -> String {
    format!(
        "{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        le_u32(b, 0),
        le_u16(b, 4),
        le_u16(b, 6),
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

fn read_gpt<R: Read + Seek>(disk: &mut Disk<R>, ss: u64, s0: &[u8], hdr: &[u8]) -> io::Result<PartitionMap> {
    let mut warnings = Vec::new();
    let header_size = le_u32(hdr, 12) as usize;
    if (92..=512).contains(&header_size) {
        let mut h = hdr[..header_size].to_vec();
        let stored = le_u32(&h, 16);
        h[16..20].fill(0);
        if crc32(&h) != stored {
            warnings.push("GPT header checksum does not match".into());
        }
    } else {
        warnings.push(format!("GPT header has an invalid size ({header_size})"));
    }
    let has_protective = s0[510] == 0x55 && s0[511] == 0xAA && (0..4).any(|i| s0[446 + i * 16 + 4] == 0xEE);
    if !has_protective {
        warnings.push("GPT without a protective MBR".into());
    }
    let entries_lba = le_u64(hdr, 72);
    let num_entries = le_u32(hdr, 80).min(1024) as u64;
    let entry_size = le_u32(hdr, 84) as u64;
    let entries_crc = le_u32(hdr, 88);
    let backup_lba = le_u64(hdr, 32);
    let disk_guid = guid_to_string(&hdr[56..72]);

    let mut partitions = Vec::new();
    if !(128..=4096).contains(&entry_size) || entry_size % 8 != 0 {
        warnings.push(format!("GPT entry size {entry_size} is invalid"));
    } else {
        let table_len = num_entries * entry_size;
        let table_end = entries_lba.checked_mul(ss).and_then(|o| o.checked_add(table_len));
        if table_end.is_some_and(|end| end <= disk.size()) {
            let table = disk.read_at(entries_lba * ss, table_len as usize)?;
            if crc32(&table) != entries_crc {
                warnings.push("GPT partition array checksum does not match".into());
            }
            for i in 0..num_entries as usize {
                let e = &table[i * entry_size as usize..(i + 1) * entry_size as usize];
                if e[0..16].iter().all(|&b| b == 0) {
                    continue;
                }
                let type_guid = guid_to_string(&e[0..16]);
                let first = le_u64(e, 32);
                let last = le_u64(e, 40);
                if last < first {
                    warnings.push(format!("GPT entry {} has end before start", i + 1));
                    continue;
                }
                let name: Vec<u16> = (0..36).map(|k| le_u16(e, 56 + k * 2)).take_while(|&c| c != 0).collect();
                let kind = match type_guid.as_str() {
                    GUID_BASIC_DATA => PartitionKind::BasicData,
                    GUID_EFI_SYSTEM => PartitionKind::EfiSystem,
                    GUID_HFS_PLUS => PartitionKind::HfsPlus,
                    GUID_APFS => PartitionKind::Apfs,
                    GUID_LINUX_DATA => PartitionKind::Linux,
                    _ => PartitionKind::Other,
                };
                let (Some(start_bytes), Some(size_bytes)) =
                    (first.checked_mul(ss), (last - first).checked_add(1).and_then(|n| n.checked_mul(ss)))
                else {
                    warnings.push(format!("GPT entry {} has an impossible size", i + 1));
                    continue;
                };
                partitions.push(PartitionEntry {
                    index: i as u32 + 1,
                    start_bytes,
                    size_bytes,
                    kind,
                    mbr_type: None,
                    gpt_type: Some(type_guid),
                    gpt_name: Some(String::from_utf16_lossy(&name)).filter(|n| !n.is_empty()),
                    bootable: false,
                });
            }
        } else {
            warnings.push("GPT partition array lies past the end of the device".into());
        }
    }
    let last_lba = disk.size() / ss - 1;
    if backup_lba != last_lba {
        warnings.push(
            "GPT backup header is not at the end of the device (image may have been copied to a different-size drive)"
                .into(),
        );
    } else if let Ok(b) = disk.read_at(backup_lba * ss, 8) {
        if &b[..] != b"EFI PART" {
            warnings.push("GPT backup header is missing".into());
        }
    }
    Ok(PartitionMap {
        scheme: PartitionScheme::Gpt,
        sector_size: ss as u32,
        partitions,
        warnings,
        disk_id: Some(disk_guid),
    })
}

/// True when the disk has an HFS+/HFSX volume header at `offset + 1024`.
pub(crate) fn is_hfs_signature(b: &[u8]) -> bool {
    matches!(be_u16(b, 0), 0x482B | 0x4858)
}
