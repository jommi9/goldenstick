//! `boothready export-pdb`: write a rekordbox Device Library tree (export.pdb,
//! analysis files and the audio) into a local folder from playlist folders.
//!
//! The folder is later copied onto a stick with `boothready copy`, which
//! keeps the device confirmation in one place. This command refuses a target
//! that sits on a USB or other removable drive.

use anyhow::{bail, Context, Result};
use boothready_core::audio::is_audio_path;
use boothready_core::export::device_library::{
    export_device_library, Analysis, ExportReport, ExportRequest, PlaylistSpec,
};
use boothready_core::library::pdb_writer::file_type_for_path;
use boothready_core::sources::device_for_path;
use boothready_model::{BusType, PhysicalDevice};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Parse one `--playlist NAME=FOLDER` argument.
pub fn parse_playlist(arg: &str) -> Result<(String, PathBuf)> {
    let Some((name, folder)) = arg.split_once('=') else {
        bail!("--playlist must look like NAME=FOLDER, got '{arg}'");
    };
    let name = name.trim();
    if name.is_empty() {
        bail!("--playlist '{arg}' has no name before the '='");
    }
    if folder.is_empty() {
        bail!("--playlist '{name}' has no folder after the '='");
    }
    Ok((name.to_string(), PathBuf::from(folder)))
}

/// The tracks and playlists for an export: every Device Library audio file
/// under each folder, in name order, each file exported once however many
/// playlists hold it. Files in other audio formats are reported, not exported.
pub fn collect(playlists: &[(String, PathBuf)]) -> Result<(Vec<PathBuf>, Vec<PlaylistSpec>, Vec<String>)> {
    let mut tracks = Vec::new();
    let mut index_of: BTreeMap<PathBuf, usize> = BTreeMap::new();
    let mut specs = Vec::new();
    let mut warnings = Vec::new();
    for (name, folder) in playlists {
        if !folder.is_dir() {
            bail!("playlist '{name}': {} is not a folder", folder.display());
        }
        let mut members = Vec::new();
        for entry in
            walkdir::WalkDir::new(folder).follow_links(false).sort_by_file_name().into_iter().filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let file_name = entry.file_name().to_string_lossy();
            if file_name.starts_with('.') || !is_audio_path(entry.path()) {
                continue;
            }
            if file_type_for_path(&file_name).is_none() {
                warnings.push(format!("skipped {}: not a format a Device Library can hold", entry.path().display()));
                continue;
            }
            let canonical =
                entry.path().canonicalize().with_context(|| format!("resolving {}", entry.path().display()))?;
            let index = *index_of.entry(canonical).or_insert_with(|| {
                tracks.push(entry.path().to_path_buf());
                tracks.len() - 1
            });
            members.push(index);
        }
        if members.is_empty() {
            warnings.push(format!("playlist '{name}' has no tracks in {}", folder.display()));
        }
        specs.push(PlaylistSpec { name: name.clone(), tracks: members });
    }
    if tracks.is_empty() {
        bail!("no audio to export: the playlist folders hold no MP3, M4A, FLAC, WAV or AIFF files");
    }
    Ok((tracks, specs, warnings))
}

/// Refuse a target on a USB or other removable drive. Exports go onto a stick
/// only through `boothready copy`, after the user confirms the exact device.
pub fn guard_target(root: &Path, devices: &[PhysicalDevice]) -> Result<()> {
    let mut ancestor = root;
    while !ancestor.exists() {
        ancestor = ancestor.parent().context("the output folder has no existing parent")?;
    }
    // Absolute rather than canonical: on Windows a canonical path carries a
    // `\\?\` prefix that no mount point has, and on every OS the device
    // lookup compares against the mount points as the platform lists them.
    let ancestor = std::path::absolute(ancestor)?;
    if let Some(device) = device_for_path(&ancestor, devices) {
        if device.bus == BusType::Usb || device.removable {
            bail!(
                "{} is on a USB or removable drive ({}). Export into a folder on this computer, then copy it with `boothready copy`.",
                root.display(),
                device.raw_display_name()
            );
        }
    }
    Ok(())
}

pub fn analysis(skip: bool, rhythm_mode: &str) -> Result<Analysis> {
    if skip {
        return Ok(Analysis::Skip);
    }
    Ok(match rhythm_mode {
        "normal" => Analysis::Normal,
        "dynamic" => Analysis::Dynamic,
        "auto" => Analysis::Auto,
        other => bail!("unknown rhythm mode '{other}': use normal, dynamic or auto"),
    })
}

/// Build the request and run it, printing progress to stderr unless `quiet`.
pub fn run(
    out: &Path,
    playlists: &[(String, PathBuf)],
    analysis: Analysis,
    quiet: bool,
) -> Result<(ExportReport, Vec<String>)> {
    let (tracks, specs, warnings) = collect(playlists)?;
    if !quiet {
        eprintln!("{} tracks in {} playlists.", tracks.len(), specs.len());
    }
    let request = ExportRequest { root: out.to_path_buf(), tracks, playlists: specs, analysis };
    let report = export_device_library(&request, |p| {
        if !quiet {
            let verb = match p.phase {
                "analyze" => "Analysing",
                "probe" => "Reading",
                "copy" => "Copying",
                "database" => "Writing export.pdb",
                _ => "Installing",
            };
            if p.total > 0 && p.current.is_empty() {
                eprintln!("{verb}…");
            } else {
                eprintln!("{verb} {}/{}: {}", p.done + 1, p.total, p.current);
            }
        }
    })?;
    Ok((report, warnings))
}

/// The closing lines for the terminal.
pub fn describe(report: &ExportReport, warnings: &[String]) -> String {
    let mut out = format!(
        "Wrote {} tracks and {} playlists to {} ({} analysis files, {}).",
        report.tracks.len(),
        report.playlists,
        report.root.display(),
        report.analysis_files,
        boothready_core::verify::human_bytes(report.bytes_copied)
    );
    for w in warnings.iter().chain(&report.warnings) {
        out.push_str("\n  ");
        out.push_str(w);
    }
    out.push_str(&format!("\nCheck it with `boothready check-dir {}`.", report.root.display()));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::device;
    use boothready_core::audio::fixtures;
    use std::fs;

    #[test]
    fn playlist_arguments_split_at_the_first_equals_sign() {
        assert_eq!(parse_playlist("Friday=/music/fri").unwrap(), ("Friday".into(), PathBuf::from("/music/fri")));
        assert_eq!(parse_playlist("A=B=/music/a=b").unwrap(), ("A".into(), PathBuf::from("B=/music/a=b")));
        assert!(parse_playlist("nofolder").is_err());
        assert!(parse_playlist("=/music").is_err());
        assert!(parse_playlist("Name=").is_err());
        assert!(analysis(false, "fast").is_err());
        assert!(matches!(analysis(true, "fast").unwrap(), Analysis::Skip));
        assert!(matches!(analysis(false, "dynamic").unwrap(), Analysis::Dynamic));
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
            ("Friday".to_string(), fri.clone()),
            ("Saturday".to_string(), tmp.path().join("sat")),
            ("Empty".to_string(), tmp.path().join("sat/none")),
        ];
        assert!(collect(&lists).is_err(), "a missing folder is an error");
        fs::create_dir_all(tmp.path().join("sat/none")).unwrap();
        let (tracks, specs, warnings) = collect(&lists).unwrap();
        let names: Vec<String> = tracks.iter().map(|t| t.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["a.wav", "b.wav", "a.wav", "c.wav"], "symlinks, dotfiles and other formats are left out");
        assert_eq!(specs[0], PlaylistSpec { name: "Friday".into(), tracks: vec![0, 1] });
        assert_eq!(specs[1], PlaylistSpec { name: "Saturday".into(), tracks: vec![2, 3] });
        assert_eq!(specs[2].tracks, Vec::<usize>::new());
        assert!(warnings.iter().any(|w| w.contains("odd.ogg")));
        assert!(warnings.iter().any(|w| w.contains("'Empty'")));

        let out = tmp.path().join("out");
        let request = ExportRequest { root: out.clone(), tracks, playlists: specs, analysis: Analysis::Skip };
        let report = export_device_library(&request, |_| {}).unwrap();
        assert_eq!(report.tracks.len(), 4);
        let libs = boothready_core::library::scan_libraries(&out);
        let dl = libs.rekordbox.unwrap().device_library.unwrap();
        assert_eq!(dl.tracks.len(), 4);
        assert_eq!(dl.playlists.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Friday", "Saturday", "Empty"]);
        assert!(describe(&report, &warnings).contains("Wrote 4 tracks and 3 playlists"));
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
    }
}
