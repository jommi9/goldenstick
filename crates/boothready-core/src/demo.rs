//! Realistic demo drive contents: a rekordbox-style export with real audio
//! headers (tiny files), playlists, and the usual problems DJs run into.
//! Used by the CLI's `demo` command and the app's demo mode.

use crate::audio::fixtures as audio;
use crate::library::pdb_fixture::{build_pdb, FixturePlaylist, FixtureTrack};
use crate::library::{DEVICE_LIBRARY_FILE, ONE_LIBRARY_FILE};
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemoContent {
    /// Device Library only, with hi-res FLAC, an unreadable file and macOS
    /// litter. The PRD's "USB A".
    DeviceLibraryWithIssues,
    /// Device Library and OneLibrary, all tracks legacy-safe.
    BothLibraries,
    Empty,
}

struct Tr {
    path: &'static str,
    title: &'static str,
    artist: u32,
    bytes: Vec<u8>,
    rate: u32,
    depth: u16,
}

fn tracks(with_issues: bool) -> Vec<Tr> {
    let mut v = vec![
        Tr {
            path: "Contents/Kolsch/Speicher/Grey.mp3",
            title: "Grey",
            artist: 1,
            bytes: audio::mp3_cbr(120),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Kolsch/Speicher/Goldfisch.mp3",
            title: "Goldfisch",
            artist: 1,
            bytes: audio::mp3_cbr(140),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Charlotte de Witte/Selected/Doppler.aiff",
            title: "Doppler",
            artist: 2,
            bytes: audio::aiff(44_100, 16, 2, 22_050, false),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Charlotte de Witte/Selected/Sgadi Li Mi.wav",
            title: "Sgadi Li Mi",
            artist: 2,
            bytes: audio::wav(48_000, 24, 2, 24_000, false),
            rate: 48_000,
            depth: 24,
        },
        Tr {
            path: "Contents/Ben Klock/Subzero/Subzero.aiff",
            title: "Subzero",
            artist: 3,
            bytes: audio::aiff(44_100, 24, 2, 22_050, false),
            rate: 44_100,
            depth: 24,
        },
        Tr {
            path: "Contents/Ben Klock/Subzero/Compression.m4a",
            title: "Compression",
            artist: 3,
            bytes: audio::m4a("aac-lc", 44_100, 360),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Paula Temple/Colonized/Colonized.wav",
            title: "Colonized",
            artist: 4,
            bytes: audio::wav(44_100, 16, 2, 22_050, false),
            rate: 44_100,
            depth: 16,
        },
        Tr {
            path: "Contents/Paula Temple/Colonized/Deathvox.mp3",
            title: "Deathvox",
            artist: 4,
            bytes: audio::mp3_cbr(100),
            rate: 44_100,
            depth: 16,
        },
    ];
    if with_issues {
        v.push(Tr {
            path: "Contents/Hi-Res/Ambient Intro.flac",
            title: "Ambient Intro",
            artist: 5,
            bytes: audio::flac(96_000, 24, 2, 96_000 * 400),
            rate: 96_000,
            depth: 24,
        });
        v.push(Tr {
            path: "Contents/Hi-Res/Sunrise Edit.flac",
            title: "Sunrise Edit",
            artist: 5,
            bytes: audio::flac(44_100, 16, 2, 44_100 * 300),
            rate: 44_100,
            depth: 16,
        });
        v.push(Tr {
            path: "Contents/Hi-Res/Studio Master.wav",
            title: "Studio Master",
            artist: 5,
            bytes: audio::wav(96_000, 24, 2, 9_600, true),
            rate: 96_000,
            depth: 24,
        });
        v.push(Tr {
            path: "Contents/Hi-Res/Vinyl Rip.m4a",
            title: "Vinyl Rip",
            artist: 5,
            bytes: audio::m4a("alac", 44_100, 280),
            rate: 44_100,
            depth: 16,
        });
    }
    v
}

fn write(root: &Path, rel: &str, bytes: &[u8]) -> io::Result<()> {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap())?;
    fs::write(p, bytes)
}

pub fn populate(root: &Path, content: DemoContent) -> io::Result<()> {
    fs::create_dir_all(root)?;
    if content == DemoContent::Empty {
        return Ok(());
    }
    let with_issues = content == DemoContent::DeviceLibraryWithIssues;
    let list = tracks(with_issues);
    let mut fixture = Vec::new();
    for (i, t) in list.iter().enumerate() {
        write(root, t.path, &t.bytes)?;
        fixture.push(FixtureTrack {
            id: i as u32 + 1,
            title: t.title.into(),
            artist_id: t.artist,
            file_path: format!("/{}", t.path),
            sample_rate: t.rate,
            sample_depth: t.depth,
            bitrate: 320,
            file_size: t.bytes.len() as u32,
            duration_secs: 360,
            tempo: 13_000,
        });
    }
    let n = fixture.len() as u32;
    let mut playlists = vec![
        FixturePlaylist { id: 1, parent_id: 0, name: "Gigs".into(), is_folder: true, sort_order: 0, track_ids: vec![] },
        FixturePlaylist {
            id: 2,
            parent_id: 1,
            name: "Berghain".into(),
            is_folder: false,
            sort_order: 0,
            track_ids: vec![5, 3, 7, 8],
        },
        FixturePlaylist {
            id: 3,
            parent_id: 0,
            name: "Warm-up".into(),
            is_folder: false,
            sort_order: 1,
            track_ids: vec![1, 2, 6],
        },
        FixturePlaylist {
            id: 4,
            parent_id: 0,
            name: "Peak time".into(),
            is_folder: false,
            sort_order: 2,
            track_ids: vec![3, 4, 5, 7, 8],
        },
    ];
    if with_issues {
        playlists.push(FixturePlaylist {
            id: 5,
            parent_id: 0,
            name: "Closing".into(),
            is_folder: false,
            sort_order: 3,
            track_ids: (9..=n).collect(),
        });
    }
    let artists = vec![
        (1, "Kölsch".to_string()),
        (2, "Charlotte de Witte".to_string()),
        (3, "Ben Klock".to_string()),
        (4, "Paula Temple".to_string()),
        (5, "Various".to_string()),
    ];
    write(root, DEVICE_LIBRARY_FILE, &build_pdb(&fixture, &artists, &playlists))?;
    write(root, "PIONEER/MYSETTING.DAT", &[0u8; 160])?;
    for t in &fixture {
        write(root, &format!("PIONEER/USBANLZ/P000/{:08X}/ANLZ0000.DAT", t.id), b"PMAI")?;
    }
    if with_issues {
        // Damaged download and macOS metadata litter.
        write(root, "Contents/Downloads/Unknown Track (1).mp3", b"<html>Access denied</html>")?;
        write(root, "Contents/Kolsch/Speicher/._Grey.mp3", &[0u8; 4096])?;
        write(root, "Contents/Hi-Res/._Ambient Intro.flac", &[0u8; 4096])?;
    } else {
        // OneLibrary databases are encrypted; random-looking bytes stand in.
        let bytes: Vec<u8> = (0..65_536u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        write(root, ONE_LIBRARY_FILE, &bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{scan_libraries, LibraryFormat};
    use crate::scan::scan_audio;

    #[test]
    fn demo_contents_scan_as_intended() {
        let d = tempfile::tempdir().unwrap();
        populate(d.path(), DemoContent::DeviceLibraryWithIssues).unwrap();
        let libs = scan_libraries(d.path());
        assert_eq!(libs.formats, vec![LibraryFormat::RekordboxDeviceLibrary]);
        assert!(libs.problems().is_empty(), "{:?}", libs.problems());
        let audio = scan_audio(d.path(), |_, _| {});
        assert_eq!(audio.tracks.len(), 13);
        assert_eq!(audio.apple_double, 2);
        assert_eq!(
            audio.tracks.iter().filter(|t| matches!(t.probe, crate::scan::TrackProbe::Unreadable(_))).count(),
            1
        );

        let e = tempfile::tempdir().unwrap();
        populate(e.path(), DemoContent::BothLibraries).unwrap();
        let libs = scan_libraries(e.path());
        assert!(libs.has(LibraryFormat::RekordboxOneLibrary) && libs.has(LibraryFormat::RekordboxDeviceLibrary));
        assert!(libs.problems().is_empty(), "{:?}", libs.problems());
    }
}
