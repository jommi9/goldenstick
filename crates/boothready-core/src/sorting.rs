//! Sorting a music library whose folders are its playlists: add a track to a
//! playlist (copy it into that folder), move it (inbox triage), remove it, and
//! undo. A track may sit in several folders, one copy in each.
//!
//! Nothing is ever deleted. Removing a track moves it into
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

/// Make a new playlist folder.
pub fn new_folder(root: &Path, folder: &str) -> Result<()> {
    let rel = checked(folder)?;
    fs::create_dir_all(root.join(rel)).map_err(io(format!("making {folder}")))
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
/// a move or removal is moved back where it was. A file that has gone, or a
/// place that has been taken since, is left alone. Returns what undoing did.
pub fn undo(root: &Path, changes: &[Change], date: &str) -> Result<Vec<Change>> {
    let mut out = Vec::new();
    for c in changes.iter().rev() {
        let to = root.join(checked(&c.to)?);
        let from = root.join(checked(&c.from)?);
        if !to.is_file() {
            continue;
        }
        match c.kind {
            ChangeKind::Copied => out.extend(remove(root, std::slice::from_ref(&c.to), date)?),
            ChangeKind::Moved | ChangeKind::Removed => {
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
/// file, a move renames it, a removal drops it. Dropped entries are kept in
/// `set_aside` under their removed path, so undoing a removal brings the
/// entry back.
pub fn apply_to_scan(scan: &mut SourceScan, changes: &[Change], set_aside: &mut HashMap<String, SourceTrack>) {
    for c in changes {
        let inside = |p: &str| !p.starts_with(REMOVED_DIR);
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
        }
    }
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
