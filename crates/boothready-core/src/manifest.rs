//! The small BoothReady manifest kept on each prepared drive.
//!
//! It holds preparation metadata and, after BoothReady copies files onto the
//! drive, each copied file's path, size and hash so verification can prove
//! the copy. The paths are ones the drive already shows to anyone holding
//! it, and none of this leaves the computer. It lives in a hidden
//! `.boothready` folder that DJ players ignore, and it's always replaced
//! atomically so a yanked drive never ends up with half a manifest.

use crate::planner::Role;
use boothready_model::{FilesystemKind, PartitionScheme};
use serde::{Deserialize, Serialize};
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MANIFEST_DIR: &str = ".boothready";
pub const MANIFEST_FILE: &str = ".boothready/manifest.json";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PreparationState {
    /// Written before the first change to the drive and cleared at the end,
    /// so an interrupted preparation is detectable on the next insert.
    InProgress {
        step: String,
        started_unix: u64,
    },
    Complete {
        finished_unix: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    /// BLAKE3, hex.
    pub hash: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VerifyMode {
    Quick,
    Full,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerificationRecord {
    pub mode: VerifyMode,
    pub completed_unix: u64,
    pub passed: bool,
    /// Fingerprint of the drive's contents when verification finished.
    pub content_fingerprint: String,
    pub files_checked: u32,
    pub bytes_checked: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub schema: u32,
    pub media_uuid: String,
    pub role: Option<Role>,
    pub profile: Option<String>,
    pub targets: Vec<String>,
    pub app_version: String,
    pub scheme: Option<PartitionScheme>,
    pub filesystem: Option<FilesystemKind>,
    pub source_library_revision: Option<String>,
    pub preparation: PreparationState,
    #[serde(default)]
    pub files: Vec<ManifestFile>,
    pub verification: Option<VerificationRecord>,
}

pub fn now_unix() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Manifest {
    pub fn new(role: Option<Role>, targets: Vec<String>) -> Manifest {
        Manifest {
            schema: 1,
            media_uuid: uuid::Uuid::new_v4().to_string(),
            role,
            profile: None,
            targets,
            app_version: env!("CARGO_PKG_VERSION").to_string(),
            scheme: None,
            filesystem: None,
            source_library_revision: None,
            preparation: PreparationState::InProgress { step: "starting".into(), started_unix: now_unix() },
            files: vec![],
            verification: None,
        }
    }

    pub fn is_interrupted(&self) -> bool {
        matches!(self.preparation, PreparationState::InProgress { .. })
    }
}

/// Read the manifest from a mounted volume, if there is a valid one.
pub fn read_manifest(root: &Path) -> Option<Manifest> {
    let text = fs::read_to_string(root.join(MANIFEST_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Write the manifest atomically: temp file, fsync, rename, fsync directory.
pub fn write_manifest(root: &Path, m: &Manifest) -> io::Result<()> {
    let dir = root.join(MANIFEST_DIR);
    fs::create_dir_all(&dir)?;
    let tmp = dir.join("manifest.json.tmp");
    let json = serde_json::to_vec_pretty(m).map_err(io::Error::other)?;
    {
        let mut f = File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, root.join(MANIFEST_FILE))?;
    #[cfg(unix)]
    {
        if let Ok(d) = File::open(&dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Cheap fingerprint of everything on the drive that matters for playback:
/// every file's path, size and modification time. Any copy, delete, edit or
/// re-export changes it, which invalidates an earlier verification.
pub fn content_fingerprint(root: &Path) -> String {
    let mut entries: Vec<(String, u64, u64)> = walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !crate::library::is_ignored_dir(e))
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file())
        .filter(|e| !e.file_name().to_string_lossy().starts_with("._"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            let mtime = m.modified().ok()?.duration_since(UNIX_EPOCH).ok()?.as_secs();
            Some((crate::library::relative(root, e.path()), m.len(), mtime))
        })
        .collect();
    entries.sort();
    let mut h = blake3::Hasher::new();
    for (p, size, mtime) in entries {
        h.update(p.as_bytes());
        h.update(&[0]);
        h.update(&size.to_le_bytes());
        h.update(&mtime.to_le_bytes());
    }
    h.finalize().to_hex().to_string()
}

/// Whether a drive still counts as verified: it must have a passing
/// verification and nothing may have changed since.
pub fn verification_still_valid(root: &Path, m: &Manifest) -> bool {
    match &m.verification {
        Some(v) if v.passed && !m.is_interrupted() => v.content_fingerprint == content_fingerprint(root),
        _ => false,
    }
}

pub fn manifest_path(root: &Path) -> PathBuf {
    root.join(MANIFEST_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_and_fingerprint_invalidation() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join("Contents")).unwrap();
        fs::write(d.path().join("Contents/a.mp3"), b"abc").unwrap();
        let mut m = Manifest::new(Some(Role::Main), vec!["cdj-3000".into()]);
        assert!(m.is_interrupted());
        write_manifest(d.path(), &m).unwrap();
        assert_eq!(read_manifest(d.path()).unwrap(), m);
        assert!(!d.path().join(".boothready/manifest.json.tmp").exists());

        m.preparation = PreparationState::Complete { finished_unix: now_unix() };
        m.verification = Some(VerificationRecord {
            mode: VerifyMode::Full,
            completed_unix: now_unix(),
            passed: true,
            content_fingerprint: content_fingerprint(d.path()),
            files_checked: 1,
            bytes_checked: 3,
        });
        write_manifest(d.path(), &m).unwrap();
        // Writing the manifest itself must not invalidate verification.
        assert!(verification_still_valid(d.path(), &read_manifest(d.path()).unwrap()));
        fs::write(d.path().join("Contents/b.mp3"), b"new").unwrap();
        assert!(!verification_still_valid(d.path(), &m));
    }

    #[test]
    fn corrupt_manifest_is_ignored() {
        let d = tempfile::tempdir().unwrap();
        fs::create_dir_all(d.path().join(MANIFEST_DIR)).unwrap();
        fs::write(d.path().join(MANIFEST_FILE), b"{ half a manif").unwrap();
        assert!(read_manifest(d.path()).is_none());
    }
}
