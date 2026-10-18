use super::device_library::*;
use crate::library::{scan_libraries, LibraryFormat};
use djbooth_analyzer::{AnlzFile, DatAnalysis, ExtAnalysis, ThreeBandAnalysis};
use std::fs;
use std::path::{Path, PathBuf};

/// A 16-bit stereo WAV of short clicks at `bpm`, `seconds` long: the
/// analyser finds a beat grid in it, which silence would not give.
fn click_track(path: &Path, bpm: f64, seconds: u32) {
    let rate = 44_100u32;
    let frames = rate * seconds;
    let beat = (f64::from(rate) * 60.0 / bpm) as u32;
    let mut pcm = Vec::with_capacity(frames as usize * 4);
    for i in 0..frames {
        let pos = i % beat;
        let s = if pos < 400 {
            let env = 1.0 - f64::from(pos) / 400.0;
            (f64::from(pos) * 2.0 * std::f64::consts::PI * 1000.0 / f64::from(rate)).sin() * env * 0.8
        } else {
            0.0
        };
        let v = (s * f64::from(i16::MAX)) as i16;
        pcm.extend_from_slice(&v.to_le_bytes());
        pcm.extend_from_slice(&v.to_le_bytes());
    }
    let mut wav = crate::audio::fixtures::wav(rate, 16, 2, 0, false);
    let data_len = pcm.len() as u32;
    let riff_len = wav.len() as u32 - 8 + data_len;
    wav[4..8].copy_from_slice(&riff_len.to_le_bytes());
    let n = wav.len();
    wav[n - 4..].copy_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(&pcm);
    fs::write(path, wav).unwrap();
}

fn tag(path: &Path, title: &str, artist: &str) {
    use lofty::config::WriteOptions;
    use lofty::prelude::{Accessor, TagExt};
    use lofty::tag::{Tag, TagType};
    let mut tag = Tag::new(TagType::Id3v2);
    tag.set_title(title.into());
    tag.set_artist(artist.into());
    tag.set_album("Test Album".into());
    tag.save_to_path(path, WriteOptions::default()).unwrap();
}

fn source_tracks(dir: &Path, names: &[(&str, &str, &str)], seconds: u32) -> Vec<PathBuf> {
    names
        .iter()
        .enumerate()
        .map(|(i, (file, title, artist))| {
            let path = dir.join(file);
            click_track(&path, 120.0 + i as f64 * 4.0, seconds);
            tag(&path, title, artist);
            path
        })
        .collect()
}

#[test]
fn export_without_analysis_reads_back_through_the_library_scanner() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("music");
    fs::create_dir(&src).unwrap();
    let tracks = source_tracks(
        &src,
        &[
            ("one.wav", "First Track", "Ääkkönen"),
            ("two: the sequel?.wav", "Second", "Artist B"),
            ("three.wav", "Third", "Ääkkönen"),
        ],
        2,
    );
    let root = tmp.path().join("stick");
    let request = ExportRequest {
        root: root.clone(),
        tracks: tracks.clone(),
        playlists: vec![
            PlaylistSpec { name: "Friday".into(), tracks: vec![2, 0] },
            PlaylistSpec { name: "Warm-up ☀".into(), tracks: vec![1] },
        ],
        analysis: Analysis::Skip,
    };
    let mut phases = Vec::new();
    let report = export_device_library(&request, |p| phases.push(p.phase)).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.tracks.len(), 3);
    assert_eq!(report.playlists, 2);
    assert_eq!(report.analysis_files, 0);
    assert_eq!(report.tracks[1].usb_path, "/Contents/00000002/two_ the sequel_.wav");
    assert_eq!(report.tracks[0].title, "First Track");
    assert_eq!(report.tracks[0].artist.as_deref(), Some("Ääkkönen"));
    assert!(phases.contains(&"copy") && phases.contains(&"install"));
    assert!(root.join("Contents/00000001/one.wav").is_file());
    assert!(root.join("PIONEER/rekordbox/export.pdb").is_file());
    assert!(!root.join("PIONEER/USBANLZ").exists());
    let leftovers: Vec<_> =
        fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(leftovers.iter().filter(|n| n.starts_with(".boothready-export")).count(), 0, "{leftovers:?}");

    let libs = scan_libraries(&root);
    assert!(libs.has(LibraryFormat::RekordboxDeviceLibrary));
    let dl = libs.rekordbox.as_ref().unwrap().device_library.as_ref().unwrap();
    assert_eq!(dl.parse_error, None);
    assert!(dl.warnings.is_empty(), "{:?}", dl.warnings);
    assert_eq!(dl.tracks.len(), 3);
    assert!(dl.missing.is_empty() && dl.size_mismatch.is_empty(), "{:?} {:?}", dl.missing, dl.size_mismatch);
    let titles: Vec<(&str, Option<&str>, &str)> =
        dl.tracks.iter().map(|t| (t.title.as_str(), t.artist.as_deref(), t.path.as_str())).collect();
    assert_eq!(
        titles,
        vec![
            ("First Track", Some("Ääkkönen"), "Contents/00000001/one.wav"),
            ("Second", Some("Artist B"), "Contents/00000002/two_ the sequel_.wav"),
            ("Third", Some("Ääkkönen"), "Contents/00000003/three.wav"),
        ]
    );
    assert_eq!(dl.tracks[0].db_sample_rate, Some(44_100));
    assert_eq!(dl.tracks[0].db_bit_depth, Some(16));
    assert_eq!(dl.tracks[0].db_duration_secs, Some(2));
    let playlists: Vec<(&str, &[u32])> =
        dl.playlists.iter().map(|p| (p.name.as_str(), p.track_ids.as_slice())).collect();
    assert_eq!(playlists, vec![("Friday", &[3u32, 1][..]), ("Warm-up ☀", &[2u32][..])]);
    assert_eq!(libs.rekordbox.as_ref().unwrap().analysis_files, 0);
    assert!(libs.problems().is_empty(), "{:?}", libs.problems());
}

#[test]
fn export_with_analysis_points_the_anlz_files_at_the_media_path() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("music");
    fs::create_dir(&src).unwrap();
    let tracks = source_tracks(&src, &[("clicks.wav", "Clicks", "Metronome")], 20);
    let root = tmp.path().join("stick");
    let request = ExportRequest {
        root: root.clone(),
        tracks,
        playlists: vec![PlaylistSpec { name: "Set".into(), tracks: vec![0] }],
        analysis: Analysis::Normal,
    };
    let report = export_device_library(&request, |_| {}).unwrap();
    assert!(report.warnings.is_empty(), "{:?}", report.warnings);
    assert_eq!(report.analysis_files, 3);
    let track = &report.tracks[0];
    assert_eq!(track.analysis_files, 3);
    let bpm = track.tempo_bpm.unwrap();
    assert!((bpm - 120.0).abs() < 0.6, "tempo {bpm}");
    assert!(track.key.is_some());
    assert_eq!(track.duration_secs, 20);

    let libs = scan_libraries(&root);
    let rb = libs.rekordbox.as_ref().unwrap();
    assert_eq!(rb.analysis_files, 3);
    let dl = rb.device_library.as_ref().unwrap();
    assert_eq!(dl.tracks.len(), 1);
    assert_eq!(dl.tracks[0].db_duration_secs, Some(20));

    let pdb = crate::library::pdb::parse_pdb(&fs::read(&report.pdb_path).unwrap()).unwrap();
    let analyze_path = &pdb.tracks[0].analyze_path;
    assert!(
        analyze_path.starts_with("/PIONEER/USBANLZ/P") && analyze_path.ends_with("/ANLZ0000.DAT"),
        "{analyze_path}"
    );
    assert_eq!(pdb.tracks[0].tempo, (bpm * 100.0).round() as u32);
    let dir = root.join(analyze_path.trim_start_matches('/')).parent().unwrap().to_path_buf();
    let dat = AnlzFile::read(&dir.join("ANLZ0000.DAT")).unwrap();
    assert_eq!(DatAnalysis::parse(&dat).unwrap().path, "/Contents/00000001/clicks.wav");
    let ext = AnlzFile::read(&dir.join("ANLZ0000.EXT")).unwrap();
    assert_eq!(ExtAnalysis::parse(&ext).unwrap().path, "/Contents/00000001/clicks.wav");
    let two_ex = AnlzFile::read(&dir.join("ANLZ0000.2EX")).unwrap();
    assert_eq!(ThreeBandAnalysis::parse(&two_ex).unwrap().path, "/Contents/00000001/clicks.wav");
}

#[test]
fn targets_with_an_export_or_unreadable_tracks_are_refused_and_leave_nothing_behind() {
    let tmp = tempfile::tempdir().unwrap();
    let src = tmp.path().join("music");
    fs::create_dir(&src).unwrap();
    let tracks = source_tracks(&src, &[("one.wav", "One", "A")], 1);

    let occupied = tmp.path().join("occupied");
    fs::create_dir_all(occupied.join("Contents")).unwrap();
    let request = ExportRequest { root: occupied, tracks: tracks.clone(), playlists: vec![], analysis: Analysis::Skip };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Target(_))));

    let root = tmp.path().join("stick");
    let request = ExportRequest {
        root: root.clone(),
        tracks: vec![tracks[0].clone(), src.join("missing.wav")],
        playlists: vec![],
        analysis: Analysis::Skip,
    };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Io { .. })));
    assert_eq!(fs::read_dir(&root).unwrap().count(), 0, "staging must be removed and nothing installed");

    let unsupported = src.join("odd.ogg");
    fs::write(&unsupported, b"OggS").unwrap();
    let request =
        ExportRequest { root: root.clone(), tracks: vec![unsupported], playlists: vec![], analysis: Analysis::Skip };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Track(_))));

    let request = ExportRequest {
        root: root.clone(),
        tracks: vec![tracks[0].clone(), tracks[0].clone()],
        playlists: vec![],
        analysis: Analysis::Skip,
    };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Request(_))));
    let request = ExportRequest {
        root,
        tracks: tracks.clone(),
        playlists: vec![PlaylistSpec { name: "Bad".into(), tracks: vec![1] }],
        analysis: Analysis::Skip,
    };
    assert!(matches!(export_device_library(&request, |_| {}), Err(ExportError::Request(_))));
}

#[test]
fn file_names_are_made_safe_for_player_filesystems() {
    assert_eq!(safe_filename("a:b/c?d.mp3", 1), "a_b_c_d.mp3");
    assert_eq!(safe_filename("trailing dots... ", 1), "trailing dots");
    assert_eq!(safe_filename("\u{1}\u{2}", 7), "__");
    assert_eq!(safe_filename("   ", 7), "track-00000007");
    assert_eq!(safe_filename(&"x".repeat(300), 1).chars().count(), 180);
}
