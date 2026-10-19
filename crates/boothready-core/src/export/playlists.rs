//! Playlists from folders of music: the shared half of `boothready
//! export-pdb` and the desktop app's Export to USB flow.
//!
//! A playlist is a named list of audio files. The CLI gets its lists by
//! walking folders; the app takes them from a source's last scan. Both end
//! up in [`collect_from_lists`], which exports each file once however many
//! playlists hold it and leaves out formats a Device Library cannot store.
//! [`guard_target`] keeps the export folder off USB and removable drives,
//! because an export reaches a stick only through the copy flow, after the
//! user has confirmed the exact device.

use super::device_library::{ExportError, PlaylistSpec};
use crate::audio::is_audio_path;
use crate::library::pdb_writer::file_type_for_path;
use crate::sources::device_for_path;
use boothready_model::{BusType, PhysicalDevice};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A playlist named after a folder of music.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistFolder {
    pub name: String,
    pub folder: PathBuf,
}

/// The tracks and playlists an export request needs, plus what was left out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Collected {
    /// Every audio file once, in the order it was first met.
    pub tracks: Vec<PathBuf>,
    pub playlists: Vec<PlaylistSpec>,
    /// Files in other formats and playlists without tracks, in plain words.
    pub warnings: Vec<String>,
}

/// Seconds the analyser takes per track on a laptop, for time estimates.
pub const ANALYSIS_SECS_PER_TRACK: u64 = 6;

/// Build playlists by walking folders: every Device Library audio file under
/// each folder, in name order. Symlinks and dot files are skipped.
pub fn collect_from_folders(playlists: &[PlaylistFolder]) -> Result<Collected, ExportError> {
    let mut lists = Vec::with_capacity(playlists.len());
    for p in playlists {
        if !p.folder.is_dir() {
            return Err(ExportError::Request(format!("playlist '{}': {} is not a folder", p.name, p.folder.display())));
        }
        let mut files = Vec::new();
        for entry in
            walkdir::WalkDir::new(&p.folder).follow_links(false).sort_by_file_name().into_iter().filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy();
            if file_name.starts_with('.') || !is_audio_path(entry.path()) {
                continue;
            }
            files.push(entry.path().to_path_buf());
        }
        lists.push((p.name.clone(), files));
    }
    collect_from_lists(&lists)
}

/// Build playlists from lists of audio files, one list per playlist in
/// playing order. A file in several lists is exported once; a file in a
/// format a Device Library cannot hold is reported and left out.
pub fn collect_from_lists(playlists: &[(String, Vec<PathBuf>)]) -> Result<Collected, ExportError> {
    let mut out = Collected::default();
    let mut index_of: BTreeMap<PathBuf, usize> = BTreeMap::new();
    for (name, files) in playlists {
        if name.trim().is_empty() {
            return Err(ExportError::Request("a playlist has no name".into()));
        }
        let mut members = Vec::with_capacity(files.len());
        for file in files {
            let file_name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if file_type_for_path(&file_name).is_none() {
                out.warnings.push(format!("skipped {}: not a format a Device Library can hold", file.display()));
                continue;
            }
            let canonical = file
                .canonicalize()
                .map_err(|e| ExportError::Io { context: format!("resolving {}", file.display()), source: e })?;
            let index = *index_of.entry(canonical).or_insert_with(|| {
                out.tracks.push(file.clone());
                out.tracks.len() - 1
            });
            if !members.contains(&index) {
                members.push(index);
            }
        }
        if members.is_empty() {
            out.warnings.push(format!("playlist '{name}' has no tracks"));
        }
        out.playlists.push(PlaylistSpec { name: name.clone(), tracks: members });
    }
    if out.tracks.is_empty() {
        return Err(ExportError::Request(
            "no audio to export: the playlist folders hold no MP3, M4A, FLAC, WAV or AIFF files".into(),
        ));
    }
    Ok(out)
}

/// Refuse an export folder on a USB or other removable drive. Exports go
/// onto a stick only through the copy flow, after the user confirms the
/// exact device. `devices` is the platform's device list; unknown storage
/// is not refused here.
pub fn guard_target(root: &Path, devices: &[PhysicalDevice]) -> Result<(), ExportError> {
    let mut ancestor = root;
    while !ancestor.exists() {
        ancestor =
            ancestor.parent().ok_or_else(|| ExportError::Target("the output folder has no existing parent".into()))?;
    }
    // Absolute rather than canonical: on Windows a canonical path carries a
    // `\\?\` prefix that no mount point has, and on every OS the device
    // lookup compares against the mount points as the platform lists them.
    let ancestor = std::path::absolute(ancestor)
        .map_err(|e| ExportError::Io { context: format!("resolving {}", ancestor.display()), source: e })?;
    if let Some(device) = device_for_path(&ancestor, devices) {
        if device.bus == BusType::Usb || device.removable {
            return Err(ExportError::Target(format!(
                "{} is on a USB or removable drive ({}). Export into a folder on this computer, then copy it onto the stick.",
                root.display(),
                device.raw_display_name()
            )));
        }
    }
    Ok(())
}

/// How long analysing `tracks` tracks takes, roughly.
pub fn analysis_estimate_secs(tracks: usize) -> u64 {
    tracks as u64 * ANALYSIS_SECS_PER_TRACK
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::fixtures;
    use boothready_model::Volume;
    use std::fs;

    fn device(mount: PathBuf, bus: BusType, removable: bool, is_system: bool) -> PhysicalDevice {
        PhysicalDevice {
            id: "synthetic".into(),
            os_path: "synthetic".into(),
            bus,
            removable,
            is_system,
            size_bytes: 1000,
            logical_sector_size: 512,
            storage_vendor: None,
            storage_model: Some("Stick".into()),
            storage_revision: None,
            usb: None,
            partition_scheme: None,
            volumes: vec![Volume {
                os_path: "synthetic".into(),
                mount_point: Some(mount),
                label: None,
                filesystem: None,
                size_bytes: 1000,
                offset_bytes: None,
                uuid: None,
                efi_system: false,
            }],
        }
    }

    #[test]
    fn folders_become_playlists_and_shared_files_are_exported_once() {
        let tmp = tempfile::tempdir().unwrap();
        let fri = tmp.path().join("fri");
        let sat = tmp.path().join("sat/deeper");
        fs::create_dir_all(&fri).unwrap();
        fs::create_dir_all(&sat).unwrap();
        for name in ["b.wav", "a.wav", "._a.wav", "notes.txt", "odd.ogg"] {
            fs::write(fri.join(name), fixtures::wav(44_100, 16, 2, 10, false)).unwrap();
        }
        fs::write(sat.join("c.wav"), fixtures::wav(44_100, 16, 2, 10, false)).unwrap();
        fs::write(tmp.path().join("sat/a.wav"), fixtures::wav(44_100, 16, 2, 10, false)).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(fri.join("b.wav"), sat.join("b-link.wav")).unwrap();

        let lists = [
            PlaylistFolder { name: "Friday".into(), folder: fri.clone() },
            PlaylistFolder { name: "Saturday".into(), folder: tmp.path().join("sat") },
            PlaylistFolder { name: "Empty".into(), folder: tmp.path().join("sat/none") },
        ];
        assert!(collect_from_folders(&lists).is_err(), "a missing folder is an error");
        fs::create_dir_all(tmp.path().join("sat/none")).unwrap();
        let got = collect_from_folders(&lists).unwrap();
        let names: Vec<String> =
            got.tracks.iter().map(|t| t.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a.wav", "b.wav", "a.wav", "c.wav"], "symlinks, dotfiles and other formats are left out");
        assert_eq!(got.playlists[0], PlaylistSpec { name: "Friday".into(), tracks: vec![0, 1] });
        assert_eq!(got.playlists[1], PlaylistSpec { name: "Saturday".into(), tracks: vec![2, 3] });
        assert_eq!(got.playlists[2].tracks, Vec::<usize>::new());
        assert!(got.warnings.iter().any(|w| w.contains("odd.ogg")));
        assert!(got.warnings.iter().any(|w| w.contains("'Empty'")));

        // The same file in two lists is one track in both playlists.
        let shared = collect_from_lists(&[
            ("One".into(), vec![fri.join("a.wav"), fri.join("b.wav")]),
            ("Two".into(), vec![fri.join("b.wav"), fri.join("b.wav"), fri.join("odd.ogg")]),
        ])
        .unwrap();
        assert_eq!(shared.tracks.len(), 2);
        assert_eq!(shared.playlists[1].tracks, vec![1]);
        assert_eq!(shared.warnings.len(), 1);
        assert!(collect_from_lists(&[("".into(), vec![fri.join("a.wav")])]).is_err());
        assert!(collect_from_lists(&[("Nothing".into(), vec![])]).is_err());
        assert!(collect_from_lists(&[("Gone".into(), vec![fri.join("missing.wav")])]).is_err());
    }

    #[test]
    fn targets_on_removable_drives_are_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let internal = device(tmp.path().to_path_buf(), BusType::Sata, false, true);
        let usb = device(tmp.path().to_path_buf(), BusType::Usb, true, false);
        let out = tmp.path().join("export/new");
        assert!(guard_target(&out, &[internal]).is_ok());
        assert!(guard_target(&out, &[usb]).is_err());
        assert!(guard_target(&out, &[]).is_ok(), "unknown storage is not refused here");
        assert_eq!(analysis_estimate_secs(10), 60);
    }
}
