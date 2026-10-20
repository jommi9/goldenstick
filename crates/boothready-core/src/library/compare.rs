//! Compare a music library (folders of audio, such as a Google Drive folder)
//! with the rekordbox library on a stick.
//!
//! Playlists pair by name: a folder of the library is a playlist, and it
//! pairs with the stick playlist of the same name, ignoring case, spaces and
//! punctuation. Tracks pair by file size first, because rekordbox and
//! BoothReady copy audio byte for byte and files still in the cloud have
//! nothing else to compare, and then by name, for files whose tags were
//! rewritten after the export (Serato does this) or renamed on the way.

use serde::Serialize;
use std::collections::HashMap;

/// A track of the library, as the comparison needs it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryFile {
    /// Path inside the library, forward slashes, e.g. `DJ/psy/track.mp3`.
    pub path: String,
    pub size: u64,
    pub title: Option<String>,
    pub artist: Option<String>,
}

impl LibraryFile {
    fn folder(&self) -> &str {
        self.path.rsplit_once('/').map(|(f, _)| f).unwrap_or("")
    }
}

/// A track of the stick's library.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickFile {
    pub id: u32,
    pub title: String,
    pub artist: Option<String>,
    /// Volume-relative path.
    pub path: String,
    pub size: Option<u64>,
}

/// A playlist on the stick: its name and track IDs in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StickPlaylist {
    pub name: String,
    pub track_ids: Vec<u32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaylistState {
    /// Same tracks on both sides.
    Same,
    /// On both sides with different tracks.
    Differs,
    /// A library folder with no playlist of that name on the stick.
    LibraryOnly,
    /// A stick playlist with no library folder of that name.
    StickOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaylistCompare {
    pub name: String,
    /// The library folder, e.g. `DJ/psy`, when there is one.
    pub folder: Option<String>,
    pub state: PlaylistState,
    pub library_tracks: usize,
    pub stick_tracks: usize,
    pub in_both: usize,
    /// Library paths of tracks in the folder that the stick playlist lacks.
    pub missing_on_stick: Vec<String>,
    /// Titles of stick playlist tracks the folder lacks.
    pub only_on_stick: Vec<String>,
}

/// A stick track with no copy anywhere in the library, which a regenerated
/// stick would lose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Orphan {
    pub id: u32,
    pub title: String,
    pub artist: Option<String>,
    pub path: String,
    /// The stick playlists it is in.
    pub playlists: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Comparison {
    pub playlists: Vec<PlaylistCompare>,
    /// Stick tracks with no copy in the library.
    pub orphans: Vec<Orphan>,
    pub stick_tracks: usize,
    /// Stick tracks matched to a library file, by size or by name.
    pub matched_by_size: usize,
    pub matched_by_name: usize,
}

/// Lowercase letters and digits of a file name or title, with a leading
/// track number, a trailing `-1` or `(1)` copy marker and the extension
/// removed, so `01. Artist - Title-1.mp3` and `Artist - Title` compare equal.
pub fn name_key(text: &str) -> String {
    let stem = text.rsplit('/').next().unwrap_or(text);
    let stem = match stem.rsplit_once('.') {
        Some((s, ext)) if ext.len() <= 4 && ext.chars().all(|c| c.is_ascii_alphanumeric()) => s,
        _ => stem,
    };
    let mut s = stem.trim().to_lowercase();
    // A leading track number: "01. ", "01 - ", "1 ".
    let digits = s.chars().take_while(|c| c.is_ascii_digit()).count();
    if (1..=3).contains(&digits) {
        let after = &s[digits..];
        // "01. x", "01 - x", "01_x", or "01 Artist - Title"; "12 Inch" keeps its number.
        let numbered = after.starts_with(['.', '-', '_']) || (after.starts_with(' ') && after.contains(" - "));
        if numbered {
            s = after.trim_start_matches(['.', '-', ' ', '_']).to_string();
        }
    }
    for marker in ["-1", "-2", " (1)", " (2)"] {
        if let Some(cut) = s.strip_suffix(marker) {
            s = cut.to_string();
        }
    }
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_alphanumeric() {
            out.push(c);
        } else if !out.ends_with(' ') && !out.is_empty() {
            out.push(' ');
        }
    }
    out.trim_end().to_string()
}

fn stick_keys(t: &StickFile) -> Vec<String> {
    let mut keys = vec![name_key(&t.path)];
    if let Some(a) = &t.artist {
        keys.push(name_key(&format!("{a} - {}", t.title)));
    }
    keys.push(name_key(&t.title));
    keys.retain(|k| !k.is_empty());
    keys
}

fn library_keys(f: &LibraryFile) -> Vec<String> {
    let mut keys = vec![name_key(&f.path)];
    if let (Some(a), Some(t)) = (&f.artist, &f.title) {
        keys.push(name_key(&format!("{a} - {t}")));
    }
    if let Some(t) = &f.title {
        keys.push(name_key(t));
    }
    keys.retain(|k| !k.is_empty());
    keys
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum How {
    Size,
    Name,
}

/// For each stick track, every library file it is a copy of (a track often
/// sits in several playlist folders), and how the first one was found.
fn match_tracks(library: &[LibraryFile], stick: &[StickFile]) -> Vec<(Vec<usize>, Option<How>)> {
    let mut by_size: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut by_name: HashMap<String, Vec<usize>> = HashMap::new();
    for (i, f) in library.iter().enumerate() {
        by_size.entry(f.size).or_default().push(i);
        for k in library_keys(f) {
            by_name.entry(k).or_default().push(i);
        }
    }
    stick
        .iter()
        .map(|t| {
            let keys = stick_keys(t);
            if let Some(cands) = t.size.and_then(|s| by_size.get(&s)) {
                // Same size. Where several files share the size, keep those whose
                // name agrees too, unless none does.
                let named: Vec<usize> = cands
                    .iter()
                    .copied()
                    .filter(|&i| library_keys(&library[i]).iter().any(|k| keys.contains(k)))
                    .collect();
                let all = if named.is_empty() { cands.clone() } else { named };
                return (all, Some(How::Size));
            }
            let mut all: Vec<usize> = keys.iter().filter_map(|k| by_name.get(k)).flatten().copied().collect();
            all.sort_unstable();
            all.dedup();
            let how = (!all.is_empty()).then_some(How::Name);
            (all, how)
        })
        .collect()
}

/// The folder's own name, without its parents or a `<source>|` prefix.
fn folder_name(folder: &str) -> &str {
    let inner = folder.split_once('|').map(|(_, f)| f).unwrap_or(folder);
    inner.rsplit('/').next().unwrap_or(inner)
}

/// Compare `library` with the stick's tracks and playlists.
pub fn compare(library: &[LibraryFile], stick: &[StickFile], playlists: &[StickPlaylist]) -> Comparison {
    let matches = match_tracks(library, stick);
    let by_id: HashMap<u32, usize> = stick.iter().enumerate().map(|(i, t)| (t.id, i)).collect();
    let matched_by_size = matches.iter().filter(|m| m.1 == Some(How::Size)).count();
    let matched_by_name = matches.iter().filter(|m| m.1 == Some(How::Name)).count();

    // Library folders by name, each holding every track below it, as an export
    // turns a folder into a playlist of everything under it. When two folders
    // share a name the first in path order wins, as rekordbox playlists are flat.
    let mut folders: Vec<(String, Vec<usize>)> = Vec::new();
    {
        let mut index: HashMap<String, usize> = HashMap::new();
        let mut order: Vec<usize> = (0..library.len()).collect();
        order.sort_by(|&a, &b| library[a].path.cmp(&library[b].path));
        for i in order {
            let folder = library[i].folder();
            // The folder and each of its ancestors, below the source's root.
            let (root, inner) =
                folder.split_once('|').map(|(r, f)| (format!("{r}|"), f)).unwrap_or((String::new(), folder));
            let parts: Vec<&str> = inner.split('/').filter(|p| !p.is_empty()).collect();
            for end in 1..=parts.len() {
                let key = format!("{root}{}", parts[..end].join("/"));
                let at = *index.entry(key.clone()).or_insert_with(|| {
                    folders.push((key.clone(), Vec::new()));
                    folders.len() - 1
                });
                folders[at].1.push(i);
            }
        }
    }
    // Names pair ignoring case, spaces and punctuation: "hypno trance" is "hypnotrance".
    let key = |s: &str| s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect::<String>();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    for (i, (f, _)) in folders.iter().enumerate() {
        by_name.entry(key(folder_name(f))).or_insert(i);
    }

    let mut out = Vec::new();
    let mut paired = vec![false; folders.len()];
    for p in playlists {
        match by_name.get(&key(&p.name)) {
            Some(&fi) => {
                paired[fi] = true;
                let (folder, files) = &folders[fi];
                // A stick track is in this folder when any of its copies is.
                let in_folder: std::collections::HashSet<usize> = files.iter().copied().collect();
                let mut covered: std::collections::HashSet<usize> = std::collections::HashSet::new();
                let mut only = Vec::new();
                for &i in p.track_ids.iter().filter_map(|id| by_id.get(id)) {
                    let here: Vec<usize> = matches[i].0.iter().copied().filter(|l| in_folder.contains(l)).collect();
                    if here.is_empty() {
                        only.push(stick[i].title.clone());
                    }
                    covered.extend(here);
                }
                let missing: Vec<String> =
                    files.iter().filter(|i| !covered.contains(i)).map(|&i| library[i].path.clone()).collect();
                let in_both = files.len() - missing.len();
                out.push(PlaylistCompare {
                    name: p.name.clone(),
                    folder: Some(folder.clone()),
                    state: if missing.is_empty() && only.is_empty() {
                        PlaylistState::Same
                    } else {
                        PlaylistState::Differs
                    },
                    library_tracks: files.len(),
                    stick_tracks: p.track_ids.len(),
                    in_both,
                    missing_on_stick: missing,
                    only_on_stick: only,
                });
            }
            None => out.push(PlaylistCompare {
                name: p.name.clone(),
                folder: None,
                state: PlaylistState::StickOnly,
                library_tracks: 0,
                stick_tracks: p.track_ids.len(),
                in_both: 0,
                missing_on_stick: Vec::new(),
                only_on_stick: Vec::new(),
            }),
        }
    }
    for (fi, (folder, files)) in folders.iter().enumerate() {
        if paired[fi] {
            continue;
        }
        out.push(PlaylistCompare {
            name: folder_name(folder).to_string(),
            folder: Some(folder.clone()),
            state: PlaylistState::LibraryOnly,
            library_tracks: files.len(),
            stick_tracks: 0,
            in_both: 0,
            missing_on_stick: files.iter().map(|&i| library[i].path.clone()).collect(),
            only_on_stick: Vec::new(),
        });
    }

    let mut member_of: HashMap<u32, Vec<String>> = HashMap::new();
    for p in playlists {
        for id in &p.track_ids {
            member_of.entry(*id).or_default().push(p.name.clone());
        }
    }
    let orphans = stick
        .iter()
        .zip(&matches)
        .filter(|(_, m)| m.0.is_empty())
        .map(|(t, _)| Orphan {
            id: t.id,
            title: t.title.clone(),
            artist: t.artist.clone(),
            path: t.path.clone(),
            playlists: member_of.get(&t.id).cloned().unwrap_or_default(),
        })
        .collect();
    Comparison { playlists: out, orphans, stick_tracks: stick.len(), matched_by_size, matched_by_name }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib(path: &str, size: u64) -> LibraryFile {
        LibraryFile { path: path.into(), size, title: None, artist: None }
    }
    fn st(id: u32, title: &str, artist: &str, path: &str, size: u64) -> StickFile {
        StickFile { id, title: title.into(), artist: Some(artist.into()), path: path.into(), size: Some(size) }
    }

    #[test]
    fn names_compare_without_numbers_copies_or_punctuation() {
        assert_eq!(name_key("DJ/psy/01. Force Mass Motion - The Pressor-1.mp3"), "force mass motion the pressor");
        assert_eq!(name_key("Force Mass Motion - The Pressor"), "force mass motion the pressor");
        assert_eq!(name_key("Curses - Gold & Silber.mp3"), "curses gold silber");
        assert_eq!(
            name_key("12 Inch.flac"),
            "12 inch",
            "a title that starts with a number keeps it when no separator follows"
        );
    }

    #[test]
    fn playlists_pair_by_name_and_tracks_by_size_then_name() {
        let library = vec![
            lib("DJ/warmup/01. A - One.mp3", 100),
            lib("DJ/warmup/B - Two.mp3", 200),
            lib("DJ/peak/C - Three.mp3", 300),
            lib("DJ/new/D - Four.mp3", 400),
        ];
        let stick = vec![
            st(1, "One", "A", "Contents/A/x/One.mp3", 100),
            // Retagged on the stick: size differs, the name still pairs it.
            st(2, "Two", "B", "Contents/B/y/Two.mp3", 205),
            st(3, "Three", "C", "Contents/C/z/Three.mp3", 300),
            // On the stick only.
            st(4, "Lost", "E", "Contents/E/Lost.mp3", 999),
        ];
        let playlists = vec![
            StickPlaylist { name: "Warmup".into(), track_ids: vec![1, 2] },
            StickPlaylist { name: "peak ".into(), track_ids: vec![3, 4] },
            StickPlaylist { name: "Old".into(), track_ids: vec![4] },
        ];
        let c = compare(&library, &stick, &playlists);
        assert_eq!((c.matched_by_size, c.matched_by_name), (2, 1));
        let get = |n: &str| c.playlists.iter().find(|p| p.name == n).unwrap();
        assert_eq!(get("Warmup").state, PlaylistState::Same);
        assert_eq!(get("Warmup").folder.as_deref(), Some("DJ/warmup"));
        let peak = get("peak ");
        assert_eq!(peak.state, PlaylistState::Differs);
        assert_eq!(peak.only_on_stick, vec!["Lost".to_string()]);
        assert_eq!(get("Old").state, PlaylistState::StickOnly);
        let new = get("new");
        assert_eq!(
            (new.state, new.missing_on_stick.clone()),
            (PlaylistState::LibraryOnly, vec!["DJ/new/D - Four.mp3".to_string()])
        );
        assert_eq!(c.orphans.len(), 1);
        assert_eq!(c.orphans[0].title, "Lost");
        assert_eq!(c.orphans[0].playlists, vec!["peak ".to_string(), "Old".to_string()]);
    }

    #[test]
    fn a_track_in_several_folders_counts_in_the_playlists_own_folder() {
        let library = vec![lib("DJ/hypno/Push - Universal.mp3", 7), lib("DJ/trance/Push - Universal.mp3", 7)];
        let stick = vec![st(1, "Universal", "Push", "Contents/Push/Universal.mp3", 7)];
        let c = compare(&library, &stick, &[StickPlaylist { name: "Tran ce".into(), track_ids: vec![1] }]);
        let p = c.playlists.iter().find(|p| p.name == "Tran ce").unwrap();
        assert_eq!(p.folder.as_deref(), Some("DJ/trance"), "names pair ignoring case and spaces");
        assert_eq!((p.state, p.in_both), (PlaylistState::Same, 1));
        assert!(c.orphans.is_empty());
    }

    #[test]
    fn a_folder_holds_the_tracks_of_its_subfolders_as_an_export_does() {
        let library = vec![lib("src|Ben Klock/Subzero/A.mp3", 1), lib("src|Ben Klock/Subzero/B.mp3", 2)];
        let stick = vec![st(1, "A", "x", "Contents/A.mp3", 1), st(2, "B", "x", "Contents/B.mp3", 2)];
        let c = compare(&library, &stick, &[StickPlaylist { name: "Ben Klock".into(), track_ids: vec![1, 2] }]);
        let p = c.playlists.iter().find(|p| p.name == "Ben Klock").unwrap();
        assert_eq!((p.state, p.folder.as_deref(), p.in_both), (PlaylistState::Same, Some("src|Ben Klock"), 2));
    }

    #[test]
    fn a_folder_missing_tracks_on_the_stick_differs() {
        let library = vec![lib("DJ/set/A.mp3", 1), lib("DJ/set/B.mp3", 2)];
        let stick = vec![st(1, "A", "x", "Contents/A.mp3", 1)];
        let c = compare(&library, &stick, &[StickPlaylist { name: "set".into(), track_ids: vec![1] }]);
        let p = &c.playlists[0];
        assert_eq!(
            (p.state, p.in_both, p.missing_on_stick.clone()),
            (PlaylistState::Differs, 1, vec!["DJ/set/B.mp3".to_string()])
        );
    }
}
