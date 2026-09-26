//! Verification: proving a prepared drive is what we think it is.
//!
//! Quick mode checks structure, databases and a spread of sampled files.
//! Full mode reads every audio file end to end (and checks hashes when the
//! manifest has them), which is what catches failing flash.

use crate::audio::probe_file;
use crate::library::{walk_audio_files, LibraryReport};
use crate::manifest::{content_fingerprint, now_unix, Manifest, VerificationRecord, VerifyMode};
use crate::media::MediaLayout;
use boothready_model::{FilesystemKind, PartitionScheme};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyCheck {
    pub name: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileFailure {
    pub path: String,
    pub problem: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VerifyReport {
    pub mode: VerifyMode,
    pub passed: bool,
    pub cancelled: bool,
    pub checks: Vec<VerifyCheck>,
    pub files_checked: u32,
    pub bytes_checked: u64,
    pub failures: Vec<FileFailure>,
    pub content_fingerprint: String,
    pub completed_unix: u64,
}

impl VerifyReport {
    pub fn record(&self) -> VerificationRecord {
        VerificationRecord {
            mode: self.mode,
            completed_unix: self.completed_unix,
            passed: self.passed,
            content_fingerprint: self.content_fingerprint.clone(),
            files_checked: self.files_checked,
            bytes_checked: self.bytes_checked,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VerifyProgress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: u32,
    pub files_total: u32,
    pub current: String,
    /// Only reported once there's enough throughput data to mean something.
    pub eta_secs: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expectations {
    pub scheme: Option<PartitionScheme>,
    pub filesystem: Option<FilesystemKind>,
}

pub fn human_bytes(b: u64) -> String {
    match b {
        b if b >= 1_000_000_000 => format!("{:.1} GB", b as f64 / 1e9),
        b if b >= 1_000_000 => format!("{:.1} MB", b as f64 / 1e6),
        b => format!("{} KB", b.div_ceil(1000)),
    }
}

/// Throughput-based ETA that stays silent until it has at least 5 seconds
/// and 5 % of the work behind it.
pub fn eta(elapsed_secs: f64, done: u64, total: u64) -> Option<u64> {
    if elapsed_secs < 5.0 || total == 0 || done * 20 < total || done == 0 {
        return None;
    }
    let rate = done as f64 / elapsed_secs;
    Some(((total - done) as f64 / rate).ceil() as u64)
}

/// Read a file end to end, returning its BLAKE3 hash.
fn read_and_hash(path: &Path, cancel: &AtomicBool, mut on_bytes: impl FnMut(u64)) -> io::Result<Option<String>> {
    let mut f = File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
        on_bytes(n as u64);
    }
    Ok(Some(h.finalize().to_hex().to_string()))
}

/// What to verify. `layout` is the raw-device inspection when available
/// (the app may not have permission to read the raw device).
pub struct VerifyRequest<'a> {
    pub root: &'a Path,
    pub layout: Option<&'a MediaLayout>,
    pub expect: Expectations,
    pub libs: &'a LibraryReport,
    pub manifest: Option<&'a Manifest>,
    pub mode: VerifyMode,
}

pub fn verify_volume(
    req: &VerifyRequest<'_>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&VerifyProgress),
) -> VerifyReport {
    let VerifyRequest { root, layout, expect, libs, manifest, mode } = *req;
    let mut checks = Vec::new();
    let mut failures = Vec::new();

    // Filesystem and partition layout.
    if let Some(layout) = layout {
        let fs = layout.primary_filesystem();
        let ok_scheme = expect.scheme.is_none_or(|s| s == layout.scheme);
        let ok_fs = expect.filesystem.is_none_or(|f| Some(f) == fs);
        checks.push(VerifyCheck {
            name: "Filesystem".into(),
            passed: ok_scheme && ok_fs,
            detail: format!("{} + {}", layout.scheme.label(), fs.map(|f| f.label()).unwrap_or("no filesystem")),
        });
        let dirty = layout.primary().and_then(|p| p.filesystem.as_ref()).and_then(|f| f.dirty) == Some(true);
        if dirty {
            checks.push(VerifyCheck {
                name: "Clean unmount".into(),
                passed: false,
                detail: "The drive was not ejected safely last time.".into(),
            });
        }
    }

    // Databases.
    let problems = libs.problems();
    checks.push(VerifyCheck {
        name: "DJ library".into(),
        passed: problems.is_empty(),
        detail: if problems.is_empty() {
            let names: Vec<&str> = libs.formats.iter().map(|f| f.label()).collect();
            if names.is_empty() {
                "No DJ library on this drive".into()
            } else {
                format!("{} OK", names.join(", "))
            }
        } else {
            problems.join(" ")
        },
    });

    // Audio files.
    let files: Vec<(String, std::path::PathBuf, u64)> = walk_audio_files(root).collect();
    let selected: Vec<&(String, std::path::PathBuf, u64)> = match mode {
        VerifyMode::Full => files.iter().collect(),
        VerifyMode::Quick => {
            // An even spread across the drive, including first and last.
            let n = files.len();
            let want = n.min(24);
            (0..want).map(|i| &files[if want <= 1 { 0 } else { i * (n - 1) / (want - 1) }]).collect()
        }
    };
    let expected_hashes: HashMap<&str, &crate::manifest::ManifestFile> =
        manifest.map(|m| m.files.iter().map(|f| (f.path.as_str(), f)).collect()).unwrap_or_default();
    let bytes_total: u64 = selected.iter().map(|f| f.2).sum();
    let mut p = VerifyProgress {
        bytes_done: 0,
        bytes_total,
        files_done: 0,
        files_total: selected.len() as u32,
        current: String::new(),
        eta_secs: None,
    };
    let started = Instant::now();
    let mut cancelled = false;
    for (rel, full, size) in selected {
        p.current = rel.clone();
        progress(&p);
        if let Err(e) = probe_file(full) {
            failures.push(FileFailure { path: rel.clone(), problem: e.to_string() });
        }
        let mut last_emit = Instant::now();
        let before = p.bytes_done;
        let hashed = read_and_hash(full, cancel, |n| {
            p.bytes_done += n;
            if last_emit.elapsed().as_millis() > 250 {
                p.eta_secs = eta(started.elapsed().as_secs_f64(), p.bytes_done, p.bytes_total);
                progress(&p);
                last_emit = Instant::now();
            }
        });
        match hashed {
            Ok(None) => {
                cancelled = true;
                break;
            }
            Ok(Some(hash)) => {
                if let Some(exp) = expected_hashes.get(rel.as_str()) {
                    if exp.size != *size {
                        failures.push(FileFailure {
                            path: rel.clone(),
                            problem: format!("size is {size} bytes, expected {}", exp.size),
                        });
                    } else if exp.hash.as_deref().is_some_and(|h| h != hash) {
                        failures.push(FileFailure {
                            path: rel.clone(),
                            problem: "contents differ from what was copied".into(),
                        });
                    }
                }
            }
            Err(e) => failures.push(FileFailure { path: rel.clone(), problem: format!("read error: {e}") }),
        }
        p.bytes_done = before + size;
        p.files_done += 1;
    }
    p.eta_secs = None;
    progress(&p);

    // Files the manifest says we copied must still be there.
    let present: std::collections::HashSet<&str> = files.iter().map(|f| f.0.as_str()).collect();
    let mut missing = 0;
    for f in expected_hashes.keys() {
        if !present.contains(f) && !root.join(f).exists() {
            missing += 1;
            failures.push(FileFailure { path: f.to_string(), problem: "missing".into() });
        }
    }
    checks.push(VerifyCheck {
        name: if mode == VerifyMode::Full { "Every file read back".into() } else { "Sampled files read back".into() },
        passed: failures.is_empty() && !cancelled,
        detail: if cancelled {
            "Verification was cancelled".into()
        } else if failures.is_empty() {
            format!("{} files, {}", p.files_done, human_bytes(p.bytes_done))
        } else {
            let count = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
            format!(
                "{}{}",
                count(failures.len(), "problem", "problems"),
                if missing > 0 { format!(", {} missing", count(missing, "file", "files")) } else { String::new() }
            )
        },
    });

    let passed = !cancelled && checks.iter().all(|c| c.passed);
    VerifyReport {
        mode,
        passed,
        cancelled,
        checks,
        files_checked: p.files_done,
        bytes_checked: p.bytes_done,
        failures,
        content_fingerprint: content_fingerprint(root),
        completed_unix: now_unix(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::scan_libraries;
    use crate::manifest::ManifestFile;

    #[test]
    fn eta_needs_enough_data() {
        assert_eq!(eta(1.0, 50, 100), None);
        assert_eq!(eta(10.0, 1, 100), None);
        assert_eq!(eta(10.0, 50, 100), Some(10));
    }

    #[test]
    fn full_verification_catches_corruption_and_missing_files() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        std::fs::create_dir_all(root.join("Contents")).unwrap();
        let good = crate::audio::fixtures::mp3_cbr(30);
        std::fs::write(root.join("Contents/a.mp3"), &good).unwrap();
        std::fs::write(root.join("Contents/b.mp3"), &good).unwrap();
        let hash = blake3::hash(&good).to_hex().to_string();
        let mut m = Manifest::new(None, vec![]);
        for p in ["Contents/a.mp3", "Contents/b.mp3", "Contents/c.mp3"] {
            m.files.push(ManifestFile { path: p.into(), size: good.len() as u64, hash: Some(hash.clone()) });
        }
        // b is bit-flipped, c never made it.
        let mut bad = good.clone();
        bad[2000] ^= 0xFF;
        std::fs::write(root.join("Contents/b.mp3"), &bad).unwrap();
        let libs = scan_libraries(root);
        let cancel = AtomicBool::new(false);
        let mut ticks = 0;
        let exp = Expectations { scheme: None, filesystem: None };
        let req =
            VerifyRequest { root, layout: None, expect: exp, libs: &libs, manifest: Some(&m), mode: VerifyMode::Full };
        let r = verify_volume(&req, &cancel, |_| ticks += 1);
        assert!(!r.passed);
        assert!(ticks > 0);
        let problems: Vec<(&str, &str)> = r.failures.iter().map(|f| (f.path.as_str(), f.problem.as_str())).collect();
        assert!(problems.contains(&("Contents/b.mp3", "contents differ from what was copied")), "{problems:?}");
        assert!(problems.contains(&("Contents/c.mp3", "missing")), "{problems:?}");
        assert_eq!(r.files_checked, 2);

        std::fs::write(root.join("Contents/b.mp3"), &good).unwrap();
        m.files.pop();
        let req =
            VerifyRequest { root, layout: None, expect: exp, libs: &libs, manifest: Some(&m), mode: VerifyMode::Quick };
        let r = verify_volume(&req, &cancel, |_| {});
        assert!(r.passed, "{:?}", r.checks);
    }

    #[test]
    fn cancel_stops_early() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("a.mp3"), crate::audio::fixtures::mp3_cbr(10)).unwrap();
        let cancel = AtomicBool::new(true);
        let libs = scan_libraries(d.path());
        let req = VerifyRequest {
            root: d.path(),
            layout: None,
            expect: Expectations { scheme: None, filesystem: None },
            libs: &libs,
            manifest: None,
            mode: VerifyMode::Full,
        };
        let r = verify_volume(&req, &cancel, |_| {});
        assert!(r.cancelled && !r.passed);
    }
}
