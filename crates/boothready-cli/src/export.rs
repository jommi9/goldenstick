//! `boothready export-pdb`: write a rekordbox Device Library tree (export.pdb,
//! analysis files and the audio) into a local folder from playlist folders.
//!
//! The folder is later copied onto a stick with `boothready copy`, which
//! keeps the device confirmation in one place. This command refuses a target
//! that sits on a USB or other removable drive.

use anyhow::{bail, Result};
use boothready_core::export::device_library::{export_device_library, Analysis, ExportReport, ExportRequest};
use boothready_core::export::playlists::{collect_from_folders, guard_target as guard, PlaylistFolder};
use boothready_model::PhysicalDevice;
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

/// Refuse a target on a USB or other removable drive. Exports go onto a stick
/// only through `boothready copy`, after the user confirms the exact device.
pub fn guard_target(root: &Path, devices: &[PhysicalDevice]) -> Result<()> {
    Ok(guard(root, devices)?)
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
    let folders: Vec<PlaylistFolder> =
        playlists.iter().map(|(name, folder)| PlaylistFolder { name: name.clone(), folder: folder.clone() }).collect();
    let collected = collect_from_folders(&folders)?;
    if !quiet {
        eprintln!("{} tracks in {} playlists.", collected.tracks.len(), collected.playlists.len());
    }
    let request = ExportRequest {
        root: out.to_path_buf(),
        tracks: collected.tracks,
        playlists: collected.playlists,
        analysis,
        analysis_cache: None,
    };
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
    Ok((report, collected.warnings))
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
    fn folders_are_exported_and_described() {
        let tmp = tempfile::tempdir().unwrap();
        let fri = tmp.path().join("fri");
        fs::create_dir_all(&fri).unwrap();
        for name in ["b.wav", "a.wav", "odd.ogg"] {
            fs::write(fri.join(name), fixtures::wav(44_100, 16, 2, 10, false)).unwrap();
        }
        let lists = [("Friday".to_string(), fri.clone())];
        let out = tmp.path().join("out");
        let (report, warnings) = run(&out, &lists, Analysis::Skip, true).unwrap();
        assert_eq!(report.tracks.len(), 2);
        assert!(warnings.iter().any(|w| w.contains("odd.ogg")));
        let libs = boothready_core::library::scan_libraries(&out);
        let dl = libs.rekordbox.unwrap().device_library.unwrap();
        assert_eq!(dl.playlists.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["Friday"]);
        assert!(describe(&report, &warnings).contains("Wrote 2 tracks and 1 playlists"));
        assert!(run(&out, &lists, Analysis::Skip, true).is_err(), "a folder that holds an export is refused");
    }
}
