//! Disk-image tests. Images are built with the same tools a Linux user would
//! use (sfdisk, sgdisk, mkfs.fat, mkfs.exfat) so the parser is checked against
//! independent implementations. Tests skip when a tool is missing.

use super::*;
use crate::testutil::{have_tool, run_tool, splice_partition, sparse_file};
use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};

const MIB: u64 = 1024 * 1024;

#[test]
fn mbr_fat32_image() {
    if !have_tool("sfdisk") || !have_tool("mkfs.fat") {
        eprintln!("skipping: sfdisk/mkfs.fat not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("disk.img");
    sparse_file(&img, 300 * MIB);
    run_tool("sfdisk", &[img.to_str().unwrap()], Some("label: dos\nstart=2048, type=c\n"));
    let part = dir.path().join("part.img");
    sparse_file(&part, 300 * MIB - MIB);
    run_tool("mkfs.fat", &["-F", "32", "-n", "JOMMI_DJ", "-i", "1234ABCD", part.to_str().unwrap()], None);
    splice_partition(&img, &part, MIB);

    let layout = inspect_path(&img).unwrap();
    assert_eq!(layout.scheme, PartitionScheme::Mbr);
    assert_eq!(layout.partitions.len(), 1);
    let p = &layout.partitions[0];
    assert_eq!(p.entry.start_bytes, MIB);
    assert_eq!(p.entry.mbr_type, Some(0x0C));
    assert!(p.aligned_1mib);
    let fs = p.filesystem.as_ref().unwrap();
    assert_eq!(fs.kind, FilesystemKind::Fat32);
    assert_eq!(fs.label.as_deref(), Some("JOMMI_DJ"));
    assert_eq!(fs.serial.as_deref(), Some("1234-ABCD"));
    assert_eq!(fs.dirty, Some(false));
    assert!(layout.warnings.is_empty(), "{:?}", layout.warnings);
    assert_eq!(layout.primary_filesystem(), Some(FilesystemKind::Fat32));
}

#[test]
fn gpt_macos_style_layout_with_exfat() {
    if !have_tool("sgdisk") || !have_tool("mkfs.exfat") || !have_tool("mkfs.fat") {
        eprintln!("skipping: sgdisk/mkfs.exfat not installed");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("disk.img");
    sparse_file(&img, 512 * MIB);
    // macOS Disk Utility layout: 200 MB EFI system partition, then data.
    run_tool(
        "sgdisk",
        &["-a", "8", "-n", "1:40:409639", "-t", "1:EF00", "-n", "2:411648:0", "-t", "2:0700", "-c", "2:Untitled", img.to_str().unwrap()],
        None,
    );
    let efi = dir.path().join("efi.img");
    sparse_file(&efi, 409600 * 512);
    run_tool("mkfs.fat", &["-F", "32", "-n", "EFI", efi.to_str().unwrap()], None);
    splice_partition(&img, &efi, 40 * 512);
    let data = dir.path().join("data.img");
    let data_len = 512 * MIB - 411648 * 512 - 34 * 512;
    sparse_file(&data, data_len);
    run_tool("mkfs.exfat", &["-L", "FESTIVAL26", data.to_str().unwrap()], None);
    splice_partition(&img, &data, 411648 * 512);

    let layout = inspect_path(&img).unwrap();
    assert_eq!(layout.scheme, PartitionScheme::Gpt);
    assert_eq!(layout.partitions.len(), 2);
    assert_eq!(layout.partitions[0].entry.kind, partition::PartitionKind::EfiSystem);
    assert!(!layout.partitions[0].aligned_1mib);
    let primary = layout.primary().unwrap();
    assert_eq!(primary.entry.index, 2);
    assert_eq!(primary.entry.gpt_name.as_deref(), Some("Untitled"));
    let fs = primary.filesystem.as_ref().unwrap();
    assert_eq!(fs.kind, FilesystemKind::Exfat);
    assert_eq!(fs.label.as_deref(), Some("FESTIVAL26"));
    assert_eq!(fs.dirty, Some(false));
    assert!(!layout.primary_is_first());
    assert!(layout.warnings.is_empty(), "{:?}", layout.warnings);
}

#[test]
fn superfloppy_fat32() {
    if !have_tool("mkfs.fat") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("sf.img");
    sparse_file(&img, 128 * MIB);
    run_tool("mkfs.fat", &["-F", "32", "-n", "RAW", img.to_str().unwrap()], None);
    let layout = inspect_path(&img).unwrap();
    assert_eq!(layout.scheme, PartitionScheme::Superfloppy);
    assert_eq!(layout.primary_filesystem(), Some(FilesystemKind::Fat32));
}

#[test]
fn fat16_is_classified_by_cluster_count() {
    if !have_tool("sfdisk") || !have_tool("mkfs.fat") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("disk.img");
    sparse_file(&img, 64 * MIB);
    run_tool("sfdisk", &[img.to_str().unwrap()], Some("label: dos\nstart=2048, type=6\n"));
    let part = dir.path().join("p.img");
    sparse_file(&part, 63 * MIB);
    run_tool("mkfs.fat", &["-F", "16", "-n", "OLDSTICK", part.to_str().unwrap()], None);
    splice_partition(&img, &part, MIB);
    let layout = inspect_path(&img).unwrap();
    let fs = layout.partitions[0].filesystem.as_ref().unwrap();
    assert_eq!(fs.kind, FilesystemKind::Fat16);
    assert_eq!(fs.label.as_deref(), Some("OLDSTICK"));
}

#[test]
fn dirty_fat32_is_reported() {
    if !have_tool("mkfs.fat") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("sf.img");
    sparse_file(&img, 128 * MIB);
    run_tool("mkfs.fat", &["-F", "32", img.to_str().unwrap()], None);
    // Clear ClnShutBitMask in FAT[1] of the first FAT.
    let mut f = OpenOptions::new().read(true).write(true).open(&img).unwrap();
    let mut boot = [0u8; 512];
    std::io::Read::read_exact(&mut f, &mut boot).unwrap();
    let reserved = u16::from_le_bytes([boot[14], boot[15]]) as u64;
    let bps = u16::from_le_bytes([boot[11], boot[12]]) as u64;
    f.seek(SeekFrom::Start(reserved * bps + 4)).unwrap();
    f.write_all(&0x0000_0000u32.to_le_bytes()).unwrap();
    drop(f);
    let layout = inspect_path(&img).unwrap();
    assert_eq!(layout.partitions[0].filesystem.as_ref().unwrap().dirty, Some(true));
}

#[test]
fn mbr_type_mismatch_warns() {
    if !have_tool("sfdisk") || !have_tool("mkfs.fat") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let img = dir.path().join("disk.img");
    sparse_file(&img, 300 * MIB);
    run_tool("sfdisk", &[img.to_str().unwrap()], Some("label: dos\nstart=2048, type=7\n"));
    let part = dir.path().join("p.img");
    sparse_file(&part, 299 * MIB);
    run_tool("mkfs.fat", &["-F", "32", part.to_str().unwrap()], None);
    splice_partition(&img, &part, MIB);
    let layout = inspect_path(&img).unwrap();
    assert!(layout.warnings.iter().any(|w| w.contains("type 0x07")), "{:?}", layout.warnings);
}

#[test]
fn blank_and_garbage_devices_do_not_panic() {
    let blank = std::io::Cursor::new(vec![0u8; 4 * MIB as usize]);
    let layout = inspect(blank).unwrap();
    assert_eq!(layout.scheme, PartitionScheme::Unknown);
    assert!(layout.partitions.is_empty());

    // Deterministic pseudo-random bytes with an MBR signature bolted on.
    let mut junk = vec![0u8; 2 * MIB as usize];
    let mut x = 0x1234_5678u32;
    for b in junk.iter_mut() {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *b = x as u8;
    }
    junk[510] = 0x55;
    junk[511] = 0xAA;
    let _ = inspect(std::io::Cursor::new(junk.clone())).unwrap();
    junk[512..520].copy_from_slice(b"EFI PART");
    let _ = inspect(std::io::Cursor::new(junk)).unwrap();
}
