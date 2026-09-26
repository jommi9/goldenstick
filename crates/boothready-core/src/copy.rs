//! Copying one prepared drive's contents onto another.
//!
//! rekordbox and Engine DJ exports store track paths relative to the volume
//! root, so a copy of the whole tree is a working export on the second
//! drive. BoothReady never writes a partial DJ database (PRD §77): when the
//! whole tree doesn't fit, the answer is a second export from the DJ
//! software.
//!
//! Each file is written under a temporary name, given the source's
//! modification time and then renamed into place, and a journal of finished
//! files with their BLAKE3 hashes is flushed every few seconds. An
//! interrupted copy resumes from the journal (PRD §61), and copying an
//! unchanged drive again copies nothing (PRD §71). When the copy finishes,
//! the hashes go into the destination's manifest, so a full verification of
//! the destination proves every byte arrived.

use crate::manifest::{read_manifest, write_manifest, Manifest, ManifestFile, MANIFEST_DIR};
use crate::verify::{eta, VerifyProgress};
use boothready_model::FilesystemKind;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// The journal of finished files, kept next to the manifest.
pub const JOURNAL_FILE: &str = ".boothready/copy-journal.json";
/// Suffix of a file that is still being written.
const PART_SUFFIX: &str = ".brpart";
/// FAT32 stores file sizes in 32 bits.
pub const FAT32_MAX_FILE: u64 = u32::MAX as u64;
/// Space left free on the destination beyond what the files need.
const SPACE_MARGIN: u64 = 16 * 1024 * 1024;
const JOURNAL_EVERY: Duration = Duration::from_secs(5);

/// Copy progress has the same shape as verification progress.
pub type CopyProgress = VerifyProgress;

/// What the destination looks like, from the platform layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CopyTarget {
    pub filesystem: Option<FilesystemKind>,
    pub free_bytes: u64,
    /// Allocation unit; every file occupies a whole number of these.
    pub cluster_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyEntry {
    /// Path relative to the volume root, with `/` separators.
    pub path: String,
    pub size: u64,
    pub modified_unix: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CopyProblem {
    /// FAT32 can't hold a file of 4 GiB or more.
    TooLargeForFat32 {
        path: String,
        size: u64,
    },
    /// FAT and exFAT, and Windows, refuse this name.
    NameNotAllowed {
        path: String,
        reason: String,
    },
    NotEnoughSpace {
        needed: u64,
        free: u64,
    },
    /// The destination holds files this copy didn't put there. Copying
    /// would mix two libraries, so the drive has to be prepared first.
    DestinationNotEmpty {
        files: u64,
        example: String,
    },
}

impl CopyProblem {
    pub fn describe(&self) -> String {
        match self {
            CopyProblem::TooLargeForFat32 { path, size } => {
                format!("{path} is {} and FAT32 can't store files of 4 GB or more", crate::verify::human_bytes(*size))
            }
            CopyProblem::NameNotAllowed { path, reason } => format!("{path}: {reason}"),
            CopyProblem::NotEnoughSpace { needed, free } => format!(
                "The copy needs {} but the destination has {} free",
                crate::verify::human_bytes(*needed),
                crate::verify::human_bytes(*free)
            ),
            CopyProblem::DestinationNotEmpty { files, example } => format!(
                "The destination already holds {files} other files (for example {example}). Prepare it first so two libraries don't get mixed"
            ),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct JournalEntry {
    size: u64,
    modified_unix: Option<u64>,
    hash: String,
}

/// Files a previous run of this copy finished, by path.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Journal {
    files: BTreeMap<String, JournalEntry>,
}

impl Journal {
    fn read(dst: &Path) -> Journal {
        fs::read_to_string(dst.join(JOURNAL_FILE)).ok().and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn write(&self, dst: &Path) -> io::Result<()> {
        fs::create_dir_all(dst.join(MANIFEST_DIR))?;
        let tmp = dst.join(format!("{JOURNAL_FILE}.tmp"));
        {
            let mut f = File::create(&tmp)?;
            f.write_all(&serde_json::to_vec(self).map_err(io::Error::other)?)?;
            f.sync_all()?;
        }
        fs::rename(tmp, dst.join(JOURNAL_FILE))
    }

    /// Whether `e` was finished by an earlier run and is still intact.
    fn has(&self, e: &CopyEntry, dst: &Path) -> bool {
        self.files.get(&e.path).is_some_and(|j| {
            j.size == e.size
                && j.modified_unix == e.modified_unix
                && fs::metadata(dst.join(&e.path)).is_ok_and(|m| m.is_file() && m.len() == e.size)
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyPlan {
    /// Everything the destination will hold, in copy order.
    pub files: Vec<CopyEntry>,
    pub files_to_copy: u32,
    pub bytes_to_copy: u64,
    /// Finished by an earlier run and skipped.
    pub files_done: u32,
    /// Files an earlier run of this copy wrote that the source no longer has.
    pub stale: Vec<String>,
    pub problems: Vec<CopyProblem>,
}

impl CopyPlan {
    pub fn can_start(&self) -> bool {
        self.problems.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CopyReport {
    pub files_copied: u32,
    pub bytes_copied: u64,
    pub files_skipped: u32,
    pub files_removed: u32,
    pub cancelled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum CopyError {
    #[error("the copy can't start: {0}")]
    Blocked(String),
    #[error("{path}: {source}")]
    File { path: String, source: io::Error },
    #[error(transparent)]
    Io(#[from] io::Error),
}

fn ignored_dir(e: &walkdir::DirEntry) -> bool {
    // Unlike the scanner, this keeps PIONEER/USBANLZ: that's where the
    // waveforms, beatgrids and cues live.
    e.depth() > 0
        && e.file_type().is_dir()
        && matches!(
            e.file_name().to_string_lossy().as_ref(),
            ".Spotlight-V100"
                | ".Trashes"
                | ".fseventsd"
                | ".TemporaryItems"
                | ".DocumentRevisions-V100"
                | "System Volume Information"
                | "$RECYCLE.BIN"
                | MANIFEST_DIR
        )
}

fn ignored_file(name: &str) -> bool {
    name.starts_with("._") || name == ".DS_Store" || name.ends_with(PART_SUFFIX)
}

fn modified_unix(m: &fs::Metadata) -> Option<u64> {
    m.modified().ok()?.duration_since(UNIX_EPOCH).ok().map(|d| d.as_secs())
}

/// Every file on a volume that belongs in a copy, sorted by path.
pub fn list_files(root: &Path) -> io::Result<Vec<CopyEntry>> {
    let mut out = Vec::new();
    for e in walkdir::WalkDir::new(root).follow_links(false).into_iter().filter_entry(|e| !ignored_dir(e)) {
        let e = e.map_err(io::Error::other)?;
        if !e.file_type().is_file() || ignored_file(&e.file_name().to_string_lossy()) {
            continue;
        }
        let m = e.metadata().map_err(io::Error::other)?;
        out.push(CopyEntry {
            path: crate::library::relative(root, e.path()),
            size: m.len(),
            modified_unix: modified_unix(&m),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Why FAT, exFAT or Windows would refuse this path, if they would.
pub fn name_problem(path: &str) -> Option<String> {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8", "COM9", "LPT1",
        "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    for part in path.split('/') {
        if let Some(c) = part.chars().find(|c| matches!(c, '<' | '>' | ':' | '"' | '\\' | '|' | '?' | '*') || *c < ' ')
        {
            return Some(format!("the name contains {c:?}, which FAT and exFAT drives can't store"));
        }
        if part.ends_with('.') || part.ends_with(' ') {
            return Some("names can't end with a dot or a space on FAT and exFAT drives".into());
        }
        let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
        if RESERVED.contains(&stem.as_str()) {
            return Some(format!("{stem} is a reserved name on Windows"));
        }
        if part.encode_utf16().count() > 255 {
            return Some("the name is longer than 255 characters".into());
        }
    }
    None
}

/// Work out what copying `src` onto `dst` involves, without writing
/// anything.
pub fn plan_copy(src: &Path, dst: &Path, target: &CopyTarget) -> io::Result<CopyPlan> {
    let (s, d) = (src.canonicalize()?, dst.canonicalize()?);
    if s.starts_with(&d) || d.starts_with(&s) {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "the source and destination are the same drive"));
    }
    let files = list_files(src)?;
    let journal = Journal::read(dst);
    let mut problems = Vec::new();
    for f in &files {
        if let Some(reason) = name_problem(&f.path) {
            problems.push(CopyProblem::NameNotAllowed { path: f.path.clone(), reason });
        }
        if target.filesystem == Some(FilesystemKind::Fat32) && f.size > FAT32_MAX_FILE {
            problems.push(CopyProblem::TooLargeForFat32 { path: f.path.clone(), size: f.size });
        }
    }

    // Anything already on the destination must be ours: the same file from
    // an earlier run, or one the journal says we wrote.
    let wanted: BTreeSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    let existing = list_files(dst)?;
    let foreign: Vec<&CopyEntry> =
        existing.iter().filter(|e| !wanted.contains(e.path.as_str()) && !journal.files.contains_key(&e.path)).collect();
    if let Some(first) = foreign.first() {
        problems.push(CopyProblem::DestinationNotEmpty { files: foreign.len() as u64, example: first.path.clone() });
    }
    // Files an earlier run wrote that the source no longer has.
    let stale: Vec<String> = existing
        .iter()
        .filter(|e| !wanted.contains(e.path.as_str()) && journal.files.contains_key(&e.path))
        .map(|e| e.path.clone())
        .collect();

    let cluster = target.cluster_bytes.max(512);
    let on_disk = |size: u64| size.div_ceil(cluster) * cluster;
    let mut done = BTreeSet::new();
    let (mut files_to_copy, mut bytes_to_copy, mut needed) = (0u32, 0u64, 0u64);
    for f in &files {
        if journal.has(f, dst) {
            done.insert(f.path.as_str());
        } else {
            files_to_copy += 1;
            bytes_to_copy += f.size;
            needed += on_disk(f.size);
        }
    }
    // Directory entries take clusters too; one per folder is a fair bound.
    let dirs: BTreeSet<&str> = files.iter().filter_map(|f| f.path.rsplit_once('/').map(|(d, _)| d)).collect();
    needed += dirs.len() as u64 * cluster;
    // Files being replaced or removed give their space back first.
    let reclaimed: u64 = existing
        .iter()
        .filter(|e| !done.contains(e.path.as_str()))
        .filter(|e| wanted.contains(e.path.as_str()) || journal.files.contains_key(&e.path))
        .map(|e| on_disk(e.size))
        .sum();
    let free = target.free_bytes + reclaimed;
    if needed + SPACE_MARGIN > free {
        problems.push(CopyProblem::NotEnoughSpace { needed: needed + SPACE_MARGIN, free });
    }
    let files_done = done.len() as u32;
    Ok(CopyPlan { files, files_to_copy, bytes_to_copy, files_done, stale, problems })
}

/// Copy one file to `<dst>.brpart`, hashing what was read, then give it the
/// source's modification time and rename it into place.
fn copy_file(
    src: &Path,
    dst: &Path,
    modified: Option<u64>,
    cancel: &AtomicBool,
    mut on_bytes: impl FnMut(u64),
) -> io::Result<Option<String>> {
    let part = PathBuf::from(format!("{}{PART_SUFFIX}", dst.display()));
    let mut input = File::open(src)?;
    let mut output = File::create(&part)?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        if cancel.load(Ordering::Relaxed) {
            drop(output);
            let _ = fs::remove_file(&part);
            return Ok(None);
        }
        let n = input.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        output.write_all(&buf[..n])?;
        on_bytes(n as u64);
    }
    output.sync_all()?;
    if let Some(t) = modified {
        output.set_modified(UNIX_EPOCH + Duration::from_secs(t))?;
    }
    drop(output);
    fs::rename(&part, dst)?;
    Ok(Some(hasher.finalize().to_hex().to_string()))
}

fn remove_parts(dst: &Path) {
    for e in walkdir::WalkDir::new(dst).into_iter().filter_entry(|e| !ignored_dir(e)).flatten() {
        if e.file_type().is_file() && e.file_name().to_string_lossy().ends_with(PART_SUFFIX) {
            let _ = fs::remove_file(e.path());
        }
    }
}

/// Carry out a plan from [`plan_copy`]. Stops after the current file when
/// `cancel` is set; running the same copy again picks up from there.
pub fn run_copy(
    plan: &CopyPlan,
    src: &Path,
    dst: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(&CopyProgress),
) -> Result<CopyReport, CopyError> {
    if let Some(p) = plan.problems.first() {
        return Err(CopyError::Blocked(p.describe()));
    }
    let mut journal = Journal::read(dst);
    remove_parts(dst);
    let mut report =
        CopyReport { files_copied: 0, bytes_copied: 0, files_skipped: 0, files_removed: 0, cancelled: false };

    // Mirror deletions first so the space is free for the new files.
    for path in &plan.stale {
        if journal.files.remove(path).is_some() {
            fs::remove_file(dst.join(path)).map_err(|source| CopyError::File { path: path.clone(), source })?;
            report.files_removed += 1;
        }
    }

    let mut p = CopyProgress {
        bytes_done: 0,
        bytes_total: plan.bytes_to_copy,
        files_done: 0,
        files_total: plan.files_to_copy,
        current: String::new(),
        eta_secs: None,
    };
    let started = Instant::now();
    let mut last_journal = Instant::now();
    for f in &plan.files {
        if journal.has(f, dst) {
            report.files_skipped += 1;
            continue;
        }
        p.current = f.path.clone();
        progress(&p);
        let target = dst.join(&f.path);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).map_err(|source| CopyError::File { path: f.path.clone(), source })?;
        }
        let mut last_emit = Instant::now();
        let hash = copy_file(&src.join(&f.path), &target, f.modified_unix, cancel, |n| {
            p.bytes_done += n;
            if last_emit.elapsed() > Duration::from_millis(250) {
                p.eta_secs = eta(started.elapsed().as_secs_f64(), p.bytes_done, p.bytes_total);
                progress(&p);
                last_emit = Instant::now();
            }
        })
        .map_err(|source| CopyError::File { path: f.path.clone(), source })?;
        let Some(hash) = hash else {
            report.cancelled = true;
            break;
        };
        journal.files.insert(f.path.clone(), JournalEntry { size: f.size, modified_unix: f.modified_unix, hash });
        report.files_copied += 1;
        report.bytes_copied += f.size;
        p.files_done += 1;
        if last_journal.elapsed() > JOURNAL_EVERY {
            journal.write(dst)?;
            last_journal = Instant::now();
        }
    }
    journal.write(dst)?;
    p.eta_secs = None;
    progress(&p);
    if !report.cancelled {
        record_in_manifest(dst, &journal)?;
    }
    Ok(report)
}

/// Put every copied file's hash into the destination's manifest, so a full
/// verification checks the copy. An earlier verification only survives if
/// the copy changed nothing.
fn record_in_manifest(dst: &Path, journal: &Journal) -> io::Result<()> {
    let existing = read_manifest(dst);
    let files: Vec<ManifestFile> = journal
        .files
        .iter()
        .map(|(path, j)| ManifestFile { path: path.clone(), size: j.size, hash: Some(j.hash.clone()) })
        .collect();
    if existing.as_ref().is_some_and(|m| m.files == files) {
        return Ok(());
    }
    let mut m = existing.unwrap_or_else(|| Manifest::new(None, vec![]));
    m.files = files;
    m.verification = None;
    write_manifest(dst, &m)
}

#[cfg(test)]
mod tests;
