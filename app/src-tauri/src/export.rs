//! Export to USB: a rekordbox Device Library built from ticked folders of a
//! music source, into a fresh folder of its own on this computer.
//!
//! The writer in `boothready_core::export::device_library` does the work.
//! Nothing here touches a USB: a finished export is offered to the copy
//! flow, which puts it on the one stick the user confirms and then runs
//! the usual verification. Exports live under `exports/` in the app's data
//! folder (or in the demo folder in demo mode), one folder each, and are
//! never written into an existing library.

use crate::library::{self, ExportSelection};
use crate::{e2s, existing_ancestor, AppState, Res};
use boothready_core::export::device_library::{
    cached_analyses, export_device_library_cancellable, Analysis, ExportError, ExportProgress, ExportRequest,
};
use boothready_core::export::playlists::{analysis_estimate_secs, collect_from_lists, guard_target, Collected};
use boothready_core::manifest::{now_unix, MANIFEST_DIR};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager, State};

/// The record a finished export carries in its `.boothready` folder, which
/// the copy flow leaves behind when it copies the tree onto a stick.
const EXPORT_RECORD: &str = "export.json";

/// Which folders to export: a source and folders inside it, each of which
/// becomes a playlist named after the folder.
#[derive(Deserialize)]
pub struct ExportRequestDto {
    pub id: String,
    pub folders: Vec<String>,
    /// The export's name, which names its folder. Empty asks for the suggested one.
    pub name: String,
}

#[derive(Serialize)]
pub struct PlannedPlaylist {
    pub folder: String,
    pub name: String,
    pub tracks: u32,
}

/// What exporting the ticked folders involves, before anything is written.
#[derive(Serialize)]
pub struct ExportPlan {
    pub playlists: Vec<PlannedPlaylist>,
    /// Every track once, however many playlists hold it.
    pub tracks: u32,
    pub bytes: u64,
    pub cloud_only: u32,
    pub cloud_only_bytes: u64,
    /// Tracks left out: damaged, not audio, or a format a Device Library can't hold.
    pub left_out: u32,
    /// Tracks whose analysis is already on this computer from an earlier export.
    pub analysed: u32,
    pub to_analyse: u32,
    /// Roughly how long the analysis takes, from the tracks still to analyse.
    pub estimate_secs: u64,
    pub name: String,
    pub destination: String,
    /// A folder with that name is already there; an export never goes into one.
    pub exists: bool,
    pub needed_bytes: u64,
    pub free_bytes: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct ExportProgressEvent {
    pub source_id: String,
    /// `analyze`, `probe`, `copy`, `database` or `install`.
    pub phase: String,
    pub done: u32,
    pub total: u32,
    pub current: String,
    /// Tracks analysed afresh so far, out of `to_analyse`.
    pub analysed: u32,
    pub to_analyse: u32,
}

/// A finished export on this computer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalExport {
    pub name: String,
    pub path: String,
    pub tracks: u32,
    pub playlists: u32,
    pub analysis_files: u32,
    pub bytes: u64,
    pub created_unix: u64,
    /// The name of the source the tracks came from.
    pub source: String,
    #[serde(default)]
    pub warnings: Vec<String>,
}

/// Where exports are kept.
pub fn exports_root(app: &AppHandle, state: &AppState) -> Res<PathBuf> {
    match state.demo_root() {
        Some(root) => Ok(root.join("exports")),
        None => Ok(app.path().app_data_dir().map_err(e2s)?.join("exports")),
    }
}

/// Where analyses from earlier exports are kept for the next one.
fn analysis_cache_dir(app: &AppHandle, state: &AppState) -> Res<PathBuf> {
    match state.demo_root() {
        Some(root) => Ok(root.join("analysis-cache")),
        None => Ok(app.path().app_data_dir().map_err(e2s)?.join("analysis-cache")),
    }
}

/// The folder of a finished export named by the copy flow as
/// `export:<name>`. Only a plain folder name under the exports folder is
/// accepted, so the UI can never name a path of its own.
pub fn export_path(app: &AppHandle, state: &AppState, name: &str) -> Res<PathBuf> {
    if name.is_empty() || !Path::new(name).components().all(|c| matches!(c, std::path::Component::Normal(_))) {
        return Err("That isn't an export on this computer.".to_string());
    }
    let dir = exports_root(app, state)?.join(name);
    if read_record(&dir).is_none() || !dir.join(boothready_core::library::DEVICE_LIBRARY_FILE).is_file() {
        return Err("That export isn't on this computer any more.".to_string());
    }
    Ok(dir)
}

fn read_record(dir: &Path) -> Option<LocalExport> {
    let bytes = std::fs::read(dir.join(MANIFEST_DIR).join(EXPORT_RECORD)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_record(dir: &Path, record: &LocalExport) -> std::io::Result<()> {
    std::fs::create_dir_all(dir.join(MANIFEST_DIR))?;
    std::fs::write(dir.join(MANIFEST_DIR).join(EXPORT_RECORD), serde_json::to_vec_pretty(record)?)
}

/// Every finished export, newest first.
pub fn list_exports(root: &Path) -> Vec<LocalExport> {
    let Ok(entries) = std::fs::read_dir(root) else { return Vec::new() };
    let mut out: Vec<LocalExport> = entries
        .filter_map(Result::ok)
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let mut record = read_record(&e.path())?;
            if !e.path().join(boothready_core::library::DEVICE_LIBRARY_FILE).is_file() {
                return None;
            }
            record.path = e.path().display().to_string();
            record.name = e.file_name().to_string_lossy().into_owned();
            Some(record)
        })
        .collect();
    out.sort_by(|a, b| b.created_unix.cmp(&a.created_unix).then_with(|| a.name.cmp(&b.name)));
    out
}

/// The selection, the request the writer will get and the plan to show.
struct Prepared {
    plan: ExportPlan,
    selection: ExportSelection,
    collected: Collected,
    destination: PathBuf,
    cache: PathBuf,
}

fn prepare(app: &AppHandle, state: &AppState, request: &ExportRequestDto) -> Res<Prepared> {
    let selection = state.with_library(app, |library| library.export_selection(&request.id, &request.folders))??;
    let lists: Vec<(String, Vec<PathBuf>)> = selection
        .playlists
        .iter()
        .map(|p| (p.name.clone(), p.paths.iter().map(|rel| selection.source.path.join(rel_path(rel))).collect()))
        .collect();
    let collected = collect_from_lists(&lists).map_err(describe)?;
    let source_name = library::name_of_source(&selection.source);
    let typed = if request.name.trim().is_empty() {
        format!("{source_name} {}", library::date_of(now_unix()))
    } else {
        request.name.clone()
    };
    let name = library::collection_folder_name(&typed)?;
    let destination = exports_root(app, state)?.join(&name);
    let cache = analysis_cache_dir(app, state)?;
    let analysed = cached_analyses(&cache, &collected.tracks, Analysis::Normal) as u32;
    let tracks = collected.tracks.len() as u32;
    let to_analyse = tracks.saturating_sub(analysed);
    let space = boothready_platform::space(existing_ancestor(&destination)).ok();
    // Three analysis files per track come to a couple of megabytes.
    let needed_bytes = library::space_needed(
        selection.bytes + u64::from(tracks) * 2_000_000,
        u64::from(tracks) * 4,
        space.map(|s| s.cluster_bytes).unwrap_or(4096),
    );
    let plan = ExportPlan {
        playlists: selection
            .playlists
            .iter()
            .zip(&collected.playlists)
            .map(|(p, spec)| PlannedPlaylist {
                folder: p.folder.clone(),
                name: p.name.clone(),
                tracks: spec.tracks.len() as u32,
            })
            .collect(),
        tracks,
        bytes: selection.bytes,
        cloud_only: selection.cloud_only,
        cloud_only_bytes: selection.cloud_only_bytes,
        left_out: selection.left_out + collected.warnings.iter().filter(|w| w.starts_with("skipped ")).count() as u32,
        analysed,
        to_analyse,
        estimate_secs: analysis_estimate_secs(to_analyse as usize),
        name,
        destination: destination.display().to_string(),
        exists: destination.exists(),
        needed_bytes,
        free_bytes: space.map(|s| s.free_bytes),
    };
    Ok(Prepared { plan, selection, collected, destination, cache })
}

/// A scan's relative path as a path of the operating system.
fn rel_path(rel: &str) -> PathBuf {
    rel.split('/').collect()
}

/// The writer's errors in the words the panel shows.
fn describe(e: ExportError) -> String {
    match e {
        ExportError::Cancelled => "Stopped. Nothing was written to the export folder.".to_string(),
        ExportError::Request(m) | ExportError::Target(m) | ExportError::Track(m) => m,
        other => other.to_string(),
    }
}

#[tauri::command]
pub fn library_export_plan(app: AppHandle, state: State<'_, AppState>, request: ExportRequestDto) -> Res<ExportPlan> {
    prepare(&app, &state, &request).map(|p| p.plan)
}

/// Build the export into a fresh folder, streaming `library-export-progress`.
/// Reading a cloud-only track makes the cloud client download it.
#[tauri::command]
pub async fn library_export(app: AppHandle, state: State<'_, AppState>, request: ExportRequestDto) -> Res<LocalExport> {
    let prepared = prepare(&app, &state, &request)?;
    if prepared.plan.exists {
        return Err("An export with this name is already there. Choose another name.".to_string());
    }
    if prepared.plan.free_bytes.is_some_and(|free| free < prepared.plan.needed_bytes) {
        return Err("There isn't enough free space on this computer for this export.".to_string());
    }
    let devices = state.platform().list_devices().unwrap_or_default();
    guard_target(&prepared.destination, &devices).map_err(describe)?;
    state.with_library(&app, |library| {
        if library.is_busy() {
            return Err("Another scan, collection or export is still running.".to_string());
        }
        library.set_exporting(true);
        Ok(())
    })??;
    let cancel = state.cancel_library.clone();
    cancel.store(false, Ordering::Relaxed);
    let emitter = app.clone();
    let source_id = request.id.clone();
    let to_analyse = prepared.plan.to_analyse;
    let destination = prepared.destination.clone();
    let writer_request = ExportRequest {
        root: destination.clone(),
        tracks: prepared.collected.tracks,
        playlists: prepared.collected.playlists,
        analysis: Analysis::Normal,
        analysis_cache: Some(prepared.cache),
    };
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        let mut last = std::time::Instant::now();
        let mut phase = "";
        let progress = |p: &ExportProgress| {
            if phase != p.phase || last.elapsed() >= Duration::from_millis(120) {
                phase = p.phase;
                last = std::time::Instant::now();
                let event = ExportProgressEvent {
                    source_id: source_id.clone(),
                    phase: p.phase.to_string(),
                    done: p.done as u32,
                    total: p.total as u32,
                    current: p.current.clone(),
                    analysed: p.analysed as u32,
                    to_analyse,
                };
                let _ = emitter.emit("library-export-progress", event);
            }
        };
        export_device_library_cancellable(&writer_request, &cancel, progress)
    })
    .await;
    state.with_library(&app, |library| library.set_exporting(false))?;
    let report = match outcome.map_err(e2s)? {
        Ok(report) => report,
        Err(e) => {
            // The writer removes its staging folder; an empty target folder
            // it made goes too, so a stopped export leaves nothing behind.
            let _ = std::fs::remove_dir(&destination);
            return Err(describe(e));
        }
    };
    let record = LocalExport {
        name: prepared.plan.name.clone(),
        path: report.root.display().to_string(),
        tracks: report.tracks.len() as u32,
        playlists: report.playlists as u32,
        analysis_files: report.analysis_files as u32,
        bytes: report.bytes_copied,
        created_unix: now_unix(),
        source: library::name_of_source(&prepared.selection.source),
        warnings: prepared.collected.warnings.iter().chain(&report.warnings).cloned().collect(),
    };
    write_record(&report.root, &record)
        .map_err(|e| format!("The export was built but its record couldn't be written: {e}"))?;
    Ok(record)
}

#[tauri::command]
pub fn library_cancel_export(state: State<'_, AppState>) {
    state.cancel_library.store(true, Ordering::Relaxed);
}

/// The finished exports on this computer, newest first.
#[tauri::command]
pub fn library_exports(app: AppHandle, state: State<'_, AppState>) -> Res<Vec<LocalExport>> {
    Ok(list_exports(&exports_root(&app, &state)?))
}

/// Delete a finished export from this computer. Nothing on a USB changes.
#[tauri::command]
pub fn library_export_remove(app: AppHandle, state: State<'_, AppState>, name: String) -> Res<()> {
    let dir = export_path(&app, &state, &name)?;
    std::fs::remove_dir_all(&dir).map_err(|e| format!("The export couldn't be removed: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_are_listed_from_their_records_newest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let make = |name: &str, created: u64, with_pdb: bool| {
            let dir = tmp.path().join(name);
            std::fs::create_dir_all(dir.join("PIONEER/rekordbox")).unwrap();
            if with_pdb {
                std::fs::write(dir.join("PIONEER/rekordbox/export.pdb"), b"pdb").unwrap();
            }
            write_record(
                &dir,
                &LocalExport {
                    name: "typed".into(),
                    path: String::new(),
                    tracks: 3,
                    playlists: 1,
                    analysis_files: 9,
                    bytes: 100,
                    created_unix: created,
                    source: "DJ".into(),
                    warnings: vec![],
                },
            )
            .unwrap();
        };
        make("Older", 10, true);
        make("Newer", 20, true);
        make("Broken", 30, false);
        std::fs::create_dir_all(tmp.path().join("Stray/Contents")).unwrap();
        let listed = list_exports(tmp.path());
        assert_eq!(listed.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), ["Newer", "Older"]);
        assert_eq!(listed[0].path, tmp.path().join("Newer").display().to_string());
        assert_eq!(listed[0].tracks, 3);
        assert!(list_exports(&tmp.path().join("missing")).is_empty());
        assert_eq!(rel_path("a/b/c.mp3"), PathBuf::from("a").join("b").join("c.mp3"));
    }
}
