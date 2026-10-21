//! Sorting a music library whose folders are its playlists: add a track to a
//! playlist (copy it into that folder), move it (inbox triage), remove it, and
//! undo. A track may sit in several folders, one copy in each. The folders
//! themselves are made, renamed, moved and removed the way a file manager
//! does it, and those changes undo too.
//!
//! Nothing is ever deleted. Removing a track or a folder moves it into
//! [`REMOVED_DIR`]`/<date>/` inside the library, where it keeps its path, so
//! it can be restored by hand or by [`undo`]; undoing a copy moves the copy
//! there too. Moves are renames, so a file still in the cloud is moved without
//! being downloaded; a copy has to read the file, which makes the cloud client
//! download it first.

use crate::sources::{Availability, SourceScan, SourceTrack};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Component, Path, PathBuf};

/// The folder removed tracks go to, at the top of the library. Library scans skip it.
pub const REMOVED_DIR: &str = "_Removed by BoothReady";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    Copied,
    Moved,
    /// Moved into the removed folder.
    Removed,
    /// A playlist folder was made; `from` is empty.
    FolderMade,
    /// A folder was renamed or moved, with everything below it.
    FolderMoved,
    /// A folder was moved into the removed folder, or taken away again
    /// while still empty, in which case `to` is empty.
    FolderRemoved,
}

/// One file change, with paths relative to the library root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Change {
    pub kind: ChangeKind,
    pub from: String,
    pub to: String,
}

#[derive(Debug, thiserror::Error)]
pub enum SortError {
    #[error("{0} isn't a path inside the library")]
    Path(String),
    #[error("{0} isn't in the library any more")]
    Missing(String),
    #[error("there is already a folder called {0}")]
    Taken(String),
    #[error("{0}")]
    Name(String),
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

type Result<T> = std::result::Result<T, SortError>;

fn io(context: impl Into<String>) -> impl FnOnce(std::io::Error) -> SortError {
    move |source| SortError::Io { context: context.into(), source }
}

/// A relative path made only of plain names, never the removed folder itself.
fn checked(rel: &str) -> Result<&Path> {
    let p = Path::new(rel);
    if rel.is_empty() || !p.components().all(|c| matches!(c, Component::Normal(_))) {
        return Err(SortError::Path(rel.to_string()));
    }
    Ok(p)
}

fn rel_string(p: &Path) -> String {
    p.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/")
}

/// `dir/name`, or `dir/name (2)`, `(3)` and so on when that is taken.
fn free_name(dir: &Path, name: &OsStr) -> PathBuf {
    let first = dir.join(name);
    if !first.exists() {
        return first;
    }
    let p = Path::new(name);
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    (2..).map(|n| dir.join(format!("{stem} ({n}){ext}"))).find(|c| !c.exists()).unwrap_or(first)
}

fn same_size(a: &Path, b: &Path) -> bool {
    matches!((fs::metadata(a), fs::metadata(b)), (Ok(x), Ok(y)) if x.len() == y.len())
}

/// Make a new playlist folder. Its parents are made as needed.
pub fn new_folder(root: &Path, folder: &str) -> Result<Vec<Change>> {
    let rel = checked(folder)?;
    folder_name_ok(rel.file_name().map(|n| n.to_string_lossy()).as_deref().unwrap_or(""))?;
    let dir = root.join(rel);
    if dir.exists() {
        return Err(SortError::Taken(rel_string(rel)));
    }
    fs::create_dir_all(&dir).map_err(io(format!("making {folder}")))?;
    Ok(vec![Change { kind: ChangeKind::FolderMade, from: String::new(), to: rel_string(rel) }])
}

/// A name for one folder: no slashes, not empty, not the removed folder.
fn folder_name_ok(name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name == "." || name == ".." {
        return Err(SortError::Name("Give the playlist a name.".into()));
    }
    if name.contains('/') || name.contains('\\') {
        return Err(SortError::Name("A playlist name can't contain a slash.".into()));
    }
    if name == REMOVED_DIR {
        return Err(SortError::Name(format!("{REMOVED_DIR} is where removed tracks go.")));
    }
    Ok(())
}

/// The same place on disk under a different spelling of the name, which a
/// case-insensitive file system reports as existing already.
fn same_place(a: &Path, b: &Path) -> bool {
    a.parent() == b.parent()
        && matches!((a.file_name(), b.file_name()), (Some(x), Some(y)) if x.to_string_lossy().to_lowercase() == y.to_string_lossy().to_lowercase())
}

/// Give the folder a new name, keeping it where it is.
pub fn rename_folder(root: &Path, folder: &str, name: &str) -> Result<Vec<Change>> {
    let rel = checked(folder)?;
    let name = name.trim();
    folder_name_ok(name)?;
    let from = root.join(rel);
    if !from.is_dir() {
        return Err(SortError::Missing(folder.to_string()));
    }
    let to = from.with_file_name(name);
    if from == to {
        return Ok(Vec::new());
    }
    if to.exists() && !same_place(&from, &to) {
        return Err(SortError::Taken(name.to_string()));
    }
    fs::rename(&from, &to).map_err(io(format!("renaming {folder}")))?;
    Ok(vec![Change {
        kind: ChangeKind::FolderMoved,
        from: rel_string(rel),
        to: rel_string(to.strip_prefix(root).unwrap_or(&to)),
    }])
}

/// Move folders into `into`, which may be empty for the top of the library.
/// A folder can't go into itself or below itself, and a folder already
/// called the same in the destination stops the move.
pub fn move_folders(root: &Path, folders: &[String], into: &str) -> Result<Vec<Change>> {
    let dir = if into.is_empty() { root.to_path_buf() } else { root.join(checked(into)?) };
    if !dir.is_dir() {
        return Err(SortError::Missing(into.to_string()));
    }
    let mut out = Vec::new();
    for folder in folders {
        let rel = checked(folder)?;
        if into == folder || into.starts_with(&format!("{folder}/")) {
            return Err(SortError::Name(format!("{} can't be moved into itself.", rel_string(rel))));
        }
        let from = root.join(rel);
        if !from.is_dir() {
            return Err(SortError::Missing(folder.clone()));
        }
        let name = from.file_name().ok_or_else(|| SortError::Path(folder.clone()))?.to_owned();
        let to = dir.join(&name);
        if from == to {
            continue;
        }
        if to.exists() {
            return Err(SortError::Taken(name.to_string_lossy().into_owned()));
        }
        fs::rename(&from, &to).map_err(io(format!("moving {folder}")))?;
        out.push(Change {
            kind: ChangeKind::FolderMoved,
            from: rel_string(rel),
            to: rel_string(to.strip_prefix(root).unwrap_or(&to)),
        });
    }
    Ok(out)
}

/// Move folders into the removed folder, keeping their paths under a dated folder.
pub fn remove_folders(root: &Path, folders: &[String], date: &str) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    for folder in folders {
        let rel = checked(folder)?;
        let from = root.join(rel);
        if !from.is_dir() {
            return Err(SortError::Missing(folder.clone()));
        }
        let target_dir = root.join(REMOVED_DIR).join(date).join(rel.parent().unwrap_or(Path::new("")));
        fs::create_dir_all(&target_dir).map_err(io("making the removed folder"))?;
        let name = from.file_name().ok_or_else(|| SortError::Path(folder.clone()))?.to_owned();
        let to = free_name(&target_dir, &name);
        fs::rename(&from, &to).map_err(io(format!("removing {folder}")))?;
        out.push(Change {
            kind: ChangeKind::FolderRemoved,
            from: rel_string(rel),
            to: rel_string(to.strip_prefix(root).unwrap_or(&to)),
        });
    }
    Ok(out)
}

/// Copy tracks into `folder`. A track already there under the same name and
/// size is left out; a different file with that name keeps its name and the
/// copy gets a numbered one.
pub fn add_to_folder(root: &Path, paths: &[String], folder: &str) -> Result<Vec<Change>> {
    let dir = root.join(checked(folder)?);
    fs::create_dir_all(&dir).map_err(io(format!("making {folder}")))?;
    let mut out = Vec::new();
    for path in paths {
        let from = root.join(checked(path)?);
        let name = from.file_name().ok_or_else(|| SortError::Path(path.clone()))?.to_owned();
        if !from.is_file() {
            return Err(SortError::Missing(path.clone()));
        }
        let existing = dir.join(&name);
        if existing.exists() && same_size(&from, &existing) {
            continue;
        }
        let to = free_name(&dir, &name);
        fs::copy(&from, &to).map_err(io(format!("copying {path}")))?;
        let to_rel = rel_string(to.strip_prefix(root).unwrap_or(&to));
        out.push(Change { kind: ChangeKind::Copied, from: path.clone(), to: to_rel });
    }
    Ok(out)
}

/// Move tracks into `folder`. When the folder already holds the same track
/// (same name and size), the moved copy goes to the removed folder instead,
/// so the track ends up there once.
pub fn move_to_folder(root: &Path, paths: &[String], folder: &str, date: &str) -> Result<Vec<Change>> {
    let dir = root.join(checked(folder)?);
    fs::create_dir_all(&dir).map_err(io(format!("making {folder}")))?;
    let mut out = Vec::new();
    for path in paths {
        let rel = checked(path)?;
        let from = root.join(rel);
        if !from.is_file() {
            return Err(SortError::Missing(path.clone()));
        }
        if from.parent() == Some(dir.as_path()) {
            continue;
        }
        let name = from.file_name().ok_or_else(|| SortError::Path(path.clone()))?.to_owned();
        let existing = dir.join(&name);
        if existing.exists() && same_size(&from, &existing) {
            out.extend(remove(root, std::slice::from_ref(path), date)?);
            continue;
        }
        let to = free_name(&dir, &name);
        fs::rename(&from, &to).map_err(io(format!("moving {path}")))?;
        out.push(Change {
            kind: ChangeKind::Moved,
            from: path.clone(),
            to: rel_string(to.strip_prefix(root).unwrap_or(&to)),
        });
    }
    Ok(out)
}

/// Move tracks into the removed folder, keeping their paths under a dated folder.
pub fn remove(root: &Path, paths: &[String], date: &str) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    for path in paths {
        let rel = checked(path)?;
        let from = root.join(rel);
        if !from.is_file() {
            return Err(SortError::Missing(path.clone()));
        }
        let target_dir = root.join(REMOVED_DIR).join(date).join(rel.parent().unwrap_or(Path::new("")));
        fs::create_dir_all(&target_dir).map_err(io("making the removed folder"))?;
        let name = from.file_name().ok_or_else(|| SortError::Path(path.clone()))?.to_owned();
        let to = free_name(&target_dir, &name);
        fs::rename(&from, &to).map_err(io(format!("removing {path}")))?;
        out.push(Change {
            kind: ChangeKind::Removed,
            from: path.clone(),
            to: rel_string(to.strip_prefix(root).unwrap_or(&to)),
        });
    }
    Ok(out)
}

/// Reverse `changes`, newest first. A copy is moved to the removed folder,
/// a move or removal is moved back where it was, a made folder is taken away
/// while it is still empty. A file that has gone, or a place that has been
/// taken since, is left alone. Returns what undoing did.
pub fn undo(root: &Path, changes: &[Change], date: &str) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    for c in changes.iter().rev() {
        match c.kind {
            ChangeKind::FolderMade => {
                let dir = root.join(checked(&c.to)?);
                let empty = fs::read_dir(&dir).map(|mut d| d.next().is_none()).unwrap_or(false);
                if dir.is_dir() && empty {
                    fs::remove_dir(&dir).map_err(io(format!("taking {} away again", c.to)))?;
                    out.push(Change { kind: ChangeKind::FolderRemoved, from: c.to.clone(), to: String::new() });
                }
            }
            ChangeKind::FolderMoved | ChangeKind::FolderRemoved => {
                if c.to.is_empty() {
                    continue;
                }
                let to = root.join(checked(&c.to)?);
                let from = root.join(checked(&c.from)?);
                if !to.is_dir() || from.exists() {
                    continue;
                }
                if let Some(parent) = from.parent() {
                    fs::create_dir_all(parent).map_err(io(format!("restoring {}", c.from)))?;
                }
                fs::rename(&to, &from).map_err(io(format!("restoring {}", c.from)))?;
                out.push(Change { kind: ChangeKind::FolderMoved, from: c.to.clone(), to: c.from.clone() });
            }
            ChangeKind::Copied | ChangeKind::Moved | ChangeKind::Removed => {
                let to = root.join(checked(&c.to)?);
                let from = root.join(checked(&c.from)?);
                if !to.is_file() {
                    continue;
                }
                if c.kind == ChangeKind::Copied {
                    out.extend(remove(root, std::slice::from_ref(&c.to), date)?);
                    continue;
                }
                if from.exists() {
                    continue;
                }
                if let Some(parent) = from.parent() {
                    fs::create_dir_all(parent).map_err(io(format!("restoring {}", c.from)))?;
                }
                fs::rename(&to, &from).map_err(io(format!("restoring {}", c.from)))?;
                out.push(Change { kind: ChangeKind::Moved, from: c.to.clone(), to: c.from.clone() });
            }
        }
    }
    Ok(out)
}

/// Bring a saved scan up to date with `changes`, without walking the library
/// again: a copy repeats the track's entry under its new path as a local
/// file, a move renames it, a removal drops it, and a folder change does the
/// same to every track and folder below it. Dropped entries are kept in
/// `set_aside` under their removed path, so undoing a removal brings the
/// entry back.
pub fn apply_to_scan(scan: &mut SourceScan, changes: &[Change], set_aside: &mut HashMap<String, SourceTrack>) {
    for c in changes {
        let inside = |p: &str| !p.starts_with(REMOVED_DIR);
        if matches!(c.kind, ChangeKind::FolderMade | ChangeKind::FolderMoved | ChangeKind::FolderRemoved) {
            apply_folder_change(scan, c, set_aside);
            continue;
        }
        let at = scan.tracks.iter().position(|t| t.path == c.from);
        match c.kind {
            ChangeKind::Copied => {
                if let Some(i) = at {
                    let mut t = scan.tracks[i].clone();
                    t.path = c.to.clone();
                    t.availability = Availability::Local;
                    if inside(&c.to) {
                        scan.tracks.push(t);
                    }
                }
            }
            ChangeKind::Moved | ChangeKind::Removed => {
                if let Some(i) = at {
                    if inside(&c.to) {
                        scan.tracks[i].path = c.to.clone();
                    } else {
                        set_aside.insert(c.to.clone(), scan.tracks.remove(i));
                    }
                } else if let Some(mut t) = set_aside.remove(&c.from) {
                    if inside(&c.to) {
                        t.path = c.to.clone();
                        scan.tracks.push(t);
                    } else {
                        set_aside.insert(c.to.clone(), t);
                    }
                }
            }
            ChangeKind::FolderMade | ChangeKind::FolderMoved | ChangeKind::FolderRemoved => {}
        }
    }
    scan.tracks.sort_by(|a, b| a.path.cmp(&b.path));
}

/// Re-root everything below a folder that was made, moved, renamed or
/// removed. Tracks that leave the library are set aside under their new
/// path; tracks set aside under the folder's old path come back.
fn apply_folder_change(scan: &mut SourceScan, c: &Change, set_aside: &mut HashMap<String, SourceTrack>) {
    let inside = |p: &str| !p.is_empty() && !p.starts_with(REMOVED_DIR);
    if c.kind == ChangeKind::FolderMade {
        if inside(&c.to) && !scan.folders.contains(&c.to) {
            scan.folders.push(c.to.clone());
            scan.folders.sort();
        }
        return;
    }
    let prefix = format!("{}/", c.from);
    let gone = !inside(&c.to);
    let moved = |p: &str| format!("{}/{}", c.to, &p[prefix.len()..]);
    let mut i = 0;
    while i < scan.tracks.len() {
        if !scan.tracks[i].path.starts_with(&prefix) {
            i += 1;
            continue;
        }
        if gone {
            let t = scan.tracks.remove(i);
            if !c.to.is_empty() {
                set_aside.insert(moved(&t.path), t);
            }
        } else {
            let to = moved(&scan.tracks[i].path);
            scan.tracks[i].path = to;
            i += 1;
        }
    }
    if !gone {
        let back: Vec<String> = set_aside.keys().filter(|k| k.starts_with(&prefix)).cloned().collect();
        for key in back {
            if let Some(mut t) = set_aside.remove(&key) {
                t.path = moved(&key);
                scan.tracks.push(t);
            }
        }
    }
    let mut folders = Vec::with_capacity(scan.folders.len() + 1);
    for f in scan.folders.drain(..) {
        if f == c.from || f.starts_with(&prefix) {
            if !gone {
                folders.push(if f == c.from { c.to.clone() } else { moved(&f) });
            }
        } else {
            folders.push(f);
        }
    }
    if !gone && !folders.contains(&c.to) {
        folders.push(c.to.clone());
    }
    folders.sort();
    folders.dedup();
    scan.folders = folders;
    scan.tracks.sort_by(|a, b| a.path.cmp(&b.path));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lib() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (p, bytes) in [
            ("00 INBOX/A - One.mp3", &b"one"[..]),
            ("00 INBOX/B - Two.mp3", b"two!"),
            ("DJ/peak/C - Three.mp3", b"three"),
            ("DJ/warmup/B - Two.mp3", b"two!"),
        ] {
            let f = d.path().join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, bytes).unwrap();
        }
        d
    }
    const DAY: &str = "2026-10-20";

    #[test]
    fn adding_copies_into_the_folder_and_skips_what_is_already_there() {
        let d = lib();
        let ch = add_to_folder(d.path(), &["DJ/peak/C - Three.mp3".into(), "00 INBOX/B - Two.mp3".into()], "DJ/warmup")
            .unwrap();
        assert_eq!(
            ch,
            vec![Change {
                kind: ChangeKind::Copied,
                from: "DJ/peak/C - Three.mp3".into(),
                to: "DJ/warmup/C - Three.mp3".into()
            }]
        );
        assert!(d.path().join("DJ/peak/C - Three.mp3").exists(), "the original stays");
        assert_eq!(fs::read(d.path().join("DJ/warmup/C - Three.mp3")).unwrap(), b"three");
    }

    #[test]
    fn moving_triages_the_inbox_and_never_overwrites() {
        let d = lib();
        fs::write(d.path().join("DJ/peak/A - One.mp3"), b"other one").unwrap();
        let ch =
            move_to_folder(d.path(), &["00 INBOX/A - One.mp3".into(), "00 INBOX/B - Two.mp3".into()], "DJ/peak", DAY)
                .unwrap();
        assert_eq!(
            ch[0],
            Change {
                kind: ChangeKind::Moved,
                from: "00 INBOX/A - One.mp3".into(),
                to: "DJ/peak/A - One (2).mp3".into()
            }
        );
        assert_eq!(
            fs::read(d.path().join("DJ/peak/A - One.mp3")).unwrap(),
            b"other one",
            "a different file with the name is kept"
        );
        assert!(!d.path().join("00 INBOX/A - One.mp3").exists());
        assert_eq!(ch[1].kind, ChangeKind::Moved);
        // Moving a track into a folder that already holds it removes the extra copy.
        let again = move_to_folder(d.path(), &["DJ/warmup/B - Two.mp3".into()], "DJ/peak", DAY).unwrap();
        assert_eq!(again[0].kind, ChangeKind::Removed);
        assert!(d.path().join(REMOVED_DIR).join(DAY).join("DJ/warmup/B - Two.mp3").exists());
    }

    #[test]
    fn removing_moves_aside_and_undo_puts_everything_back() {
        let d = lib();
        let mut log = remove(d.path(), &["DJ/peak/C - Three.mp3".into()], DAY).unwrap();
        assert!(!d.path().join("DJ/peak/C - Three.mp3").exists());
        assert!(
            d.path().join(REMOVED_DIR).join(DAY).join("DJ/peak/C - Three.mp3").exists(),
            "removal keeps the file and its path"
        );
        log.extend(add_to_folder(d.path(), &["00 INBOX/A - One.mp3".into()], "DJ/peak").unwrap());
        log.extend(move_to_folder(d.path(), &["00 INBOX/B - Two.mp3".into()], "DJ/new", DAY).unwrap());
        undo(d.path(), &log, DAY).unwrap();
        assert!(d.path().join("DJ/peak/C - Three.mp3").exists(), "the removal is restored");
        assert!(d.path().join("00 INBOX/B - Two.mp3").exists(), "the move is reversed");
        assert!(!d.path().join("DJ/peak/A - One.mp3").exists(), "the copy is taken out again");
        assert!(
            d.path().join(REMOVED_DIR).join(DAY).join("DJ/peak/A - One.mp3").exists(),
            "and kept in the removed folder"
        );
        assert!(d.path().join("00 INBOX/A - One.mp3").exists());
    }

    #[test]
    fn folders_are_made_renamed_moved_removed_and_undone() {
        let d = lib();
        let mut log = new_folder(d.path(), "DJ/new night").unwrap();
        assert!(d.path().join("DJ/new night").is_dir());
        assert!(matches!(new_folder(d.path(), "DJ/new night"), Err(SortError::Taken(_))));
        assert!(new_folder(d.path(), "DJ/a/b").is_ok(), "parents are made as needed");
        assert!(matches!(rename_folder(d.path(), "DJ/peak", "peak/time"), Err(SortError::Name(_))));
        assert!(matches!(rename_folder(d.path(), "DJ/peak", "warmup"), Err(SortError::Taken(_))));
        log.extend(rename_folder(d.path(), "DJ/peak", "Peak Time").unwrap());
        assert_eq!(log[1], Change { kind: ChangeKind::FolderMoved, from: "DJ/peak".into(), to: "DJ/Peak Time".into() });
        assert!(d.path().join("DJ/Peak Time/C - Three.mp3").is_file());
        assert!(matches!(move_folders(d.path(), &["DJ".into()], "DJ/warmup"), Err(SortError::Name(_))));
        log.extend(move_folders(d.path(), &["DJ/warmup".into()], "").unwrap());
        assert!(d.path().join("warmup/B - Two.mp3").is_file(), "moved to the top of the library");
        log.extend(move_folders(d.path(), &["warmup".into()], "DJ/Peak Time").unwrap());
        assert!(d.path().join("DJ/Peak Time/warmup/B - Two.mp3").is_file(), "and into another folder");
        log.extend(remove_folders(d.path(), &["00 INBOX".into()], DAY).unwrap());
        assert!(!d.path().join("00 INBOX").exists());
        assert!(d.path().join(REMOVED_DIR).join(DAY).join("00 INBOX/A - One.mp3").is_file());
        let undone = undo(d.path(), &log, DAY).unwrap();
        assert_eq!(undone.len(), 5);
        assert!(d.path().join("00 INBOX/A - One.mp3").is_file(), "the removed folder is back");
        assert!(d.path().join("DJ/warmup/B - Two.mp3").is_file(), "both moves are reversed");
        assert!(d.path().join("DJ/peak/C - Three.mp3").is_file(), "the rename is reversed");
        assert!(!d.path().join("DJ/new night").exists(), "the empty new folder is taken away");
    }

    #[test]
    fn the_scan_follows_folder_changes() {
        let track = |p: &str| SourceTrack {
            path: p.into(),
            size: 3,
            modified_unix: None,
            availability: Availability::Local,
            probe: None,
            tags: None,
        };
        let mut scan = SourceScan {
            tracks: vec![track("DJ/peak/A.mp3"), track("DJ/peak/late/B.mp3"), track("DJ/warmup/C.mp3")],
            folders: vec!["DJ".into(), "DJ/peak".into(), "DJ/peak/late".into(), "DJ/warmup".into()],
            ..SourceScan::default()
        };
        let mut aside = HashMap::new();
        apply_to_scan(
            &mut scan,
            &[
                Change { kind: ChangeKind::FolderMade, from: String::new(), to: "DJ/new".into() },
                Change { kind: ChangeKind::FolderMoved, from: "DJ/peak".into(), to: "DJ/Peak Time".into() },
                Change {
                    kind: ChangeKind::FolderRemoved,
                    from: "DJ/warmup".into(),
                    to: format!("{REMOVED_DIR}/{DAY}/DJ/warmup"),
                },
            ],
            &mut aside,
        );
        let paths: Vec<&str> = scan.tracks.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(paths, vec!["DJ/Peak Time/A.mp3", "DJ/Peak Time/late/B.mp3"]);
        assert_eq!(scan.folders, vec!["DJ", "DJ/Peak Time", "DJ/Peak Time/late", "DJ/new"]);
        assert_eq!(aside.len(), 1, "the removed folder's track is set aside");
        // Undoing the removal brings the folder and its track back.
        apply_to_scan(
            &mut scan,
            &[Change {
                kind: ChangeKind::FolderMoved,
                from: format!("{REMOVED_DIR}/{DAY}/DJ/warmup"),
                to: "DJ/warmup".into(),
            }],
            &mut aside,
        );
        assert!(scan.tracks.iter().any(|t| t.path == "DJ/warmup/C.mp3"));
        assert!(scan.folders.contains(&"DJ/warmup".to_string()));
        assert!(aside.is_empty());
        // Undoing the made folder drops it from the list.
        apply_to_scan(
            &mut scan,
            &[Change { kind: ChangeKind::FolderRemoved, from: "DJ/new".into(), to: String::new() }],
            &mut aside,
        );
        assert!(!scan.folders.contains(&"DJ/new".to_string()));
    }

    #[test]
    fn paths_outside_the_library_are_refused() {
        let d = lib();
        assert!(matches!(add_to_folder(d.path(), &["../x.mp3".into()], "DJ/peak"), Err(SortError::Path(_))));
        assert!(matches!(
            move_to_folder(d.path(), &["00 INBOX/A - One.mp3".into()], "/etc", DAY),
            Err(SortError::Path(_))
        ));
        assert!(matches!(remove(d.path(), &["".into()], DAY), Err(SortError::Path(_))));
    }

    #[test]
    fn the_scan_follows_the_changes() {
        let track = |p: &str, a: Availability| SourceTrack {
            path: p.into(),
            size: 3,
            modified_unix: None,
            availability: a,
            probe: None,
            tags: None,
        };
        let mut scan = SourceScan {
            tracks: vec![track("00 INBOX/A.mp3", Availability::CloudOnly), track("DJ/x/B.mp3", Availability::Local)],
            ..SourceScan::default()
        };
        let mut set_aside = HashMap::new();
        apply_to_scan(
            &mut scan,
            &[
                Change { kind: ChangeKind::Copied, from: "00 INBOX/A.mp3".into(), to: "DJ/x/A.mp3".into() },
                Change { kind: ChangeKind::Moved, from: "00 INBOX/A.mp3".into(), to: "DJ/y/A.mp3".into() },
                Change {
                    kind: ChangeKind::Removed,
                    from: "DJ/x/B.mp3".into(),
                    to: format!("{REMOVED_DIR}/{DAY}/DJ/x/B.mp3"),
                },
            ],
            &mut set_aside,
        );
        let paths: Vec<(&str, Availability)> = scan.tracks.iter().map(|t| (t.path.as_str(), t.availability)).collect();
        assert_eq!(paths, vec![("DJ/x/A.mp3", Availability::Local), ("DJ/y/A.mp3", Availability::CloudOnly)]);
        // Undoing the removal brings the entry back from where it was set aside.
        let undo = Change {
            kind: ChangeKind::Moved,
            from: format!("{REMOVED_DIR}/{DAY}/DJ/x/B.mp3"),
            to: "DJ/x/B.mp3".into(),
        };
        apply_to_scan(&mut scan, &[undo], &mut set_aside);
        let paths: Vec<&str> = scan.tracks.iter().map(|t| t.path.as_str()).collect();
        assert_eq!(paths, vec!["DJ/x/A.mp3", "DJ/x/B.mp3", "DJ/y/A.mp3"]);
        assert!(set_aside.is_empty());
    }
}
