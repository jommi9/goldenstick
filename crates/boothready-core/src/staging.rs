//! Local collection staging. Callers must validate the destination's storage type.
//! Manifests contain private source paths and must remain outside version control.
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    Local,
    GoogleDriveDesktop,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SourceFile {
    pub path: PathBuf,
    pub bytes: u64,
    pub blake3: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Collection {
    pub version: u32,
    pub kind: SourceKind,
    pub source: PathBuf,
    pub revision: String,
    pub files: Vec<SourceFile>,
}
fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}

/// Reject symlinks in every existing component, including the selected root.
pub fn checked_path(path: &Path) -> io::Result<PathBuf> {
    let path = if path.is_absolute() { path.to_owned() } else { std::env::current_dir()?.join(path) };
    let mut result = PathBuf::new();
    for component in path.components() {
        if matches!(component, Component::ParentDir) {
            return Err(invalid("parent path components are forbidden"));
        }
        result.push(component);
        // A drive prefix or root on its own is not a file to inspect. Windows
        // answers "Incorrect function" for a bare verbatim prefix such as
        // `\\?\C:`, which is how every canonical path there begins.
        if !matches!(component, Component::Normal(_)) {
            continue;
        }
        match fs::symlink_metadata(&result) {
            Ok(m) if m.file_type().is_symlink() => return Err(invalid("symlinks are forbidden")),
            Ok(metadata) => {
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    if metadata.file_attributes() & 0x400 != 0 {
                        return Err(invalid("Windows reparse points are unsupported"));
                    }
                }
                #[cfg(not(windows))]
                let _ = metadata;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(result)
}
fn hash(path: &Path) -> io::Result<(u64, String)> {
    checked_path(path)?;
    if !fs::symlink_metadata(path)?.is_file() {
        return Err(invalid("only regular files are supported"));
    }
    let mut input = File::open(path)?;
    let mut digest = blake3::Hasher::new();
    let mut bytes = 0;
    let mut buffer = [0; 65536];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        digest.update(&buffer[..n]);
        bytes += n as u64;
    }
    Ok((bytes, digest.finalize().to_hex().to_string()))
}
/// Fully reads the selected directory, including cloud placeholders. Read errors abort.
pub fn inventory(source: &Path, kind: SourceKind) -> io::Result<Collection> {
    let source = checked_path(source)?.canonicalize()?;
    if !source.is_dir() {
        return Err(invalid("source must be a directory"));
    }
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(&source).follow_links(false) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            continue;
        }
        let (bytes, blake3) = hash(entry.path())?;
        files.push(SourceFile {
            path: entry.path().strip_prefix(&source).map_err(io::Error::other)?.to_owned(),
            bytes,
            blake3,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let revision = blake3::hash(&serde_json::to_vec(&files)?).to_hex().to_string();
    Ok(Collection { version: 1, kind, source, revision, files })
}
fn matches(path: &Path, file: &SourceFile) -> io::Result<bool> {
    Ok(hash(path)? == (file.bytes, file.blake3.clone()))
}
fn write_new(path: &Path, content: &[u8]) -> io::Result<()> {
    let mut f = OpenOptions::new().write(true).create_new(true).open(path)?;
    f.write_all(content)?;
    f.sync_all()
}
/// Rehash a completed collection without opening the original source.
/// Exporters must call this before consuming `files/`; a marker alone is insufficient.
pub fn verify_collection(destination: &Path) -> io::Result<Collection> {
    let destination = checked_path(destination)?;
    let plan: Collection = serde_json::from_reader(File::open(checked_path(&destination.join("collection.json"))?)?)?;
    let complete: Collection = serde_json::from_reader(File::open(checked_path(&destination.join("complete.json"))?)?)?;
    let actual = inventory(&destination.join("files"), SourceKind::Local)?;
    if plan.version != 1 || complete != plan || actual.files != plan.files || actual.revision != plan.revision {
        return Err(invalid("collection failed verification"));
    }
    Ok(plan)
}

/// Creates a dedicated collection or resumes an identical source snapshot.
/// Completed files are rehashed. Publication uses hard links so existing files
/// are never overwritten. Interrupted temporary files remain in `.work`.
/// Do not run concurrently with another writer or change directory mounts.
pub fn stage(source: &Path, kind: SourceKind, destination: &Path, cancel: &AtomicBool) -> io::Result<Collection> {
    let source = checked_path(source)?.canonicalize()?;
    let destination = checked_path(destination)?;
    let parent = destination.parent().ok_or_else(|| invalid("destination needs a parent"))?.canonicalize()?;
    let destination = parent.join(destination.file_name().ok_or_else(|| invalid("destination needs a name"))?);
    if source.starts_with(&destination) || destination.starts_with(&source) {
        return Err(invalid("source and destination overlap"));
    }
    let plan = inventory(&source, kind)?;
    let plan_path = destination.join("collection.json");
    match fs::create_dir(&destination) {
        Ok(()) => write_new(&plan_path, &serde_json::to_vec_pretty(&plan)?)?,
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
            checked_path(&plan_path)?;
            let old: Collection = serde_json::from_reader(File::open(&plan_path)?)?;
            if old != plan {
                return Err(invalid("source changed or destination belongs to another collection"));
            }
        }
        Err(e) => return Err(e),
    }
    for dir in ["files", ".work"] {
        let p = checked_path(&destination.join(dir))?;
        fs::create_dir_all(p)?;
    }
    // Unrecognized files are preserved and prevent publication as a verified collection.
    for entry in fs::read_dir(&destination)? {
        let entry = entry?;
        if !["collection.json", "complete.json", "files", ".work"].iter().any(|n| entry.file_name() == *n) {
            return Err(invalid("unrecognized collection content"));
        }
    }
    for entry in walkdir::WalkDir::new(destination.join("files")) {
        let entry = entry?;
        checked_path(entry.path())?;
        if entry.file_type().is_dir() {
            continue;
        }
        let relative = entry.path().strip_prefix(destination.join("files")).map_err(io::Error::other)?;
        if !plan.files.iter().any(|f| f.path == relative) {
            return Err(invalid("unrecognized staged file"));
        }
    }
    for file in &plan.files {
        if cancel.load(Ordering::Relaxed) {
            return Err(io::Error::new(io::ErrorKind::Interrupted, "staging cancelled; rerun to resume"));
        }
        let target = checked_path(&destination.join("files").join(&file.path))?;
        if target.try_exists()? {
            if !matches(&target, file)? {
                return Err(invalid("existing staged file failed verification"));
            }
            continue;
        }
        fs::create_dir_all(target.parent().ok_or_else(|| invalid("invalid target"))?)?;
        let temp = destination.join(".work").join(uuid::Uuid::new_v4().to_string());
        let mut output = OpenOptions::new().write(true).create_new(true).open(&temp)?;
        let input_path = checked_path(&source.join(&file.path))?;
        let mut input = File::open(input_path)?;
        let mut buffer = [0; 65536];
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(io::Error::new(io::ErrorKind::Interrupted, "staging cancelled; rerun to resume"));
            }
            let n = input.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            output.write_all(&buffer[..n])?;
        }
        output.sync_all()?;
        drop(output);
        if !matches(&temp, file)? {
            return Err(invalid("source changed during staging"));
        }
        fs::hard_link(&temp, &target)?;
        fs::remove_file(temp)?;
    }
    if inventory(&source, kind)? != plan {
        return Err(invalid("source changed during staging"));
    }
    for file in &plan.files {
        if !matches(&destination.join("files").join(&file.path), file)? {
            return Err(invalid("staged collection changed"));
        }
    }
    let complete = checked_path(&destination.join("complete.json"))?;
    if complete.try_exists()? {
        let old: Collection = serde_json::from_reader(File::open(complete)?)?;
        if old != plan {
            return Err(invalid("completion record does not match"));
        }
    } else {
        write_new(&complete, &serde_json::to_vec_pretty(&plan)?)?;
    }
    verify_collection(&destination)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn setup() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let src = root.path().canonicalize().unwrap().join("source");
        fs::create_dir(&src).unwrap();
        fs::write(src.join("track.wav"), b"synthetic audio fixture").unwrap();
        let dest = root.path().canonicalize().unwrap().join("collection");
        (root, src, dest)
    }
    #[test]
    fn stages_and_reverifies_resume() {
        let (_root, src, dest) = setup();
        let cancel = AtomicBool::new(false);
        let first = stage(&src, SourceKind::GoogleDriveDesktop, &dest, &cancel).unwrap();
        assert_eq!(first, stage(&src, SourceKind::GoogleDriveDesktop, &dest, &cancel).unwrap());
        fs::rename(&src, src.with_file_name("offline-source")).unwrap();
        assert_eq!(first, verify_collection(&dest).unwrap());
        fs::write(dest.join("files/track.wav"), b"corrupt").unwrap();
        assert!(verify_collection(&dest).is_err());
        assert!(stage(&src, SourceKind::GoogleDriveDesktop, &dest, &cancel).is_err());
    }
    #[test]
    fn interrupted_file_is_recopied_and_unexpected_content_is_rejected() {
        let (_root, src, dest) = setup();
        let cancel = AtomicBool::new(false);
        stage(&src, SourceKind::Local, &dest, &cancel).unwrap();
        fs::remove_file(dest.join("complete.json")).unwrap();
        fs::remove_file(dest.join("files/track.wav")).unwrap();
        fs::write(dest.join(".work/interrupted"), b"partial").unwrap();
        stage(&src, SourceKind::Local, &dest, &cancel).unwrap();
        assert_eq!(fs::read(dest.join("files/track.wav")).unwrap(), b"synthetic audio fixture");
        fs::write(dest.join("files/unexpected"), b"keep").unwrap();
        assert!(verify_collection(&dest).is_err());
        assert!(stage(&src, SourceKind::Local, &dest, &cancel).is_err());
        assert_eq!(fs::read(dest.join("files/unexpected")).unwrap(), b"keep");
    }
    #[test]
    fn cancelled_collection_resumes() {
        let (_root, src, dest) = setup();
        assert_eq!(
            stage(&src, SourceKind::Local, &dest, &AtomicBool::new(true)).unwrap_err().kind(),
            io::ErrorKind::Interrupted
        );
        assert!(!dest.join("complete.json").exists());
        stage(&src, SourceKind::Local, &dest, &AtomicBool::new(false)).unwrap();
    }
    #[test]
    fn refuses_source_changes_overlap_and_existing_user_folder() {
        let (_root, src, dest) = setup();
        let cancel = AtomicBool::new(false);
        assert!(stage(&src, SourceKind::Local, &src.join("nested"), &cancel).is_err());
        fs::create_dir(&dest).unwrap();
        fs::write(dest.join("personal.txt"), b"keep").unwrap();
        assert!(stage(&src, SourceKind::Local, &dest, &cancel).is_err());
        assert_eq!(fs::read(dest.join("personal.txt")).unwrap(), b"keep");
        let other = dest.with_file_name("other");
        stage(&src, SourceKind::Local, &other, &cancel).unwrap();
        fs::write(src.join("track.wav"), b"changed").unwrap();
        assert!(stage(&src, SourceKind::Local, &other, &cancel).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlink_sources_and_destination_ancestors() {
        let (root, src, dest) = setup();
        std::os::unix::fs::symlink(src.join("track.wav"), src.join("link")).unwrap();
        assert!(inventory(&src, SourceKind::Local).is_err());
        std::os::unix::fs::symlink(root.path(), &dest).unwrap();
        assert!(checked_path(&dest.join("collection")).is_err());
    }
}
